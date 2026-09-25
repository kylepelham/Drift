use crate::{mcp, store::Store};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use tauri::{Emitter, State};

const KEY: &str = "tool_routing_policy";
pub(crate) const POLICY_FILE: &str = "tool-routing.json";
pub(crate) const STATUS_FILE: &str = "tool-routing-status.json";

pub(crate) fn module_path(extensions: &Path) -> Option<PathBuf> {
    ["tool-routing.js", "tool-routing.ts"]
        .into_iter()
        .map(|name| extensions.join(name))
        .find(|path| path.is_file())
}

#[derive(Clone, Default, Deserialize, Serialize, PartialEq, Debug)]
#[serde(deny_unknown_fields)]
pub(crate) struct Policy {
    pub enabled: bool,
}

#[derive(Clone, Deserialize, Serialize, PartialEq, Debug)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Status {
    outcome: String,
    at: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    hidden: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    http_status: Option<u16>,
}

pub(crate) struct ToolRouting {
    file: PathBuf,
    status: PathBuf,
    mutation: Mutex<()>,
}

impl ToolRouting {
    pub(crate) fn new(directory: &Path, store: &Store) -> Result<Self, String> {
        let routing = Self {
            file: directory.join(POLICY_FILE),
            status: directory.join(STATUS_FILE),
            mutation: Mutex::new(()),
        };
        routing.write(&load(store)?)?;
        Ok(routing)
    }

    fn read_status(&self) -> Option<Status> {
        serde_json::from_slice(&std::fs::read(&self.status).ok()?).ok()
    }

    fn write(&self, policy: &Policy) -> Result<(), String> {
        let contents = serde_json::to_vec(policy).map_err(|error| error.to_string())?;
        mcp::write_raw(&self.file, &contents)
    }

    fn update(&self, store: &Store, policy: Policy, publish: impl FnOnce(&Policy)) -> Result<Policy, String> {
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
        let _ = std::fs::remove_file(&self.status);
        publish(&policy);
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
pub(crate) fn tool_routing_status(routing: State<ToolRouting>) -> Option<Status> {
    routing.read_status()
}

#[tauri::command]
pub(crate) fn tool_routing_update(
    app: tauri::AppHandle,
    routing: State<ToolRouting>,
    store: State<Store>,
    policy: Policy,
) -> Result<Policy, String> {
    routing.update(&store, policy, |saved| {
        let _ = app.emit("tool-routing-changed", saved);
    })
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
    fn module_resolution_prefers_bundle_and_supports_source_fallback() {
        let root = directory();
        std::fs::create_dir_all(&root).unwrap();
        assert_eq!(module_path(&root), None);
        let source = root.join("tool-routing.ts");
        let bundle = root.join("tool-routing.js");
        std::fs::write(&source, "export {}").unwrap();
        assert_eq!(module_path(&root), Some(source.clone()));
        std::fs::create_dir(&bundle).unwrap();
        assert_eq!(module_path(&root), Some(source.clone()));
        std::fs::remove_dir(&bundle).unwrap();
        std::fs::write(&bundle, "export {}").unwrap();
        assert_eq!(module_path(&root), Some(bundle.clone()));
        std::fs::remove_file(&bundle).unwrap();
        assert_eq!(module_path(&root), Some(source));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn policy_publication_holds_mutation_lock_and_follows_persistence() {
        let root = directory();
        let store = crate::store::open(&root).unwrap();
        let routing = ToolRouting::new(&root, &store).unwrap();
        let mut published = Vec::new();
        for enabled in [true, false] {
            routing.update(&store, Policy { enabled }, |saved| {
                assert!(matches!(routing.mutation.try_lock(), Err(std::sync::TryLockError::WouldBlock)));
                assert_eq!(load(&store).unwrap(), *saved);
                let materialized: Policy = serde_json::from_slice(&std::fs::read(&routing.file).unwrap()).unwrap();
                assert_eq!(materialized, *saved);
                published.push(saved.enabled);
            }).unwrap();
        }
        assert_eq!(published, vec![true, false]);
        drop(store);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn routing_policy_defaults_off_persists_and_materializes() {
        let root = directory();
        let store = crate::store::open(&root).unwrap();
        let routing = ToolRouting::new(&root, &store).unwrap();
        assert_eq!(load(&store).unwrap(), Policy { enabled: false });
        routing.update(&store, Policy { enabled: true }, |_| {}).unwrap();
        drop(store);
        let store = crate::store::open(&root).unwrap();
        let routing = ToolRouting::new(&root, &store).unwrap();
        let contents: Policy =
            serde_json::from_slice(&std::fs::read(&routing.file).unwrap()).unwrap();
        assert_eq!(contents, Policy { enabled: true });
        routing.update(&store, Policy { enabled: false }, |_| {}).unwrap();
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
        assert!(routing.update(&store, Policy { enabled: true }, |_| panic!("failed updates must not publish")).is_err());
        assert!(!load(&store).unwrap().enabled);
        assert!(serde_json::from_str::<Policy>(r#"{"enabled":"yes"}"#).is_err());
        assert!(
            serde_json::from_str::<Policy>(r#"{"enabled":true,"url":"https://other"}"#).is_err()
        );
        drop(store);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn routing_status_reads_engine_reports_and_clears_on_toggle() {
        let root = directory();
        let store = crate::store::open(&root).unwrap();
        let routing = ToolRouting::new(&root, &store).unwrap();
        assert_eq!(routing.read_status(), None);
        std::fs::write(&routing.status, r#"{"outcome":"insufficient-funds","at":5,"httpStatus":402}"#).unwrap();
        let status = routing.read_status().unwrap();
        assert_eq!(status.outcome, "insufficient-funds");
        assert_eq!(status.http_status, Some(402));
        std::fs::write(&routing.status, "{partial").unwrap();
        assert_eq!(routing.read_status(), None);
        routing.update(&store, Policy { enabled: true }, |_| {}).unwrap();
        assert!(!routing.status.exists());
        drop(store);
        std::fs::remove_dir_all(root).unwrap();
    }
}
