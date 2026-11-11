use crate::remote::RemoteAccess;
use crate::store::{RemoteDevice, Store};
use axum::Json;
use axum::extract::{ConnectInfo, Extension, State};
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use serde::Deserialize;
use serde_json::json;
use std::net::SocketAddr;
use std::time::{Duration, Instant};
use tauri::{Emitter, Manager};

use super::requests::Answer;
use super::{constant_time_eq, device_name, random_bytes, session_cookie, verify_password, with_cookie};

/// How long one wait for an answer is held open before the device asks again.
const ANSWER_POLL: Duration = Duration::from_secs(25);

#[derive(Deserialize)]
pub(crate) struct OpenRequest {
    name: Option<String>,
}

#[derive(Deserialize)]
pub(crate) struct RevealRequest {
    id: String,
    secret: String,
}

#[derive(Deserialize)]
pub(crate) struct RequestId {
    id: String,
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

/// Tells the desktop the requests to ask about changed, and brings its window up when one was just shown.
pub(crate) fn requests_changed(app: &tauri::AppHandle, shown: bool) {
    let _ = app.emit("remote-access-requests", ());
    if shown && let Some(window) = app.get_webview_window("main") {
        let _ = window.unminimize();
        let _ = window.show();
        let _ = window.set_focus();
    }
}

fn gone() -> Response {
    failure(StatusCode::GONE, &super::requests::RequestError::Unknown.to_string())
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

/// What a signed-out client may know: whether password sign-in is on, and which PC and API version this is.
pub(crate) async fn options(State(app): State<tauri::AppHandle>) -> Response {
    let password = app.state::<RemoteAccess>().auth().password.is_some();
    Json(json!({
        "password": password,
        "api": drift_pairing::API_VERSION,
        "version": env!("CARGO_PKG_VERSION"),
        "computer": crate::remote::computer_name(),
    }))
    .into_response()
}

/// A device asks to connect. This computer commits to its secret now and reveals it only once the device sent its own.
/// Declined requests count toward the same lockout as wrong passwords.
pub(crate) async fn request_open(
    State(app): State<tauri::AppHandle>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    Json(request): Json<OpenRequest>,
) -> Response {
    let access = app.state::<RemoteAccess>();
    let address = peer.ip();
    let mut auth = access.auth();
    if let Some(wait) = auth.locked_for(address, Instant::now()) {
        return locked_response(wait);
    }

    let opened = auth
        .requests
        .open(device_name(request.name), address, random_bytes(), Instant::now());
    drop(auth);
    match opened {
        Ok((id, commitment)) => {
            requests_changed(&app, false);
            Json(json!({ "id": id, "commitment": drift_pairing::hex(&commitment) })).into_response()
        }
        Err(error) => failure(StatusCode::TOO_MANY_REQUESTS, &error.to_string()),
    }
}

/// Takes the device's secret and reveals this computer's, so both can show the code; the user is asked from now on.
pub(crate) async fn request_reveal(
    State(app): State<tauri::AppHandle>,
    Json(request): Json<RevealRequest>,
) -> Response {
    let Some(device) = drift_pairing::parse_fingerprint(&request.secret) else {
        return failure(StatusCode::BAD_REQUEST, "The secret is not valid.");
    };
    let access = app.state::<RemoteAccess>();
    let authority = access.authority();
    let revealed = access
        .auth()
        .requests
        .reveal(&request.id, &device, &authority, Instant::now());

    match revealed {
        Ok(secret) => {
            requests_changed(&app, true);
            Json(json!({ "secret": drift_pairing::hex(&secret) })).into_response()
        }
        Err(_) => gone(),
    }
}

/// Holds the device until the user answers, or for a while, after which it asks again. Accepted, it gets its session.
pub(crate) async fn request_wait(State(app): State<tauri::AppHandle>, Json(request): Json<RequestId>) -> Response {
    let access = app.state::<RemoteAccess>();
    let Some(mut answer) = access.auth().requests.watch(&request.id) else {
        return gone();
    };
    let _ = tokio::time::timeout(ANSWER_POLL, answer.wait_for(|answer| *answer != Answer::Waiting)).await;

    let mut auth = access.auth();
    let settled = auth.requests.settle(&request.id);
    let response = match settled {
        None if auth.requests.watch(&request.id).is_some() => return Json(json!({ "waiting": true })).into_response(),
        None => return gone(),
        Some((Answer::Accepted, name)) => match auth.create_device(name, "app", &app.state::<Store>()) {
            Ok(token) => Json(json!({ "token": token })).into_response(),
            Err(error) => failure(StatusCode::INTERNAL_SERVER_ERROR, &error.to_string()),
        },
        Some(_) => {
            let message = format!("{} declined the connection.", crate::remote::computer_name());
            failure(StatusCode::FORBIDDEN, &message)
        }
    };
    drop(auth);
    notify(&app);
    requests_changed(&app, false);

    response
}

/// The device stopped waiting; its request is no longer shown.
pub(crate) async fn request_cancel(State(app): State<tauri::AppHandle>, Json(request): Json<RequestId>) -> Response {
    if app.state::<RemoteAccess>().auth().requests.cancel(&request.id) {
        requests_changed(&app, false);
    }

    StatusCode::NO_CONTENT.into_response()
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

/// The signed-in device, and where this PC can be reached now, so the app keeps its addresses current.
pub(crate) async fn me(State(app): State<tauri::AppHandle>, Extension(device): Extension<RemoteDevice>) -> Response {
    let mut answer = json!(device);
    answer["reach"] = app.state::<RemoteAccess>().reach();

    Json(answer).into_response()
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
