//! A Drift plugin that refuses shell commands which rewrite history. Build with
//! `cargo build --release --target wasm32-wasip2`; the component is `target/wasm32-wasip2/release/guard.wasm`.

wit_bindgen::generate!({ world: "plugin", path: "../../crates/drift-engine/wit" });

use drift::plugin::host::{log, Level};

struct Guard;

const REFUSED: &[&str] = &["git push --force", "git push -f", "git reset --hard", "git clean -f"];

impl Guest for Guard {
    fn name() -> String {
        "guard".into()
    }

    fn before_tool(call: ToolCall) -> BeforeTool {
        if call.tool != "bash" {
            return BeforeTool::Allow;
        }
        let command = field(&call.input, "command");
        match REFUSED.iter().find(|refused| command.contains(*refused)) {
            Some(refused) => BeforeTool::Deny(format!("`{refused}` rewrites history; ask the user to run it")),
            None => BeforeTool::Allow,
        }
    }

    fn after_tool(outcome: ToolResult) -> AfterTool {
        if outcome.tool == "bash" && outcome.failed {
            return AfterTool::Note("The guard plugin saw this command fail.".into());
        }
        AfterTool::Keep
    }

    fn session(session: Session, kind: SessionKind) {
        if kind == SessionKind::Created {
            log(Level::Info, &format!("session {} started in {}", session.id, session.workspace));
        }
    }
}

/// A string field of a JSON object, without a JSON parser: enough for a tool input's command.
fn field(json: &str, name: &str) -> String {
    let key = format!("\"{name}\":");
    let Some(start) = json.find(&key).map(|at| at + key.len()) else { return String::new() };
    let rest = json[start..].trim_start();
    let Some(rest) = rest.strip_prefix('"') else { return String::new() };
    let mut out = String::new();
    let mut chars = rest.chars();
    while let Some(ch) = chars.next() {
        match ch {
            '"' => break,
            '\\' => match chars.next() {
                Some('n') => out.push('\n'),
                Some('t') => out.push('\t'),
                Some(other) => out.push(other),
                None => break,
            },
            other => out.push(other),
        }
    }
    out
}

export!(Guard);
