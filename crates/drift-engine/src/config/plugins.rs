//! Installing registry plugins: fetched, hash-checked, written under the user's config and listed in their drift.json.

use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::hook::PluginEntry;

const PLUGINS_DIR: &str = "plugins";
const MAX_COMPONENT_BYTES: usize = 64 * 1024 * 1024;

#[derive(Debug, thiserror::Error)]
pub enum PluginError {
    #[error("no home directory")]
    NoHome,
    #[error("a plugin id is letters, digits, dashes and underscores")]
    InvalidId,
    #[error("a plugin is fetched over https only")]
    RequiresHttps,
    #[error("the download does not match the registry's hash; nothing was installed")]
    HashMismatch,
    #[error("{0}")]
    Fetch(String),
    #[error("could not {operation} {}: {source}", path.display())]
    Io {
        operation: &'static str,
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("{} is not a JSON object", .0.display())]
    NotObject(PathBuf),
    #[error("{} could not be parsed: {source}", path.display())]
    Parse { path: PathBuf, source: serde_json::Error },
    #[error("plugins in {}: {source}", path.display())]
    Entries { path: PathBuf, source: serde_json::Error },
    #[error(transparent)]
    Json(#[from] serde_json::Error),
}

impl From<String> for PluginError {
    fn from(message: String) -> Self {
        Self::Fetch(message)
    }
}

fn file_error(operation: &'static str, path: &Path, source: std::io::Error) -> PluginError {
    PluginError::Io {
        operation,
        path: path.to_path_buf(),
        source,
    }
}

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
    /// The registry source it was listed by, whose token and trust apply to the download; none for Drift's own.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub registry: Option<String>,
}

/// The user's config directory, where plugins and drift.json live.
pub fn config_dir() -> Result<PathBuf, PluginError> {
    super::home()
        .map(|home| home.join(".config/drift"))
        .ok_or(PluginError::NoHome)
}

fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '-' || ch == '_')
}

/// Fetches, checks and writes the component; returns its drift.json entry path. With a source, the
/// download goes through it: a path inside its repository or folder, or a URL with its token on its host.
pub async fn fetch_component(
    fetcher: &super::sources::Fetcher,
    source: Option<&super::sources::RegistrySource>,
    install: &Install,
) -> Result<String, PluginError> {
    if !valid_id(&install.id) {
        return Err(PluginError::InvalidId);
    }
    let bytes = match source {
        Some(source) => {
            let token = fetcher.token(source);
            fetcher
                .read(
                    source,
                    source.file(&install.url, token.as_deref())?,
                    MAX_COMPONENT_BYTES,
                )
                .await?
        }
        None => {
            if !install.url.starts_with("https://") {
                return Err(PluginError::RequiresHttps);
            }
            let drift = super::sources::RegistrySource {
                id: String::new(),
                name: "Drift".into(),
                kind: super::sources::RegistryKind::Plugins,
                source: Default::default(),
                url: install.url.clone(),
                r#ref: String::new(),
                path: String::new(),
                has_token: false,
                allow_http: false,
                ca_pem: None,
            };
            fetcher
                .read(
                    &drift,
                    super::sources::Location::Http {
                        url: install.url.clone(),
                        headers: Vec::new(),
                    },
                    MAX_COMPONENT_BYTES,
                )
                .await?
        }
    };
    let digest = hex(&ring::digest::digest(&ring::digest::SHA256, &bytes));
    if !digest.eq_ignore_ascii_case(install.sha256.trim()) {
        return Err(PluginError::HashMismatch);
    }
    let dir = config_dir()?.join(PLUGINS_DIR);
    std::fs::create_dir_all(&dir).map_err(|error| file_error("create", &dir, error))?;
    let file = dir.join(format!("{}.wasm", install.id));
    std::fs::write(&file, &bytes).map_err(|error| file_error("write", &file, error))?;
    Ok(format!("{PLUGINS_DIR}/{}.wasm", install.id))
}

fn hex(digest: &ring::digest::Digest) -> String {
    crate::hex_bytes(digest.as_ref())
}

