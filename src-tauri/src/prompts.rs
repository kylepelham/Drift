//! Agent overrides from Settings > Agents (`agent:<name>`): kept in Drift's store and handed to the
//! engine, which applies them from each agent's next turn. Base prompts are the engine's (`/prompts`).

use serde::Serialize;
use serde_json::{Map, Value};
use tauri::{AppHandle, State};

use crate::store::{PromptOverride, Store};

const MAX_OVERRIDES: usize = 128;
const MAX_OVERRIDE_BYTES: usize = 256 * 1024;
const MAX_AGENT_NAME_CHARS: usize = 128;
/// What the engine applies from an agent override (`AgentOverride`); a field it would ignore is refused, not stored.
const AGENT_FIELDS: [&str; 6] = ["prompt", "model", "steps", "tools", "permissions", "variant"];

#[derive(Debug, thiserror::Error)]
enum PromptError {
    #[error("An override key names one agent, as agent:<name>")]
    InvalidKey,
    #[error("An agent override holds at most {} KB", MAX_OVERRIDE_BYTES / 1024)]
    TooLarge,
    #[error("Agent overrides must be JSON objects")]
    NotObject,
    #[error("Agent {0} is not applied by the engine; use prompt, model, steps, tools, permissions or variant")]
    UnknownField(String),
    #[error("Agent {0} must be text")]
    NotText(&'static str),
    #[error("Agent steps must be a positive integer")]
    InvalidSteps,
    #[error("Agent tools must list one or more tool names")]
    InvalidTools,
    #[error("Agent permissions must list kind/pattern/decision rules")]
    InvalidPermissions,
    #[error(transparent)]
    Json(#[from] serde_json::Error),
}

#[derive(Serialize)]
pub(crate) struct PromptSnapshot {
    overrides: Vec<PromptOverride>,
}

#[tauri::command]
pub(crate) fn prompt_snapshot(store: State<Store>) -> Result<PromptSnapshot, String> {
    Ok(PromptSnapshot {
        overrides: store.prompt_overrides().map_err(|error| error.to_string())?,
    })
}

#[tauri::command]
pub(crate) fn prompt_save(
    app: AppHandle,
    store: State<Store>,
    key: String,
    value: Value,
    original: Option<Value>,
) -> Result<(), String> {
    validate(&key, &value).map_err(|error| error.to_string())?;
    if let Some(original) = &original {
        validate(&key, original).map_err(|error| error.to_string())?;
    }

    let overrides = store.prompt_overrides().map_err(|error| error.to_string())?;
    if overrides.len() >= MAX_OVERRIDES && !overrides.iter().any(|item| item.key == key) {
        return Err(format!("Drift keeps at most {MAX_OVERRIDES} agent overrides"));
    }
    store
        .save_prompt_override(&key, &value, original.as_ref())
        .map_err(|error| error.to_string())?;
    crate::native::push_agent_overrides(&app, &store).map_err(|error| error.to_string())
}

#[tauri::command]
pub(crate) fn prompt_reset(app: AppHandle, store: State<Store>, key: String) -> Result<(), String> {
    agent_name(&key).map_err(|error| error.to_string())?;
    store.reset_prompt_override(&key).map_err(|error| error.to_string())?;
    crate::native::push_agent_overrides(&app, &store).map_err(|error| error.to_string())
}

fn agent_name(key: &str) -> Result<&str, PromptError> {
    key.strip_prefix("agent:")
        .filter(|name| {
            !name.is_empty()
                && name.chars().count() <= MAX_AGENT_NAME_CHARS
                && name
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
        })
        .ok_or(PromptError::InvalidKey)
}

fn validate(key: &str, value: &Value) -> Result<(), PromptError> {
    agent_name(key)?;
    let encoded = serde_json::to_vec(value)?;
    if encoded.len() > MAX_OVERRIDE_BYTES {
        return Err(PromptError::TooLarge);
    }

    let agent = value.as_object().ok_or(PromptError::NotObject)?;
    fields(agent)
}

fn fields(agent: &Map<String, Value>) -> Result<(), PromptError> {
    if let Some(field) = agent.keys().find(|field| !AGENT_FIELDS.contains(&field.as_str())) {
        return Err(PromptError::UnknownField(field.clone()));
    }

    if let Some(field) = ["prompt", "model", "variant"]
        .into_iter()
        .find(|field| agent.get(*field).is_some_and(|value| !value.is_string()))
    {
        return Err(PromptError::NotText(field));
    }
    if agent
        .get("steps")
        .is_some_and(|value| !matches!(value.as_u64(), Some(steps) if steps > 0 && steps <= u64::from(u32::MAX)))
    {
        return Err(PromptError::InvalidSteps);
    }
    if agent.get("tools").is_some_and(|tools| {
        tools
            .as_array()
            .is_none_or(|names| names.is_empty() || !names.iter().all(Value::is_string))
    }) {
        return Err(PromptError::InvalidTools);
    }
    if let Some(permissions) = agent.get("permissions") {
        serde_json::from_value::<Vec<drift_engine::permission::Rule>>(permissions.clone())
            .map_err(|_| PromptError::InvalidPermissions)?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn only_what_the_engine_applies_to_one_named_agent_is_kept() {
        let valid = json!({
            "prompt": "Be brief.",
            "steps": 20,
            "tools": ["read"],
            "permissions": [{ "kind": "bash", "pattern": "*", "decision": "ask" }]
        });
        assert!(validate("agent:build", &valid).is_ok());
        for key in ["family:gpt", "agent:", "agent:a b", "build"] {
            assert!(validate(key, &json!({})).is_err(), "{key}");
        }
        for value in [
            json!("text"),
            json!({ "temperature": 0.2 }),
            json!({ "steps": 0 }),
            json!({ "tools": [] }),
            json!({ "model": 4 }),
            json!({ "permissions": [{ "kind": "bash" }] }),
        ] {
            assert!(validate("agent:build", &value).is_err(), "{value}");
        }
        assert!(
            validate("agent:build", &json!({ "prompt": "x".repeat(MAX_OVERRIDE_BYTES) })).is_err(),
            "too large"
        );
    }
}
