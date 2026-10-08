//! What a subscription-authenticated request must look like: the shape Claude Code itself sends.

use std::fmt::Write as _;

use serde_json::{Value, json};

use super::oauth::sha256;

pub const BETAS: &str = "oauth-2025-04-20,interleaved-thinking-2025-05-14";
const IDENTITY: &str = "You are a Claude agent, built on Anthropic's Claude Agent SDK.";
const VERSION: &str = "2.1.280";
const ENTRYPOINT: &str = "sdk-cli";
const SALT: &str = "59cf53e54c78";
const SALT_POSITIONS: [usize; 3] = [4, 7, 20];
const TOOL_PREFIX: &str = "mcp_";

pub fn user_agent() -> String {
    format!("claude-cli/{VERSION} (external, cli)")
}

/// Rewrites a Messages API body in place: identity and billing system blocks, prefixed tool names.
pub fn transform(body: &mut Value) {
    let mut system = vec![json!({ "type": "text", "text": IDENTITY })];
    if let Some(text) = first_user_text(body) {
        system.insert(0, json!({ "type": "text", "text": billing(&text) }));
    }
    if let Some(existing) = body.get("system").and_then(Value::as_array) {
        system.extend(existing.iter().cloned());
    }
    body["system"] = Value::Array(system);

    if let Some(tools) = body.get_mut("tools").and_then(Value::as_array_mut) {
        for tool in tools {
            rename(&mut tool["name"]);
        }
    }

    if let Some(messages) = body.get_mut("messages").and_then(Value::as_array_mut) {
        for block in messages
            .iter_mut()
            .filter_map(|message| message["content"].as_array_mut())
            .flatten()
        {
            if block["type"] == "tool_use" {
                rename(&mut block["name"]);
            }
        }
    }
}

/// Undoes `rename` for names the model sends back.
pub fn original_name(name: &str) -> String {
    let Some(rest) = name.strip_prefix(TOOL_PREFIX) else {
        return name.to_string();
    };

    let mut chars = rest.chars();
    match chars.next() {
        Some(first) => first.to_lowercase().chain(chars).collect(),
        None => rest.to_string(),
    }
}

fn rename(name: &mut Value) {
    let Some(text) = name.as_str().filter(|name| !name.is_empty()) else {
        return;
    };

    let mut chars = text.chars();
    let renamed: String = chars
        .next()
        .map(|character| character.to_uppercase().collect::<String>())
        .unwrap_or_default()
        + chars.as_str();

    *name = Value::String(format!("{TOOL_PREFIX}{renamed}"));
}

fn first_user_text(body: &Value) -> Option<String> {
    let message = body["messages"]
        .as_array()?
        .iter()
        .find(|message| message["role"] == "user")?;
    let text = match &message["content"] {
        Value::String(text) => text.clone(),
        Value::Array(blocks) => blocks
            .iter()
            .find(|block| block["type"] == "text")
            .and_then(|block| block["text"].as_str())
            .map(str::to_string)
            .unwrap_or_default(),
        _ => String::new(),
    };

    Some(text)
}

fn billing(text: &str) -> String {
    let hash = hex(&sha256(text.as_bytes()));

    let mut salted = SALT.to_string();
    let chars: Vec<char> = text.chars().collect();
    for position in SALT_POSITIONS {
        salted.push(chars.get(position).copied().unwrap_or('0'));
    }
    salted.push_str(VERSION);
    let salted_hash = hex(&sha256(salted.as_bytes()));
    let suffix = &salted_hash[..3];

    format!(
        "x-anthropic-billing-header: cc_version={VERSION}.{suffix}; cc_entrypoint={ENTRYPOINT}; cch={};",
        &hash[..5]
    )
}

fn hex(bytes: &[u8]) -> String {
    let mut encoded = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        write!(encoded, "{byte:02x}").unwrap();
    }

    encoded
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn system_gets_billing_then_identity_then_ours() {
        let mut body = json!({
            "system": [{ "type": "text", "text": "You are Drift." }],
            "messages": [{ "role": "user", "content": [{ "type": "text", "text": "hello world" }] }],
            "tools": [{ "name": "bash" }, { "name": "read" }]
        });
        transform(&mut body);
        let system = body["system"].as_array().unwrap();
        assert_eq!(system.len(), 3);
        let billing = system[0]["text"].as_str().unwrap();
        assert!(
            billing.starts_with("x-anthropic-billing-header: cc_version=2.1.280."),
            "{billing}"
        );
        assert!(billing.contains("cc_entrypoint=sdk-cli; cch="));
        assert_eq!(system[1]["text"], IDENTITY);
        assert_eq!(system[2]["text"], "You are Drift.");
        assert_eq!(body["tools"][0]["name"], "mcp_Bash");
        assert_eq!(body["tools"][1]["name"], "mcp_Read");
    }

    #[test]
    fn tool_use_blocks_are_renamed_and_names_come_back() {
        let mut body = json!({
            "messages": [
                { "role": "user", "content": "x" },
                { "role": "assistant", "content": [{ "type": "tool_use", "name": "read", "id": "t" }] }
            ]
        });
        transform(&mut body);
        assert_eq!(body["messages"][1]["content"][0]["name"], "mcp_Read");
        assert_eq!(original_name("mcp_Read"), "read");
        assert_eq!(original_name("mcp_Bash"), "bash");
        assert_eq!(original_name("plain"), "plain");
    }

    #[test]
    fn cch_is_five_hex_of_the_first_user_text() {
        let text = billing("hello world");
        let expected = hex(&sha256(b"hello world"));
        assert!(text.ends_with(&format!("cch={};", &expected[..5])));
        assert!(!billing("").is_empty());
    }
}
