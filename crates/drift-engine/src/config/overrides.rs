//! The user's per-agent choices from Settings, applied over whatever defines the agent.

use std::collections::HashMap;

use serde_json::Value;

use super::{parse_model, Config};
use crate::session::types::ModelRef;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct AgentOverride {
    pub model: Option<ModelPin>,
    pub prompt: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum ModelPin {
    /// Drop any pinned model so the agent inherits its default again.
    Inherit,
    Use(ModelRef),
}

impl AgentOverride {
    /// Reads the stored Settings value: `model` is `provider/model`, or empty to inherit.
    pub fn from_json(value: &Value) -> Self {
        let model = value.get("model").and_then(Value::as_str).map(|model| match parse_model(model) {
            Some(model) => ModelPin::Use(model),
            None => ModelPin::Inherit,
        });
        let prompt = value.get("prompt").and_then(Value::as_str).map(str::to_string);
        Self { model, prompt }
    }
}

impl Config {
    /// Settings win over workspace and built-in definitions; overrides for unknown agents are ignored.
    pub fn apply_overrides(&mut self, overrides: &HashMap<String, AgentOverride>) {
        for agent in &mut self.agents {
            let Some(chosen) = overrides.get(&agent.name) else { continue };
            match &chosen.model {
                Some(ModelPin::Use(model)) => agent.model = Some(model.clone()),
                Some(ModelPin::Inherit) => agent.model = None,
                None => {}
            }
            if let Some(prompt) = &chosen.prompt {
                agent.prompt = prompt.clone();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn settings_pin_clear_and_replace_agent_fields() {
        let mut config = Config::load(&std::env::temp_dir().join("drift-no-such-workspace"));
        let overrides = HashMap::from([
            ("title".to_string(), AgentOverride::from_json(&json!({ "model": "openai/gpt-5-nano" }))),
            ("plan".to_string(), AgentOverride::from_json(&json!({ "model": "", "prompt": "Plan briefly." }))),
            ("summary".to_string(), AgentOverride::from_json(&json!({ "model": "openai/gpt-5" }))),
        ]);
        config.agents.iter_mut().find(|a| a.name == "plan").unwrap().model = parse_model("anthropic/claude");
        config.apply_overrides(&overrides);
        assert_eq!(config.agent_model("title"), parse_model("openai/gpt-5-nano"));
        assert_eq!(config.agent_model("plan"), None, "an empty model restores inheritance");
        assert_eq!(config.agent("plan").unwrap().prompt, "Plan briefly.");
        assert!(config.agent("summary").is_none(), "an override never creates an agent");
    }
}
