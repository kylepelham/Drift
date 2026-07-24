use crate::store::{McpServer, McpState, PromptOverride, Store};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};
use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

const POLICY_VERSION: u8 = 3;
const MAX_CONFIG_BYTES: usize = 65_536;
const SENTINEL_FILE: &str = "mcp-fail-closed.json";
static TEMP_ID: AtomicU64 = AtomicU64::new(0);

pub struct McpRuntime {
    root: PathBuf,
    extensions: PathBuf,
    mutation: Mutex<()>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct McpSnapshot {
    generation: i64,
    directory: String,
    servers: Vec<McpServer>,
    observed: Vec<ObservedServer>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PromptSnapshot {
    catalog: Value,
    overrides: Vec<PromptOverride>,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum McpDecision {
    Pending,
    Approved,
    Rejected,
    Invalid,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ObservedServer {
    pub name: String,
    #[serde(rename = "type")]
    pub transport: String,
    pub fingerprint: String,
    pub decision: McpDecision,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PendingReport {
    version: u8,
    generation: i64,
    directory: String,
    servers: Vec<ObservedServer>,
}

impl McpRuntime {
    pub fn new(data_dir: &Path, extensions: PathBuf) -> Self {
        Self {
            root: data_dir.join("engine-config"),
            extensions,
            mutation: Mutex::new(()),
        }
    }

    pub fn config_dir(&self) -> &Path {
        &self.root
    }

    pub fn materialize(&self, store: &Store) -> Result<(), String> {
        self.materialize_inner(store, true)
    }

    fn materialize_prompts(&self, store: &Store) -> Result<(), String> {
        self.materialize_inner(store, false)
    }

    fn materialize_inner(&self, store: &Store, reset_approvals: bool) -> Result<(), String> {
        std::fs::create_dir_all(self.root.join("pending")).map_err(|error| error.to_string())?;
        let state = store.mcp_state().map_err(|error| error.to_string())?;
        if reset_approvals {
            self.write_policy(state.generation, &[])?;
            self.clear_reports()?;
        }

        let mut config = self.base_config()?;
        let root = config
            .as_object_mut()
            .ok_or("Drift extension config must be a JSON object")?;
        let plugins = root.entry("plugin").or_insert_with(|| json!([]));
        let plugins = plugins
            .as_array_mut()
            .ok_or("Drift extension plugin config must be an array")?;
        plugins.push(Value::String(self.plugin_path("spawn-thread")?));
        plugins.push(json!([
            self.plugin_path("prompt-overrides")?,
            {
                "catalogPath": self.extensions.join("prompt-catalog.json").to_string_lossy(),
                "settingsPath": self.root.join("prompt-overrides.json").to_string_lossy()
            }
        ]));
        plugins.push(json!([
            self.plugin_path("mcp-approval")?,
            {
                "policyPath": self.root.join("mcp-approvals.json").to_string_lossy(),
                "pendingDirectory": self.root.join("pending").to_string_lossy(),
                "sentinelPath": self.root.join(SENTINEL_FILE).to_string_lossy(),
                "generation": state.generation
            }
        ]));
        root.insert(
            "mcp".into(),
            Value::Object(
                state
                    .servers
                    .iter()
                    .map(|server| (server.name.clone(), server.config.clone()))
                    .collect(),
            ),
        );
        let overrides = store
            .prompt_overrides()
            .map_err(|error| error.to_string())?;
        let families = overrides
            .iter()
            .filter_map(|item| {
                item.key
                    .strip_prefix("family:")
                    .map(|key| (key.to_string(), item.value.clone()))
            })
            .collect::<Map<String, Value>>();
        write_json(
            &self.root.join("prompt-overrides.json"),
            &json!({ "version": 1, "families": families }),
        )?;
        let agents = root.entry("agent").or_insert_with(|| json!({}));
        let agents = agents
            .as_object_mut()
            .ok_or("Drift extension agent config must be an object")?;
        for item in &overrides {
            if let Some(name) = item.key.strip_prefix("agent:") {
                agents.insert(name.to_string(), item.value.clone());
            }
        }
        write_json(&self.root.join("opencode.json"), &config)?;
        if reset_approvals {
            self.write_policy(state.generation, &state.decisions)?;
            store
                .mark_mcp_materialized(state.generation)
                .map_err(|error| error.to_string())?;
            self.clear_sentinel()?;
        }
        Ok(())
    }

    pub fn prompt_snapshot(&self, store: &Store) -> Result<PromptSnapshot, String> {
        let catalog = std::fs::read(self.extensions.join("prompt-catalog.json"))
            .map_err(|error| error.to_string())?;
        Ok(PromptSnapshot {
            catalog: serde_json::from_slice(&catalog).map_err(|error| error.to_string())?,
            overrides: store
                .prompt_overrides()
                .map_err(|error| error.to_string())?,
        })
    }

    pub fn save_prompt(
        &self,
        store: &Store,
        key: &str,
        value: Value,
        original: Option<Value>,
    ) -> Result<(), String> {
        let _guard = self
            .mutation
            .lock()
            .map_err(|_| "Prompt mutation lock is poisoned")?;
        validate_prompt_override(key, &value)?;
        if let Some(original) = original.as_ref() {
            validate_prompt_override(key, original)?;
        }
        let overrides = store
            .prompt_overrides()
            .map_err(|error| error.to_string())?;
        let previous = overrides.iter().find(|item| item.key == key).cloned();
        if previous.is_none() && overrides.len() >= 128 {
            return Err("Drift supports at most 128 prompt overrides".into());
        }
        store
            .save_prompt_override(key, &value, original.as_ref())
            .map_err(|error| error.to_string())?;
        if let Err(error) = self.materialize_prompts(store) {
            self.restore_prompt(store, key, previous.as_ref())?;
            return Err(error);
        }
        Ok(())
    }

    pub fn reset_prompt(&self, store: &Store, key: &str) -> Result<(), String> {
        let _guard = self
            .mutation
            .lock()
            .map_err(|_| "Prompt mutation lock is poisoned")?;
        validate_prompt_key(key)?;
        let previous = store
            .prompt_overrides()
            .map_err(|error| error.to_string())?
            .into_iter()
            .find(|item| item.key == key);
        store
            .reset_prompt_override(key)
            .map_err(|error| error.to_string())?;
        if let Err(error) = self.materialize_prompts(store) {
            self.restore_prompt(store, key, previous.as_ref())?;
            return Err(error);
        }
        Ok(())
    }

    fn restore_prompt(
        &self,
        store: &Store,
        key: &str,
        previous: Option<&PromptOverride>,
    ) -> Result<(), String> {
        match previous {
            Some(item) => store.save_prompt_override(key, &item.value, item.original.as_ref()),
            None => store.reset_prompt_override(key),
        }
        .map_err(|error| error.to_string())?;
        self.materialize_prompts(store)
    }

    pub fn snapshot(&self, store: &Store, directory: &str) -> Result<McpSnapshot, String> {
        let state = store.mcp_state().map_err(|error| error.to_string())?;
        let observed = self
            .report(directory, state.generation)?
            .map(|report| report.servers)
            .unwrap_or_default();
        Ok(McpSnapshot {
            generation: state.generation,
            directory: directory.to_string(),
            servers: state.servers,
            observed,
        })
    }

    pub fn save<F>(
        &self,
        store: &Store,
        name: &str,
        previous: Option<&str>,
        config: Value,
        generation: i64,
        stop: F,
    ) -> Result<(), String>
    where
        F: Fn() -> Result<(), String>,
    {
        let _guard = self
            .mutation
            .lock()
            .map_err(|_| "MCP mutation lock is poisoned")?;
        let state = self.require_generation(store, generation)?;
        validate_name(name)?;
        validate_config(&config)?;
        if state
            .servers
            .iter()
            .any(|server| server.name == name && previous != Some(name))
        {
            return Err(format!("An MCP server named {name} already exists"));
        }
        if previous
            .is_some_and(|previous| !state.servers.iter().any(|server| server.name == previous))
        {
            return Err("The MCP server being edited no longer exists".into());
        }
        self.change(store, state, stop, |store| {
            store
                .save_mcp_server(name, previous, &config)
                .map(|_| ())
                .map_err(|error| error.to_string())
        })
    }

    pub fn remove<F>(
        &self,
        store: &Store,
        name: &str,
        generation: i64,
        stop: F,
    ) -> Result<(), String>
    where
        F: Fn() -> Result<(), String>,
    {
        let _guard = self
            .mutation
            .lock()
            .map_err(|_| "MCP mutation lock is poisoned")?;
        let state = self.require_generation(store, generation)?;
        if !state.servers.iter().any(|server| server.name == name) {
            return Err("The MCP server no longer exists".into());
        }
        self.change(store, state, stop, |store| {
            store
                .remove_mcp_server(name)
                .map(|_| ())
                .map_err(|error| error.to_string())
        })
    }

    pub fn decide<F>(
        &self,
        store: &Store,
        directory: &str,
        name: &str,
        fingerprint: &str,
        generation: i64,
        decision: McpDecision,
        stop: F,
    ) -> Result<(), String>
    where
        F: Fn() -> Result<(), String>,
    {
        if matches!(decision, McpDecision::Pending | McpDecision::Invalid) {
            return Err("Only approval or rejection can be persisted as an MCP decision".into());
        }
        let _guard = self
            .mutation
            .lock()
            .map_err(|_| "MCP mutation lock is poisoned")?;
        let state = self.require_generation(store, generation)?;
        let report = self
            .report(directory, generation)?
            .ok_or("The current MCP approval report is not available")?;
        let server = report
            .servers
            .iter()
            .find(|server| {
                server.name == name
                    && server.fingerprint == fingerprint
                    && server.decision == McpDecision::Pending
            })
            .ok_or("The pending MCP fingerprint changed; reload before deciding")?;
        let persisted = match decision {
            McpDecision::Approved => "approved",
            McpDecision::Rejected => "rejected",
            McpDecision::Pending | McpDecision::Invalid => unreachable!(),
        };
        let name = server.name.clone();
        let fingerprint = server.fingerprint.clone();
        self.change(store, state, stop, move |store| {
            store
                .decide_mcp(&name, &fingerprint, persisted)
                .map(|_| ())
                .map_err(|error| error.to_string())
        })
    }

    pub fn revoke<F>(
        &self,
        store: &Store,
        directory: &str,
        name: &str,
        fingerprint: &str,
        generation: i64,
        stop: F,
    ) -> Result<(), String>
    where
        F: Fn() -> Result<(), String>,
    {
        let _guard = self
            .mutation
            .lock()
            .map_err(|_| "MCP mutation lock is poisoned")?;
        let state = self.require_generation(store, generation)?;
        let report = self
            .report(directory, generation)?
            .ok_or("The current MCP approval report is not available")?;
        if !report.servers.iter().any(|server| {
            server.name == name
                && server.fingerprint == fingerprint
                && matches!(
                    server.decision,
                    McpDecision::Approved | McpDecision::Rejected
                )
        }) {
            return Err("The decided MCP fingerprint changed; reload before revoking".into());
        }
        let fingerprint = fingerprint.to_string();
        self.change(store, state, stop, move |store| {
            store
                .revoke_mcp(&fingerprint)
                .map(|_| ())
                .map_err(|error| error.to_string())
        })
    }

    pub fn reload<F>(&self, store: &Store, stop: F) -> Result<(), String>
    where
        F: Fn() -> Result<(), String>,
    {
        let _guard = self
            .mutation
            .lock()
            .map_err(|_| "MCP mutation lock is poisoned")?;
        let state = store.mcp_state().map_err(|error| error.to_string())?;
        self.change(store, state, stop, |store| {
            store
                .advance_mcp_generation()
                .map(|_| ())
                .map_err(|error| error.to_string())
        })
    }

    fn require_generation(&self, store: &Store, generation: i64) -> Result<McpState, String> {
        let state = store.mcp_state().map_err(|error| error.to_string())?;
        if state.generation != generation {
            return Err("MCP state is stale; reload before making changes".into());
        }
        Ok(state)
    }

    fn change<F, M>(
        &self,
        store: &Store,
        previous: McpState,
        stop: F,
        mutate: M,
    ) -> Result<(), String>
    where
        F: Fn() -> Result<(), String>,
        M: FnOnce(&Store) -> Result<(), String>,
    {
        self.write_policy(previous.generation + 1, &[])?;
        if let Err(error) = self.clear_reports() {
            return match stop() {
                Ok(()) => self.restore_files(store, error, &stop),
                Err(stop_error) => self.force_closed(
                    &stop,
                    format!(
                        "{error}; could not stop active MCP clients after report invalidation failed: {stop_error}"
                    ),
                ),
            };
        }
        if let Err(error) = stop() {
            return self.restore_files(store, error, &stop);
        }
        if let Err(error) = mutate(store) {
            return self.restore_files(store, error, &stop);
        }
        if let Err(error) = self.materialize(store) {
            let rollback = match store.restore_mcp_state(&previous) {
                Ok(generation) => generation,
                Err(failure) => {
                    return self.force_closed(
                        &stop,
                        format!("{error}; MCP database rollback failed: {failure}"),
                    )
                }
            };
            return match self.materialize(store) {
                Ok(()) => Err(error),
                Err(failure) => self.force_closed(
                    &stop,
                    format!(
                        "{error}; restored database generation {rollback}, but policy rollback failed: {failure}"
                    ),
                ),
            };
        }
        Ok(())
    }

    fn restore_files<F>(&self, store: &Store, error: String, stop: &F) -> Result<(), String>
    where
        F: Fn() -> Result<(), String>,
    {
        match self.materialize(store) {
            Ok(()) => Err(error),
            Err(failure) => self.force_closed(
                stop,
                format!("{error}; could not restore MCP policy: {failure}"),
            ),
        }
    }

    fn force_closed<F>(&self, stop: &F, error: String) -> Result<(), String>
    where
        F: Fn() -> Result<(), String>,
    {
        let sentinel = write_json(
            &self.root.join(SENTINEL_FILE),
            &json!({ "version": 1, "failClosed": true }),
        );
        let policy = write_json(
            &self.root.join("mcp-approvals.json"),
            &json!({ "version": 0, "generation": -1, "decisions": [] }),
        );
        let reports = self.clear_reports();
        let stopped = stop();
        let failures = [
            sentinel.err().map(|failure| format!("sentinel: {failure}")),
            policy.err().map(|failure| format!("policy: {failure}")),
            reports.err().map(|failure| format!("reports: {failure}")),
            stopped
                .err()
                .map(|failure| format!("active instances: {failure}")),
        ]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>();
        if failures.is_empty() {
            return Err(format!("{error}; MCP was forced closed"));
        }
        Err(format!(
            "{error}; MCP fail-closed recovery was incomplete: {}",
            failures.join("; ")
        ))
    }

    fn clear_sentinel(&self) -> Result<(), String> {
        match std::fs::remove_file(self.root.join(SENTINEL_FILE)) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error.to_string()),
        }
    }

    fn write_policy(
        &self,
        generation: i64,
        decisions: &[crate::store::McpDecision],
    ) -> Result<(), String> {
        write_json(
            &self.root.join("mcp-approvals.json"),
            &json!({
                "version": POLICY_VERSION,
                "generation": generation,
                "decisions": decisions.iter().map(|item| json!({
                    "fingerprint": item.fingerprint,
                    "decision": item.decision,
                })).collect::<Vec<_>>()
            }),
        )
    }

    fn base_config(&self) -> Result<Value, String> {
        let raw = std::fs::read_to_string(self.extensions.join("opencode.json"))
            .map_err(|error| error.to_string())?;
        serde_json::from_str(&raw).map_err(|error| error.to_string())
    }

    fn plugin_path(&self, name: &str) -> Result<String, String> {
        ["js", "ts"]
            .into_iter()
            .map(|extension| {
                self.extensions
                    .join("plugin")
                    .join(format!("{name}.{extension}"))
            })
            .find(|path| path.is_file())
            .map(|path| path.to_string_lossy().to_string())
            .ok_or_else(|| format!("Drift engine extension {name} is missing"))
    }

    fn report(&self, directory: &str, generation: i64) -> Result<Option<PendingReport>, String> {
        let path = report_path(&self.root.join("pending"), directory);
        let raw = match std::fs::read_to_string(path) {
            Ok(raw) => raw,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error.to_string()),
        };
        if raw.len() > 1_048_576 {
            return Err("MCP approval report exceeds 1 MiB".into());
        }
        let report: PendingReport =
            serde_json::from_str(&raw).map_err(|_| "MCP approval report is malformed")?;
        if report.version != POLICY_VERSION
            || report.generation != generation
            || normalize_directory(&report.directory) != normalize_directory(directory)
        {
            return Err("MCP approval report is stale".into());
        }
        let mut names = std::collections::HashSet::new();
        if report.servers.iter().any(|server| {
            !names.insert(server.name.as_str())
                || !matches!(server.transport.as_str(), "local" | "remote")
                || !valid_fingerprint(&server.fingerprint)
        }) {
            return Err("MCP approval report contains invalid entries".into());
        }
        Ok(Some(report))
    }

    fn clear_reports(&self) -> Result<(), String> {
        let entries = match std::fs::read_dir(self.root.join("pending")) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(error.to_string()),
        };
        for entry in entries {
            let path = entry.map_err(|error| error.to_string())?.path();
            if path.is_file() {
                std::fs::remove_file(path).map_err(|error| error.to_string())?;
            }
        }
        Ok(())
    }
}

fn validate_name(name: &str) -> Result<(), String> {
    if name.is_empty() || name.len() > 128 || name.chars().any(char::is_control) {
        return Err("MCP server name must be 1-128 characters without control characters".into());
    }
    Ok(())
}

fn validate_prompt_key(key: &str) -> Result<(), String> {
    if let Some(family) = key.strip_prefix("family:") {
        if [
            "meta",
            "beast",
            "codex",
            "gpt",
            "gemini",
            "anthropic",
            "trinity",
            "kimi",
            "default",
        ]
        .contains(&family)
        {
            return Ok(());
        }
        return Err("Unknown model prompt family".into());
    }
    if let Some(agent) = key.strip_prefix("agent:") {
        if !agent.is_empty()
            && agent.len() <= 128
            && !agent.chars().any(char::is_control)
            && !["__proto__", "constructor", "prototype"].contains(&agent)
        {
            return Ok(());
        }
    }
    Err("Prompt override key is invalid".into())
}

fn validate_prompt_override(key: &str, value: &Value) -> Result<(), String> {
    validate_prompt_key(key)?;
    let size = serde_json::to_vec(value)
        .map_err(|error| error.to_string())?
        .len();
    if size > 262_144 {
        return Err("Prompt override exceeds 256 KiB".into());
    }
    reject_unsafe_keys(value)?;
    if key.starts_with("family:") && !value.is_string() {
        return Err("Model-family prompts must be text".into());
    }
    if key.starts_with("agent:") && !value.is_object() {
        return Err("Agent overrides must be JSON objects".into());
    }
    if let Some(agent) = value.as_object().filter(|_| key.starts_with("agent:")) {
        validate_agent_override(agent)?;
    }
    Ok(())
}

fn validate_agent_override(agent: &Map<String, Value>) -> Result<(), String> {
    if agent.contains_key("name") {
        return Err("Agent names are controlled by the configuration key".into());
    }
    for field in ["prompt", "description", "model", "variant", "color"] {
        if agent.get(field).is_some_and(|value| !value.is_string()) {
            return Err(format!("Agent {field} must be text"));
        }
    }
    for field in ["hidden", "disable"] {
        if agent.get(field).is_some_and(|value| !value.is_boolean()) {
            return Err(format!("Agent {field} must be true or false"));
        }
    }
    if agent
        .get("mode")
        .is_some_and(|value| !matches!(value.as_str(), Some("primary" | "subagent" | "all")))
    {
        return Err("Agent mode must be primary, subagent, or all".into());
    }
    for field in ["temperature", "top_p"] {
        if agent.get(field).is_some_and(|value| !value.is_number()) {
            return Err(format!("Agent {field} must be a number"));
        }
    }
    for field in ["steps", "maxSteps"] {
        if agent
            .get(field)
            .is_some_and(|value| !matches!(value.as_u64(), Some(steps) if steps > 0))
        {
            return Err(format!("Agent {field} must be a positive integer"));
        }
    }
    if agent.get("options").is_some_and(|value| !value.is_object()) {
        return Err("Agent options must be a JSON object".into());
    }
    if let Some(tools) = agent.get("tools") {
        let Some(tools) = tools.as_object() else {
            return Err("Agent tools must be a JSON object".into());
        };
        if tools.values().any(|value| !value.is_boolean()) {
            return Err("Agent tool values must be true or false".into());
        }
    }
    if let Some(color) = agent.get("color").and_then(Value::as_str) {
        let named = [
            "primary",
            "secondary",
            "accent",
            "success",
            "warning",
            "error",
            "info",
        ];
        let hex = color.len() == 7
            && color.starts_with('#')
            && color[1..].bytes().all(|byte| byte.is_ascii_hexdigit());
        if !hex && !named.contains(&color) {
            return Err("Agent color must be a six-digit hex or theme color".into());
        }
    }
    if let Some(permission) = agent.get("permission") {
        validate_permission(permission)?;
    }
    Ok(())
}

fn validate_permission(value: &Value) -> Result<(), String> {
    if value.as_str().is_some_and(permission_action) {
        return Ok(());
    }
    let Some(rules) = value.as_object() else {
        return Err("Agent permission must be ask, allow, deny, or a JSON object".into());
    };
    for rule in rules.values() {
        if rule.as_str().is_some_and(permission_action) {
            continue;
        }
        let Some(patterns) = rule.as_object() else {
            return Err("Agent permission rules must contain ask, allow, or deny".into());
        };
        if patterns
            .values()
            .any(|action| !action.as_str().is_some_and(permission_action))
        {
            return Err("Agent permission rules must contain ask, allow, or deny".into());
        }
    }
    Ok(())
}

fn permission_action(value: &str) -> bool {
    matches!(value, "ask" | "allow" | "deny")
}

fn validate_config(config: &Value) -> Result<(), String> {
    if serde_json::to_vec(config)
        .map_err(|error| error.to_string())?
        .len()
        > MAX_CONFIG_BYTES
    {
        return Err("MCP configuration exceeds 64 KiB".into());
    }
    reject_unsafe_keys(config)?;
    let object = config
        .as_object()
        .ok_or("MCP configuration must be a JSON object")?;
    validate_enabled_timeout(object)?;
    match object.get("type").and_then(Value::as_str) {
        Some("local") => validate_local(object),
        Some("remote") => validate_remote(object),
        _ => Err("MCP configuration type must be local or remote".into()),
    }
}

fn validate_local(config: &Map<String, Value>) -> Result<(), String> {
    let command = config
        .get("command")
        .and_then(Value::as_array)
        .filter(|command| !command.is_empty())
        .ok_or("Local MCP command must be a non-empty string array")?;
    if command.len() > 128
        || command
            .iter()
            .any(|part| part.as_str().is_none_or(|part| part.len() > 4096))
        || command[0]
            .as_str()
            .is_none_or(|part| part.trim().is_empty())
    {
        return Err("Local MCP command contains invalid arguments".into());
    }
    validate_optional_string(config.get("cwd"), "working directory", 4096)?;
    validate_string_map(config.get("environment"), "environment")
}

fn validate_remote(config: &Map<String, Value>) -> Result<(), String> {
    let url = config
        .get("url")
        .and_then(Value::as_str)
        .ok_or("Remote MCP URL is required")?;
    let parsed = url::Url::parse(url).map_err(|_| "Remote MCP URL is invalid")?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return Err("Remote MCP URL must use HTTP or HTTPS".into());
    }
    validate_string_map(config.get("headers"), "headers")?;
    validate_oauth(config.get("oauth"))
}

