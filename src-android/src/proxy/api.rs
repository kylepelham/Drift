//! What the computers page asks of the app: the linked PCs, scanning a pairing code, and choosing or forgetting one.

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::json;

use super::Proxy;
use crate::pc;

pub(super) fn routes() -> Router<Proxy> {
    Router::new()
        .route("/computers", get(computers))
        .route("/scan", post(scan))
        .route("/pair", post(pair))
        .route("/use", post(use_computer))
        .route("/forget", post(forget))
}

#[derive(Deserialize)]
struct Uri {
    uri: String,
}

#[derive(Deserialize)]
struct Id {
    id: String,
}

async fn computers(State(proxy): State<Proxy>) -> Response {
    let computers = proxy.computers.lock().unwrap();
    let active = computers.active().map(|computer| computer.id.clone());
    let list: Vec<_> = computers
        .list()
        .iter()
        .map(|computer| {
            json!({
                "id": computer.id,
                "name": computer.name,
                "address": computer.addresses.first(),
                "anywhere": computer.tunnel.is_some(),
                "active": active.as_deref() == Some(computer.id.as_str()),
            })
        })
        .collect();

    Json(json!({ "computers": list })).into_response()
}

async fn scan(State(proxy): State<Proxy>) -> Response {
    let native = proxy.native.clone();
    let scanned = tokio::task::spawn_blocking(move || native.scan()).await;
    match scanned {
        Ok(Ok(Some(text))) => redeem(&proxy, &text).await,
        Ok(Ok(None)) => Json(json!({ "cancelled": true })).into_response(),
        Ok(Err(error)) => refuse(&error),
        Err(error) => refuse(&error.to_string()),
    }
}

async fn pair(State(proxy): State<Proxy>, Json(body): Json<Uri>) -> Response {
    redeem(&proxy, &body.uri).await
}

async fn redeem(proxy: &Proxy, text: &str) -> Response {
    let pairing = match drift_pairing::Pairing::parse(text) {
        Ok(pairing) => pairing,
        Err(error) => return refuse(&error.to_string()),
    };
    let device = proxy.device_name().await;

    match pc::pair(&pairing, &device).await {
        Ok((computer, pc)) => {
            let name = computer.name.clone();
            if let Err(error) = proxy.adopt(computer, pc) {
                return refuse(&error.to_string());
            }
            Json(json!({ "linked": name })).into_response()
        }
        Err(error) => refuse(&error.to_string()),
    }
}

async fn use_computer(State(proxy): State<Proxy>, Json(body): Json<Id>) -> Response {
    let computer = proxy.computers.lock().unwrap().get(&body.id).cloned();
    let Some(computer) = computer else {
        return refuse("that computer is not linked");
    };

    match proxy.connect(&computer).await {
        Ok(()) => Json(json!({ "ok": true })).into_response(),
        Err(error) => refuse(&error.to_string()),
    }
}

/// Removes a PC from this phone, signing the phone out of it when it can be reached.
async fn forget(State(proxy): State<Proxy>, Json(body): Json<Id>) -> Response {
    let computer = proxy.computers.lock().unwrap().get(&body.id).cloned();
    let Some(computer) = computer else {
        return Json(json!({ "ok": true })).into_response();
    };

    let mut route = proxy.route.lock().unwrap();
    if route.as_ref().is_some_and(|route| route.id == computer.id) {
        *route = None;
    }
    drop(route);
    let _ = proxy.computers.lock().unwrap().remove(&computer.id);

    tokio::spawn(async move {
        if let Ok((pc, _)) = pc::reach(&computer).await {
            pc.sign_out(&computer.token).await;
        }
    });
    Json(json!({ "ok": true })).into_response()
}

fn refuse(message: &str) -> Response {
    (StatusCode::BAD_REQUEST, Json(json!({ "error": message }))).into_response()
}
