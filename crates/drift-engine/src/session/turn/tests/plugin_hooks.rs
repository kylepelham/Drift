use super::*;

struct Steward;
struct Gatekeeper;
struct Rewriter;

#[async_trait::async_trait]
impl crate::hook::Hook for Steward {
    fn name(&self) -> &str {
        "steward"
    }

    async fn prompt_submit(&self, prompt: &crate::hook::PromptEvent) -> crate::hook::PromptSubmit {
        match prompt.text.as_str() {
            "forbidden" => crate::hook::PromptSubmit::Deny("not in this workspace".into()),
            "shorthand" => crate::hook::PromptSubmit::Replace("the long form".into()),
            _ => crate::hook::PromptSubmit::AddContext("ticket 42 is about login".into()),
        }
    }

    async fn turn_end(&self, reply: &crate::hook::ReplyEvent) -> crate::hook::TurnEnd {
        if reply.text.contains("done") {
            crate::hook::TurnEnd::Note("heard it".into())
        } else {
            crate::hook::TurnEnd::Continue("say done".into())
        }
    }
}

#[async_trait::async_trait]
impl crate::hook::Hook for Gatekeeper {
    fn name(&self) -> &str {
        "gatekeeper"
    }

    async fn permission(&self, ask: &crate::hook::PermissionAsk) -> crate::hook::PermissionDecision {
        match ask.commands.as_deref() {
            Some([command]) if command.starts_with("echo ") => crate::hook::PermissionDecision::Allow,
            Some([command]) if command.starts_with("touch ") => {
                crate::hook::PermissionDecision::Deny("no new files today".into())
            }
            _ => crate::hook::PermissionDecision::Pass,
        }
    }
}

#[async_trait::async_trait]
impl crate::hook::Hook for Rewriter {
    fn name(&self) -> &str {
        "rewriter"
    }

    async fn before_tool(&self, call: &crate::hook::ToolCall) -> crate::hook::BeforeTool {
        match call.input["path"].as_str() {
            Some("secret.txt") => crate::hook::BeforeTool::Deny("secret.txt is off limits".into()),
            Some("b.txt") => crate::hook::BeforeTool::Replace(json!({ "path": "a.txt" })),
            Some("a.txt") => crate::hook::BeforeTool::Replace(json!({ "nonsense": true })),
            _ => crate::hook::BeforeTool::Allow,
        }
    }

    async fn after_tool(&self, result: &crate::hook::ToolResult) -> crate::hook::AfterTool {
        if result.failed {
            return crate::hook::AfterTool::Note("a plugin saw it fail".into());
        }

        if result.output.contains("alpha") {
            crate::hook::AfterTool::Note("read by a plugin too".into())
        } else {
            crate::hook::AfterTool::Keep
        }
    }
}

#[tokio::test]
async fn a_plugin_may_refuse_rewrite_or_add_context_to_a_prompt_and_keep_a_turn_going() {
    let h = harness().await;
    h.engine.hooks.set(vec![Arc::new(Steward)], vec![]);

    let refused = h.engine.submit(&h.session.id, prompt("forbidden")).await;
    let expected = "The steward plugin refused this prompt: not in this workspace";
    assert!(
        matches!(refused, Err(TurnError::Refused(ref reason)) if reason == expected),
        "{refused:?}"
    );
    assert!(transcript(&h).is_empty(), "nothing was written");

    h.provider.push(text("ok")).push(text("ok, done"));
    h.engine.submit(&h.session.id, prompt("shorthand")).await.await_ok();
    until_idle(&h).await;

    let messages = transcript(&h);
    assert_eq!(
        messages[0].parts[0].part,
        Part::Text {
            text: "the long form".into()
        },
        "the model reads the replacement"
    );
    assert_eq!(messages[1].parts[0].part, Part::Text { text: "ok".into() });
    assert_eq!(
        messages[2].parts[0].part,
        Part::Context {
            plugin: "steward".into(),
            text: "say done".into()
        },
        "the plugin kept the turn going"
    );
    assert_eq!(
        messages[3].parts[0].part,
        Part::Text {
            text: "ok, done".into()
        }
    );
    assert_eq!(
        messages[3].parts[1].part,
        Part::Context {
            plugin: "steward".into(),
            text: "heard it".into()
        },
        "a note sits under the reply"
    );
    assert_eq!(messages.len(), 4);
    assert_steward_replay(&h).await;
}

