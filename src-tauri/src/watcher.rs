//! Polls engine configuration and skill files, then invalidates cached instances on change.

use crate::engine::{reload_engine_mcp, stop_engine_instances};
use crate::mcp;
use crate::store::Store;
use std::collections::{hash_map::DefaultHasher, HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use tauri::{Emitter, Manager, State};

/// Config files are polled rather than watched, so the interval bounds how stale a change can be.
const CONFIG_POLL_INTERVAL: std::time::Duration = std::time::Duration::from_secs(1);
const MAX_WATCHED_MCP_FILES: usize = 4096;
const MAX_WATCHED_FILE_BYTES: u64 = 1_048_576;
/// Bounds on `{file:...}` reference expansion inside a config file.
const MAX_FILE_REFERENCES: usize = 256;
const FILE_REFERENCE_PREFIX: &str = "{file:";
const MAX_REFERENCE_PATH_CHARS: usize = 4096;
/// Plugin and skill directories are scanned recursively; this bounds symlink loops.
const MAX_RECURSIVE_SCAN_DEPTH: usize = 16;
const MAX_RECURSIVE_SCAN_DIRECTORIES: usize = 4096;
const MAX_CONFIGURED_SKILL_PATHS: usize = 256;

#[derive(Default)]
pub(crate) struct SkillWatchRoots(Mutex<HashMap<PathBuf, Vec<PathBuf>>>);

impl SkillWatchRoots {
    pub(crate) fn replace(&self, directory: PathBuf, paths: Vec<PathBuf>) {
        self.0.lock().unwrap().insert(directory, paths);
    }

    pub(crate) fn paths(&self, workspaces: Option<&HashSet<PathBuf>>) -> Vec<PathBuf> {
        let mut configured = self.0.lock().unwrap();
        if let Some(workspaces) = workspaces {
            configured.retain(|directory, _| workspaces.contains(directory));
        }
        configured.values().flatten().cloned().collect()
    }
}

#[tauri::command]
pub(crate) fn watcher_set_skill_paths(
    state: State<SkillWatchRoots>,
    directory: String,
    paths: Vec<String>,
) -> Result<(), String> {
    if paths.len() > MAX_CONFIGURED_SKILL_PATHS {
        return Err("Too many configured skill paths".into());
    }
    let roots = paths
        .into_iter()
        .map(|path| resolve_skill_path(&directory, &path))
        .collect::<Result<Vec<_>, _>>()?;
    state.replace(PathBuf::from(directory), roots);
    Ok(())
}

pub(crate) fn resolve_skill_path(directory: &str, value: &str) -> Result<PathBuf, String> {
    let value = value.trim();
    if value.is_empty() || value.len() > MAX_REFERENCE_PATH_CHARS {
        return Err("Configured skill path is invalid".into());
    }
    if let Some(relative) = value.strip_prefix("~/") {
        return std::env::var_os("USERPROFILE")
            .or_else(|| std::env::var_os("HOME"))
            .map(PathBuf::from)
            .ok_or("Home directory is unavailable".into())
            .map(|home| home.join(relative));
    }
    let path = PathBuf::from(value);
    Ok(if path.is_absolute() {
        path
    } else {
        Path::new(directory).join(path)
    })
}

pub(crate) fn watch_engine_configs(app: tauri::AppHandle) {
    std::thread::spawn(move || {
        let mut previous_mcp = external_mcp_signature(&app.state::<Store>());
        let mut previous_skills = external_skill_signature(
            &app.state::<Store>(),
            &app.state::<SkillWatchRoots>(),
        );
        loop {
            std::thread::sleep(CONFIG_POLL_INTERVAL);
            let current_mcp = external_mcp_signature(&app.state::<Store>());
            let current_skills = external_skill_signature(
                &app.state::<Store>(),
                &app.state::<SkillWatchRoots>(),
            );
            let mcp_changed = current_mcp != previous_mcp;
            let skills_changed = current_skills != previous_skills;
            if !mcp_changed && !skills_changed {
                continue;
            }

            if mcp_changed {
                // Reconnecting the servers in place leaves running sessions alone, so this no
                // longer stands in for the disposal a skill change still needs.
                let reloaded = app
                    .state::<mcp::McpRuntime>()
                    .reload(&app.state::<Store>(), || reload_engine_mcp(&app));
                match reloaded {
                    Ok(()) => {
                        previous_mcp = current_mcp;
                        let _ = app.emit("mcp-config-changed", ());
                    }
                    Err(error) => {
                        eprintln!("failed to reload changed MCP configuration: {error}");
                    }
                }
            }

            if skills_changed {
                if let Err(error) = stop_engine_instances(&app) {
                    eprintln!("failed to reload changed skills: {error}");
                    continue;
                }
                previous_skills = current_skills;
                let _ = app.emit("skill-config-changed", ());
            }
        }
    });
}

fn external_skill_signature(
    store: &Store,
    configured: &SkillWatchRoots,
) -> Vec<(PathBuf, u64, u128, u64)> {
    let workspaces = store.workspaces().ok();
    let workspace_paths = workspaces.as_ref().map(|workspaces| {
        workspaces
            .iter()
            .map(|workspace| PathBuf::from(&workspace.path))
            .collect::<HashSet<_>>()
    });
    let mut roots = configured.paths(workspace_paths.as_ref());
    if let Some(home) = std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME")) {
        let home = PathBuf::from(home);
        roots.push(home.join(".claude/skills"));
        roots.push(home.join(".agents/skills"));
        add_config_skill_roots(&home.join(".config/opencode"), &mut roots);
        add_config_skill_roots(&home.join(".opencode"), &mut roots);
    }
    if let Some(root) = std::env::var_os("XDG_CONFIG_HOME") {
        add_config_skill_roots(&PathBuf::from(root).join("opencode"), &mut roots);
    }
    if let Some(root) = std::env::var_os("APPDATA") {
        add_config_skill_roots(&PathBuf::from(root).join("opencode"), &mut roots);
    }
    add_config_skill_roots(&managed_config_root(), &mut roots);
    for workspace in workspaces.into_iter().flatten() {
        for ancestor in Path::new(&workspace.path).ancestors() {
            roots.push(ancestor.join(".claude/skills"));
            roots.push(ancestor.join(".agents/skills"));
            add_config_skill_roots(&ancestor.join(".opencode"), &mut roots);
        }
    }
    file_signatures(watched_skill_paths(roots))
}

