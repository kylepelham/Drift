//! The in-process Drift engine: opened at setup, served on loopback, handed to the UI by URL.

use std::collections::HashMap;
use std::net::{Ipv4Addr, SocketAddr};
use std::path::Path;
use std::sync::{Arc, Mutex};

use drift_engine::config::AgentOverride;
use drift_engine::{Engine, Server};
use serde::Serialize;
use tauri::{AppHandle, Manager, State};

use crate::store::Store;

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

pub(crate) fn start(app: &AppHandle, data_dir: &Path) -> Result<Arc<Engine>, drift_engine::Error> {
    let engine = Engine::open(data_dir)?;

    app.manage(Native {
        engine: engine.clone(),
        state: Mutex::new(Listening::Pending),
    });
    let started = engine.clone();
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

    Ok(started)
}

/// Hands the engine the per-agent model and prompt choices saved in Settings.
pub(crate) fn push_agent_overrides(app: &AppHandle, store: &Store) -> rusqlite::Result<()> {
    let overrides = store
        .prompt_overrides()?
        .into_iter()
        .filter_map(|item| {
            let name = item.key.strip_prefix("agent:")?.to_string();
            Some((name, AgentOverride::from_json(&item.value)))
        })
        .collect::<HashMap<_, _>>();

    app.state::<Native>().engine.set_agent_overrides(overrides);

    Ok(())
}

/// Hands the engine the shell time limit from Settings; None means commands run until done.
pub(crate) fn push_shell_timeout(app: &AppHandle, timeout_ms: Option<u64>) {
    app.state::<Native>()
        .engine
        .set_shell_timeout(timeout_ms.map(std::time::Duration::from_millis));
}

impl Native {
    pub(crate) fn engine(&self) -> &Arc<Engine> {
        &self.engine
    }
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
