//! Polls MCP-relevant config files and notifies the frontend when any of them change.

use crate::engine::stop_engine_instances;
use crate::mcp;
use crate::store::Store;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::io::Read;
use std::path::{Path, PathBuf};
use tauri::{Emitter, Manager};

/// Config files are polled rather than watched, so the interval bounds how stale a change can be.
const CONFIG_POLL_INTERVAL: std::time::Duration = std::time::Duration::from_secs(1);
const MAX_WATCHED_MCP_FILES: usize = 4096;
const MAX_WATCHED_FILE_BYTES: u64 = 1_048_576;
/// Bounds on `{file:...}` reference expansion inside a config file.
const MAX_FILE_REFERENCES: usize = 256;
const FILE_REFERENCE_PREFIX: &str = "{file:";
const MAX_REFERENCE_PATH_CHARS: usize = 4096;
/// Plugin directories are scanned recursively; this stops a symlink loop from running forever.
const MAX_PLUGIN_SCAN_DEPTH: usize = 16;

pub(crate) fn watch_mcp_configs(app: tauri::AppHandle) {
    std::thread::spawn(move || {
        let mut previous = external_mcp_signature(&app.state::<Store>());
        loop {
            std::thread::sleep(CONFIG_POLL_INTERVAL);
            let current = external_mcp_signature(&app.state::<Store>());
            if current == previous {
                continue;
            }
            let reloaded = app
                .state::<mcp::McpRuntime>()
                .reload(&app.state::<Store>(), || stop_engine_instances(&app));
            match reloaded {
                Ok(()) => {
                    previous = current;
                    let _ = app.emit("mcp-config-changed", ());
                }
                Err(error) => eprintln!("failed to reload changed MCP configuration: {error}"),
            }
        }
    });
}

fn external_mcp_signature(store: &Store) -> Vec<(PathBuf, u64, u128, u64)> {
    let mut roots = Vec::new();
    if let Some(home) = std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME")) {
        let home = PathBuf::from(home);
        roots.push(home.join(".config/opencode"));
        roots.push(home.join(".opencode"));
    }
    if let Some(root) = std::env::var_os("XDG_CONFIG_HOME") {
        roots.push(PathBuf::from(root).join("opencode"));
    }
    if let Some(root) = std::env::var_os("APPDATA") {
        roots.push(PathBuf::from(root).join("opencode"));
    }
    roots.push(managed_config_root());
    if let Ok(workspaces) = store.workspaces() {
        for workspace in workspaces {
            for ancestor in Path::new(&workspace.path).ancestors() {
                roots.push(ancestor.to_path_buf());
                roots.push(ancestor.join(".opencode"));
            }
        }
    }
    roots.sort();
    roots.dedup();
    roots.truncate(MAX_WATCHED_MCP_FILES);
    let configs = roots
        .iter()
        .flat_map(|root| {
            [
                root.join("opencode.json"),
                root.join("opencode.jsonc"),
                root.join("config.json"),
            ]
        })
        .collect::<Vec<_>>();
    let plugins = roots
        .into_iter()
        .flat_map(|root| [root.join("plugin"), root.join("plugins")])
        .collect::<Vec<_>>();
    let mut paths = watched_mcp_paths(configs, plugins);
    if cfg!(target_os = "macos") {
        let managed = PathBuf::from("/Library/Managed Preferences");
        paths.push(managed.join("ai.opencode.managed.plist"));
        if let Some(user) = std::env::var_os("USER") {
            paths.push(managed.join(user).join("ai.opencode.managed.plist"));
        }
    }
    file_signatures(paths)
}

