//! What the computers page asks of the app: the linked PCs, the computers nearby, asking one to connect and waiting
//! for its answer, choosing or forgetting one, and going back to this device's own Drift where it has one.

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::json;

use super::Proxy;

pub(super) fn routes() -> Router<Proxy> {
    Router::new()
        .route("/computers", get(computers))
        .route("/nearby", get(nearby))
        .route("/online", get(online))
        .route("/tabs", post(set_tabs))
        .route("/ask", post(ask))
        .route("/answer", post(answer))
        .route("/cancel", post(cancel))
        .route("/use", post(use_computer))
        .route("/forget", post(forget))
        .route("/local", post(local))
}

#[derive(Deserialize)]
struct Address {
    address: String,
}

#[derive(Deserialize)]
struct Switch {
    on: bool,
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

    Json(json!({
        "computers": list,
        "local": proxy.host.this_computer(),
        "tabs": computers.tabs(),
    }))
    .into_response()
}

async fn nearby(State(proxy): State<Proxy>) -> Response {
    match proxy.nearby().await {
        Ok(found) => Json(json!({ "nearby": found })).into_response(),
        Err(error) => refuse(&error),
    }
}

async fn online(State(proxy): State<Proxy>) -> Response {
    Json(json!({ "online": proxy.online().await })).into_response()
}

/// Shows each computer as a tab at the top of a desktop's sidebar, or stops.
async fn set_tabs(State(proxy): State<Proxy>, Json(body): Json<Switch>) -> Response {
    match proxy.computers.lock().unwrap().set_tabs(body.on) {
        Ok(()) => Json(json!({ "ok": true })).into_response(),
        Err(error) => refuse(&error.to_string()),
    }
}

async fn ask(State(proxy): State<Proxy>, Json(body): Json<Address>) -> Response {
    match proxy.ask(&body.address).await {
        Ok(waiting) => Json(waiting).into_response(),
        Err(error) => refuse(&error),
    }
}

async fn answer(State(proxy): State<Proxy>, Json(body): Json<Id>) -> Response {
    match proxy.answer(&body.id).await {
        Ok((id, name)) => Json(json!({ "id": id, "linked": name })).into_response(),
        Err(error) => refuse(&error),
    }
}

async fn cancel(State(proxy): State<Proxy>, Json(body): Json<Id>) -> Response {
    proxy.cancel(&body.id).await;
    Json(json!({ "ok": true })).into_response()
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

/// Removes a PC from this device, signing the device out of it when it can be reached.
async fn forget(State(proxy): State<Proxy>, Json(body): Json<Id>) -> Response {
    proxy.forget(&body.id);
    Json(json!({ "ok": true })).into_response()
}

async fn local(State(proxy): State<Proxy>) -> Response {
    match proxy.leave() {
        Ok(()) => Json(json!({ "ok": true })).into_response(),
        Err(error) => refuse(&error),
    }
}

fn refuse(message: &str) -> Response {
    (StatusCode::BAD_REQUEST, Json(json!({ "error": message }))).into_response()
}