/// drift.json as a JSON object, an empty one when there is no file yet.
fn read_file(path: &Path) -> Result<serde_json::Map<String, Value>, PluginError> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Default::default()),
        Err(error) => return Err(file_error("read", path, error)),
    };
    match serde_json::from_str::<Value>(&super::jsonc::strip(&text)) {
        Ok(Value::Object(map)) => Ok(map),
        Ok(_) => Err(PluginError::NotObject(path.to_path_buf())),
        Err(source) => Err(PluginError::Parse {
            path: path.to_path_buf(),
            source,
        }),
    }
}

/// Rewrites the `plugins` list with `change` applied; the rest of the file is kept, comments aside.
pub fn edit_plugins(dir: &Path, change: impl FnOnce(&mut Vec<PluginEntry>)) -> Result<(), PluginError> {
    let path = dir.join(super::FILE);
    let mut file = read_file(&path)?;
    let mut plugins: Vec<PluginEntry> = file
        .get("plugins")
        .cloned()
        .map(serde_json::from_value)
        .transpose()
        .map_err(|source| PluginError::Entries {
            path: path.clone(),
            source,
        })?
        .unwrap_or_default();
    change(&mut plugins);
    file.insert("plugins".into(), serde_json::to_value(&plugins)?);
    std::fs::create_dir_all(dir).map_err(|error| file_error("create", dir, error))?;
    let text = serde_json::to_string_pretty(&Value::Object(file))?;

    std::fs::write(&path, format!("{text}\n")).map_err(|error| file_error("write", &path, error))
}

/// Adds or replaces the entry for `path`, with `config` when it has anything in it.
pub fn set_entry(plugins: &mut Vec<PluginEntry>, path: &str, config: Value) {
    plugins.retain(|entry| entry.path() != path);
    let entry = match config {
        Value::Object(map) if !map.is_empty() => PluginEntry::Configured {
            path: path.to_owned(),
            config: Value::Object(map),
        },
        _ => PluginEntry::Path(path.to_owned()),
    };
    plugins.push(entry);
}

/// Removes the entry and, for a component under the plugins directory, its file.
pub fn remove(dir: &Path, path: &str) -> Result<(), PluginError> {
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
        std::fs::write(
            dir.join(super::super::FILE),
            "{\n  // mine\n  \"model\": \"anthropic/claude\",\n  \"plugins\": [\"plugins/old.wasm\"]\n}\n",
        )
        .unwrap();
        std::fs::write(dir.join("plugins/guard.wasm"), b"wasm").unwrap();
        edit_plugins(&dir, |plugins| {
            set_entry(
                plugins,
                "plugins/guard.wasm",
                serde_json::json!({ "test": ["cargo", "test"] }),
            );
        })
        .unwrap();
        let file: Value =
            serde_json::from_str(&std::fs::read_to_string(dir.join(super::super::FILE)).unwrap()).unwrap();
        assert_eq!(file["model"], "anthropic/claude", "other settings stay");
        assert_eq!(
            file["plugins"],
            serde_json::json!(["plugins/old.wasm", { "path": "plugins/guard.wasm", "config": { "test": ["cargo", "test"] } }])
        );
        edit_plugins(&dir, |plugins| {
            set_entry(plugins, "plugins/guard.wasm", serde_json::json!({}));
        })
        .unwrap();
        let file: Value =
            serde_json::from_str(&std::fs::read_to_string(dir.join(super::super::FILE)).unwrap()).unwrap();
        assert_eq!(
            file["plugins"],
            serde_json::json!(["plugins/old.wasm", "plugins/guard.wasm"]),
            "an empty config is a bare path"
        );
        remove(&dir, "plugins/guard.wasm").unwrap();
        let file: Value =
            serde_json::from_str(&std::fs::read_to_string(dir.join(super::super::FILE)).unwrap()).unwrap();
        assert_eq!(file["plugins"], serde_json::json!(["plugins/old.wasm"]));
        assert!(
            !dir.join("plugins/guard.wasm").exists(),
            "the component goes with its entry"
        );
        assert!(!valid_id("../x") && valid_id("git-context"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
