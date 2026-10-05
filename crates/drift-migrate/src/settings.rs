//! opencode's sign-ins, MCP servers and global config, each brought in once. Nothing Drift already
//! has is replaced: a provider signed in here, a server of the same name or an existing
//! `~/.config/drift/drift.json` wins. What has no place in Drift is reported, never guessed at.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use drift_engine::llm::credentials::Credentials;
use drift_engine::llm::Credential;
use drift_engine::mcp::{OAuthClient, ServerConfig};
use drift_engine::store::Store;
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};

/// What was brought in before, so a sign-in the user removed or a server they deleted stays gone.
const LEDGER: &str = "opencodeImported";
/// The last run's report, for the user to read.
pub const REPORT: &str = "opencodeImportReport";
/// Providers that take no key, whose opencode placeholder key means nothing.
const KEYLESS: [&str; 2] = ["lmstudio", "ollama"];
/// Sign-ins Drift can refresh itself; any other would stop working within hours.
const SIGN_INS: [&str; 2] = ["anthropic", "openai"];

/// What opencode holds: its `auth.json`, its global config and where that lives (for `{file:...}`), and MCP servers.
pub struct Settings {
    pub auth: Option<Value>,
    pub config: Option<Value>,
    pub config_dir: PathBuf,
    pub servers: Vec<OcServer>,
}

/// An MCP server in opencode's shape; `approved` when the user allowed it in Drift's old approval step.
pub struct OcServer {
    pub name: String,
    pub definition: Value,
    pub approved: bool,
}

#[derive(Default, Serialize, Deserialize)]
#[serde(default)]
struct Ledger {
    credentials: BTreeSet<String>,
    servers: BTreeSet<String>,
    config: bool,
    /// opencode's global instructions, agents, commands and skills were copied.
    files: bool,
}

/// opencode's folders under its config directory, with the folder Drift reads the same things from.
const FOLDERS: [(&str, &str); 5] = [("agents", "agents"), ("agent", "agents"), ("commands", "commands"), ("command", "commands"), ("skills", "skills")];

#[derive(Debug, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SettingsReport {
    /// Providers given opencode's key or sign-in.
    pub credentials: Vec<String>,
    /// Servers saved, and of those the ones left switched off (never approved, or off in opencode).
    pub servers: Vec<String>,
    pub disabled_servers: Vec<String>,
    /// The drift.json written from opencode's config, when one was.
    pub config_written: Option<String>,
    /// Files copied from opencode's config directory into `~/.config/drift`, relative to it.
    pub files: Vec<String>,
    /// Everything left behind, each with why.
    pub skipped: Vec<String>,
}

pub fn import_settings(store: &Store, credentials: &Credentials, providers: &[String], home: &Path, settings: &Settings) -> rusqlite::Result<SettingsReport> {
    let mut ledger: Ledger = store.setting(LEDGER)?.unwrap_or_default();
    let mut report = SettingsReport::default();
    if let Some(auth) = settings.auth.as_ref().and_then(Value::as_object) {
        for (provider, entry) in auth {
            if ledger.credentials.contains(provider) {
                continue;
            }
            match credential(credentials, providers, provider, entry) {
                Ok(()) => report.credentials.push(provider.clone()),
                Err(Some(why)) => report.skipped.push(format!("sign-in for {provider}: {why}")),
                Err(None) => continue,
            }
            ledger.credentials.insert(provider.clone());
        }
    }
    let mut seen = BTreeSet::new();
    for server in &settings.servers {
        if !seen.insert(server.name.clone()) || ledger.servers.contains(&server.name) {
            continue;
        }
        match self::server(store, server, &settings.config_dir) {
            Ok(enabled) => {
                report.servers.push(server.name.clone());
                if !enabled {
                    report.disabled_servers.push(server.name.clone());
                }
            }
            Err(why) => report.skipped.push(format!("MCP server {}: {why}", server.name)),
        }
        ledger.servers.insert(server.name.clone());
    }
    if let (Some(config), false) = (settings.config.as_ref(), ledger.config) {
        let (file, mut unmapped) = self::config(config, &settings.config_dir);
        report.skipped.append(&mut unmapped);
        report.config_written = write_config(home, file, &mut report.skipped);
        ledger.config = true;
    }
    if !ledger.files && settings.config_dir.is_dir() {
        copy_home(&settings.config_dir, &home.join(".config").join("drift"), &mut report);
        ledger.files = true;
    }
    store.set_setting(LEDGER, &ledger)?;
    store.set_setting(REPORT, &report)?;
    Ok(report)
}

