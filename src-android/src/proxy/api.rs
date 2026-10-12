//! What the computers page asks of the app: the linked PCs, discovery, scanning and linking, and choosing one.

use std::net::IpAddr;
use std::time::Duration;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::json;

use super::Proxy;
use crate::discovery;
use crate::pc::{self, Linked};

const DISCOVERY_WAIT: Duration = Duration::from_millis(1500);
const GATEWAY_PORT: u16 = 41718;

pub(super) fn routes() -> Router<Proxy> {
    Router::new()
        .route("/computers", get(computers))
        .route("/discover", post(discover))
        .route("/scan", post(scan))
        .route("/pair", post(pair))
        .route("/link", post(start_link))
        .route("/link/{id}", get(poll_link))
        .route("/use", post(use_computer))
        .route("/forget", post(forget))
}

#[derive(Deserialize)]
struct Uri {
    uri: String,
}

#[derive(Deserialize)]
struct Address {
    address: IpAddr,
    #[serde(default)]
    port: Option<u16>,
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
                "words": computer.words(),
                "active": active.as_deref() == Some(computer.id.as_str()),
            })
        })
        .collect();

    Json(json!({ "computers": list })).into_response()
}

/// PCs answering on this network, each marked when it is already linked.
async fn discover(State(proxy): State<Proxy>) -> Response {
    let found = discovery::discover(DISCOVERY_WAIT).await;
    let computers = proxy.computers.lock().unwrap();
    let list: Vec<_> = found
        .into_iter()
        .map(|found| {
            let linked = computers.get(&found.fingerprint).is_some();
            json!({ "found": found, "linked": linked })
        })
        .collect();

    Json(json!({ "found": list })).into_response()
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
            proxy.adopt(computer, pc);
            Json(json!({ "linked": name })).into_response()
        }
        Err(error) => refuse(&error.to_string()),
    }
}

/// Starts linking by typed code: the code to type on the PC, and the words to compare before typing it.
async fn start_link(State(proxy): State<Proxy>, Json(body): Json<Address>) -> Response {
    let device = proxy.device_name().await;
    let linking = match pc::start_link(body.address, body.port.unwrap_or(GATEWAY_PORT), &device).await {
        Ok(linking) => linking,
        Err(error) => return refuse(&error.to_string()),
    };

    let id = pc::hex(&random());
    let answer = json!({
        "id": id,
        "name": linking.name,
        "code": linking.code,
        "words": drift_pairing::words(&linking.authority),
    });
    proxy.links.lock().unwrap().insert(id, linking);

    Json(answer).into_response()
}

async fn poll_link(State(proxy): State<Proxy>, Path(id): Path<String>) -> Response {
    let linking = proxy.links.lock().unwrap().remove(&id);
    let Some(linking) = linking else {
        return Json(json!({ "status": "expired" })).into_response();
    };

    match pc::poll_link(&linking).await {
        Ok(Linked::Waiting) => {
            proxy.links.lock().unwrap().insert(id, linking);
            Json(json!({ "status": "waiting" })).into_response()
        }
        Ok(Linked::Expired) => Json(json!({ "status": "expired" })).into_response(),
        Ok(Linked::Approved(computer)) => {
            proxy.adopt(computer, linking.pc);
            Json(json!({ "status": "linked" })).into_response()
        }
        Err(error) => {
            proxy.links.lock().unwrap().insert(id, linking);
            refuse(&error.to_string())
        }
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
        if let Ok(pc) = pc::reach(&computer).await {
            pc.sign_out(&computer.token).await;
        }
    });
    Json(json!({ "ok": true })).into_response()
}

fn random() -> [u8; 16] {
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes).expect("the system random source works");
    bytes
}

fn refuse(message: &str) -> Response {
    (StatusCode::BAD_REQUEST, Json(json!({ "error": message }))).into_response()
}