pub(crate) fn watched_mcp_paths(mut configs: Vec<PathBuf>, mut plugin_roots: Vec<PathBuf>) -> Vec<PathBuf> {
    configs.sort();
    configs.dedup();
    configs.truncate(MAX_WATCHED_MCP_FILES);
    plugin_roots.sort();
    plugin_roots.dedup();
    plugin_roots.truncate(MAX_WATCHED_MCP_FILES);
    let mut paths = configs.clone();
    for config in configs {
        let Ok(metadata) = std::fs::metadata(&config) else {
            continue;
        };
        if metadata.len() > MAX_WATCHED_FILE_BYTES {
            continue;
        }
        let Ok(contents) = std::fs::read_to_string(&config) else {
            continue;
        };
        paths.extend(config_file_references(
            &contents,
            config.parent().unwrap_or(Path::new(".")),
        ));
    }
    for root in plugin_roots {
        collect_plugin_files(&root, &mut paths);
        if paths.len() >= MAX_WATCHED_MCP_FILES {
            break;
        }
    }
    paths.sort();
    paths.dedup();
    paths.truncate(MAX_WATCHED_MCP_FILES);
    paths
}

fn config_file_references(contents: &str, parent: &Path) -> Vec<PathBuf> {
    let mut references = Vec::new();
    let mut cursor = 0;
    while references.len() < MAX_FILE_REFERENCES {
        let Some(start) = contents[cursor..]
            .find(FILE_REFERENCE_PREFIX)
            .map(|index| cursor + index + FILE_REFERENCE_PREFIX.len())
        else {
            break;
        };
        let Some(end) = contents[start..].find('}').map(|index| start + index) else {
            break;
        };
        let value = contents[start..end].trim();
        if !value.is_empty() && value.len() <= MAX_REFERENCE_PATH_CHARS {
            let path = if let Some(relative) = value.strip_prefix("~/") {
                std::env::var_os("USERPROFILE")
                    .or_else(|| std::env::var_os("HOME"))
                    .map(PathBuf::from)
                    .unwrap_or_else(|| parent.to_path_buf())
                    .join(relative)
            } else {
                let path = PathBuf::from(value);
                if path.is_absolute() {
                    path
                } else {
                    parent.join(path)
                }
            };
            references.push(path);
        }
        cursor = end + 1;
    }
    references
}

fn collect_plugin_files(root: &Path, paths: &mut Vec<PathBuf>) {
    let mut pending = vec![(root.to_path_buf(), 0usize)];
    while let Some((directory, depth)) = pending.pop() {
        if paths.len() >= MAX_WATCHED_MCP_FILES {
            break;
        }
        if depth > MAX_PLUGIN_SCAN_DEPTH {
            continue;
        }
        let Ok(entries) = std::fs::read_dir(directory) else {
            continue;
        };
        let mut entries = entries
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .collect::<Vec<_>>();
        entries.sort_by(|left, right| right.cmp(left));
        for path in entries {
            if path.is_dir() {
                pending.push((path, depth + 1));
            } else if path.is_file() {
                paths.push(path);
            }
        }
    }
}

pub(crate) fn file_signatures(mut paths: Vec<PathBuf>) -> Vec<(PathBuf, u64, u128, u64)> {
    paths.sort();
    paths.dedup();
    paths.truncate(MAX_WATCHED_MCP_FILES);
    paths
        .into_iter()
        .filter_map(|path| {
            let metadata = std::fs::metadata(&path).ok()?;
            if !metadata.is_file() {
                return None;
            }
            let modified = metadata
                .modified()
                .ok()?
                .duration_since(std::time::UNIX_EPOCH)
                .ok()?
                .as_nanos();
            let mut bytes = Vec::new();
            std::fs::File::open(&path)
                .ok()?
                .take(MAX_WATCHED_FILE_BYTES)
                .read_to_end(&mut bytes)
                .ok()?;
            let mut hasher = DefaultHasher::new();
            bytes.hash(&mut hasher);
            Some((path, metadata.len(), modified, hasher.finish()))
        })
        .collect()
}

fn managed_config_root() -> PathBuf {
    if cfg!(target_os = "windows") {
        return std::env::var_os("ProgramData")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(r"C:\ProgramData"))
            .join("opencode");
    }
    if cfg!(target_os = "macos") {
        return PathBuf::from("/Library/Application Support/opencode");
    }
    PathBuf::from("/etc/opencode")
}