/// `Err(None)` for a key that means nothing to record (a local server's placeholder); `Err(Some)` says why it was left.
fn credential(credentials: &Credentials, providers: &[String], provider: &str, entry: &Value) -> Result<(), Option<String>> {
    let field = |key: &str| entry[key].as_str().filter(|value| !value.is_empty()).map(String::from);
    let needed = |key: &str, what: &str| field(key).ok_or_else(|| Some(format!("it has no {what}")));
    let found = match entry["type"].as_str() {
        Some("api") if KEYLESS.contains(&provider) => return Err(None),
        Some("api") if providers.iter().any(|known| known == provider) => Credential::ApiKey { key: needed("key", "key")? },
        Some("api") => return Err(Some("Drift has no provider by that name; add it to drift.json's providers to use the key".into())),
        Some("oauth") if SIGN_INS.contains(&provider) => Credential::OAuth {
            access: needed("access", "access token")?,
            refresh: needed("refresh", "refresh token")?,
            expires_at: entry["expires"].as_i64().unwrap_or(0),
            account: field("accountId"),
        },
        Some("oauth") => return Err(Some("Drift cannot renew this sign-in; sign in again in Settings".into())),
        _ => return Err(Some("not a key or sign-in Drift reads".into())),
    };
    if credentials.get(provider).is_some() {
        return Err(Some("already signed in to Drift, kept".into()));
    }
    credentials.set(provider, &found).map_err(Some)
}

/// Saves the server; whether it was left switched on.
fn server(store: &Store, server: &OcServer, config_dir: &Path) -> Result<bool, String> {
    if store.mcp_server(&server.name).map_err(|e| e.to_string())?.is_some() || store.unreadable_mcp_servers().map_err(|e| e.to_string())?.contains(&server.name) {
        return Err("Drift already has a server by that name, kept".into());
    }
    if server.name.is_empty() || !server.name.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_')) {
        return Err("its name has characters Drift does not allow in a server name".into());
    }
    let config = mcp_config(&server.definition, config_dir)?;
    let enabled = server.definition["enabled"] != false && server.approved;
    store.save_mcp_server(&server.name, &config).map_err(|e| e.to_string())?;
    store.set_mcp_enabled(&server.name, enabled).map_err(|e| e.to_string())?;
    Ok(enabled)
}

/// opencode's `local` and `remote` shapes as this engine's; `{env:NAME}` and `{file:path}` are read now.
pub fn mcp_config(definition: &Value, config_dir: &Path) -> Result<ServerConfig, String> {
    let text = |value: &Value| value.as_str().map(|text| substitute(text, config_dir)).transpose();
    let table = |value: &Value| -> Result<BTreeMap<String, String>, String> {
        value.as_object().into_iter().flatten().filter_map(|(key, value)| text(value).transpose().map(|value| value.map(|value| (key.clone(), value)))).collect()
    };
    match definition["type"].as_str() {
        Some("local") => {
            let command: Vec<String> = definition["command"].as_array().into_iter().flatten().map(text).collect::<Result<Vec<_>, _>>()?.into_iter().flatten().collect();
            let (program, args) = command.split_first().ok_or("it has no command")?;
            Ok(ServerConfig::Stdio { command: program.clone(), args: args.to_vec(), env: table(&definition["environment"])?, cwd: None, timeout_seconds: None })
        }
        Some("remote") => {
            let url = text(&definition["url"])?.ok_or("it has no url")?;
            let oauth = definition["oauth"].as_object().and_then(|client| {
                Some(OAuthClient {
                    client_id: client.get("clientId")?.as_str()?.into(),
                    client_secret: client.get("clientSecret").and_then(Value::as_str).map(String::from),
                    scopes: client.get("scope").and_then(Value::as_str).map(|scope| scope.split_whitespace().map(String::from).collect()).unwrap_or_default(),
                })
            });
            Ok(ServerConfig::Http { url, headers: table(&definition["headers"])?, oauth, timeout_seconds: None })
        }
        _ => Err("it is neither a local nor a remote server".into()),
    }
}

