use crate::store::Store;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, VecDeque};
use std::sync::Mutex;
use tauri::Emitter;
use tokio::sync::broadcast;

const UI_STATE_KEY: &str = "ui_mirror_snapshot";
const SHELL_TIMEOUT_KEY: &str = "shell_timeout_policy";
const MAX_DEDUPLICATION_ENTRIES: usize = 256;

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
pub(crate) struct UiMirrorSnapshot {
    pub schema: u8,
    pub revision: u64,
    pub theme: UiTheme,
    pub selection: UiSelection,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct UiStateMutation {
    pub client_id: String,
    pub mutation_id: String,
    pub theme: Option<UiTheme>,
    pub selection: Option<UiSelection>,
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
    pub(crate) fn load(store: &Store) -> Result<Self, String> {
        let snapshot = store
            .app_setting(UI_STATE_KEY)
            .map_err(|error| error.to_string())?
            .map(|value| serde_json::from_str(&value).map_err(|error| error.to_string()))
            .transpose()?;
        if let Some(snapshot) = snapshot.as_ref() {
            validate_snapshot(snapshot)?;
        }
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

    pub(crate) fn snapshot(&self) -> Result<UiMirrorSnapshot, String> {
        self.inner
            .lock()
            .unwrap()
            .snapshot
            .clone()
            .ok_or_else(|| "desktop UI state has not been initialized".into())
    }

    pub(crate) fn subscribe(&self) -> broadcast::Receiver<UiMirrorSnapshot> {
        self.events.subscribe()
    }

    fn initialize(
        &self,
        store: &Store,
        mut snapshot: UiMirrorSnapshot,
    ) -> Result<UiMirrorSnapshot, String> {
        snapshot.schema = 1;
        snapshot.revision = 0;
        validate_snapshot(&snapshot)?;
        validate_workspace(store, &snapshot.selection)?;
        let encoded = serde_json::to_string(&snapshot).map_err(|error| error.to_string())?;
        let stored = store
            .initialize_app_setting(UI_STATE_KEY, &encoded)
            .map_err(|error| error.to_string())?;
        let current: UiMirrorSnapshot =
            serde_json::from_str(&stored).map_err(|error| error.to_string())?;
        validate_snapshot(&current)?;
        self.inner.lock().unwrap().snapshot = Some(current.clone());
        Ok(current)
    }

    fn update(
        &self,
        store: &Store,
        mutation: UiStateMutation,
    ) -> Result<(UiMirrorSnapshot, bool), String> {
        validate_identifier("clientId", &mutation.client_id)?;
        validate_identifier("mutationId", &mutation.mutation_id)?;
        if mutation.theme.is_none() && mutation.selection.is_none() {
            return Err("UI state mutation is empty".into());
        }
        let key = (mutation.client_id, mutation.mutation_id);
        let mut inner = self.inner.lock().unwrap();
        if let Some(snapshot) = inner.deduplicated.get(&key) {
            return Ok((snapshot.clone(), false));
        }
        let mut next = inner
            .snapshot
            .clone()
            .ok_or_else(|| "desktop UI state has not been initialized".to_string())?;
        if let Some(theme) = mutation.theme {
            next.theme = theme;
        }
        if let Some(selection) = mutation.selection {
            next.selection = selection;
        }
        next.revision = next
            .revision
            .checked_add(1)
            .ok_or_else(|| "UI state revision overflow".to_string())?;
        validate_snapshot(&next)?;
        validate_workspace(store, &next.selection)?;
        store
            .save_app_setting(
                UI_STATE_KEY,
                &serde_json::to_string(&next).map_err(|error| error.to_string())?,
            )
            .map_err(|error| error.to_string())?;
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
    authority.initialize(&store, snapshot)
}

#[tauri::command]
pub(crate) fn ui_state_snapshot(
    authority: tauri::State<'_, UiStateAuthority>,
) -> Result<UiMirrorSnapshot, String> {
    authority.snapshot()
}

#[tauri::command]
pub(crate) fn ui_state_update(
    app: tauri::AppHandle,
    authority: tauri::State<'_, UiStateAuthority>,
    store: tauri::State<'_, Store>,
    mutation: UiStateMutation,
) -> Result<UiMirrorSnapshot, String> {
    let (snapshot, changed) = authority.update(&store, mutation)?;
    if changed {
        authority.publish(&app, &snapshot);
    }
    Ok(snapshot)
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ShellTimeoutPolicy {
    pub timeout_ms: Option<u64>,
}

pub(crate) struct ShellTimeoutAuthority(Mutex<Option<ShellTimeoutPolicy>>);

impl ShellTimeoutAuthority {
    pub(crate) fn load(store: &Store) -> Result<Self, String> {
        let policy: Option<ShellTimeoutPolicy> = store
            .app_setting(SHELL_TIMEOUT_KEY)
            .map_err(|error| error.to_string())?
            .map(|value| serde_json::from_str(&value).map_err(|error| error.to_string()))
            .transpose()?;
        if let Some(policy) = policy.as_ref() {
            validate_timeout(policy.timeout_ms)?;
        }
        Ok(Self(Mutex::new(policy)))
    }

    fn initialize(
        &self,
        store: &Store,
        policy: ShellTimeoutPolicy,
    ) -> Result<ShellTimeoutPolicy, String> {
        validate_timeout(policy.timeout_ms)?;
        let stored = store
            .initialize_app_setting(
                SHELL_TIMEOUT_KEY,
                &serde_json::to_string(&policy).map_err(|error| error.to_string())?,
            )
            .map_err(|error| error.to_string())?;
        let current: ShellTimeoutPolicy =
            serde_json::from_str(&stored).map_err(|error| error.to_string())?;
        *self.0.lock().unwrap() = Some(current.clone());
        Ok(current)
    }

    fn snapshot(&self) -> Result<ShellTimeoutPolicy, String> {
        self.0
            .lock()
            .unwrap()
            .clone()
            .ok_or_else(|| "shell timeout policy has not been initialized".into())
    }

    fn update(
        &self,
        store: &Store,
        policy: ShellTimeoutPolicy,
    ) -> Result<ShellTimeoutPolicy, String> {
        validate_timeout(policy.timeout_ms)?;
        store
            .save_app_setting(
                SHELL_TIMEOUT_KEY,
                &serde_json::to_string(&policy).map_err(|error| error.to_string())?,
            )
            .map_err(|error| error.to_string())?;
        *self.0.lock().unwrap() = Some(policy.clone());
        Ok(policy)
    }
}

#[tauri::command]
pub(crate) fn shell_timeout_initialize(
    authority: tauri::State<'_, ShellTimeoutAuthority>,
    store: tauri::State<'_, Store>,
    policy: ShellTimeoutPolicy,
) -> Result<ShellTimeoutPolicy, String> {
    authority.initialize(&store, policy)
}

#[tauri::command]
pub(crate) fn shell_timeout_snapshot(
    authority: tauri::State<'_, ShellTimeoutAuthority>,
) -> Result<ShellTimeoutPolicy, String> {
    authority.snapshot()
}

#[tauri::command]
pub(crate) fn shell_timeout_update(
    app: tauri::AppHandle,
    authority: tauri::State<'_, ShellTimeoutAuthority>,
    store: tauri::State<'_, Store>,
    policy: ShellTimeoutPolicy,
) -> Result<ShellTimeoutPolicy, String> {
    let policy = authority.update(&store, policy)?;
    let _ = app.emit("shell-timeout-changed", &policy);
    Ok(policy)
}

fn validate_snapshot(snapshot: &UiMirrorSnapshot) -> Result<(), String> {
    if snapshot.schema != 1 {
        return Err("unsupported UI state schema".into());
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
        return Err("invalid theme name".into());
    }
    for (name, color) in [
        ("background", &snapshot.theme.custom.background),
        ("surface", &snapshot.theme.custom.surface),
        ("text", &snapshot.theme.custom.text),
        ("accent", &snapshot.theme.custom.accent),
    ] {
        if color.len() != 7
            || !color.starts_with('#')
            || !color[1..].bytes().all(|byte| byte.is_ascii_hexdigit())
        {
            return Err(format!("invalid custom theme {name} color"));
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
        return Err("sessionId requires workspaceId".into());
    }
    Ok(())
}

fn validate_workspace(store: &Store, selection: &UiSelection) -> Result<(), String> {
    if let Some(id) = selection.workspace_id.as_deref() {
        if !store
            .workspace_exists(id)
            .map_err(|error| error.to_string())?
        {
            return Err("selected workspace does not exist".into());
        }
    }
    Ok(())
}

fn validate_identifier(name: &str, value: &str) -> Result<(), String> {
    if value.is_empty() || value.len() > 256 || value.chars().any(char::is_control) {
        return Err(format!("invalid {name}"));
    }
    Ok(())
}

fn validate_text(name: &str, value: &str, max: usize) -> Result<(), String> {
    if value.len() > max {
        return Err(format!("{name} is too long"));
    }
    Ok(())
}

fn validate_timeout(timeout: Option<u64>) -> Result<(), String> {
    if timeout.is_some_and(|value| !(60_000..=86_400_000).contains(&value)) {
        return Err("shell timeout must be null or between 1 and 1,440 minutes".into());
    }
    Ok(())
}

#[cfg(test)]
#[path = "ui_state_tests.rs"]
mod tests;
