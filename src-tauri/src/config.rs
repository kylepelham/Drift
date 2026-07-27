//! Sandboxed reads of files under the app config directory.

use std::path::{Component, Path, PathBuf};
use tauri::State;

/// A config file larger than this is almost certainly not one of ours; refuse rather than
/// read an unbounded amount into memory.
const MAX_CONFIG_FILE_BYTES: u64 = 1_048_576;

/// The app config directory. Every `config_read` path is resolved relative to this and must
/// stay inside it.
pub(crate) struct ConfigRoot(pub(crate) PathBuf);

pub(crate) fn config_path(root: &Path, path: &str) -> Result<PathBuf, String> {
    let relative = Path::new(path);
    if relative
        .components()
        .any(|part| !matches!(part, Component::Normal(_)))
    {
        return Err("config path must be relative".into());
    }
    Ok(root.join(relative))
}

#[tauri::command]
pub(crate) fn config_read(config: State<ConfigRoot>, path: String) -> Result<Option<String>, String> {
    let root = config.0.canonicalize().map_err(|e| e.to_string())?;
    let requested = config_path(&root, &path)?;
    if !requested.exists() {
        return Ok(None);
    }
    let requested = requested.canonicalize().map_err(|e| e.to_string())?;
    if !requested.starts_with(&root) {
        return Err("config path escapes Drift's config directory".into());
    }
    if requested.metadata().map_err(|e| e.to_string())?.len() > MAX_CONFIG_FILE_BYTES {
        return Err("config file exceeds 1 MiB".into());
    }
    std::fs::read_to_string(requested)
        .map(Some)
        .map_err(|e| e.to_string())
}