/// `{env:NAME}` and `{file:path}` as opencode reads them; one that cannot be read refuses the server rather than save a blank secret.
fn substitute(text: &str, config_dir: &Path) -> Result<String, String> {
    let mut out = String::new();
    let mut rest = text;
    while let Some(start) = rest.find('{') {
        let Some(end) = rest[start..].find('}').map(|end| start + end) else { break };
        out.push_str(&rest[..start]);
        let token = &rest[start + 1..end];
        let value = match token.split_once(':') {
            Some(("env", name)) => std::env::var(name).map_err(|_| format!("the environment variable {name} is not set"))?,
            Some(("file", path)) => {
                let path = expand(path, config_dir);
                std::fs::read_to_string(&path).map_err(|_| format!("{} could not be read", path.display()))?.trim().to_string()
            }
            _ => rest[start..=end].to_string(),
        };
        out.push_str(&value);
        rest = &rest[end + 1..];
    }
    out.push_str(rest);
    Ok(out)
}

fn expand(path: &str, base: &Path) -> PathBuf {
    match path.strip_prefix("~/") {
        Some(rest) => drift_engine::config::home().map_or_else(|| PathBuf::from(path), |home| home.join(rest)),
        None if Path::new(path).is_absolute() => PathBuf::from(path),
        None => base.join(path),
    }
}

/// opencode keys with a Drift equivalent, as drift.json; and every other key, said by name.
fn config(config: &Value, config_dir: &Path) -> (Map<String, Value>, Vec<String>) {
    let mut file = Map::new();
    let mut unmapped = Vec::new();
    for (key, value) in config.as_object().into_iter().flatten() {
        match key.as_str() {
            "$schema" | "mcp" => {}
            "model" => match value.as_str().and_then(|model| model.split_once('/')) {
                Some((provider, model)) => drop(file.insert("model".into(), json!({ "provider": provider, "model": model }))),
                None => unmapped.push("config model: not written as provider/model".into()),
            },
            "instructions" => {
                let paths: Vec<Value> = value.as_array().into_iter().flatten().filter_map(Value::as_str).map(|path| json!(instruction(path, config_dir))).collect();
                file.insert("instructions".into(), Value::Array(paths));
            }
            "permission" => {
                let (rules, mut left) = permissions(value);
                unmapped.append(&mut left);
                if !rules.is_empty() {
                    file.insert("permissions".into(), Value::Array(rules));
                }
            }
            "plugin" => unmapped.extend(value.as_array().into_iter().flatten().map(|plugin| format!("plugin {}: opencode plugins are JavaScript and Drift runs none", plugin.as_str().unwrap_or("?")))),
            other => unmapped.push(format!("config {other}: Drift has no setting for it")),
        }
    }
    (file, unmapped)
}

/// A relative or `~/` path stays meaningful from `~/.config/drift`: it becomes absolute.
fn instruction(path: &str, config_dir: &Path) -> String {
    if path.starts_with("~/") || Path::new(path).is_absolute() {
        return path.into();
    }
    config_dir.join(path).to_string_lossy().replace('\\', "/")
}

