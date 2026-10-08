use super::{FILE, File, ProviderConfig, home, jsonc};
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
