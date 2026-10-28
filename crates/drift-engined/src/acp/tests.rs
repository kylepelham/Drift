//! An editor scripted over an in-memory pipe, against the engine with a scripted provider.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use drift_engine::Engine;
use drift_engine::llm::scripted::Scripted;
use drift_engine::llm::{Chunk, Credential, Provider, StopReason};
use drift_engine::permission::{Decision, Policy, Rule};
use drift_engine::session::types::{ModelRef, Usage};
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, DuplexStream, Lines};

use super::{Options, serve};

struct Editor {
    engine: Arc<Engine>,
    provider: Scripted,
    workspace: PathBuf,
    dir: PathBuf,
    to_agent: DuplexStream,
    from_agent: Lines<BufReader<DuplexStream>>,
    next_id: u64,
    /// The option this editor's user picks whenever the agent asks.
    answer: &'static str,
}

impl Drop for Editor {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

static EDITORS: AtomicU64 = AtomicU64::new(0);

async fn editor() -> Editor {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let unique = format!("{}-{}", std::process::id(), EDITORS.fetch_add(1, Ordering::SeqCst));
    let dir = drift_engine::platform::temp::test_run().join(format!("drift-acp-{unique}"));
    let workspace = dir.join("ws");
    std::fs::create_dir_all(&workspace).unwrap();
    let options = drift_engine::Options {
        file_credentials: true,
        ..Default::default()
    };
    let engine = Engine::open_with(&dir.join("data"), options).unwrap();
    let provider = Scripted::default();
    *engine.turns.provider_override.lock().unwrap() = Some(Provider::Scripted(provider.clone()));
    engine
        .credentials
        .set("anthropic", &Credential::ApiKey { key: "k".into() })
        .unwrap();

    let (to_agent, agent_in) = tokio::io::duplex(1 << 16);
    let (agent_out, from_agent) = tokio::io::duplex(1 << 16);
    let options = Options { model: Some(model()) };
    tokio::spawn(serve(engine.clone(), options, agent_in, agent_out));

    Editor {
        engine,
        provider,
        workspace,
        dir,
        to_agent,
        from_agent: BufReader::new(from_agent).lines(),
        next_id: 0,
        answer: "reject_once",
    }
}

impl Editor {
    async fn send(&mut self, message: Value) {
        let line = format!("{message}\n");
        self.to_agent.write_all(line.as_bytes()).await.unwrap();
    }

    async fn request(&mut self, method: &str, params: Value) -> u64 {
        self.next_id += 1;
        let id = self.next_id;
        self.send(json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params }))
            .await;
        id
    }

    async fn call(&mut self, method: &str, params: Value) -> (Vec<Value>, Value) {
        let id = self.request(method, params).await;
        self.until_response(id).await
    }

    /// Everything the agent sent before its response to `id`, answering its asks on the way.
    async fn until_response(&mut self, id: u64) -> (Vec<Value>, Value) {
        let mut seen = Vec::new();
        loop {
            let line = tokio::time::timeout(Duration::from_secs(20), self.from_agent.next_line())
                .await
                .expect("the agent went quiet")
                .unwrap()
                .expect("the agent hung up");
            let message: Value = serde_json::from_str(&line).expect("one JSON message per line");
            if message["id"] == id && message.get("method").is_none() {
                return (seen, message);
            }
            if message["method"] == "session/request_permission" {
                let outcome = json!({ "outcome": { "outcome": "selected", "optionId": self.answer } });
                self.send(json!({ "jsonrpc": "2.0", "id": message["id"], "result": outcome }))
                    .await;
            }
            seen.push(message);
        }
    }

    async fn new_session(&mut self) -> String {
        let cwd = self.workspace.to_string_lossy().into_owned();
        let (_, response) = self.call("session/new", json!({ "cwd": cwd, "mcpServers": [] })).await;
        let id = response["result"]["sessionId"].as_str().unwrap().to_string();
        // A titled session asks no model for a title, which would take a scripted reply meant for the turn.
        self.engine.store.update_session(&id, Some("Test"), None, None).unwrap();
        id
    }

    async fn prompt(&mut self, session: &str, text: &str) -> (Vec<Value>, Value) {
        let params = json!({ "sessionId": session, "prompt": [{ "type": "text", "text": text }] });
        self.call("session/prompt", params).await
    }

    /// A new session's prompt whose reply never comes, sent once its turn runs; its request id.
    async fn stalled_prompt(&mut self) -> (String, u64) {
        self.provider.push_stall();
        let session = self.new_session().await;
        let params = json!({ "sessionId": session, "prompt": [{ "type": "text", "text": "think forever" }] });
        let id = self.request("session/prompt", params).await;
        for _ in 0..500 {
            if self.engine.turns.is_running(&session) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }

        (session, id)
    }

    fn asks_for(&self, kind: &str) {
        let rule = Rule {
            kind: kind.into(),
            pattern: "*".into(),
            decision: Decision::Ask,
        };
        self.engine.permissions.set_policy(Policy {
            rules: vec![rule],
            ..Default::default()
        });
    }
}

