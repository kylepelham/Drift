//! What a bundle's folder holds, read as Claude Code lays a plugin out: `skills/`, `commands/`, `agents/`,
//! `.mcp.json`, `bin/` and `hooks/hooks.json`, plus the paths `.claude-plugin/plugin.json` adds.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde_json::Value;

use super::super::{archive, frontmatter, skill_files};

const MANIFEST: &str = ".claude-plugin/plugin.json";

/// A skill, command or agent: its name, its description and the file (a skill's folder) it lives in.
#[derive(Clone, Debug, PartialEq)]
pub struct Item {
    pub name: String,
    pub description: String,
    pub path: PathBuf,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Layout {
    pub skills: Vec<Item>,
    pub commands: Vec<Item>,
    pub agents: Vec<Item>,
    /// MCP servers as `.mcp.json` declares them, by name.
    pub servers: BTreeMap<String, Value>,
    /// The programs in `bin/`, by file name.
    pub programs: Vec<String>,
    /// The events its hooks run on; Drift lists them and never runs them.
    pub hooks: Vec<String>,
}

impl Layout {
    pub fn read(root: &Path) -> Self {
        let manifest = read_json(&root.join(MANIFEST)).unwrap_or(Value::Null);
        let listed = |key: &str, default: &str| {
            let mut paths = vec![root.join(default)];
            paths.extend(manifest_paths(root, &manifest[key]));
            paths
        };

        let lone_skill = !root.join("skills").is_dir() && root.join("SKILL.md").is_file();
        let mut skills = skills(&listed("skills", "skills"));
        if lone_skill && let Some(skill) = item(&root.join("SKILL.md"), root) {
            skills.insert(0, skill);
        }

        Self {
            skills,
            commands: markdown(&listed("commands", "commands")),
            agents: markdown(&listed("agents", "agents")),
            servers: servers(root, &manifest),
            programs: programs(&root.join("bin")),
            hooks: hooks(root, &manifest),
        }
    }
}

fn read_json(path: &Path) -> Option<Value> {
    serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()
}

/// The paths a manifest key names, one or a list, each inside the bundle.
fn manifest_paths(root: &Path, value: &Value) -> Vec<PathBuf> {
    let listed: Vec<&str> = match value {
        Value::String(path) => vec![path],
        Value::Array(paths) => paths.iter().filter_map(Value::as_str).collect(),
        _ => Vec::new(),
    };

    listed.into_iter().filter_map(|path| inside(root, path)).collect()
}

/// `./x` or `x` under the bundle; nothing that climbs out of it.
fn inside(root: &Path, path: &str) -> Option<PathBuf> {
    let relative = Path::new(path.trim_start_matches("./").trim_end_matches('/'));
    archive::safe(relative).then(|| root.join(relative))
}

fn skills(dirs: &[PathBuf]) -> Vec<Item> {
    let mut found: Vec<Item> = Vec::new();
    for file in dirs.iter().flat_map(|dir| skill_files(dir)) {
        let Some(item) = item(&file, file.parent().unwrap_or(&file)) else {
            continue;
        };
        if !found.iter().any(|known| known.name == item.name) {
            found.push(item);
        }
    }

    found
}

/// Markdown files directly in each folder, or a file named on its own; a name found first wins.
fn markdown(paths: &[PathBuf]) -> Vec<Item> {
    let mut files: Vec<PathBuf> = Vec::new();
    for path in paths {
        if path.is_file() {
            files.push(path.clone());
            continue;
        }
        let Ok(entries) = std::fs::read_dir(path) else { continue };
        let mut inner: Vec<PathBuf> = entries
            .flatten()
            .map(|entry| entry.path())
            .filter(|file| file.is_file() && file.extension().is_some_and(|extension| extension == "md"))
            .collect();
        inner.sort();
        files.extend(inner);
    }

    let mut found: Vec<Item> = Vec::new();
    for file in files {
        let Some(item) = item(&file, &file) else { continue };
        if !found.iter().any(|known| known.name == item.name) {
            found.push(item);
        }
    }

    found
}

/// Named by its front matter, else by its file (a skill by its folder).
fn item(file: &Path, path: &Path) -> Option<Item> {
    let text = std::fs::read_to_string(file).ok()?;
    let document = frontmatter::parse(&text);
    let fallback = if file == path {
        path.file_stem()
    } else {
        path.file_name()
    };
    let name = document
        .field("name")
        .filter(|name| !name.trim().is_empty())
        .or_else(|| fallback.map(|name| name.to_string_lossy().into_owned()))?;

    Some(Item {
        name,
        description: document.field("description").unwrap_or_default(),
        path: path.to_path_buf(),
    })
}

/// `.mcp.json` (with or without its `mcpServers` wrapper), then the manifest's own, inline or by path.
fn servers(root: &Path, manifest: &Value) -> BTreeMap<String, Value> {
    let mut found = BTreeMap::new();
    let mut add = |value: Option<Value>| {
        let Some(value) = value else { return };
        let servers = value.get("mcpServers").cloned().unwrap_or(value);
        if let Value::Object(servers) = servers {
            for (name, server) in servers {
                found.entry(name).or_insert(server);
            }
        }
    };

    add(read_json(&root.join(".mcp.json")));
    match &manifest["mcpServers"] {
        Value::String(path) => add(inside(root, path).and_then(|path| read_json(&path))),
        inline @ Value::Object(_) => add(Some(inline.clone())),
        _ => {}
    }

    found
}

fn programs(bin: &Path) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(bin) else {
        return Vec::new();
    };
    let mut names: Vec<String> = entries
        .flatten()
        .filter(|entry| entry.path().is_file())
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();

    names
}

fn hooks(root: &Path, manifest: &Value) -> Vec<String> {
    let declared = match &manifest["hooks"] {
        Value::String(path) => inside(root, path).and_then(|path| read_json(&path)),
        inline @ Value::Object(_) => Some(inline.clone()),
        _ => None,
    };
    let mut events: Vec<String> = [read_json(&root.join("hooks/hooks.json")), declared]
        .into_iter()
        .flatten()
        .filter_map(|file| file.get("hooks").cloned())
        .filter_map(|hooks| {
            hooks
                .as_object()
                .map(|events| events.keys().cloned().collect::<Vec<_>>())
        })
        .flatten()
        .collect();
    events.sort();
    events.dedup();

    events
}
