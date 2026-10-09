pub(crate) mod timeout;

pub(crate) use timeout::ShellTimeoutAuthority;
#[cfg(test)]
use timeout::ShellTimeoutPolicy;

use crate::store::Store;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, VecDeque};
use std::sync::Mutex;
use tauri::Emitter;
use tokio::sync::broadcast;

const UI_STATE_KEY: &str = "ui_mirror_snapshot";
const SHELL_TIMEOUT_KEY: &str = "shell_timeout_policy";
const MAX_DEDUPLICATION_ENTRIES: usize = 256;

#[derive(Debug, thiserror::Error)]
pub(crate) enum UiStateError {
    #[error("desktop UI state has not been initialized")]
    NotInitialized,
    #[error("shell timeout policy has not been initialized")]
    TimeoutNotInitialized,
    #[error("UI state mutation is empty")]
    EmptyMutation,
    #[error("UI state revision overflow")]
    RevisionOverflow,
    #[error("unsupported UI state schema")]
    UnsupportedSchema,
    #[error("invalid theme name")]
    InvalidTheme,
    #[error("invalid custom theme {0} color")]
    InvalidColor(&'static str),
    #[error("sessionId requires workspaceId")]
    SessionWithoutWorkspace,
    #[error("workspace order is too long")]
    OrderTooLong,
    #[error("invalid {0}")]
    InvalidIdentifier(&'static str),
    #[error("{0} is too long")]
    TextTooLong(&'static str),
    #[error("shell timeout must be null or between 1 and 1,440 minutes")]
    InvalidTimeout,
    #[error(transparent)]
    Database(#[from] rusqlite::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct UiTheme {
    pub name: String,
    pub custom: CustomTheme,
    pub ui_font: String,
    pub code_font: String,
    pub custom_css: String,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub(crate) struct CustomTheme {
    pub background: String,
    pub surface: String,
    pub text: String,
    pub accent: String,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct UiSelection {
    pub workspace_id: Option<String>,
    pub session_id: Option<String>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct UiMirrorSnapshot {
    pub schema: u8,
    pub revision: u64,
    pub theme: UiTheme,
    pub selection: UiSelection,
    #[serde(default)]
    pub workspace_order: Vec<String>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct UiStateMutation {
    pub client_id: String,
    pub mutation_id: String,
    pub theme: Option<UiTheme>,
    pub selection: Option<UiSelection>,
    #[serde(default)]
    pub workspace_order: Option<Vec<String>>,
}

struct UiStateInner {
    snapshot: Option<UiMirrorSnapshot>,
    deduplicated: HashMap<(String, String), UiMirrorSnapshot>,
    order: VecDeque<(String, String)>,
}

pub(crate) struct UiStateAuthority {
    inner: Mutex<UiStateInner>,
    events: broadcast::Sender<UiMirrorSnapshot>,
}

impl UiStateAuthority {
    pub(crate) fn load(store: &Store) -> Result<Self, UiStateError> {
        let snapshot = load_valid_setting(store, UI_STATE_KEY, validate_snapshot)?;
        let (events, _) = broadcast::channel(32);

        Ok(Self {
            inner: Mutex::new(UiStateInner {
                snapshot,
                deduplicated: HashMap::new(),
                order: VecDeque::new(),
            }),
            events,
        })
    }

    pub(crate) fn snapshot(&self) -> Result<UiMirrorSnapshot, UiStateError> {
        self.inner
            .lock()
            .unwrap()
            .snapshot
            .clone()
            .ok_or(UiStateError::NotInitialized)
    }

    pub(crate) fn subscribe(&self) -> broadcast::Receiver<UiMirrorSnapshot> {
        self.events.subscribe()
    }

    fn initialize(&self, store: &Store, mut snapshot: UiMirrorSnapshot) -> Result<UiMirrorSnapshot, UiStateError> {
        snapshot.schema = 1;
        snapshot.revision = 0;
        validate_snapshot(&snapshot)?;

        let encoded = serde_json::to_string(&snapshot)?;
        let stored = store.initialize_app_setting(UI_STATE_KEY, &encoded)?;
        let current: UiMirrorSnapshot = serde_json::from_str(&stored)?;
        validate_snapshot(&current)?;

        self.inner.lock().unwrap().snapshot = Some(current.clone());

        Ok(current)
    }

    fn update(&self, store: &Store, mutation: UiStateMutation) -> Result<(UiMirrorSnapshot, bool), UiStateError> {
        validate_identifier("clientId", &mutation.client_id)?;
        validate_identifier("mutationId", &mutation.mutation_id)?;
        if mutation.theme.is_none() && mutation.selection.is_none() && mutation.workspace_order.is_none() {
            return Err(UiStateError::EmptyMutation);
        }

        let key = (mutation.client_id, mutation.mutation_id);
        let mut inner = self.inner.lock().unwrap();
        if let Some(snapshot) = inner.deduplicated.get(&key) {
            return Ok((snapshot.clone(), false));
        }

        let mut next = inner.snapshot.clone().ok_or(UiStateError::NotInitialized)?;
        if let Some(theme) = mutation.theme {
            next.theme = theme;
        }
        if let Some(selection) = mutation.selection {
            next.selection = selection;
        }
        if let Some(order) = mutation.workspace_order {
            next.workspace_order = order;
        }
        next.revision = next.revision.checked_add(1).ok_or(UiStateError::RevisionOverflow)?;
        validate_snapshot(&next)?;

        let encoded = serde_json::to_string(&next)?;
        store.save_app_setting(UI_STATE_KEY, &encoded)?;

        inner.snapshot = Some(next.clone());
        inner.deduplicated.insert(key.clone(), next.clone());
        inner.order.push_back(key);
        while inner.order.len() > MAX_DEDUPLICATION_ENTRIES {
            if let Some(oldest) = inner.order.pop_front() {
                inner.deduplicated.remove(&oldest);
            }
        }

        Ok((next, true))
    }

    fn publish(&self, app: &tauri::AppHandle, snapshot: &UiMirrorSnapshot) {
        let _ = self.events.send(snapshot.clone());
        let _ = app.emit("ui-state-changed", snapshot);
    }
}

#[tauri::command]
pub(crate) fn ui_state_initialize(
    authority: tauri::State<'_, UiStateAuthority>,
    store: tauri::State<'_, Store>,
    snapshot: UiMirrorSnapshot,
) -> Result<UiMirrorSnapshot, String> {
    authority
        .initialize(&store, snapshot)
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub(crate) fn ui_state_snapshot(authority: tauri::State<'_, UiStateAuthority>) -> Result<UiMirrorSnapshot, String> {
    authority.snapshot().map_err(|error| error.to_string())
}

#[tauri::command]
pub(crate) fn ui_state_update(
    app: tauri::AppHandle,
    authority: tauri::State<'_, UiStateAuthority>,
    store: tauri::State<'_, Store>,
    mutation: UiStateMutation,
) -> Result<UiMirrorSnapshot, String> {
    let (snapshot, changed) = authority.update(&store, mutation).map_err(|error| error.to_string())?;
    if changed {
        authority.publish(&app, &snapshot);
    }

    Ok(snapshot)
}

fn load_valid_setting<T: DeserializeOwned>(
    store: &Store,
    key: &str,
    validate: impl FnOnce(&T) -> Result<(), UiStateError>,
) -> Result<Option<T>, UiStateError> {
    let Some(value) = store.app_setting(key)? else {
        return Ok(None);
    };

    let parsed = serde_json::from_str(&value)
        .map_err(UiStateError::from)
        .and_then(|value| validate(&value).map(|()| value));

    match parsed {
        Ok(value) => Ok(Some(value)),
        Err(_) => {
            store.delete_app_setting(key)?;
            Ok(None)
        }
    }
}

fn validate_snapshot(snapshot: &UiMirrorSnapshot) -> Result<(), UiStateError> {
    if snapshot.schema != 1 {
        return Err(UiStateError::UnsupportedSchema);
    }

    if !matches!(
        snapshot.theme.name.as_str(),
        "drift-dark"
            | "drift-graphite"
            | "drift-midnight"
            | "drift-slate"
            | "drift-forest"
            | "drift-aubergine"
            | "drift-light"
            | "drift-paper"
            | "drift-custom"
    ) {
        return Err(UiStateError::InvalidTheme);
    }

    for (name, color) in [
        ("background", &snapshot.theme.custom.background),
        ("surface", &snapshot.theme.custom.surface),
        ("text", &snapshot.theme.custom.text),
        ("accent", &snapshot.theme.custom.accent),
    ] {
        if color.len() != 7 || !color.starts_with('#') || !color[1..].bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err(UiStateError::InvalidColor(name));
        }
    }

    validate_text("UI font", &snapshot.theme.ui_font, 256)?;
    validate_text("code font", &snapshot.theme.code_font, 256)?;
    validate_text("custom CSS", &snapshot.theme.custom_css, 20_000)?;

    if let Some(id) = snapshot.selection.workspace_id.as_deref() {
        validate_identifier("workspaceId", id)?;
    }
    if let Some(id) = snapshot.selection.session_id.as_deref() {
        validate_identifier("sessionId", id)?;
    }
    if snapshot.selection.workspace_id.is_none() && snapshot.selection.session_id.is_some() {
        return Err(UiStateError::SessionWithoutWorkspace);
    }

    if snapshot.workspace_order.len() > 500 {
        return Err(UiStateError::OrderTooLong);
    }
    for id in &snapshot.workspace_order {
        validate_identifier("workspaceId", id)?;
    }

    Ok(())
}

fn validate_identifier(name: &'static str, value: &str) -> Result<(), UiStateError> {
    if value.is_empty() || value.chars().count() > 256 || value.chars().any(char::is_control) {
        return Err(UiStateError::InvalidIdentifier(name));
    }
    Ok(())
}

fn validate_text(name: &'static str, value: &str, max: usize) -> Result<(), UiStateError> {
    if value.chars().count() > max {
        return Err(UiStateError::TextTooLong(name));
    }
    Ok(())
}

fn validate_timeout(timeout: Option<u64>) -> Result<(), UiStateError> {
    if timeout.is_some_and(|value| !(60_000..=86_400_000).contains(&value)) {
        return Err(UiStateError::InvalidTimeout);
    }
    Ok(())
}

#[cfg(test)]
#[path = "ui_state_tests.rs"]
mod tests;