fn model() -> ModelRef {
    ModelRef {
        provider: "anthropic".into(),
        model: "claude-sonnet-4-5".into(),
    }
}

fn text(text: &str) -> Vec<Chunk> {
    vec![
        Chunk::Usage(Usage {
            input: 10,
            ..Usage::default()
        }),
        Chunk::TextStart,
        Chunk::TextDelta(text.into()),
        Chunk::BlockStop,
        Chunk::Stop(StopReason::EndTurn),
    ]
}

fn tool_call(name: &str, input: &str) -> Vec<Chunk> {
    vec![
        Chunk::ToolUseStart {
            id: format!("toolu_{name}"),
            name: name.into(),
        },
        Chunk::ToolInputDelta(input.into()),
        Chunk::BlockStop,
        Chunk::Stop(StopReason::ToolUse),
    ]
}

/// The `session/update` payloads of one kind, in the order they came.
fn updates<'a>(messages: &'a [Value], kind: &str) -> Vec<&'a Value> {
    messages
        .iter()
        .filter(|message| message["method"] == "session/update")
        .map(|message| &message["params"]["update"])
        .filter(|update| update["sessionUpdate"] == kind)
        .collect()
}

fn said(messages: &[Value], kind: &str) -> String {
    updates(messages, kind)
        .iter()
        .filter_map(|update| update["content"]["text"].as_str())
        .collect()
}

#[tokio::test]
async fn initialize_and_a_new_session_open_the_cwd_as_a_workspace() {
    let mut editor = editor().await;
    let initialize = json!({ "protocolVersion": 1, "clientCapabilities": {} });
    let (_, initialized) = editor.call("initialize", initialize).await;
    assert_eq!(initialized["result"]["protocolVersion"], 1);
    assert_eq!(initialized["result"]["agentCapabilities"]["loadSession"], true);
    assert_eq!(initialized["result"]["authMethods"], json!([]));

    let session = editor.new_session().await;
    let stored = editor.engine.store.session(&session).unwrap().unwrap();
    let workspace = editor.engine.store.workspace(&stored.workspace_id).unwrap().unwrap();
    assert_eq!(PathBuf::from(&workspace.path), editor.workspace);
    assert_eq!(
        stored.model,
        Some(model()),
        "the --model flag picks the session's model"
    );

    let again = editor.new_session().await;
    let other = editor.engine.store.session(&again).unwrap().unwrap();
    assert_eq!(other.workspace_id, stored.workspace_id, "one workspace per directory");

    let (_, relative) = editor
        .call("session/new", json!({ "cwd": "ws", "mcpServers": [] }))
        .await;
    assert_eq!(relative["error"]["code"], -32602);
    let (_, unknown) = editor.call("session/fork", json!({})).await;
    assert_eq!(unknown["error"]["code"], -32601);
}