fn validate_enabled_timeout(config: &Map<String, Value>) -> Result<(), String> {
    if config
        .get("enabled")
        .is_some_and(|value| !value.is_boolean())
    {
        return Err("MCP enabled must be a boolean".into());
    }
    if config.get("timeout").is_some_and(|value| {
        value
            .as_u64()
            .is_none_or(|timeout| timeout == 0 || timeout > 9_007_199_254_740_991)
    }) {
        return Err("MCP timeout must be a positive safe integer".into());
    }
    Ok(())
}

fn validate_oauth(value: Option<&Value>) -> Result<(), String> {
    let Some(value) = value else { return Ok(()) };
    if value == &Value::Bool(false) {
        return Ok(());
    }
    let oauth = value
        .as_object()
        .ok_or("MCP OAuth must be an object or false")?;
    for field in ["clientId", "clientSecret", "scope", "redirectUri"] {
        validate_optional_string(oauth.get(field), &format!("OAuth {field}"), 16_384)?;
    }
    if oauth.get("callbackPort").is_some_and(|value| {
        value
            .as_u64()
            .is_none_or(|port| !(1..=65_535).contains(&port))
    }) {
        return Err("MCP OAuth callback port must be between 1 and 65535".into());
    }
    Ok(())
}

fn validate_optional_string(value: Option<&Value>, field: &str, max: usize) -> Result<(), String> {
    if value.is_some_and(|value| value.as_str().is_none_or(|value| value.len() > max)) {
        return Err(format!(
            "MCP {field} must be a string no longer than {max} characters"
        ));
    }
    Ok(())
}

