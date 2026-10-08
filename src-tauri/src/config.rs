use std::path::{Component, Path, PathBuf};
use tauri::State;

// Config reads must not allocate an unbounded buffer.
const MAX_CONFIG_FILE_BYTES: u64 = 1_048_576;

pub(crate) struct ConfigRoot(pub(crate) PathBuf);

#[derive(Debug, thiserror::Error)]
pub(crate) enum ConfigError {
    #[error("config path must be relative")]
    NotRelative,
    #[error("config path escapes Drift's config directory")]
    OutsideRoot,
    #[error("config file exceeds 1 MiB")]
    TooLarge,
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

pub(crate) fn config_path(root: &Path, path: &str) -> Result<PathBuf, ConfigError> {
    let relative = Path::new(path);
    if relative.components().any(|part| !matches!(part, Component::Normal(_))) {
        return Err(ConfigError::NotRelative);
    }

    Ok(root.join(relative))
}

#[tauri::command]
pub(crate) fn config_read(config: State<ConfigRoot>, path: String) -> Result<Option<String>, String> {
    read_config(&config.0, &path).map_err(|error| error.to_string())
}

fn read_config(root: &Path, path: &str) -> Result<Option<String>, ConfigError> {
    let root = root.canonicalize()?;
    let requested = config_path(&root, path)?;
    if !requested.exists() {
        return Ok(None);
    }

    let requested = requested.canonicalize()?;
    if !requested.starts_with(&root) {
        return Err(ConfigError::OutsideRoot);
    }
    if requested.metadata()?.len() > MAX_CONFIG_FILE_BYTES {
        return Err(ConfigError::TooLarge);
    }

    Ok(Some(std::fs::read_to_string(requested)?))
}
