//! The user's per-agent choices from Settings, applied over whatever defines the agent.

use std::collections::HashMap;

use serde_json::Value;

use super::{parse_model, Config};
use crate::session::types::ModelRef;

/// Exactly what Settings may change on an agent; the shell refuses to store any other field.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct AgentOverride {
    pub model: Option<ModelPin>,
    pub prompt: Option<String>,
    /// Its own step limit, in place of the workspace's.
    pub steps: Option<u32>,
    /// The tool names it may use. Never empty: on an agent an empty list means every tool, so an override names them.
    pub tools: Option<Vec<String>>,
    pub permissions: Option<Vec<crate::permission::Rule>>,
    pub variant: Option<String>,
    pub problem: Option<String>,
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
        let mut problem = value.as_object().and_then(|map| map.keys().find(|key| !["model", "prompt", "steps", "tools", "permissions", "variant"].contains(&key.as_str()))).map(|key| format!("unsupported agent control {key}"));
        let model = value.get("model").and_then(Value::as_str).map(|model| match parse_model(model) {
            Some(model) => ModelPin::Use(model),
            None => ModelPin::Inherit,
        });
        let prompt = value.get("prompt").and_then(Value::as_str).map(str::to_string);
        let steps = value.get("steps").and_then(Value::as_u64).and_then(|steps| u32::try_from(steps).ok()).filter(|steps| *steps > 0);
        let tools = value
            .get("tools")
            .and_then(Value::as_array)
            .map(|tools| tools.iter().filter_map(Value::as_str).map(str::to_string).collect::<Vec<_>>())
            .filter(|tools| !tools.is_empty());
        let permissions = value.get("permissions").and_then(|value| match serde_json::from_value(value.clone()) {
            Ok(rules) => Some(rules),
            Err(_) => { problem = Some("permissions must be kind/pattern/decision rules".into()); None },
        });
        let variant = value.get("variant").and_then(Value::as_str).map(str::to_string);
        if value.get("variant").is_some() && variant.is_none() { problem = Some("variant must be text".into()); }
        Self { model, prompt, steps, tools, permissions, variant, problem }
    }
}

impl Config {
    /// Settings win over workspace and built-in definitions; overrides for unknown agents are ignored,
    /// and an invalid one refuses only its own agent.
    pub fn apply_overrides(&mut self, overrides: &HashMap<String, AgentOverride>) {
        for agent in &mut self.agents {
            let Some(chosen) = overrides.get(&agent.name) else { continue };
            if let Some(problem) = &chosen.problem {
                agent.problem = Some(format!("its Settings override is invalid ({problem}); fix or reset it"));
                continue;
            }
            match &chosen.model {
                Some(ModelPin::Use(model)) => agent.model = Some(model.clone()),
                Some(ModelPin::Inherit) => agent.model = None,
                None => {}
            }
            if let Some(prompt) = &chosen.prompt {
                agent.prompt = prompt.clone();
            }
            if let Some(steps) = chosen.steps {
                agent.steps = Some(steps);
            }
            if let Some(tools) = &chosen.tools {
                agent.tools = tools.clone();
            }
            if let Some(permissions) = &chosen.permissions { agent.permissions = permissions.clone(); }
            if let Some(variant) = &chosen.variant { agent.variant = (!variant.is_empty()).then(|| variant.clone()); }
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

    #[test]
    fn settings_rules_resolve_like_file_rules_and_keep_their_written_order() {
        use crate::permission::Decision;
        let mut config = Config::load(&std::env::temp_dir().join("drift-no-such-workspace"));
        let written = json!([{ "kind": "bash", "pattern": "*", "decision": "ask" }, { "kind": "bash", "pattern": "git *", "decision": "allow" }]);
        config.apply_overrides(&HashMap::from([("build".to_string(), AgentOverride::from_json(&json!({ "permissions": written })))]));
        let policy = config.agent_policy("build");
        assert_eq!(policy.explicit(&crate::tool::Ask::new("bash", "git status", "")), Some(Decision::Allow), "the later rule wins");
        assert_eq!(policy.explicit(&crate::tool::Ask::new("bash", "rm x", "")), Some(Decision::Ask));
        assert_eq!(serde_json::to_value(&config.agent("build").unwrap().permissions).unwrap(), written, "shown back as written, so a save round-trips");
    }

    #[test]
    fn settings_set_an_agents_step_limit_and_tools() {
        let mut config = Config::load(&std::env::temp_dir().join("drift-no-such-workspace"));
        let overrides = HashMap::from([
            ("general".to_string(), AgentOverride::from_json(&json!({ "steps": 7, "tools": ["read", "grep"] }))),
            ("plan".to_string(), AgentOverride::from_json(&json!({ "steps": 0, "tools": [] }))),
        ]);
        let plan_before = config.agent("plan").unwrap().clone();
        config.apply_overrides(&overrides);
        let general = config.agent("general").unwrap();
        assert_eq!((general.steps, general.tools.clone()), (Some(7), vec!["read".to_string(), "grep".to_string()]));
        let plan = config.agent("plan").unwrap();
        assert_eq!(plan.steps, plan_before.steps, "zero is no limit to set");
        assert_eq!(plan.tools, plan_before.tools, "an empty list changes nothing");
        assert!(plan.read_only, "and plan still only reads");
    }
}