fn validate_string_map(value: Option<&Value>, field: &str) -> Result<(), String> {
    let Some(value) = value else { return Ok(()) };
    let object = value
        .as_object()
        .ok_or_else(|| format!("MCP {field} must be a JSON object"))?;
    if object.len() > 128
        || object.iter().any(|(key, value)| {
            key.len() > 256 || value.as_str().is_none_or(|value| value.len() > 16_384)
        })
    {
        return Err(format!("MCP {field} contains invalid entries"));
    }
    Ok(())
}

fn reject_unsafe_keys(value: &Value) -> Result<(), String> {
    match value {
        Value::Object(object) => {
            for (key, value) in object {
                if matches!(key.as_str(), "__proto__" | "prototype" | "constructor") {
                    return Err("MCP configuration contains an unsafe property name".into());
                }
                reject_unsafe_keys(value)?;
            }
        }
        Value::Array(values) => {
            for value in values {
                reject_unsafe_keys(value)?;
            }
        }
        _ => {}
    }
    Ok(())
}

fn valid_fingerprint(value: &str) -> bool {
    value.len() == 71
        && value.starts_with("sha256:")
        && value[7..]
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
}

fn normalize_directory(value: &str) -> String {
    let normalized = value.replace('\\', "/").trim_end_matches('/').to_string();
    if cfg!(windows) {
        return normalized.to_ascii_lowercase();
    }
    normalized
}

