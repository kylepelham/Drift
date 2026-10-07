//! A Drift plugin that refuses shell commands which rewrite history, notes failed commands, and
//! runs the workspace's tests when a reply says `@guard test`, keeping the turn going if they fail.
//! Its drift.json entry may set the command: `{ "path": "plugins/guard.wasm", "config": { "test": ["cargo", "test"] } }`.
//! Build with `cargo build --release --target wasm32-wasip2`; the component is `target/wasm32-wasip2/release/guard.wasm`.

wit_bindgen::generate!({ world: "plugin", path: "../../crates/drift-engine/wit" });

use drift::plugin::host::{config, log, Level};
use drift::plugin::process::run;

struct Guard;

const REFUSED: &[&str] = &["git push --force", "git push -f", "git reset --hard", "git clean -f"];
const TRIGGER: &str = "@guard test";
const TEST_TIMEOUT_MS: u32 = 60_000;

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
            return AfterTool::Note("saw this command fail".into());
        }
        AfterTool::Keep
    }

    fn prompt_submit(_prompt: Prompt) -> PromptSubmit {
        PromptSubmit::Keep
    }

    fn turn_end(reply: Reply) -> TurnEnd {
        if !reply.text.contains(TRIGGER) {
            return TurnEnd::Accept;
        }
        let command = list_field(&config(), "test");
        let Some((program, args)) = command.split_first() else {
            log(Level::Warn, "no test command configured; set config.test to a program and its arguments");
            return TurnEnd::Accept;
        };
        match run(program, args, TEST_TIMEOUT_MS) {
            Ok(output) if output.code == 0 => TurnEnd::Accept,
            Ok(output) => TurnEnd::Continue(format!("The tests failed with exit code {}. Fix them before finishing.\n\n{}", output.code, tail(&output.stderr, &output.stdout))),
            Err(error) => TurnEnd::Continue(format!("The tests could not run: {error}")),
        }
    }

    fn session(session: Session, kind: SessionKind) {
        if kind == SessionKind::Created {
            log(Level::Info, &format!("session {} started in {}", session.id, session.workspace));
        }
    }
}

/// The last lines of a run's output: where a test runner says what failed.
fn tail(stderr: &str, stdout: &str) -> String {
    let text = if stderr.trim().is_empty() { stdout } else { stderr };
    let lines: Vec<&str> = text.lines().collect();
    lines[lines.len().saturating_sub(40)..].join("\n")
}

/// A string field of a JSON object, without a JSON parser: enough for a tool input's command.
fn field(json: &str, name: &str) -> String {
    let key = format!("\"{name}\":");
    let Some(start) = json.find(&key).map(|at| at + key.len()) else { return String::new() };
    let rest = json[start..].trim_start();
    let Some(rest) = rest.strip_prefix('"') else { return String::new() };
    string_at(rest).0
}

/// A field holding a list of strings.
fn list_field(json: &str, name: &str) -> Vec<String> {
    let key = format!("\"{name}\":");
    let Some(start) = json.find(&key).map(|at| at + key.len()) else { return Vec::new() };
    let Some(mut rest) = json[start..].trim_start().strip_prefix('[') else { return Vec::new() };
    let mut items = Vec::new();
    loop {
        rest = rest.trim_start_matches(|ch: char| ch.is_whitespace() || ch == ',');
        let Some(inner) = rest.strip_prefix('"') else { return items };
        let (item, after) = string_at(inner);
        items.push(item);
        rest = after;
    }
}

/// A JSON string's value from just after its opening quote, and the text after its closing quote.
fn string_at(text: &str) -> (String, &str) {
    let mut out = String::new();
    let mut chars = text.char_indices();
    while let Some((at, ch)) = chars.next() {
        match ch {
            '"' => return (out, &text[at + 1..]),
            '\\' => match chars.next() {
                Some((_, 'n')) => out.push('\n'),
                Some((_, 't')) => out.push('\t'),
                Some((_, other)) => out.push(other),
                None => break,
            },
            other => out.push(other),
        }
    }
    (out, "")
}

export!(Guard);
