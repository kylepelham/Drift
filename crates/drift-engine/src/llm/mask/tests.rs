use serde_json::json;

use super::*;
use crate::llm::Role;

fn masks() -> Masks {
    Masks::new(vec![
        ("sk-live-123".into(), "API_KEY".into()),
        ("sk-live-1234".into(), "LONG_KEY".into()),
        ("hunter2".into(), "PASSWORD".into()),
    ])
    .0
}

#[test]
fn secrets_become_placeholders_longest_first_and_come_back_in_tool_input() {
    let masks = masks();
    assert_eq!(
        masks.hide("key=sk-live-1234 and sk-live-123, pw hunter2"),
        "key=LONG_KEY and API_KEY, pw PASSWORD"
    );
    assert_eq!(masks.hide("nothing here"), "nothing here");

    let mut input = json!({ "command": "curl -H 'auth: API_KEY' x", "env": { "PASSWORD": ["PASSWORD"] } });
    masks.reveal(&mut input);
    assert_eq!(
        input,
        json!({ "command": "curl -H 'auth: sk-live-123' x", "env": { "hunter2": ["hunter2"] } })
    );
}

#[test]
fn a_request_carries_no_secret_anywhere_and_keeps_signatures_and_file_data() {
    let request = Request {
        model: "m".into(),
        system: "Use hunter2.".into(),
        messages: vec![ChatMessage {
            role: Role::Assistant,
            blocks: vec![
                Block::Text("said sk-live-123".into()),
                Block::Signed {
                    part: Box::new(Block::ToolUse {
                        id: "c".into(),
                        name: "bash".into(),
                        input: json!({ "command": "echo hunter2" }),
                    }),
                    signature: "sig-hunter2".into(),
                },
                Block::ToolResult {
                    call_id: "c".into(),
                    content: "hunter2\n".into(),
                    is_error: true,
                },
                Block::Image {
                    mime: "image/png".into(),
                    base64: "hunter2".into(),
                },
            ],
        }],
        tools: vec![ToolSpec {
            name: "t".into(),
            description: "token hunter2".into(),
            input_schema: json!({ "default": "hunter2" }),
        }],
        max_tokens: 1,
        reasoning: None,
        temperature: None,
        cache_key: None,
        connection: None,
        effort_marks: Vec::new(),
        claude: Default::default(),
        no_tool_calls: false,
        verbosity: None,
        show_thinking: false,
        top_p: None,
        top_k: None,
        mode: None,
        context_window: None,
    };

    let hidden = masks().request(&request).into_owned();
    let text = format!("{:?}", (&hidden.system, &hidden.messages, &hidden.tools));
    assert_eq!(
        text.matches("hunter2").count(),
        2,
        "only the signature and the image data keep it: {text}"
    );
    assert!(text.contains("sig-hunter2") && text.contains("base64: \"hunter2\""));
    assert!(!text.contains("sk-live-123"));
}

#[test]
fn a_placeholder_holding_a_secret_or_a_repeated_secret_or_placeholder_is_refused() {
    let (masks, problems) = Masks::new(vec![
        ("abc".into(), "X".into()),
        ("def".into(), "has-abc".into()),
        ("abc".into(), "Y".into()),
        ("".into(), "Z".into()),
        ("ghi".into(), "X".into()),
    ]);

    assert_eq!(masks.len(), 1);
    assert_eq!(problems.len(), 4, "{problems:?}");
    assert_eq!(
        masks.hide("abc def ghi"),
        "X def ghi",
        "a placeholder two secrets share could only reveal one"
    );
    for secret in ["abc", "def", "ghi"] {
        assert!(
            problems.iter().all(|problem| !problem.contains(secret)),
            "{problems:?} names {secret}"
        );
    }
}