fn report_path(root: &Path, directory: &str) -> PathBuf {
    let hash = Sha256::digest(normalize_directory(directory).as_bytes());
    root.join(format!("{hash:x}.json"))
}

fn write_json(path: &Path, value: &Value) -> Result<(), String> {
    let contents = serde_json::to_vec_pretty(value).map_err(|error| error.to_string())?;
    let parent = path.parent().ok_or("generated MCP path has no parent")?;
    std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    let name = path
        .file_name()
        .ok_or("generated MCP path has no filename")?;
    let temporary = parent.join(format!(
        ".{}.{}.{}.tmp",
        name.to_string_lossy(),
        std::process::id(),
        TEMP_ID.fetch_add(1, Ordering::Relaxed)
    ));
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temporary)
        .map_err(|error| error.to_string())?;
    let written = file.write_all(&contents).and_then(|_| file.sync_all());
    drop(file);
    let result = written.and_then(|_| replace_file(&temporary, path));
    if let Err(error) = result {
        let _ = std::fs::remove_file(&temporary);
        return Err(error.to_string());
    }
    Ok(())
}

#[cfg(windows)]
fn replace_file(source: &Path, destination: &Path) -> std::io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::{
        MoveFileExW, MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH,
    };

    let source = source
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect::<Vec<_>>();
    let destination = destination
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect::<Vec<_>>();
    let result = unsafe {
        MoveFileExW(
            source.as_ptr(),
            destination.as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    };
    if result == 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(())
}