/// opencode's `permission` (`edit: "ask"`, `bash: { "git *": "allow" }`) as Drift rules.
fn permissions(value: &Value) -> (Vec<Value>, Vec<String>) {
    let mut rules = Vec::new();
    let mut unmapped = Vec::new();
    for (kind, setting) in value.as_object().into_iter().flatten() {
        if !matches!(kind.as_str(), "read" | "edit" | "bash" | "webfetch") {
            unmapped.push(format!("config permission.{kind}: Drift has no such permission"));
            continue;
        }
        let patterns: Vec<(&str, &Value)> = match setting {
            Value::String(_) => vec![("*", setting)],
            Value::Object(map) => map.iter().map(|(pattern, decision)| (pattern.as_str(), decision)).collect(),
            _ => Vec::new(),
        };
        for (pattern, decision) in patterns {
            match decision.as_str().filter(|decision| matches!(*decision, "allow" | "ask" | "deny")) {
                Some(decision) => rules.push(json!({ "kind": kind, "pattern": pattern, "decision": decision })),
                None => unmapped.push(format!("config permission.{kind} {pattern}: not allow, ask or deny")),
            }
        }
    }
    (rules, unmapped)
}

/// Copies opencode's global `AGENTS.md`, agents, commands and skills to where Drift reads them,
/// never over a file already there; its JavaScript plugins are named, not copied.
fn copy_home(from: &Path, to: &Path, report: &mut SettingsReport) {
    let mut copied = Vec::new();
    copy_file(&from.join("AGENTS.md"), &to.join("AGENTS.md"), "AGENTS.md", &mut copied, &mut report.skipped);
    for (source, target) in FOLDERS {
        copy_tree(&from.join(source), &to.join(target), target, &mut copied, &mut report.skipped);
    }
    for folder in ["plugins", "plugin"] {
        for entry in std::fs::read_dir(from.join(folder)).into_iter().flatten().flatten() {
            report.skipped.push(format!("plugin {}: opencode plugins are JavaScript and Drift runs none", entry.file_name().to_string_lossy()));
        }
    }
    report.files = copied;
}

fn copy_tree(from: &Path, to: &Path, shown: &str, copied: &mut Vec<String>, skipped: &mut Vec<String>) {
    for entry in std::fs::read_dir(from).into_iter().flatten().flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let path = entry.path();
        let shown = format!("{shown}/{name}");
        if path.is_dir() {
            copy_tree(&path, &to.join(&name), &shown, copied, skipped);
        } else {
            copy_file(&path, &to.join(&name), &shown, copied, skipped);
        }
    }
}

fn copy_file(from: &Path, to: &Path, shown: &str, copied: &mut Vec<String>, skipped: &mut Vec<String>) {
    if !from.is_file() {
        return;
    }
    if to.exists() {
        skipped.push(format!("{shown}: you already have one in ~/.config/drift, kept"));
        return;
    }
    let done = to.parent().map_or(Ok(()), std::fs::create_dir_all).and_then(|()| std::fs::copy(from, to));
    match done {
        Ok(_) => copied.push(shown.to_string()),
        Err(error) => skipped.push(format!("{shown}: could not be copied ({error})")),
    }
}

