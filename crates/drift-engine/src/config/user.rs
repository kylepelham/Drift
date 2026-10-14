use super::{FILE, File, ProviderConfig, home, jsonc};
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
        let dir = std::env::temp_dir().join(format!("drift-local-server-{}", crate::random_hex(6)));
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