#[cfg(not(windows))]
fn replace_file(source: &Path, destination: &Path) -> std::io::Result<()> {
    std::fs::rename(source, destination)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> (PathBuf, Store, McpRuntime) {
        let root = std::env::temp_dir().join(format!(
            "drift-mcp-runtime-test-{}-{}",
            std::process::id(),
            TEMP_ID.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::remove_dir_all(&root).ok();
        let extensions = root.join("extensions");
        std::fs::create_dir_all(extensions.join("plugin")).unwrap();
        std::fs::write(extensions.join("opencode.json"), r#"{"plugin":["auth@1"]}"#).unwrap();
        std::fs::write(extensions.join("plugin/spawn-thread.ts"), "export {}\n").unwrap();
        std::fs::write(extensions.join("plugin/mcp-approval.ts"), "export {}\n").unwrap();
        std::fs::write(extensions.join("plugin/prompt-overrides.ts"), "export {}\n").unwrap();
        std::fs::write(
            extensions.join("prompt-catalog.json"),
            r#"{"version":1,"families":[],"agents":[]}"#,
        )
        .unwrap();
        let store = crate::store::open(&root.join("data")).unwrap();
        let runtime = McpRuntime::new(&root.join("data"), extensions);
        (root, store, runtime)
    }

    #[test]
    fn validates_the_current_schema_and_preserves_top_level_extensions() {
        assert!(validate_config(&json!({
            "type": "local",
            "command": ["npx", "server"],
            "cwd": "./tools",
            "environment": { "TOKEN": "{env:TOKEN}" },
            "enabled": false,
            "timeout": 7_200_000,
            "vendor": { "mode": "fast" }
        }))
        .is_ok());
        assert!(validate_config(&json!({
            "type": "remote",
            "url": "https://example.com/mcp",
            "headers": { "Authorization": "Bearer {env:TOKEN}" },
            "oauth": { "clientId": "id", "callbackPort": 29418, "vendorOAuth": true }
        }))
        .is_ok());
        assert!(validate_config(&json!({ "type": "local", "command": [] })).is_err());
        assert!(validate_config(&json!({
            "type": "remote", "url": "http://192.0.2.10:8765/mcp"
        }))
        .is_ok());
        assert!(
            validate_config(&json!({ "type": "remote", "url": "http://127.0.0.2:3000" })).is_ok()
        );
        assert!(validate_config(&json!({ "type": "remote", "url": "http://[::1]:3000" })).is_ok());
        assert!(
            validate_config(&json!({ "type": "remote", "url": "ftp://example.com" })).is_err()
        );
        assert!(validate_config(&json!({
            "type": "remote", "url": "https://example.com", "oauth": { "callbackPort": 65536 }
        }))
        .is_err());
    }

    #[test]
    fn materialization_replaces_files_without_touching_extensions() {
        let (root, store, runtime) = fixture();
        store
            .save_mcp_server(
                "docs",
                None,
                &json!({ "type": "remote", "url": "https://example.com", "unknown": true }),
            )
            .unwrap();
        runtime.materialize(&store).unwrap();
        runtime.materialize(&store).unwrap();

        let generated: Value = serde_json::from_str(
            &std::fs::read_to_string(runtime.config_dir().join("opencode.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(generated["mcp"]["docs"]["unknown"], true);
        assert_eq!(generated["plugin"][0], "auth@1");
        assert_eq!(
            std::fs::read_to_string(root.join("extensions/opencode.json")).unwrap(),
            r#"{"plugin":["auth@1"]}"#
        );
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn decisions_require_the_exact_pending_fingerprint_and_generation() {
        let (root, store, runtime) = fixture();
        runtime.materialize(&store).unwrap();
        let directory = "S:/repo";
        let fingerprint = "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
        write_json(
            &report_path(&runtime.root.join("pending"), directory),
            &json!({
                "version": 3,
                "generation": 0,
                "directory": directory,
                "servers": [{ "name": "docs", "type": "remote", "fingerprint": fingerprint, "decision": "pending" }]
            }),
        )
        .unwrap();

        assert!(runtime
            .decide(
                &store,
                directory,
                "other",
                fingerprint,
                0,
                McpDecision::Approved,
                || Ok(())
            )
            .is_err());
        runtime
            .decide(
                &store,
                directory,
                "docs",
                fingerprint,
                0,
                McpDecision::Rejected,
                || Ok(()),
            )
            .unwrap();
        assert_eq!(store.mcp_state().unwrap().decisions[0].decision, "rejected");
        assert!(runtime
            .decide(
                &store,
                directory,
                "docs",
                fingerprint,
                0,
                McpDecision::Approved,
                || Ok(())
            )
            .is_err());
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn invalid_observations_are_visible_but_cannot_be_decided() {
        let (root, store, runtime) = fixture();
        runtime.materialize(&store).unwrap();
        let directory = "S:/repo";
        let fingerprint = "sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
        write_json(
            &report_path(&runtime.root.join("pending"), directory),
            &json!({
                "version": 3,
                "generation": 0,
                "directory": directory,
                "servers": [{ "name": "insecure", "type": "remote", "fingerprint": fingerprint, "decision": "invalid" }]
            }),
        )
        .unwrap();

        let snapshot = runtime.snapshot(&store, directory).unwrap();
        assert_eq!(snapshot.observed.len(), 1);
        assert_eq!(snapshot.observed[0].decision, McpDecision::Invalid);
        assert!(runtime
            .decide(
                &store,
                directory,
                "insecure",
                fingerprint,
                0,
                McpDecision::Approved,
                || Ok(())
            )
            .is_err());
        assert!(runtime
            .revoke(&store, directory, "insecure", fingerprint, 0, || Ok(()))
            .is_err());
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn failed_materialization_restores_registry_content() {
        let (root, store, runtime) = fixture();
        let first = json!({ "type": "local", "command": ["first"] });
        store.save_mcp_server("server", None, &first).unwrap();
        runtime.materialize(&store).unwrap();
        let config_path = runtime.config_dir().join("opencode.json");
        std::fs::remove_file(&config_path).unwrap();
        std::fs::create_dir(&config_path).unwrap();

        let stops = std::cell::Cell::new(0);
        assert!(runtime
            .save(
                &store,
                "server",
                Some("server"),
                json!({ "type": "local", "command": ["second"] }),
                1,
                || {
                    stops.set(stops.get() + 1);
                    Ok(())
                }
            )
            .is_err());
        assert_eq!(store.mcp_state().unwrap().servers[0].config, first);
        assert_eq!(stops.get(), 2);
        assert!(runtime.config_dir().join(SENTINEL_FILE).is_file());
        let policy: Value = serde_json::from_str(
            &std::fs::read_to_string(runtime.config_dir().join("mcp-approvals.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(policy["version"], 0);
        assert!(std::fs::read_dir(runtime.config_dir())
            .unwrap()
            .all(|entry| !entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .ends_with(".tmp")));

        std::fs::remove_dir(&config_path).unwrap();
        runtime.materialize(&store).unwrap();
        assert!(!runtime.config_dir().join(SENTINEL_FILE).exists());
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn prompt_changes_preserve_pending_approval_reports() {
        let (root, store, runtime) = fixture();
        runtime.materialize(&store).unwrap();
        let directory = "S:/repo";
        let report = report_path(&runtime.root.join("pending"), directory);
        write_json(
            &report,
            &json!({ "version": 3, "generation": 0, "directory": directory, "servers": [] }),
        )
        .unwrap();

        runtime
            .save_prompt(
                &store,
                "family:gpt",
                json!("Custom prompt"),
                Some(json!("Original prompt")),
            )
            .unwrap();
        assert!(report.is_file());
        let generated: Value = serde_json::from_str(
            &std::fs::read_to_string(runtime.config_dir().join("prompt-overrides.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(generated["families"]["gpt"], "Custom prompt");

        runtime.reset_prompt(&store, "family:gpt").unwrap();
        assert!(report.is_file());
        assert!(store.prompt_overrides().unwrap().is_empty());
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn prompt_validation_matches_agent_configuration_shapes() {
        let (root, store, runtime) = fixture();
        runtime
            .save_prompt(
                &store,
                "agent:build",
                json!({ "permission": "deny", "color": "#a1B2c3", "tools": { "bash": false } }),
                None,
            )
            .unwrap();
        assert!(runtime
            .save_prompt(
                &store,
                "agent:build",
                json!({ "permission": { "bash": "sometimes" } }),
                None,
            )
            .is_err());
        assert!(runtime
            .save_prompt(&store, "agent:build", json!({ "color": "purple" }), None)
            .is_err());
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn windows_directory_normalization_is_ascii_only() {
        let input = r"S:\Ünicode\İ\Repo\";
        let expected = if cfg!(windows) {
            "s:/Ünicode/İ/repo"
        } else {
            "S:/Ünicode/İ/Repo"
        };
        assert_eq!(normalize_directory(input), expected);
    }
}
