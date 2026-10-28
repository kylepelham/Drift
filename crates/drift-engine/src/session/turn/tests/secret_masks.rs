use super::*;

const SECRET: &str = "sk-live-9f3a";
const PLACEHOLDER: &str = "API_KEY_PLACEHOLDER";

struct Secrets;

#[async_trait::async_trait]
impl crate::hook::Hook for Secrets {
    fn name(&self) -> &str {
        "secrets"
    }

    fn replacements(&self) -> Vec<(String, String)> {
        vec![(SECRET.into(), PLACEHOLDER.into())]
    }
}

#[tokio::test]
async fn the_model_never_sees_a_secret_and_its_tools_only_ever_get_the_real_one() {
    let h = harness().await;
    h.engine.hooks.set(vec![Arc::new(Secrets)], Vec::new());
    let allow = |kind: &str| Rule {
        kind: kind.into(),
        pattern: "*".into(),
        decision: Decision::Allow,
    };
    // One policy for both: a second `rule` call would replace the first.
    h.engine.permissions.set_policy(Policy {
        rules: vec![allow("edit"), allow("bash")],
        ..Default::default()
    });
    std::fs::write(h._dir.join("ws/AGENTS.md"), format!("Deploy with {SECRET}.\n")).unwrap();
    h.provider
        .push(tool_call(
            "write",
            &format!(r#"{{"path": "key.txt", "content": "key={PLACEHOLDER}\n"}}"#),
        ))
        .push(tool_call("read", r#"{"path": "key.txt"}"#))
        .push(tool_call("bash", r#"{"command": "cat key.txt && cat missing-file"}"#))
        .push(text("done"));

    h.engine
        .submit(&h.session.id, prompt(&format!("store {SECRET} in key.txt")))
        .await
        .unwrap();
    until_idle(&h).await;

    assert_eq!(
        std::fs::read_to_string(h._dir.join("ws/key.txt")).unwrap(),
        format!("key={SECRET}\n"),
        "the write ran with the real secret"
    );
    let requests = h.provider.requests.lock().unwrap().clone();
    assert_eq!(requests.len(), 4);
    for (index, request) in requests.iter().enumerate() {
        let seen = format!("{request:?}");
        assert!(!seen.contains(SECRET), "request {index} carried the secret");
    }
    assert!(
        requests[0].system.contains(&format!("Deploy with {PLACEHOLDER}")),
        "instructions are masked too"
    );
    let last = format!("{:?}", requests[3].messages);
    assert!(
        last.matches(PLACEHOLDER).count() >= 4,
        "the prompt, the call, the read and the shell output all show the placeholder: {last}"
    );

    let shell = transcript(&h)
        .iter()
        .flat_map(|message| message.parts.clone())
        .find_map(|row| match row.part {
            Part::ToolCall { name, output, .. } if name == "bash" => output,
            _ => None,
        })
        .unwrap();
    assert!(
        shell.contains(SECRET),
        "the user's own transcript keeps the real output: {shell}"
    );
}
