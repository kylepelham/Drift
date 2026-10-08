use super::{Left, OcServer};
use drift_engine::mcp::{OAuthClient, ServerConfig};
use drift_engine::store::Store;
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

#[derive(Debug, PartialEq, thiserror::Error)]
pub enum McpConfigError {
    #[error("it has no command")]
    MissingCommand,
    #[error("it has no url")]
    MissingUrl,
    #[error("it is neither a local nor a remote server")]
    UnsupportedType,
    #[error("the environment variable {name} is not set")]
    MissingEnvironment { name: String },
    #[error("{} could not be read", path.display())]
    UnreadableFile { path: PathBuf },
}

/// Saves the imported server and returns whether it was left enabled.
pub(super) fn save(store: &Store, server: &OcServer, config_dir: &Path) -> Result<bool, Left> {
    let failed = |error: rusqlite::Error| Left::Out(error.to_string());
    if store.mcp_server(&server.name).map_err(failed)?.is_some()
        || store.unreadable_mcp_servers().map_err(failed)?.contains(&server.name)
    {
        return Err(Left::Kept("Drift already has a server by that name, kept".into()));
    }

    if server.name.is_empty()
        || !server
            .name
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '-' | '_'))
    {
        return Err(Left::Out(
            "its name has characters Drift does not allow in a server name".into(),
        ));
    }

    let config = mcp_config(&server.definition, config_dir).map_err(|error| Left::Out(error.to_string()))?;
    let enabled = server.definition["enabled"] != false && server.approved;
    store.save_mcp_server(&server.name, &config).map_err(failed)?;
    store.set_mcp_enabled(&server.name, enabled).map_err(failed)?;

    Ok(enabled)
}

/// Converts opencode's local and remote server shapes to native server configs.
/// Resolves environment-variable and file substitutions at import time.
pub fn mcp_config(definition: &Value, config_dir: &Path) -> Result<ServerConfig, McpConfigError> {
    match definition["type"].as_str() {
        Some("local") => local_config(definition, config_dir),
        Some("remote") => remote_config(definition, config_dir),
        _ => Err(McpConfigError::UnsupportedType),
    }
}

fn local_config(definition: &Value, config_dir: &Path) -> Result<ServerConfig, McpConfigError> {
    let mut command = Vec::new();
    for value in definition["command"].as_array().into_iter().flatten() {
        if let Some(text) = substituted_text(value, config_dir)? {
            command.push(text);
        }
    }
    let (program, args) = command.split_first().ok_or(McpConfigError::MissingCommand)?;

    Ok(ServerConfig::Stdio {
        command: program.clone(),
        args: args.to_vec(),
        env: substituted_table(&definition["environment"], config_dir)?,
        cwd: None,
        timeout_seconds: None,
    })
}

fn remote_config(definition: &Value, config_dir: &Path) -> Result<ServerConfig, McpConfigError> {
    let url = substituted_text(&definition["url"], config_dir)?.ok_or(McpConfigError::MissingUrl)?;
    let oauth = definition["oauth"].as_object().and_then(|client| {
        Some(OAuthClient {
            client_id: client.get("clientId")?.as_str()?.into(),
            client_secret: client.get("clientSecret").and_then(Value::as_str).map(String::from),
            scopes: client
                .get("scope")
                .and_then(Value::as_str)
                .map(|scope| scope.split_whitespace().map(String::from).collect())
                .unwrap_or_default(),
        })
    });

    Ok(ServerConfig::Http {
        url,
        headers: substituted_table(&definition["headers"], config_dir)?,
        oauth,
        timeout_seconds: None,
    })
}

fn substituted_text(value: &Value, config_dir: &Path) -> Result<Option<String>, McpConfigError> {
    value.as_str().map(|text| substitute(text, config_dir)).transpose()
}

fn substituted_table(value: &Value, config_dir: &Path) -> Result<BTreeMap<String, String>, McpConfigError> {
    let mut table = BTreeMap::new();
    for (key, value) in value.as_object().into_iter().flatten() {
        if let Some(text) = substituted_text(value, config_dir)? {
            table.insert(key.clone(), text);
        }
    }

    Ok(table)
}

/// Resolves `{env:NAME}` and `{file:path}` tokens as opencode reads them.
/// An unreadable substitution rejects the server rather than saving a blank secret.
fn substitute(text: &str, config_dir: &Path) -> Result<String, McpConfigError> {
    let mut output = String::new();
    let mut rest = text;

    while let Some(start) = rest.find('{') {
        let Some(end) = rest[start..].find('}').map(|end| start + end) else {
            break;
        };

        output.push_str(&rest[..start]);
        let token = &rest[start + 1..end];
        let value = match token.split_once(':') {
            Some(("env", name)) => {
                std::env::var(name).map_err(|_| McpConfigError::MissingEnvironment { name: name.into() })?
            }
            Some(("file", path)) => {
                let path = expand(path, config_dir);
                let content = std::fs::read_to_string(&path).map_err(|_| McpConfigError::UnreadableFile { path })?;

                content.trim().to_string()
            }
            _ => rest[start..=end].to_string(),
        };

        output.push_str(&value);
        rest = &rest[end + 1..];
    }

    output.push_str(rest);
    Ok(output)
}

fn expand(path: &str, base: &Path) -> PathBuf {
    match path.strip_prefix("~/") {
        Some(rest) => drift_engine::config::home().map_or_else(|| PathBuf::from(path), |home| home.join(rest)),
        None if Path::new(path).is_absolute() => PathBuf::from(path),
        None => base.join(path),
    }
}
