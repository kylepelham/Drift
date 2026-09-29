//! The in-process Drift engine: opened at setup, served on loopback, handed to the UI by URL.

use std::net::{Ipv4Addr, SocketAddr};
use std::path::Path;
use std::sync::{Arc, Mutex};

use drift_engine::{Engine, Server};
use serde::Serialize;
use tauri::{AppHandle, Manager, State};

pub(crate) struct Native {
    engine: Arc<Engine>,
    state: Mutex<Listening>,
}

/// Bind happens on the runtime after setup returns, so stop must be able to arrive first.
enum Listening {
    Pending,
    Bound(Server),
    Failed(String),
    Stopped,
}

#[derive(Serialize)]
pub(crate) struct NativeStatus {
    url: Option<String>,
    token: Option<String>,
    error: Option<String>,
}

pub(crate) fn start(app: &AppHandle, data_dir: &Path) -> Result<(), drift_engine::Error> {
    let engine = Engine::open(data_dir)?;
    app.manage(Native {
        engine: engine.clone(),
        state: Mutex::new(Listening::Pending),
    });
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let addr = SocketAddr::from((Ipv4Addr::LOCALHOST, 0));
        let bound = drift_engine::listen(engine, addr).await;
        let native = app.state::<Native>();
        let mut state = native.state.lock().unwrap();
        *state = match (bound, &*state) {
            (Ok(server), Listening::Stopped) => {
                server.stop();
                Listening::Stopped
            }
            (Ok(server), _) => Listening::Bound(server),
            (Err(error), _) => Listening::Failed(error.to_string()),
        };
    });
    Ok(())
}

pub(crate) fn stop(app: &AppHandle) {
    let native = app.state::<Native>();
    let mut state = native.state.lock().unwrap();
    if let Listening::Bound(server) = &*state {
        server.stop();
    }
    *state = Listening::Stopped;
}

#[tauri::command]
pub(crate) fn native_engine_status(native: State<Native>) -> NativeStatus {
    match &*native.state.lock().unwrap() {
        Listening::Bound(server) => NativeStatus {
            url: Some(server.url()),
            token: Some(native.engine.token.clone()),
            error: None,
        },
        Listening::Failed(error) => NativeStatus {
            url: None,
            token: None,
            error: Some(error.clone()),
        },
        Listening::Pending | Listening::Stopped => NativeStatus {
            url: None,
            token: None,
            error: None,
        },
    }
}
