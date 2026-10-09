use serde::{Deserialize, Serialize};
use std::sync::Mutex;
use tauri::Emitter;

use super::{SHELL_TIMEOUT_KEY, Store, UiStateError, load_valid_setting, validate_timeout};

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ShellTimeoutPolicy {
    pub(crate) timeout_ms: Option<u64>,
}

pub(crate) struct ShellTimeoutAuthority(Mutex<Option<ShellTimeoutPolicy>>);

impl ShellTimeoutAuthority {
    pub(crate) fn load(store: &Store) -> Result<Self, UiStateError> {
        let policy = load_valid_setting(store, SHELL_TIMEOUT_KEY, |policy: &ShellTimeoutPolicy| {
            validate_timeout(policy.timeout_ms)
        })?;

        Ok(Self(Mutex::new(policy)))
    }

    pub(super) fn initialize(
        &self,
        store: &Store,
        policy: ShellTimeoutPolicy,
    ) -> Result<ShellTimeoutPolicy, UiStateError> {
        validate_timeout(policy.timeout_ms)?;

        let encoded = serde_json::to_string(&policy)?;
        let stored = store.initialize_app_setting(SHELL_TIMEOUT_KEY, &encoded)?;
        let current: ShellTimeoutPolicy = serde_json::from_str(&stored)?;
        *self.0.lock().unwrap() = Some(current.clone());

        Ok(current)
    }

    /// The stored policy, if the UI has ever set one.
    pub(crate) fn current(&self) -> Option<ShellTimeoutPolicy> {
        self.0.lock().unwrap().clone()
    }

    pub(super) fn snapshot(&self) -> Result<ShellTimeoutPolicy, UiStateError> {
        self.0
            .lock()
            .unwrap()
            .clone()
            .ok_or(UiStateError::TimeoutNotInitialized)
    }

    pub(super) fn update(&self, store: &Store, policy: ShellTimeoutPolicy) -> Result<ShellTimeoutPolicy, UiStateError> {
        validate_timeout(policy.timeout_ms)?;

        let encoded = serde_json::to_string(&policy)?;
        store.save_app_setting(SHELL_TIMEOUT_KEY, &encoded)?;
        *self.0.lock().unwrap() = Some(policy.clone());

        Ok(policy)
    }
}

#[tauri::command]
pub(crate) fn shell_timeout_initialize(
    app: tauri::AppHandle,
    authority: tauri::State<'_, ShellTimeoutAuthority>,
    store: tauri::State<'_, Store>,
    policy: ShellTimeoutPolicy,
) -> Result<ShellTimeoutPolicy, String> {
    let policy = authority
        .initialize(&store, policy)
        .map_err(|error| error.to_string())?;
    crate::native::push_shell_timeout(&app, policy.timeout_ms);

    Ok(policy)
}

#[tauri::command]
pub(crate) fn shell_timeout_snapshot(
    authority: tauri::State<'_, ShellTimeoutAuthority>,
) -> Result<ShellTimeoutPolicy, String> {
    authority.snapshot().map_err(|error| error.to_string())
}

#[tauri::command]
pub(crate) fn shell_timeout_update(
    app: tauri::AppHandle,
    authority: tauri::State<'_, ShellTimeoutAuthority>,
    store: tauri::State<'_, Store>,
    policy: ShellTimeoutPolicy,
) -> Result<ShellTimeoutPolicy, String> {
    let policy = authority.update(&store, policy).map_err(|error| error.to_string())?;
    crate::native::push_shell_timeout(&app, policy.timeout_ms);
    let _ = app.emit("shell-timeout-changed", &policy);

    Ok(policy)
}
