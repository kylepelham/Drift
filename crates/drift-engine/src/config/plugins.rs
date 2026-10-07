//! Installing plugins from a registry: the component is fetched and checked, written under the
//! user's config directory, and listed in their drift.json, which is rewritten as JSON.

use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::hook::PluginEntry;

const PLUGINS_DIR: &str = "plugins";
const MAX_COMPONENT_BYTES: usize = 64 * 1024 * 1024;

/// What a registry entry needs to be installed.
#[derive(Clone, Debug, serde::Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Install {
    /// Becomes the file name: letters, digits, `-` and `_` only.
    pub id: String,
    pub url: String,
    /// Hex SHA-256 of the component; the download must match it.
    pub sha256: String,
    #[serde(default)]
    pub config: Value,
}

/// The user's config directory, where plugins and drift.json live.
pub fn config_dir() -> Result<PathBuf, String> {
    super::home().map(|home| home.join(".config/drift")).ok_or_else(|| "no home directory".to_owned())
}

fn valid_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 64 && id.chars().all(|ch| ch.is_ascii_alphanumeric() || ch == '-' || ch == '_')
}

/// Fetches, checks and writes the component; returns its drift.json entry path.
pub async fn fetch_component(http: &reqwest::Client, install: &Install) -> Result<String, String> {
    if !valid_id(&install.id) {
        return Err("a plugin id is letters, digits, dashes and underscores".into());
    }
    if !install.url.starts_with("https://") {
        return Err("a plugin is fetched over https only".into());
    }
    let response = http.get(&install.url).send().await.map_err(|error| format!("could not fetch the plugin: {error}"))?;
    if !response.status().is_success() {
        return Err(format!("could not fetch the plugin: {}", response.status()));
    }
    let bytes = response.bytes().await.map_err(|error| format!("could not fetch the plugin: {error}"))?;
    if bytes.len() > MAX_COMPONENT_BYTES {
        return Err("the plugin is larger than 64 MiB".into());
    }
    let digest = hex(&ring::digest::digest(&ring::digest::SHA256, &bytes));
    if !digest.eq_ignore_ascii_case(install.sha256.trim()) {
        return Err("the download does not match the registry's hash; nothing was installed".into());
    }
    let dir = config_dir()?.join(PLUGINS_DIR);
    std::fs::create_dir_all(&dir).map_err(|error| format!("could not create {}: {error}", dir.display()))?;
    let file = dir.join(format!("{}.wasm", install.id));
    std::fs::write(&file, &bytes).map_err(|error| format!("could not write {}: {error}", file.display()))?;
    Ok(format!("{PLUGINS_DIR}/{}.wasm", install.id))
}

fn hex(digest: &ring::digest::Digest) -> String {
    digest.as_ref().iter().map(|byte| format!("{byte:02x}")).collect()
}

/// drift.json as a JSON object, an empty one when there is no file yet.
fn read_file(path: &Path) -> Result<serde_json::Map<String, Value>, String> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Default::default()),
        Err(error) => return Err(format!("could not read {}: {error}", path.display())),
    };
    match serde_json::from_str::<Value>(&super::jsonc::strip(&text)) {
        Ok(Value::Object(map)) => Ok(map),
        Ok(_) => Err(format!("{} is not a JSON object", path.display())),
        Err(error) => Err(format!("{} could not be parsed: {error}", path.display())),
    }
}

/// Rewrites the `plugins` list with `change` applied; the rest of the file is kept, comments aside.
pub fn edit_plugins(dir: &Path, change: impl FnOnce(&mut Vec<PluginEntry>)) -> Result<(), String> {
    let path = dir.join(super::FILE);
    let mut file = read_file(&path)?;
    let mut plugins: Vec<PluginEntry> = file.get("plugins").cloned().map(serde_json::from_value).transpose().map_err(|error| format!("plugins in {}: {error}", path.display()))?.unwrap_or_default();
    change(&mut plugins);
    file.insert("plugins".into(), serde_json::to_value(&plugins).map_err(|error| error.to_string())?);
    std::fs::create_dir_all(dir).map_err(|error| format!("could not create {}: {error}", dir.display()))?;
    let text = serde_json::to_string_pretty(&Value::Object(file)).map_err(|error| error.to_string())?;
    std::fs::write(&path, format!("{text}\n")).map_err(|error| format!("could not write {}: {error}", path.display()))
}

/// Adds or replaces the entry for `path`, with `config` when it has anything in it.
pub fn set_entry(plugins: &mut Vec<PluginEntry>, path: &str, config: Value) {
    plugins.retain(|entry| entry.path() != path);
    let entry = match config {
        Value::Object(map) if !map.is_empty() => PluginEntry::Configured { path: path.to_owned(), config: Value::Object(map) },
        _ => PluginEntry::Path(path.to_owned()),
    };
    plugins.push(entry);
}

/// Removes the entry and, for a component under the plugins directory, its file.
pub fn remove(dir: &Path, path: &str) -> Result<(), String> {
    edit_plugins(dir, |plugins| plugins.retain(|entry| entry.path() != path))?;
    let relative = Path::new(path);
    let under_plugins = relative.components().count() == 2 && relative.starts_with(PLUGINS_DIR);
    if under_plugins {
        let _ = std::fs::remove_file(dir.join(relative));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn install_and_remove_edit_the_plugins_list_and_keep_the_rest_of_the_file() {
        let dir = std::env::temp_dir().join(format!("drift-plugin-install-{}", crate::random_hex(4)));
        std::fs::create_dir_all(dir.join(PLUGINS_DIR)).unwrap();
        std::fs::write(dir.join(super::super::FILE), "{\n  // mine\n  \"model\": \"anthropic/claude\",\n  \"plugins\": [\"plugins/old.wasm\"]\n}\n").unwrap();
        std::fs::write(dir.join("plugins/guard.wasm"), b"wasm").unwrap();
        edit_plugins(&dir, |plugins| set_entry(plugins, "plugins/guard.wasm", serde_json::json!({ "test": ["cargo", "test"] }))).unwrap();
        let file: Value = serde_json::from_str(&std::fs::read_to_string(dir.join(super::super::FILE)).unwrap()).unwrap();
        assert_eq!(file["model"], "anthropic/claude", "other settings stay");
        assert_eq!(file["plugins"], serde_json::json!(["plugins/old.wasm", { "path": "plugins/guard.wasm", "config": { "test": ["cargo", "test"] } }]));
        edit_plugins(&dir, |plugins| set_entry(plugins, "plugins/guard.wasm", serde_json::json!({}))).unwrap();
        let file: Value = serde_json::from_str(&std::fs::read_to_string(dir.join(super::super::FILE)).unwrap()).unwrap();
        assert_eq!(file["plugins"], serde_json::json!(["plugins/old.wasm", "plugins/guard.wasm"]), "an empty config is a bare path");
        remove(&dir, "plugins/guard.wasm").unwrap();
        let file: Value = serde_json::from_str(&std::fs::read_to_string(dir.join(super::super::FILE)).unwrap()).unwrap();
        assert_eq!(file["plugins"], serde_json::json!(["plugins/old.wasm"]));
        assert!(!dir.join("plugins/guard.wasm").exists(), "the component goes with its entry");
        assert!(!valid_id("../x") && valid_id("git-context"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
