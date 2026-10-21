//! Keeps secrets from the model: each secret in this plugin's config is swapped for its placeholder in
//! everything Drift sends to a model, and back in every tool call the model makes. Drift does the
//! swapping; this plugin only names the pairs, from `config.replacements` (`{ "<secret>": "<placeholder>" }`),
//! which Settings edits as rows of a secret and its placeholder.

wit_bindgen::generate!({ world: "masking", path: "../../crates/drift-engine/wit" });

use drift::plugin::host::{Level, config, log};
use drift::plugin::types::{PairsSetting, SettingKind};

struct Secrets;

impl Guest for Secrets {
    fn name() -> String {
        "secrets".into()
    }

    fn settings() -> Vec<Setting> {
        vec![Setting {
            key: "replacements".into(),
            label: "Secrets".into(),
            description: "Each secret is shown to the model as its placeholder, and put back in what the model runs."
                .into(),
            kind: SettingKind::Pairs(PairsSetting {
                default: Vec::new(),
                key_label: "Secret".into(),
                value_label: "Placeholder".into(),
                secret_keys: true,
                secret_values: false,
            }),
        }]
    }

    fn replacements() -> Vec<(String, String)> {
        let config: serde_json::Value = serde_json::from_str(&config()).unwrap_or_default();
        let Some(pairs) = config["replacements"].as_object() else {
            log(
                Level::Warn,
                "no replacements configured; add them in Settings > Plugins",
            );
            return Vec::new();
        };

        let mut found = Vec::new();
        for (secret, placeholder) in pairs {
            match placeholder.as_str() {
                Some(placeholder) => found.push((secret.clone(), placeholder.to_owned())),
                None => log(Level::Warn, "a placeholder is not text, so its secret is not hidden"),
            }
        }
        log(Level::Info, &format!("hiding {} secrets from the model", found.len()));

        found
    }

    fn before_tool(_call: ToolCall) -> BeforeTool {
        BeforeTool::Allow
    }

    fn after_tool(_outcome: ToolResult) -> AfterTool {
        AfterTool::Keep
    }

    fn prompt_submit(_prompt: Prompt) -> PromptSubmit {
        PromptSubmit::Keep
    }

    fn turn_end(_reply: Reply) -> TurnEnd {
        TurnEnd::Accept
    }

    fn permission(_ask: PermissionAsk) -> Permission {
        Permission::Pass
    }

    fn compaction(_compaction: Compaction) -> Compacting {
        Compacting::Proceed
    }

    fn session(_session: Session, _kind: SessionKind) {}
}

export!(Secrets);
