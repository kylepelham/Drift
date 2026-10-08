use crate::remote::RemoteAccess;
use crate::store::{RemoteDevice, Store};
use axum::Json;
use axum::extract::{ConnectInfo, Extension, Path as UrlPath, State};
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use serde::Deserialize;
use serde_json::json;
use std::net::SocketAddr;
use std::time::Instant;
use tauri::{Emitter, Manager};

use super::{
    LINK_TTL, Poll, constant_time_eq, device_name, display_code, session_cookie, verify_password, with_cookie,
};

#[derive(Deserialize)]
pub(crate) struct LinkRequest {
    name: Option<String>,
}

#[derive(Deserialize)]
pub(crate) struct LoginRequest {
    username: String,
    password: String,
    name: Option<String>,
}

fn failure(status: StatusCode, message: &str) -> Response {
    (status, Json(json!({ "error": message }))).into_response()
}

fn notify(app: &tauri::AppHandle) {
    let _ = app.emit("remote-access-changed", ());
}

/// The public CA certificate; installing it on a device removes the browser warning.
pub(crate) async fn certificate(State(app): State<tauri::AppHandle>) -> Response {
    let der = app.state::<RemoteAccess>().certificate();
    let mut response = der.into_response();
    let headers = response.headers_mut();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/x-x509-ca-cert"),
    );
    headers.insert(
        header::CONTENT_DISPOSITION,
        HeaderValue::from_static("attachment; filename=\"drift-remote-access.cer\""),
    );

    response
}

pub(crate) async fn options(State(app): State<tauri::AppHandle>) -> Response {
    let password = app.state::<RemoteAccess>().auth().password.is_some();
    Json(json!({ "password": password })).into_response()
}

pub(crate) async fn start_link(
    State(app): State<tauri::AppHandle>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    Json(request): Json<LinkRequest>,
) -> Response {
    let started = app
        .state::<RemoteAccess>()
        .auth()
        .request_link(peer.ip(), device_name(request.name));

    match started {
        Ok((id, code)) => {
            notify(&app);
            Json(json!({ "id": id, "code": display_code(&code), "expiresIn": LINK_TTL.as_secs() })).into_response()
        }
        Err(error) => failure(StatusCode::TOO_MANY_REQUESTS, &error.to_string()),
    }
}

pub(crate) async fn poll_link(State(app): State<tauri::AppHandle>, UrlPath(id): UrlPath<String>) -> Response {
    let polled = app.state::<RemoteAccess>().auth().poll(&id, &app.state::<Store>());

    match polled {
        Ok(Poll::Pending) => Json(json!({ "status": "pending" })).into_response(),
        Ok(Poll::Expired) => (StatusCode::NOT_FOUND, Json(json!({ "status": "expired" }))).into_response(),
        Ok(Poll::Approved(token)) => {
            notify(&app);
            with_cookie(
                Json(json!({ "status": "approved" })).into_response(),
                session_cookie(&token),
            )
        }
        Err(error) => failure(StatusCode::INTERNAL_SERVER_ERROR, &error.to_string()),
    }
}

pub(crate) async fn login(
    State(app): State<tauri::AppHandle>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    Json(request): Json<LoginRequest>,
) -> Response {
    let access = app.state::<RemoteAccess>();
    let address = peer.ip();
    let (locked, password) = {
        let mut auth = access.auth();
        (auth.locked_for(address, Instant::now()), auth.password.clone())
    };
    if let Some(wait) = locked {
        return locked_response(wait);
    }
    let Some(password) = password else {
        return failure(StatusCode::NOT_FOUND, "Password sign-in is turned off.");
    };

    let _permit = access.password_checks().acquire().await;
    if let Some(wait) = access.auth().locked_for(address, Instant::now()) {
        return locked_response(wait);
    }
    let supplied = request.password;
    let hash = password.hash.clone();
    let matches = tokio::task::spawn_blocking(move || verify_password(&supplied, &hash))
        .await
        .unwrap_or(false);
    let valid = matches && constant_time_eq(request.username.trim().as_bytes(), password.username.as_bytes());

    let mut auth = access.auth();
    if !valid {
        auth.fail(address, Instant::now());
        return failure(StatusCode::UNAUTHORIZED, "Wrong username or password.");
    }
    // A password rotation must reject an in-flight verification of the old password.
    if !auth.password_is_current(&password) {
        return failure(StatusCode::UNAUTHORIZED, "Wrong username or password.");
    }

    auth.failures.remove(&address);
    match auth.create_device(device_name(request.name), "password", &app.state::<Store>()) {
        Ok(token) => {
            drop(auth);
            notify(&app);
            with_cookie(StatusCode::NO_CONTENT.into_response(), session_cookie(&token))
        }
        Err(error) => failure(StatusCode::INTERNAL_SERVER_ERROR, &error.to_string()),
    }
}

fn locked_response(wait: std::time::Duration) -> Response {
    let message = format!("Too many attempts. Try again in {} seconds.", wait.as_secs().max(1));
    failure(StatusCode::TOO_MANY_REQUESTS, &message)
}

pub(crate) async fn me(Extension(device): Extension<RemoteDevice>) -> Response {
    Json(device).into_response()
}

pub(crate) async fn logout(
    State(app): State<tauri::AppHandle>,
    Extension(device): Extension<RemoteDevice>,
) -> Response {
    let access = app.state::<RemoteAccess>();
    if let Err(error) = access.auth().revoke(Some(&device.id), &app.state::<Store>()) {
        return failure(StatusCode::INTERNAL_SERVER_ERROR, &error.to_string());
    }

    access.invalidate_streams();
    notify(&app);
    let cleared = HeaderValue::from_static("drift_remote=; HttpOnly; Secure; SameSite=Strict; Path=/; Max-Age=0");

    with_cookie(StatusCode::NO_CONTENT.into_response(), cleared)
}