fn add_config_skill_roots(config: &Path, roots: &mut Vec<PathBuf>) {
    roots.push(config.join("skill"));
    roots.push(config.join("skills"));
}

pub(crate) fn watched_skill_paths(mut roots: Vec<PathBuf>) -> Vec<PathBuf> {
    roots.sort();
    roots.dedup();
    roots.truncate(MAX_WATCHED_MCP_FILES);
    let mut paths = Vec::new();
    for root in roots {
        collect_skill_files(&root, &mut paths);
        if paths.len() >= MAX_WATCHED_MCP_FILES {
            break;
        }
    }
    paths.sort();
    paths.dedup();
    paths.truncate(MAX_WATCHED_MCP_FILES);
    paths
}

/// Every root the engine can read user MCP config from. Shared with external MCP editing so the
/// set of files Drift will rewrite is exactly the set it watches.
pub(crate) fn external_config_roots(store: &Store) -> Vec<PathBuf> {
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
    roots
}

fn external_mcp_signature(store: &Store) -> Vec<(PathBuf, u64, u128, u64)> {
    let roots = external_config_roots(store);
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
    let mut paths = watched_mcp_paths(configs.clone(), plugins);
    if cfg!(target_os = "macos") {
        let managed = PathBuf::from("/Library/Managed Preferences");
        paths.push(managed.join("ai.opencode.managed.plist"));
        if let Some(user) = std::env::var_os("USER") {
            paths.push(managed.join(user).join("ai.opencode.managed.plist"));
        }
    }
    mcp_signatures(paths, &configs)
}

/// Signs each watched path, comparing config files by their MCP content rather than their bytes.
///
/// A reload disposes every engine instance, which interrupts whatever those sessions are doing, so
/// it must only happen when MCP behaviour actually changed. Config files carry unrelated settings
/// that the user edits far more often than their servers; those edits leave this signature alone.
/// Everything else, including plugin sources and files that will not parse, is still compared
/// whole, because there is no smaller unit that can be trusted.
pub(crate) fn mcp_signatures(
    paths: Vec<PathBuf>,
    configs: &[PathBuf],
) -> Vec<(PathBuf, u64, u128, u64)> {
    let watched = configs.iter().collect::<HashSet<_>>();
    file_signatures(paths)
        .into_iter()
        .map(|(path, len, modified, hash)| {
            if !watched.contains(&path) {
                return (path, len, modified, hash);
            }
            match crate::mcp_external::mcp_content(&path) {
                Some(content) => {
                    let mut hasher = DefaultHasher::new();
                    content.hash(&mut hasher);
                    (path, 0, 0, hasher.finish())
                }
                None => (path, len, modified, hash),
            }
        })
        .collect()
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
        if depth > MAX_RECURSIVE_SCAN_DEPTH {
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

fn collect_skill_files(root: &Path, paths: &mut Vec<PathBuf>) {
    let mut pending = vec![(root.to_path_buf(), 0usize)];
    let mut visited = HashSet::new();
    while let Some((directory, depth)) = pending.pop() {
        if paths.len() >= MAX_WATCHED_MCP_FILES {
            break;
        }
        if depth > MAX_RECURSIVE_SCAN_DEPTH || visited.len() >= MAX_RECURSIVE_SCAN_DIRECTORIES {
            continue;
        }
        let identity = directory.canonicalize().unwrap_or_else(|_| directory.clone());
        if !visited.insert(identity) {
            continue;
        }
        let Ok(entries) = std::fs::read_dir(directory) else {
            continue;
        };
        let mut entries = entries
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .take(MAX_RECURSIVE_SCAN_DIRECTORIES + MAX_WATCHED_MCP_FILES)
            .collect::<Vec<_>>();
        entries.sort_by(|left, right| right.cmp(left));
        for path in entries {
            if path.is_dir() {
                if visited.len() + pending.len() < MAX_RECURSIVE_SCAN_DIRECTORIES {
                    pending.push((path, depth + 1));
                }
            } else if path.is_file() && path.file_name().is_some_and(|name| name == "SKILL.md") {
                paths.push(path);
                if paths.len() >= MAX_WATCHED_MCP_FILES {
                    break;
                }
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