#[tokio::test]
async fn a_prompt_streams_its_tool_call_and_reply_then_ends_the_turn() {
    let mut editor = editor().await;
    std::fs::write(editor.workspace.join("a.txt"), "hello\n").unwrap();
    editor
        .provider
        .push(tool_call("read", r#"{"path":"a.txt"}"#))
        .push(text("It says hello."));
    let session = editor.new_session().await;

    let (seen, response) = editor.prompt(&session, "what is in a.txt?").await;

    assert_eq!(response["result"]["stopReason"], "end_turn");
    let call = updates(&seen, "tool_call")[0];
    assert_eq!(
        (&call["kind"], &call["status"], &call["name"]),
        (&json!("read"), &json!("pending"), &json!("read"))
    );
    let id = &call["toolCallId"];
    let changes: Vec<&Value> = updates(&seen, "tool_call_update")
        .into_iter()
        .filter(|update| &update["toolCallId"] == id)
        .collect();
    let statuses: Vec<&str> = changes.iter().filter_map(|update| update["status"].as_str()).collect();
    assert!(statuses.contains(&"in_progress"), "{statuses:?}");
    let done = changes.last().unwrap();
    assert_eq!(done["status"], "completed");
    assert_eq!(done["title"], "a.txt");
    assert!(
        done["content"][0]["content"]["text"]
            .as_str()
            .unwrap()
            .contains("hello")
    );
    assert!(done["locations"][0]["path"].as_str().unwrap().ends_with("a.txt"));
    assert_eq!(said(&seen, "agent_message_chunk"), "It says hello.");

    let position = |kind: &str| {
        seen.iter()
            .position(|message| message["params"]["update"]["sessionUpdate"] == kind)
            .unwrap()
    };
    assert!(
        position("tool_call") < position("agent_message_chunk"),
        "updates keep the turn's order"
    );
    assert!(
        seen.iter()
            .all(|message| message["params"]["sessionId"] == session.as_str()),
        "every update names the session"
    );
}

#[tokio::test]
async fn an_allowed_ask_runs_the_edit_and_shows_its_diff() {
    let mut editor = editor().await;
    editor.asks_for("edit");
    editor.answer = "allow_once";
    let file = editor.workspace.join("a.txt");
    std::fs::write(&file, "one\n").unwrap();
    editor
        .provider
        .push(tool_call(
            "edit",
            r#"{"path":"a.txt","old_string":"one","new_string":"two"}"#,
        ))
        .push(text("Done."));
    let session = editor.new_session().await;

    let (seen, response) = editor.prompt(&session, "change it").await;

    assert_eq!(response["result"]["stopReason"], "end_turn");
    let ask = seen
        .iter()
        .find(|message| message["method"] == "session/request_permission")
        .expect("the editor is asked");
    let call = updates(&seen, "tool_call")[0];
    assert_eq!(
        ask["params"]["toolCall"]["toolCallId"], call["toolCallId"],
        "the ask names the call shown"
    );
    let kinds: Vec<&str> = ask["params"]["options"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|option| option["kind"].as_str())
        .collect();
    assert!(
        kinds.contains(&"allow_once") && kinds.contains(&"reject_once"),
        "{kinds:?}"
    );
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "two\n");

    let done = *updates(&seen, "tool_call_update").last().unwrap();
    assert_eq!(done["status"], "completed");
    let diff = &done["content"][0];
    assert_eq!(
        (&diff["type"], &diff["oldText"], &diff["newText"]),
        (&json!("diff"), &json!("one"), &json!("two"))
    );
    assert!(diff["path"].as_str().unwrap().ends_with("a.txt"));
}

#[tokio::test]
async fn a_rejected_ask_fails_the_call_and_the_turn_goes_on() {
    let mut editor = editor().await;
    editor.asks_for("edit");
    let file = editor.workspace.join("a.txt");
    std::fs::write(&file, "one\n").unwrap();
    editor
        .provider
        .push(tool_call(
            "edit",
            r#"{"path":"a.txt","old_string":"one","new_string":"two"}"#,
        ))
        .push(text("Left it alone."));
    let session = editor.new_session().await;

    let (seen, response) = editor.prompt(&session, "change it").await;

    assert_eq!(response["result"]["stopReason"], "end_turn");
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "one\n");
    assert_eq!(updates(&seen, "tool_call_update").last().unwrap()["status"], "failed");
    assert_eq!(said(&seen, "agent_message_chunk"), "Left it alone.");
}

