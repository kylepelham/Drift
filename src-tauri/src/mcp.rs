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

/// Files written under the runtime root. Each is referenced from more than one place, so they are
/// named here rather than repeated as literals.
const POLICY_FILE: &str = "mcp-approvals.json";
const CONFIG_FILE: &str = "opencode.json";
const CATALOG_FILE: &str = "prompt-catalog.json";
const PROMPT_OVERRIDES_FILE: &str = "prompt-overrides.json";
const PENDING_DIR: &str = "pending";

/// Server fingerprints are `sha256:` followed by a hex-encoded digest.
const FINGERPRINT_PREFIX: &str = "sha256:";
const FINGERPRINT_HEX_LEN: usize = 64;
const FINGERPRINT_LEN: usize = FINGERPRINT_PREFIX.len() + FINGERPRINT_HEX_LEN;

/// Ceilings on untrusted input. These bound what the frontend and on-disk config can push through
/// before it reaches the engine; they are not protocol limits. Several share the value 128 by
/// coincidence rather than by meaning, so each is named separately.
const MAX_PROMPT_OVERRIDES: usize = 128;
const MAX_PROMPT_OVERRIDE_BYTES: usize = 262_144;
const MAX_SERVER_NAME_CHARS: usize = 128;
const MAX_AGENT_NAME_CHARS: usize = 128;
const MAX_COMMAND_ARGS: usize = 128;
const MAX_STRING_MAP_ENTRIES: usize = 128;
const MAX_REPORT_BYTES: usize = 1_048_576;
const MAX_PATH_CHARS: usize = 4_096;
const MAX_STRING_MAP_KEY_CHARS: usize = 256;
const MAX_STRING_MAP_VALUE_CHARS: usize = 16_384;
const MAX_OAUTH_FIELD_CHARS: usize = 16_384;
/// Numbers in the MCP config round trip through JSON and JavaScript, so anything larger than the
/// f64 safe-integer range would come back altered.
const MAX_SAFE_INTEGER: u64 = 9_007_199_254_740_991;

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
        std::fs::create_dir_all(self.root.join(PENDING_DIR)).map_err(|error| error.to_string())?;
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
                "catalogPath": self.extensions.join(CATALOG_FILE).to_string_lossy(),
                "settingsPath": self.root.join(PROMPT_OVERRIDES_FILE).to_string_lossy()
            }
        ]));
        plugins.push(json!([
            self.plugin_path("mcp-approval")?,
            {
                "policyPath": self.root.join(POLICY_FILE).to_string_lossy(),
                "pendingDirectory": self.root.join(PENDING_DIR).to_string_lossy(),
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
            &self.root.join(PROMPT_OVERRIDES_FILE),
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
        write_json(&self.root.join(CONFIG_FILE), &config)?;
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
        let catalog = std::fs::read(self.extensions.join(CATALOG_FILE))
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
        if previous.is_none() && overrides.len() >= MAX_PROMPT_OVERRIDES {
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
            &self.root.join(POLICY_FILE),
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
            &self.root.join(POLICY_FILE),
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
        let raw = std::fs::read_to_string(self.extensions.join(CONFIG_FILE))
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
        let path = report_path(&self.root.join(PENDING_DIR), directory);
        let raw = match std::fs::read_to_string(path) {
            Ok(raw) => raw,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error.to_string()),
        };
        if raw.len() > MAX_REPORT_BYTES {
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
        let entries = match std::fs::read_dir(self.root.join(PENDING_DIR)) {
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
    if name.is_empty() || name.len() > MAX_SERVER_NAME_CHARS || name.chars().any(char::is_control) {
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
            && agent.len() <= MAX_AGENT_NAME_CHARS
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
    if size > MAX_PROMPT_OVERRIDE_BYTES {
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
    if command.len() > MAX_COMMAND_ARGS
        || command
            .iter()
            .any(|part| part.as_str().is_none_or(|part| part.len() > MAX_PATH_CHARS))
        || command[0]
            .as_str()
            .is_none_or(|part| part.trim().is_empty())
    {
        return Err("Local MCP command contains invalid arguments".into());
    }
    validate_optional_string(config.get("cwd"), "working directory", MAX_PATH_CHARS)?;
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
            .is_none_or(|timeout| timeout == 0 || timeout > MAX_SAFE_INTEGER)
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
        validate_optional_string(oauth.get(field), &format!("OAuth {field}"), MAX_OAUTH_FIELD_CHARS)?;
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
    if object.len() > MAX_STRING_MAP_ENTRIES
        || object.iter().any(|(key, value)| {
            key.len() > MAX_STRING_MAP_KEY_CHARS || value.as_str().is_none_or(|value| value.len() > MAX_STRING_MAP_VALUE_CHARS)
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
    value.len() == FINGERPRINT_LEN
        && value.starts_with(FINGERPRINT_PREFIX)
        && value[FINGERPRINT_PREFIX.len()..]
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
#[path = "mcp_tests.rs"]
mod tests;
