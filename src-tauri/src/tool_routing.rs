use crate::{mcp, store::Store};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use tauri::{Emitter, State};

const KEY: &str = "tool_routing_policy";

#[derive(Clone, Default, Deserialize, Serialize, PartialEq, Debug)]
#[serde(deny_unknown_fields)]
pub(crate) struct Policy {
    pub enabled: bool,
}

pub(crate) struct ToolRouting {
    file: PathBuf,
    mutation: Mutex<()>,
}

impl ToolRouting {
    pub(crate) fn new(directory: &Path, store: &Store) -> Result<Self, String> {
        let routing = Self {
            file: directory.join("tool-routing.json"),
            mutation: Mutex::new(()),
        };
        routing.write(&load(store)?)?;
        Ok(routing)
    }

    fn write(&self, policy: &Policy) -> Result<(), String> {
        let contents = serde_json::to_vec(policy).map_err(|error| error.to_string())?;
        mcp::write_raw(&self.file, &contents)
    }

    fn update(&self, store: &Store, policy: Policy) -> Result<Policy, String> {
        let _lock = self
            .mutation
            .lock()
            .map_err(|_| "Tool routing lock is poisoned")?;
        let previous = load(store)?;
        self.write(&policy)?;
        let encoded = serde_json::to_string(&policy).map_err(|error| error.to_string())?;
        if let Err(error) = store.save_app_setting(KEY, &encoded) {
            self.write(&previous)?;
            return Err(error.to_string());
        }
        Ok(policy)
    }
}

fn load(store: &Store) -> Result<Policy, String> {
    let value = store.app_setting(KEY).map_err(|error| error.to_string())?;
    Ok(value
        .and_then(|value| serde_json::from_str(&value).ok())
        .unwrap_or_default())
}

#[tauri::command]
pub(crate) fn tool_routing_snapshot(store: State<Store>) -> Result<Policy, String> {
    load(&store)
}

#[tauri::command]
pub(crate) fn tool_routing_update(
    app: tauri::AppHandle,
    routing: State<ToolRouting>,
    store: State<Store>,
    policy: Policy,
) -> Result<Policy, String> {
    let policy = routing.update(&store, policy)?;
    let _ = app.emit("tool-routing-changed", &policy);
    Ok(policy)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn directory() -> PathBuf {
        let mut bytes = [0u8; 8];
        getrandom::fill(&mut bytes).unwrap();
        std::env::temp_dir().join(format!("drift-routing-{}", u64::from_ne_bytes(bytes)))
    }

    #[test]
    fn routing_policy_defaults_off_persists_and_materializes() {
        let root = directory();
        let store = crate::store::open(&root).unwrap();
        let routing = ToolRouting::new(&root, &store).unwrap();
        assert_eq!(load(&store).unwrap(), Policy { enabled: false });
        routing.update(&store, Policy { enabled: true }).unwrap();
        drop(store);
        let store = crate::store::open(&root).unwrap();
        let routing = ToolRouting::new(&root, &store).unwrap();
        let contents: Policy =
            serde_json::from_slice(&std::fs::read(&routing.file).unwrap()).unwrap();
        assert_eq!(contents, Policy { enabled: true });
        routing.update(&store, Policy { enabled: false }).unwrap();
        assert!(!load(&store).unwrap().enabled);
        drop(store);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn routing_policy_write_failure_keeps_the_saved_setting() {
        let root = directory();
        let store = crate::store::open(&root).unwrap();
        let routing = ToolRouting::new(&root, &store).unwrap();
        std::fs::remove_file(&routing.file).unwrap();
        std::fs::create_dir(&routing.file).unwrap();
        assert!(routing.update(&store, Policy { enabled: true }).is_err());
        assert!(!load(&store).unwrap().enabled);
        assert!(serde_json::from_str::<Policy>(r#"{"enabled":"yes"}"#).is_err());
        assert!(
            serde_json::from_str::<Policy>(r#"{"enabled":true,"url":"https://other"}"#).is_err()
        );
        drop(store);
        std::fs::remove_dir_all(root).unwrap();
    }
}
