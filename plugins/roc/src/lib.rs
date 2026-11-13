//! Adds the Roc language: a grammar that highlights `.roc` files and `roc` code fences, and Roc's language
//! server, `roc_language_server`, so an edit to a `.roc` file comes back with the errors it left. Drift starts it
//! only when it is on PATH, and Settings > Language servers turns it off.

wit_bindgen::generate!({ world: "languages", path: "../../crates/drift-engine/wit" });

const SERVER: &str = "roc_language_server";

struct Roc;

impl Guest for Roc {
    fn name() -> String {
        "roc".into()
    }

    fn settings() -> Vec<Setting> {
        Vec::new()
    }

    fn language_servers() -> Vec<LanguageServer> {
        vec![LanguageServer {
            id: "roc".into(),
            command: vec![SERVER.into()],
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