#[tokio::test]
async fn a_subagents_ask_reaches_the_editor_too() {
    let mut editor = editor().await;
    editor.asks_for("edit");
    editor.answer = "allow_once";
    let file = editor.workspace.join("a.txt");
    std::fs::write(&file, "one\n").unwrap();
    editor
        .provider
        .push(tool_call(
            "task",
            r#"{"description":"Change it","prompt":"Change one to two in a.txt"}"#,
        ))
        .push(tool_call(
            "edit",
            r#"{"path":"a.txt","old_string":"one","new_string":"two"}"#,
        ))
        .push(text("Changed."))
        .push(text("The subagent changed it."));
    let session = editor.new_session().await;

    let (seen, response) = editor.prompt(&session, "delegate the change").await;

    assert_eq!(response["result"]["stopReason"], "end_turn");
    let asks = seen
        .iter()
        .filter(|message| message["method"] == "session/request_permission")
        .count();
    assert_eq!(asks, 1, "the subagent's edit is put to the editor");
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "two\n");
    assert_eq!(updates(&seen, "tool_call")[0]["kind"], "think");
    assert_eq!(
        said(&seen, "agent_message_chunk"),
        "The subagent changed it.",
        "only the session's own reply streams"
    );
}

#[tokio::test]
async fn cancel_ends_the_turn_with_stop_reason_cancelled() {
    let mut editor = editor().await;
    let (session, id) = editor.stalled_prompt().await;

    let cancel = json!({ "jsonrpc": "2.0", "method": "session/cancel", "params": { "sessionId": session } });
    editor.send(cancel).await;
    let (_, response) = editor.until_response(id).await;

    assert_eq!(response["result"]["stopReason"], "cancelled");
    assert!(!editor.engine.turns.is_running(&session));

    editor.provider.push(text("Back again."));
    let (seen, again) = editor.prompt(&session, "hello").await;
    assert_eq!(
        again["result"]["stopReason"], "end_turn",
        "the session takes the next prompt"
    );
    assert_eq!(said(&seen, "agent_message_chunk"), "Back again.");
}

#[tokio::test]
async fn a_client_that_hangs_up_stops_its_turn_and_still_hears_the_end() {
    let mut editor = editor().await;
    let (session, id) = editor.stalled_prompt().await;

    editor.to_agent.shutdown().await.unwrap();
    let (_, response) = editor.until_response(id).await;

    assert_eq!(response["result"]["stopReason"], "cancelled");
    assert!(!editor.engine.turns.is_running(&session));
    let after = tokio::time::timeout(Duration::from_secs(10), editor.from_agent.next_line()).await;
    assert!(
        matches!(after, Ok(Ok(None))),
        "the agent closes its side once it has said everything"
    );
}

#[tokio::test]
async fn load_replays_the_conversation_and_checks_its_directory() {
    let mut editor = editor().await;
    editor.provider.push(text("Hi there."));
    let session = editor.new_session().await;
    editor.prompt(&session, "hello").await;

    let cwd = editor.workspace.to_string_lossy().into_owned();
    let params = json!({ "sessionId": session, "cwd": cwd, "mcpServers": [] });
    let (seen, response) = editor.call("session/load", params).await;

    assert_eq!(response["result"], json!({}));
    assert_eq!(said(&seen, "user_message_chunk"), "hello");
    assert_eq!(said(&seen, "agent_message_chunk"), "Hi there.");

    let elsewhere = editor.dir.to_string_lossy().into_owned();
    let params = json!({ "sessionId": session, "cwd": elsewhere, "mcpServers": [] });
    let (_, refused) = editor.call("session/load", params).await;
    assert_eq!(refused["error"]["code"], -32602);
}
