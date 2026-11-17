use super::{FILE, File, LspConfig, ProviderConfig, home, jsonc};
use serde_json::{Map, Value};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

#[derive(Debug, PartialEq, thiserror::Error)]
pub(super) enum PluginPathError {
    #[error("a plugin path must be relative and stay under the config directory")]
    OutsideConfig,
    #[error("a plugin is a .wasm component")]
    NotComponent,
}

/// The providers the user's own drift.json adds or re-points.
pub fn user_providers() -> BTreeMap<String, ProviderConfig> {
    home().map(|home| user_providers_in(&home)).unwrap_or_default()
}

fn user_providers_in(home: &Path) -> BTreeMap<String, ProviderConfig> {
    user_file(home).map(|file| file.providers).unwrap_or_default()
}

fn user_file(home: &Path) -> Option<File> {
    let path = home.join(".config/drift").join(FILE);
    let text = std::fs::read_to_string(path).ok()?;

    serde_json::from_str::<File>(&jsonc::strip(&text)).ok()
}

/// The plugins the user's own drift.json lists; an entry that leaves the directory or is not a `.wasm` carries the error instead of a path.
pub fn user_plugins() -> Vec<crate::hook::Listed> {
    home().map(|home| user_plugins_in(&home)).unwrap_or_default()
}

pub(super) fn user_plugins_in(home: &Path) -> Vec<crate::hook::Listed> {
    let root = home.join(".config/drift");
    let entries = user_file(home).map(|file| file.plugins).unwrap_or_default();

    entries
        .into_iter()
        .map(|entry| crate::hook::Listed {
            entry: entry.path().to_owned(),
            path: plugin_path(&root, entry.path()).map_err(|error| crate::hook::Error::Resolve(error.to_string())),
            config: entry.config(),
        })
        .collect()
}

fn plugin_path(root: &Path, entry: &str) -> Result<PathBuf, PluginPathError> {
    let relative = Path::new(entry);
    if relative
        .components()
        .any(|part| !matches!(part, std::path::Component::Normal(_)))
    {
        return Err(PluginPathError::OutsideConfig);
    }
    if relative
        .extension()
        .is_none_or(|extension| !extension.eq_ignore_ascii_case("wasm"))
    {
        return Err(PluginPathError::NotComponent);
    }

    Ok(root.join(relative))
}

/// The language servers the user's own drift.json turns off or adds.
pub fn user_lsp() -> BTreeMap<String, LspConfig> {
    home()
        .and_then(|home| user_file(&home))
        .map(|file| file.lsp)
        .unwrap_or_default()
}

/// Turns a language server off in the user's drift.json (`false`), or takes that away so it is on again; a server
/// the file adds itself stays as it is.
pub fn set_lsp_enabled(name: &str, enabled: bool) -> Result<(), super::plugins::PluginError> {
    set_lsp_enabled_in(&super::plugins::config_dir()?, name, enabled)
}

fn set_lsp_enabled_in(dir: &Path, name: &str, enabled: bool) -> Result<(), super::plugins::PluginError> {
    super::plugins::edit_file(dir, |_, file| {
        let lsp = file.entry("lsp").or_insert_with(|| Value::Object(Map::new()));
        let Value::Object(lsp) = lsp else {
            return Ok(());
        };
        let off = lsp.get(name) == Some(&Value::Bool(false));
        match (enabled, lsp.contains_key(name)) {
            (true, _) if off => {
                lsp.remove(name);
            }
            (false, false) => {
                lsp.insert(name.to_owned(), Value::Bool(false));
            }
            _ => {}
        }

        if lsp.is_empty() {
            file.remove("lsp");
        }
        Ok(())
    })
}

#[cfg(test)]
mod lsp_tests {
    use super::*;