/// Writes drift.json only when something maps and the user has none yet; the path written.
fn write_config(home: &Path, file: Map<String, Value>, skipped: &mut Vec<String>) -> Option<String> {
    if file.is_empty() {
        return None;
    }
    let path = home.join(".config").join("drift").join(drift_engine::config::FILE);
    let keys = file.keys().cloned().collect::<Vec<_>>().join(", ");
    if path.exists() {
        skipped.push(format!("config {keys}: you already have {}, so it was not changed", path.display()));
        return None;
    }
    let written = std::fs::create_dir_all(path.parent()?).and_then(|()| std::fs::write(&path, serde_json::to_string_pretty(&Value::Object(file)).unwrap()));
    match written {
        Ok(()) => Some(path.to_string_lossy().into_owned()),
        Err(error) => {
            skipped.push(format!("config {keys}: {} could not be written ({error})", path.display()));
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Dir(PathBuf);

    impl Drop for Dir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn setup() -> (Dir, std::sync::Arc<drift_engine::Engine>, Vec<String>) {
        let _ = rustls::crypto::ring::default_provider().install_default();
        let dir = Dir(std::env::temp_dir().join(format!("drift-settings-{}", drift_engine::id::new("t"))));
        let engine = drift_engine::Engine::open_with(&dir.0.join("data"), drift_engine::Options { file_credentials: true, ..Default::default() }).unwrap();
        let providers = engine.catalog.read().unwrap().providers.keys().cloned().collect();
        (dir, engine, providers)
    }

    fn settings(dir: &Path, auth: Value, config: Value, servers: Vec<OcServer>) -> Settings {
        Settings { auth: Some(auth), config: Some(config), config_dir: dir.to_path_buf(), servers }
    }

    #[test]
    fn keys_and_renewable_sign_ins_come_in_once_and_never_over_drifts_own() {
        let (dir, engine, providers) = setup();
        engine.credentials.set("openai", &Credential::ApiKey { key: "mine".into() }).unwrap();
        let auth = json!({
            "anthropic": { "type": "oauth", "access": "a", "refresh": "r", "expires": 123 },
            "openai": { "type": "oauth", "access": "a", "refresh": "r", "expires": 1, "accountId": "acc" },
            "xai": { "type": "oauth", "access": "a", "refresh": "r", "expires": 1 },
            "zai": { "type": "api", "key": "z-key" },
            "nvidia": { "type": "api", "key": "n-key" },
            "lmstudio": { "type": "api", "key": "lm" },
        });
        let report = import_settings(&engine.store, &engine.credentials, &providers, &dir.0, &settings(&dir.0, auth.clone(), json!({}), vec![])).unwrap();
        assert_eq!(report.credentials, ["anthropic", "zai"]);
        assert_eq!(engine.credentials.get("anthropic"), Some(Credential::OAuth { access: "a".into(), refresh: "r".into(), expires_at: 123, account: None }));
        assert_eq!(engine.credentials.get("openai"), Some(Credential::ApiKey { key: "mine".into() }), "a sign-in made in Drift stays");
        assert_eq!(engine.credentials.get("zai"), Some(Credential::ApiKey { key: "z-key".into() }));
        let said = report.skipped.join("\n");
        assert!(said.contains("openai: already signed in") && said.contains("xai: Drift cannot renew") && said.contains("nvidia: Drift has no provider") && !said.contains("lmstudio"), "{said}");
        engine.credentials.remove("anthropic").unwrap();
        let again = import_settings(&engine.store, &engine.credentials, &providers, &dir.0, &settings(&dir.0, auth, json!({}), vec![])).unwrap();
        assert_eq!((again.credentials.len(), again.skipped.len()), (0, 0), "{again:?}");
        assert!(engine.credentials.get("anthropic").is_none(), "a sign-in the user removed stays removed");
    }

    #[test]
    fn servers_come_in_switched_on_only_when_they_were_approved_and_enabled() {
        let (dir, engine, providers) = setup();
        std::fs::write(dir.0.join("token.txt"), "file-secret\n").unwrap();
        std::env::set_var("DRIFT_MIGRATE_TEST_KEY", "env-secret");
        engine.store.save_mcp_server("mine", &ServerConfig::Http { url: "https://mine.example".into(), headers: BTreeMap::new(), oauth: None, timeout_seconds: None }).unwrap();
        let server = |name: &str, definition: Value, approved: bool| OcServer { name: name.into(), definition, approved };
        let servers = vec![
            server("local", json!({ "type": "local", "command": ["npx", "-y", "tool"], "environment": { "KEY": "{env:DRIFT_MIGRATE_TEST_KEY}" }, "enabled": true }), true),
            server("remote", json!({ "type": "remote", "url": "https://r.example/mcp", "headers": { "Authorization": "Bearer {file:token.txt}" }, "oauth": { "clientId": "app", "scope": "a b" } }), true),
            server("unapproved", json!({ "type": "remote", "url": "https://u.example" }), false),
            server("off", json!({ "type": "local", "command": ["x"], "enabled": false }), true),
            server("mine", json!({ "type": "local", "command": ["other"] }), true),
            server("bad name", json!({ "type": "local", "command": ["x"] }), true),
            server("missing", json!({ "type": "local", "command": ["x"], "environment": { "K": "{env:DRIFT_MIGRATE_UNSET_VAR}" } }), true),
            server("local", json!({ "type": "local", "command": ["duplicate"] }), true),
        ];
        let report = import_settings(&engine.store, &engine.credentials, &providers, &dir.0, &settings(&dir.0, json!({}), json!({}), servers)).unwrap();
        assert_eq!((report.servers, report.disabled_servers), (vec!["local".to_string(), "remote".into(), "unapproved".into(), "off".into()], vec!["unapproved".to_string(), "off".into()]));
        let saved: BTreeMap<String, (ServerConfig, bool)> = engine.store.mcp_servers().unwrap().into_iter().map(|row| (row.name, (row.config, row.enabled))).collect();
        assert_eq!(saved["local"], (ServerConfig::Stdio { command: "npx".into(), args: vec!["-y".into(), "tool".into()], env: BTreeMap::from([("KEY".into(), "env-secret".into())]), cwd: None, timeout_seconds: None }, true));
        let oauth = Some(OAuthClient { client_id: "app".into(), client_secret: None, scopes: vec!["a".into(), "b".into()] });
        assert_eq!(saved["remote"], (ServerConfig::Http { url: "https://r.example/mcp".into(), headers: BTreeMap::from([("Authorization".into(), "Bearer file-secret".into())]), oauth, timeout_seconds: None }, true));
        assert!(matches!(&saved["mine"].0, ServerConfig::Http { url, .. } if url == "https://mine.example"), "Drift's own server is kept");
        assert!(!saved.contains_key("missing") && !saved.contains_key("bad name"));
        let said = report.skipped.join("\n");
        assert!(said.contains("DRIFT_MIGRATE_UNSET_VAR is not set") && said.contains("bad name: its name") && said.contains("mine: Drift already has"), "{said}");
    }

    #[test]
    fn opencodes_instructions_agents_commands_and_skills_are_copied_once_and_drift_reads_them() {
        let (dir, engine, providers) = setup();
        let opencode = dir.0.join("opencode");
        let home = dir.0.join("home");
        let write = |path: PathBuf, text: &str| {
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, text).unwrap();
        };
        write(opencode.join("AGENTS.md"), "Be brief.");
        write(opencode.join("agents/reviewer.md"), "---\ndescription: Reviews\nmode: subagent\n---\nReview.");
        write(opencode.join("command/ship.md"), "---\ndescription: Ship it\n---\nShip $ARGUMENTS.");
        write(opencode.join("skills/impeccable/SKILL.md"), "---\nname: impeccable\ndescription: Design\n---\nDesign well.");
        write(opencode.join("skills/impeccable/reference/colour.md"), "notes");
        write(opencode.join("skills/unslop/SKILL.md"), "---\nname: unslop\ndescription: Mine\n---\nTheirs.");
        write(opencode.join("plugins/gk-hooks.js"), "export default {}");
        write(home.join(".config/drift/skills/unslop/SKILL.md"), "---\nname: unslop\ndescription: Mine\n---\nMine.");
        let settings = Settings { auth: None, config: None, config_dir: opencode.clone(), servers: vec![] };

        let report = import_settings(&engine.store, &engine.credentials, &providers, &home, &settings).unwrap();
        let mut files = report.files.clone();
        files.sort();
        assert_eq!(files, ["AGENTS.md", "agents/reviewer.md", "commands/ship.md", "skills/impeccable/SKILL.md", "skills/impeccable/reference/colour.md"]);
        let said = report.skipped.join("\n");
        assert!(said.contains("skills/unslop/SKILL.md: you already have one") && said.contains("plugin gk-hooks.js"), "{said}");
        assert!(std::fs::read_to_string(home.join(".config/drift/skills/unslop/SKILL.md")).unwrap().contains("Mine."), "the user's own copy wins");

        let workspace = dir.0.join("ws");
        std::fs::create_dir_all(&workspace).unwrap();
        let config = drift_engine::config::Config::load_with_home(&workspace, Some(&home));
        assert!(config.agent("reviewer").is_some() && config.commands.iter().any(|c| c.name == "ship") && config.skill("impeccable").is_some());
        assert!(config.instructions.iter().any(|i| i.text.contains("Be brief.")), "the global instructions apply");

        std::fs::remove_file(home.join(".config/drift/agents/reviewer.md")).unwrap();
        let again = import_settings(&engine.store, &engine.credentials, &providers, &home, &settings).unwrap();
        assert!(again.files.is_empty() && !home.join(".config/drift/agents/reviewer.md").exists(), "a copied file the user deleted stays deleted");
    }

    #[test]
    fn mappable_config_becomes_drift_json_and_the_rest_is_named() {
        let (dir, engine, providers) = setup();
        let home = dir.0.join("home");
        let config = json!({
            "$schema": "https://opencode.ai/config.json",
            "model": "anthropic/claude-opus-5-5",
            "instructions": ["rules.md", "~/style.md"],
            "permission": { "edit": "ask", "bash": { "git *": "allow", "rm *": "deny" }, "doom_loop": "ask" },
            "tools": { "firecrawl_agent": false },
            "plugin": ["opencode-foo@1"],
            "mcp": {},
        });
        let report = import_settings(&engine.store, &engine.credentials, &providers, &home, &settings(&dir.0, json!({}), config.clone(), vec![])).unwrap();
        let written: Value = serde_json::from_str(&std::fs::read_to_string(home.join(".config/drift/drift.json")).unwrap()).unwrap();
        assert_eq!(written["model"], json!({ "provider": "anthropic", "model": "claude-opus-5-5" }));
        assert_eq!(written["instructions"], json!([dir.0.join("rules.md").to_string_lossy().replace('\\', "/"), "~/style.md"]));
        assert_eq!(written["permissions"], json!([
            { "kind": "bash", "pattern": "git *", "decision": "allow" },
            { "kind": "bash", "pattern": "rm *", "decision": "deny" },
            { "kind": "edit", "pattern": "*", "decision": "ask" },
        ]));
        let file: drift_engine::config::File = serde_json::from_value(written).unwrap();
        assert_eq!(file.permissions.len(), 3, "Drift reads what was written");
        let said = report.skipped.join("\n");
        assert!(said.contains("config tools") && said.contains("plugin opencode-foo@1") && said.contains("permission.doom_loop"), "{said}");
        assert!(report.config_written.is_some());
        let other = dir.0.join("other");
        std::fs::create_dir_all(other.join(".config/drift")).unwrap();
        std::fs::write(other.join(".config/drift/drift.json"), "{}").unwrap();
        engine.store.remove_setting(LEDGER).unwrap();
        let kept = import_settings(&engine.store, &engine.credentials, &providers, &other, &settings(&dir.0, json!({}), config, vec![])).unwrap();
        assert_eq!(std::fs::read_to_string(other.join(".config/drift/drift.json")).unwrap(), "{}", "the user's own file is never changed");
        assert!(kept.config_written.is_none() && kept.skipped.iter().any(|line| line.contains("you already have")));
    }
}
