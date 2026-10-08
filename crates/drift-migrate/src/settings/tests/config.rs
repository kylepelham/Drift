use super::*;
use drift_engine::permission::{Decision, Policy, Rule};

fn import_permissions(text: &str) -> (Policy, SettingsReport) {
    let fixture = Fixture::new();
    let home = fixture.dir.0.join("home");
    let settings = Settings {
        auth: None,
        config: OcConfig::parse(text),
        config_dir: fixture.dir.0.clone(),
        servers: vec![],
    };

    let report = fixture.import(&home, &settings);
    let written: Value = std::fs::read_to_string(home.join(".config/drift/drift.json"))
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default();
    let rules: Vec<Rule> = serde_json::from_value(written["permissions"].clone()).unwrap_or_default();

    (Policy { rules }, report)
}

fn decide(policy: &Policy, kind: &str, target: &str) -> Option<Decision> {
    policy.explicit(&drift_engine::tool::Ask::new(kind, target, ""))
}

#[test]
fn opencodes_last_matching_pattern_still_wins_once_imported() {
    let text = r#"{
        "permission": {
            "bash": { "*": "ask", "git *": "allow" },
            "read": { "*": "deny", "src/*": "allow", "*.env": "deny" },
            "edit": "ask"
        }
    }"#;
    let (policy, _) = import_permissions(text);

    assert_eq!(
        decide(&policy, "bash", "git status"),
        Some(Decision::Allow),
        "opencode's documented allow-list keeps working"
    );
    assert_eq!(decide(&policy, "bash", "rm -rf x"), Some(Decision::Ask));
    assert_eq!(decide(&policy, "read", "src/a.rs"), Some(Decision::Allow));
    assert_eq!(
        decide(&policy, "read", "src/.env"),
        Some(Decision::Deny),
        "a later pattern in opencode overrides an earlier one"
    );
    assert_eq!(decide(&policy, "read", "notes.txt"), Some(Decision::Deny));
    assert_eq!(decide(&policy, "edit", "a.rs"), Some(Decision::Ask));
}

#[test]
fn a_permission_for_every_tool_comes_in_and_still_loses_to_a_later_one() {
    let (everything, _) = import_permissions(r#"{ "permission": "allow" }"#);

    assert_eq!(
        decide(&everything, "bash", "rm -rf x"),
        Some(Decision::Allow),
        "a single decision is opencode's \"*\" for every permission"
    );

    let (mixed, _) =
        import_permissions(r#"{ "permission": { "*": "ask", "bash": "allow", "read": { "*.env": "deny" } } }"#);

    assert_eq!(
        decide(&mixed, "bash", "ls"),
        Some(Decision::Allow),
        "bash, written after *, wins"
    );
    assert_eq!(decide(&mixed, "edit", "a.rs"), Some(Decision::Ask));
    assert_eq!(decide(&mixed, "read", ".env"), Some(Decision::Deny));

    let (none, report) = import_permissions(r#"{ "permission": 7 }"#);

    assert!(
        none.rules.is_empty() && report.left_out.settings.contains(&"permission".to_string()),
        "an unreadable value is reported, not dropped in silence"
    );
}

fn mappable_config() -> Value {
    json!({
        "$schema": "https://opencode.ai/config.json",
        "model": "anthropic/claude-opus-5-5",
        "default_agent": "plan",
        "instructions": ["rules.md", "~/style.md"],
        "permission": { "edit": "ask", "bash": { "git *": "allow", "rm *": "deny" }, "doom_loop": "ask" },
        "tools": { "firecrawl_agent": false },
        "plugin": ["opencode-foo@1"],
        "mcp": {},
    })
}

fn assert_mapped_config(written: Value, config_dir: &Path) {
    assert_eq!(
        written["model"],
        json!({ "provider": "anthropic", "model": "claude-opus-5-5" })
    );
    assert_eq!(written["defaultAgent"], "plan");
    assert_eq!(
        written["instructions"],
        json!([
            config_dir.join("rules.md").to_string_lossy().replace('\\', "/"),
            "~/style.md",
        ])
    );
    assert_eq!(
        written["permissions"],
        json!([
            { "kind": "edit", "pattern": "*", "decision": "ask" },
            { "kind": "bash", "pattern": "rm *", "decision": "deny" },
            { "kind": "bash", "pattern": "git *", "decision": "allow" },
        ])
    );

    let file: drift_engine::config::File = serde_json::from_value(written).unwrap();
    assert_eq!(file.permissions.len(), 3, "Drift reads what was written");
}

#[test]
fn mappable_config_becomes_drift_json_and_the_rest_is_named() {
    let fixture = Fixture::new();
    let home = fixture.dir.0.join("home");
    let settings = settings(&fixture.dir.0, json!({}), mappable_config(), vec![]);

    let report = fixture.import(&home, &settings);

    assert_mapped_config(read_config(&home), &fixture.dir.0);
    assert!(report.config_written.is_some());
    assert_eq!(
        (report.left_out.settings, report.left_out.plugins),
        (
            vec!["permission.doom_loop".to_string(), "tools".into()],
            vec!["opencode-foo@1".to_string()],
        )
    );
    let log = report.skipped.join("\n");
    assert!(
        log.contains("config tools") && log.contains("plugin opencode-foo@1") && log.contains("permission.doom_loop"),
        "{log}"
    );

    let other = fixture.dir.0.join("other");
    std::fs::create_dir_all(other.join(".config/drift")).unwrap();
    std::fs::write(other.join(".config/drift/drift.json"), "{}").unwrap();
    fixture.engine.store.remove_setting(LEDGER).unwrap();

    let kept = fixture.import(&other, &settings);

    assert_eq!(
        std::fs::read_to_string(other.join(".config/drift/drift.json")).unwrap(),
        "{}",
        "the user's own file is never changed"
    );
    assert!(kept.config_written.is_none() && kept.skipped.iter().any(|line| line.contains("you already have")));
}