    #[test]
    fn a_server_turned_off_is_false_in_drift_json_and_on_again_takes_that_away_keeping_the_users_own() {
        let dir = crate::platform::temp::test_run().join(format!("drift-lsp-enabled-{}", crate::random_hex(6)));
        std::fs::create_dir_all(&dir).unwrap();
        let own = r#"{ "model": "m", "lsp": { "mine": { "command": ["mine"], "extensions": [".x"] } } }"#;
        std::fs::write(dir.join(FILE), own).unwrap();
        let read = || serde_json::from_str::<Value>(&std::fs::read_to_string(dir.join(FILE)).unwrap()).unwrap();

        set_lsp_enabled_in(&dir, "gopls", false).unwrap();
        set_lsp_enabled_in(&dir, "mine", false).unwrap();
        assert_eq!(read()["lsp"]["gopls"], false);
        assert_eq!(
            read()["lsp"]["mine"]["command"][0],
            "mine",
            "the user's own server is never overwritten"
        );

        set_lsp_enabled_in(&dir, "gopls", true).unwrap();
        set_lsp_enabled_in(&dir, "mine", true).unwrap();
        assert!(read()["lsp"].get("gopls").is_none());
        assert_eq!(read()["model"], "m");
        let _ = std::fs::remove_dir_all(&dir);
    }
}

/// Saves where a local server listens and the largest window its models run with in the user's drift.json;
/// `None` removes either, so the default address and the full window apply again.
pub fn set_local_server(
    id: &str,
    base_url: Option<&str>,
    context_limit: Option<u64>,
) -> Result<(), super::plugins::PluginError> {
    set_local_server_in(&super::plugins::config_dir()?, id, base_url, context_limit)
}

fn set_local_server_in(
    dir: &Path,
    id: &str,
    base_url: Option<&str>,
    context_limit: Option<u64>,
) -> Result<(), super::plugins::PluginError> {
    super::plugins::edit_file(dir, |_, file| {
        let providers = file.entry("providers").or_insert_with(|| Value::Object(Map::new()));
        let Value::Object(providers) = providers else {
            return Ok(());
        };
        let provider = providers.entry(id).or_insert_with(|| Value::Object(Map::new()));
        let Value::Object(provider) = provider else {
            return Ok(());
        };

        match base_url {
            Some(base) => provider.insert("baseUrl".into(), base.into()),
            None => provider.remove("baseUrl"),
        };
        match context_limit {
            Some(limit) => provider.insert("contextLimit".into(), limit.into()),
            None => provider.remove("contextLimit"),
        };

        // An entry left empty would still read as the user's own provider.
        if provider.is_empty() {
            providers.remove(id);
        }
        if providers.is_empty() {
            file.remove("providers");
        }
        Ok(())
    })
}

#[cfg(test)]
mod local_server_tests {
    use super::*;

    #[test]
    fn a_local_servers_address_and_limit_are_saved_beside_the_rest_and_cleared_away() {
        let dir = crate::platform::temp::test_run().join(format!("drift-local-server-{}", crate::random_hex(6)));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join(FILE),
            r#"{ "theme": "dusk", "providers": { "xai": { "name": "Grok" } } }"#,
        )
        .unwrap();
        let read = || serde_json::from_str::<Value>(&std::fs::read_to_string(dir.join(FILE)).unwrap()).unwrap();

        set_local_server_in(&dir, "ollama", Some("http://192.168.1.20:11434"), Some(65_536)).unwrap();
        assert_eq!(read()["providers"]["ollama"]["baseUrl"], "http://192.168.1.20:11434");
        assert_eq!(read()["providers"]["ollama"]["contextLimit"], 65_536);
        assert_eq!(read()["providers"]["xai"]["name"], "Grok");

        set_local_server_in(&dir, "ollama", None, None).unwrap();
        assert!(
            read()["providers"].get("ollama").is_none(),
            "nothing set leaves no entry"
        );
        assert_eq!(read()["theme"], "dusk");

        let _ = std::fs::remove_dir_all(&dir);
    }
}
