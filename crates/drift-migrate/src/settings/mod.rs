//! Imports settings once; Drift's existing credentials, servers and files always win.

use drift_engine::llm::Credential;
use drift_engine::llm::credentials::Credentials;
use drift_engine::store::Store;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

mod config;
mod files;
mod servers;

pub use config::OcConfig;
pub use servers::{McpConfigError, mcp_config};

// Keep the ledger after deletions so a removed import stays removed.
const LEDGER: &str = "opencodeImported";
pub const REPORT: &str = "opencodeImportReport";
// Local providers use placeholder keys, not real credentials.
const KEYLESS: [&str; 2] = ["lmstudio", "ollama"];
// Drift must be able to refresh an imported OAuth sign-in.
const SIGN_INS: [&str; 3] = ["anthropic", "openai", "xai"];

pub struct Settings {
    pub auth: Option<Value>,
    pub config: Option<OcConfig>,
    pub config_dir: PathBuf,
    pub servers: Vec<OcServer>,
}

pub struct OcServer {
    pub name: String,
    pub definition: Value,
    /// Approval from Drift's old MCP approval step is required to enable the imported server.
    pub approved: bool,
}

#[derive(Default, Serialize, Deserialize)]
#[serde(default)]
struct Ledger {
    credentials: BTreeSet<String>,
    servers: BTreeSet<String>,
    config: bool,
    files: bool,
}

#[derive(Debug, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SettingsReport {
    pub credentials: Vec<String>,
    pub servers: Vec<String>,
    pub disabled_servers: Vec<String>,
    pub config_written: Option<String>,
    /// Copied paths relative to `~/.config/drift`.
    pub files: Vec<String>,
    pub left_out: LeftOut,
    /// Log messages include both unsupported imports and items kept because Drift had its own.
    pub skipped: Vec<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LeftOut {
    pub sign_ins: Vec<String>,
    pub plugins: Vec<String>,
    pub settings: Vec<String>,
    pub servers: Vec<String>,
    pub failed: Vec<String>,
}

enum Left {
    Kept(String),
    Out(String),
}

impl SettingsReport {
    fn left(&mut self, item: &str, line: String, group: fn(&mut LeftOut) -> &mut Vec<String>) {
        group(&mut self.left_out).push(item.to_string());
        self.skipped.push(line);
    }
}

pub fn import_settings(
    store: &Store,
    credentials: &Credentials,
    providers: &[String],
    home: &Path,
    settings: &Settings,
) -> rusqlite::Result<SettingsReport> {
    let mut ledger: Ledger = store.setting(LEDGER)?.unwrap_or_default();
    let mut report = SettingsReport::default();

    import_credentials(credentials, providers, settings, &mut ledger, &mut report);
    import_servers(store, settings, &mut ledger, &mut report);

    if let Some(config) = &settings.config
        && !ledger.config
    {
        let file = config::convert(config, &settings.config_dir, &mut report);
        report.config_written = files::write_config(home, file, &mut report);
        ledger.config = true;
    }

    if !ledger.files && settings.config_dir.is_dir() {
        let destination = home.join(".config").join("drift");
        files::copy_home(&settings.config_dir, &destination, &mut report);
        ledger.files = true;
    }

    store.set_setting(LEDGER, &ledger)?;
    store.set_setting(REPORT, &report)?;

    Ok(report)
}

fn import_credentials(
    credentials: &Credentials,
    providers: &[String],
    settings: &Settings,
    ledger: &mut Ledger,
    report: &mut SettingsReport,
) {
    let Some(auth) = settings.auth.as_ref().and_then(Value::as_object) else {
        return;
    };

    for (provider, entry) in auth {
        if ledger.credentials.contains(provider) {
            continue;
        }

        match credential(credentials, providers, provider, entry) {
            Ok(()) => report.credentials.push(provider.clone()),
            Err(Some(Left::Kept(reason))) => report.skipped.push(format!("sign-in for {provider}: {reason}")),
            Err(Some(Left::Out(reason))) => {
                report.left(provider, format!("sign-in for {provider}: {reason}"), |left| {
                    &mut left.sign_ins
                });
            }
            Err(None) => continue,
        }

        ledger.credentials.insert(provider.clone());
    }
}

fn import_servers(store: &Store, settings: &Settings, ledger: &mut Ledger, report: &mut SettingsReport) {
    let mut seen = BTreeSet::new();

    for server in &settings.servers {
        if !seen.insert(server.name.clone()) || ledger.servers.contains(&server.name) {
            continue;
        }

        match servers::save(store, server, &settings.config_dir) {
            Ok(enabled) => {
                report.servers.push(server.name.clone());
                if !enabled {
                    report.disabled_servers.push(server.name.clone());
                }
            }
            Err(Left::Kept(reason)) => report.skipped.push(format!("MCP server {}: {reason}", server.name)),
            Err(Left::Out(reason)) => {
                report.left(&server.name, format!("MCP server {}: {reason}", server.name), |left| {
                    &mut left.servers
                });
            }
        }

        ledger.servers.insert(server.name.clone());
    }
}

// Err(None) skips local placeholder keys without recording them in the ledger.
fn credential(
    credentials: &Credentials,
    providers: &[String],
    provider: &str,
    entry: &Value,
) -> Result<(), Option<Left>> {
    let unsupported = |reason: &str| Some(Left::Out(reason.into()));
    let field = |key: &str| entry[key].as_str().filter(|value| !value.is_empty()).map(String::from);
    let needed =
        |key: &str, description: &str| field(key).ok_or_else(|| Some(Left::Out(format!("it has no {description}"))));

    let found = match entry["type"].as_str() {
        Some("api") if KEYLESS.contains(&provider) => return Err(None),
        Some("api") if providers.iter().any(|known| known == provider) => Credential::ApiKey {
            key: needed("key", "key")?,
        },
        Some("api") => {
            return Err(unsupported(
                "Drift has no provider by that name; add it to drift.json's providers to use the key",
            ));
        }
        Some("oauth") if SIGN_INS.contains(&provider) => Credential::OAuth {
            access: needed("access", "access token")?,
            refresh: needed("refresh", "refresh token")?,
            expires_at: entry["expires"].as_i64().unwrap_or(0),
            account: field("accountId"),
        },
        Some("oauth") => {
            return Err(unsupported(
                "Drift cannot renew this sign-in; sign in again in Settings",
            ));
        }
        _ => return Err(unsupported("not a key or sign-in Drift reads")),
    };

    if credentials.get(provider).is_some() {
        return Err(Some(Left::Kept("already signed in to Drift, kept".into())));
    }

    credentials
        .set(provider, &found)
        .map_err(|reason| Some(Left::Out(reason)))
}

#[cfg(test)]
mod tests;