async fn assert_steward_replay(h: &Harness) {
    {
        let requests = h.provider.requests.lock().unwrap();
        assert!(
            matches!(&requests[1].messages[2].blocks[0], llm::Block::Text(text)
            if text.contains("From the steward plugin:\nsay done")),
            "{:?}",
            requests[1].messages[2].blocks
        );
    }

    h.provider.push(text("done"));
    h.engine.submit(&h.session.id, prompt("again")).await.await_ok();
    until_idle(h).await;
    {
        let requests = h.provider.requests.lock().unwrap();
        let replayed = requests[2]
            .messages
            .iter()
            .flat_map(|message| &message.blocks)
            .filter(|block| matches!(block, llm::Block::Text(text) if text.contains("heard it")))
            .count();
        assert_eq!(replayed, 0, "the model never reads a note");
    }

    h.provider.push(text("done"));
    h.engine
        .submit(&h.session.id, prompt("about the ticket"))
        .await
        .await_ok();
    until_idle(h).await;
    let messages = transcript(h);
    let user = &messages[4];
    assert_eq!(user.parts.len(), 2);
    assert_eq!(
        user.parts[1].part,
        Part::Context {
            plugin: "steward".into(),
            text: "ticket 42 is about login".into()
        },
        "context sits beside the prompt"
    );
}

#[tokio::test]
async fn a_plugin_answers_an_ask_the_rules_leave_to_the_user_and_never_overrides_a_rule() {
    let h = harness().await;
    h.engine.hooks.set(vec![Arc::new(Gatekeeper)], vec![]);
    h.engine.permissions.set_policy(Policy {
        rules: vec![
            Rule {
                kind: "bash".into(),
                pattern: "rm *".into(),
                decision: Decision::Deny,
            },
            Rule {
                kind: "bash".into(),
                pattern: "*".into(),
                decision: Decision::Ask,
            },
        ],
    });
    h.provider
        .push(tool_call("bash", r#"{"command": "echo hi"}"#))
        .push(tool_call("bash", r#"{"command": "touch made.txt"}"#))
        .push(tool_call("bash", r#"{"command": "rm -rf made.txt"}"#))
        .push(text("done"));
    h.engine.submit(&h.session.id, prompt("do things")).await.await_ok();
    until_idle(&h).await;

    let messages = transcript(&h);
    let allowed = tool(&messages[1].parts[0]);
    assert_eq!(
        allowed.status,
        ToolStatus::Done,
        "allowed by the plugin without asking: {:?}",
        allowed.output
    );
    assert!(h.engine.permissions.pending().is_empty(), "the user was never asked");
    let denied = tool(&messages[2].parts[0]);
    assert_eq!(
        (denied.status, denied.output),
        (
            ToolStatus::Denied,
            Some("The gatekeeper plugin refused this call: no new files today")
        )
    );
    assert!(!h._dir.join("ws/made.txt").exists());
    let forbidden = tool(&messages[3].parts[0]);
    assert_eq!(
        (forbidden.status, forbidden.output),
        (ToolStatus::Denied, Some("A permission rule forbids this call.")),
        "a deny rule is not the plugin's to answer"
    );
}

#[tokio::test]
async fn a_plugin_may_refuse_a_call_change_its_input_or_add_a_note_and_a_bad_rewrite_runs_nothing() {
    let h = harness().await;
    h.engine.hooks.set(vec![Arc::new(Rewriter)], vec![]);
    std::fs::write(h._dir.join("ws/a.txt"), "alpha\n").unwrap();
    std::fs::write(h._dir.join("ws/secret.txt"), "hidden\n").unwrap();
    h.provider
        .push(tool_call("read", r#"{"path": "secret.txt"}"#))
        .push(tool_call("read", r#"{"path": "b.txt"}"#))
        .push(tool_call("read", r#"{"path": "a.txt"}"#))
        .push(tool_call("bash", r#"{"command": "exit 3", "description": "fail"}"#))
        .push(text("done"));
    h.engine.submit(&h.session.id, prompt("read them")).await.await_ok();
    until_idle(&h).await;

    let messages = transcript(&h);
    let refused = tool(&messages[1].parts[0]);
    assert_eq!(
        (refused.status, refused.output),
        (
            ToolStatus::Error,
            Some("The rewriter plugin refused this call: secret.txt is off limits")
        )
    );
    let changed = tool(&messages[2].parts[0]);
    assert_eq!(
        (changed.status, changed.output),
        (ToolStatus::Done, Some("1: alpha\n\nrewriter: read by a plugin too"))
    );
    assert_eq!(changed.input["path"], "a.txt", "the stored call shows what ran");
    let invalid = tool(&messages[3].parts[0]);
    assert_eq!(invalid.status, ToolStatus::Error);
    assert!(
        invalid
            .output
            .unwrap_or_default()
            .starts_with("A plugin changed the call so it no longer fits the tool:"),
        "{:?}",
        invalid.output
    );
    let exited = tool(&messages[4].parts[0]);
    assert_eq!(
        exited.status,
        ToolStatus::Done,
        "a non-zero exit is a result to the model"
    );
    assert!(
        exited
            .output
            .unwrap_or_default()
            .ends_with("rewriter: a plugin saw it fail"),
        "but a failure to a plugin: {:?}",
        exited.output
    );
}
