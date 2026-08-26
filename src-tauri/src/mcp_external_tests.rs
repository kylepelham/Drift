use super::*;
use serde_json::json;

/// Shared with tests/mcp.test.ts, which produces the same value through the mcp-approval plugin.
/// Both sides hashing one vector proves the Rust canonicalization matches the plugin's.
const PARITY_FINGERPRINT: &str =
    "sha256:933d9f99f6458ef8004d9f0e9b5fe8768211fe67a62e7baa87b08d8e9a5220dd";

fn parity_config() -> Value {
    json!({
        "type": "remote",
        "url": "https://example.com/mcp",
        "headers": { "Authorization": "Bearer x" },
        "enabled": true,
        "timeout": 30000
    })
}

const JSONC_FIXTURE: &str = r#"{
  // Keep this comment.
  "theme": "dark", /* and this one */
  "mcp": {
    "first": { "type": "local", "command": ["one"] }, // trailing note
    "docs": {
      "type": "remote",
      "url": "https://example.com/mcp",
      "headers": { "Authorization": "Bearer x" },
      "enabled": true,
      "timeout": 30000,
    },
    "last": { "type": "local", "command": ["three"] },
  },
  "other": ["a", "b",],
}
"#;

static FIXTURE_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

fn write_fixture(contents: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!(
        "drift-mcp-external-test-{}-{}.jsonc",
        std::process::id(),
        FIXTURE_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    std::fs::write(&path, contents).unwrap();
    path
}

#[test]
fn neutralization_preserves_offsets_strings_and_parses_strictly() {
    let neutral = neutralize_jsonc(JSONC_FIXTURE);
    assert_eq!(neutral.len(), JSONC_FIXTURE.len());
    // Comment content is gone, string content with slashes is not.
    assert!(!neutral.contains("Keep this comment"));
    assert!(!neutral.contains("and this one"));
    assert!(neutral.contains("https://example.com/mcp"));
    let parsed: Value = serde_json::from_str(&neutral).unwrap();
    assert_eq!(parsed["theme"], "dark");
    assert_eq!(parsed["other"], json!(["a", "b"]));

    let tricky = neutralize_jsonc(r#"{ "url": "http://x/*not a comment*/", "note": "// nope" }"#);
    let parsed: Value = serde_json::from_str(&tricky).unwrap();
    assert_eq!(parsed["url"], "http://x/*not a comment*/");
    assert_eq!(parsed["note"], "// nope");
}

#[test]
fn fingerprint_matches_the_plugin_parity_vector() {
    assert_eq!(
        fingerprint("docs", &parity_config()).as_deref(),
        Some(PARITY_FINGERPRINT)
    );
    // `enabled` is excluded from the effective definition, so toggling it keeps the fingerprint.
    let mut disabled = parity_config();
    disabled["enabled"] = json!(false);
    assert_eq!(
        fingerprint("docs", &disabled).as_deref(),
        Some(PARITY_FINGERPRINT)
    );
    // JSON.parse collapses whole floats before the plugin hashes, and Rust must agree.
    let mut float = parity_config();
    float["timeout"] = json!(30000.0);
    assert_eq!(
        fingerprint("docs", &float).as_deref(),
        Some(PARITY_FINGERPRINT)
    );
    assert_ne!(
        fingerprint("renamed", &parity_config()).as_deref(),
        Some(PARITY_FINGERPRINT)
    );
}

#[test]
fn locate_requires_the_exact_fingerprint() {
    let path = write_fixture(JSONC_FIXTURE);
    let files = vec![path.clone()];
    assert_eq!(locate(&files, "docs", PARITY_FINGERPRINT).len(), 1);
    assert!(locate(&files, "docs", "sha256:0000000000000000000000000000000000000000000000000000000000000000").is_empty());
    assert!(locate(&files, "missing", PARITY_FINGERPRINT).is_empty());
    std::fs::remove_file(path).ok();
}

#[test]
fn save_rewrites_only_the_member_and_preserves_comments() {
    let path = write_fixture(JSONC_FIXTURE);
    let located = locate(&[path.clone()], "docs", PARITY_FINGERPRINT);
    let replacement = json!({ "type": "remote", "url": "https://new.example.com" });
    let text = apply_save(&located[0], "docs", &replacement).unwrap();
    assert!(text.contains("Keep this comment"));
    assert!(text.contains("and this one"));
    assert!(text.contains("trailing note"));
    let parsed: Value = serde_json::from_str(&neutralize_jsonc(&text)).unwrap();
    assert_eq!(parsed["mcp"]["docs"], replacement);
    assert_eq!(parsed["mcp"]["first"]["command"], json!(["one"]));
    assert_eq!(parsed["mcp"]["last"]["command"], json!(["three"]));
    std::fs::remove_file(path).ok();
}

#[test]
fn save_renames_members_but_rejects_collisions() {
    let path = write_fixture(JSONC_FIXTURE);
    let located = locate(&[path.clone()], "docs", PARITY_FINGERPRINT);
    let replacement = json!({ "type": "local", "command": ["renamed"] });
    let text = apply_save(&located[0], "renamed", &replacement).unwrap();
    let parsed: Value = serde_json::from_str(&neutralize_jsonc(&text)).unwrap();
    assert!(parsed["mcp"].get("docs").is_none());
    assert_eq!(parsed["mcp"]["renamed"], replacement);
    assert!(apply_save(&located[0], "first", &replacement)
        .unwrap_err()
        .contains("already exists"));
    std::fs::remove_file(path).ok();
}

#[test]
fn defined_names_covers_every_candidate_file() {
    let first = write_fixture(JSONC_FIXTURE);
    let second = write_fixture(r#"{ "mcp": { "elsewhere": { "type": "local", "command": ["x"] } } }"#);
    let names = defined_names(&[first.clone(), second.clone()]);
    assert!(names.contains(&"docs".to_string()));
    assert!(names.contains(&"elsewhere".to_string()));
    // apply_save only sees its own file, so the cross-layer collision has to be caught before it.
    let located = locate(std::slice::from_ref(&first), "docs", PARITY_FINGERPRINT);
    let replacement = json!({ "type": "local", "command": ["y"] });
    assert!(apply_save(&located[0], "elsewhere", &replacement).is_ok());
    std::fs::remove_file(first).ok();
    std::fs::remove_file(second).ok();
}

#[test]
fn remove_handles_first_middle_and_last_members() {
    let path = write_fixture(JSONC_FIXTURE);
    for (name, fingerprint_for) in [("first", None), ("docs", Some(PARITY_FINGERPRINT)), ("last", None)] {
        let fingerprint = fingerprint_for.map(str::to_string).unwrap_or_else(|| {
            let neutral = neutralize_jsonc(JSONC_FIXTURE);
            let parsed: Value = serde_json::from_str(&neutral).unwrap();
            super::fingerprint(name, &parsed["mcp"][name]).unwrap()
        });
        let located = locate(&[path.clone()], name, &fingerprint);
        assert_eq!(located.len(), 1, "{name} should be locatable");
        let text = apply_remove(&located[0]).unwrap();
        let parsed: Value = serde_json::from_str(&neutralize_jsonc(&text)).unwrap();
        assert!(parsed["mcp"].get(name).is_none(), "{name} should be removed");
        assert_eq!(parsed["mcp"].as_object().unwrap().len(), 2);
        assert!(text.contains("Keep this comment"));
    }
    std::fs::remove_file(path).ok();
}

#[test]
fn removing_the_only_member_leaves_an_empty_object() {
    let path = write_fixture(
        r#"{ "mcp": { "docs": { "type": "local", "command": ["one"] } }, "keep": true }"#,
    );
    let config = json!({ "type": "local", "command": ["one"] });
    let located = locate(&[path.clone()], "docs", &fingerprint("docs", &config).unwrap());
    let text = apply_remove(&located[0]).unwrap();
    let parsed: Value = serde_json::from_str(&neutralize_jsonc(&text)).unwrap();
    assert_eq!(parsed["mcp"], json!({}));
    assert_eq!(parsed["keep"], true);
    std::fs::remove_file(path).ok();
}
