//! Adds the Roc language: a grammar that highlights `.roc` files and `roc` code fences, and Roc's language
//! server, so an edit to a `.roc` file comes back with the errors it left. The server is
//! `roc_language_server` unless `config.server` names another command; an empty one turns it off.

wit_bindgen::generate!({ world: "languages", path: "../../crates/drift-engine/wit" });

use drift::plugin::host::config;
use drift::plugin::types::{SettingKind, TextSetting};

const SERVER: &str = "roc_language_server";

struct Roc;

impl Guest for Roc {
    fn name() -> String {
        "roc".into()
    }

    fn settings() -> Vec<Setting> {
        vec![Setting {
            key: "server".into(),
            label: "Language server".into(),
            description: "The command that starts Roc's language server, with its arguments; empty turns it off."
                .into(),
            kind: SettingKind::Text(TextSetting {
                default: SERVER.into(),
                placeholder: SERVER.into(),
                multiline: false,
                secret: false,
            }),
        }]
    }

    fn language_servers() -> Vec<LanguageServer> {
        let config: serde_json::Value = serde_json::from_str(&config()).unwrap_or_default();
        let command: Vec<String> = config["server"]
            .as_str()
            .unwrap_or(SERVER)
            .split_whitespace()
            .map(String::from)
            .collect();
        if command.is_empty() {
            return Vec::new();
        }

        vec![LanguageServer {
            id: "roc".into(),
            command,
            extensions: vec![".roc".into()],
            language: Some("roc".into()),
        }]
    }

    fn grammars() -> Vec<Grammar> {
        vec![Grammar {
            name: "roc".into(),
            aliases: Vec::new(),
            extensions: vec![".roc".into()],
            json: include_str!("../roc.tmLanguage.json").into(),
        }]
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

export!(Roc);
