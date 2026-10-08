use super::*;

fn unsupported_settings(directory: &Path) -> Settings {
    settings(
        directory,
        json!({
            "legacy-host": { "type": "api", "key": "legacy-key" },
            "lmstudio": { "type": "api", "key": "placeholder" },
        }),
        json!({
            "default_agent": 7, "model": "not-a-model", "permission": 7,
            "plugin": ["opencode-format"], "theme": "dark",
        }),
        vec![
            OcServer {
                name: "bad name".into(),
                definition: json!({ "type": "local", "command": ["x"] }),
                approved: true,
            },
            OcServer {
                name: "no-command".into(),
                definition: json!({ "type": "local" }),
                approved: true,
            },
        ],
    )
}

fn expected_report() -> Value {
    json!({
        "credentials": [], "servers": [], "disabledServers": [], "configWritten": null, "files": [],
        "leftOut": {
            "signIns": ["legacy-host"], "plugins": ["opencode-format"],
            "settings": ["default_agent", "model", "permission", "theme"],
            "servers": ["bad name", "no-command"], "failed": [],
        },
        "skipped": [
            concat!("sign-in for legacy-host: Drift has no provider by that name; ",
                "add it to drift.json's providers to use the key"),
            "MCP server bad name: its name has characters Drift does not allow in a server name",
            "MCP server no-command: it has no command",
            "config default_agent: not an agent's name",
            "config model: not written as provider/model",
            "config permission: neither a decision nor a map of them",
            "plugin opencode-format: opencode plugins are JavaScript and Drift runs none",
            "config theme: Drift has no setting for it",
        ],
    })
}

#[test]
fn report_text_and_ledger_fields_are_stored_unchanged_and_items_are_reported_only_once() {
    let fixture = Fixture::new();
    let settings = unsupported_settings(&fixture.dir.0);

    let report = fixture.import(&fixture.dir.0, &settings);
    let saved_report: Value = fixture.engine.store.setting(REPORT).unwrap().unwrap();
    let ledger: Value = fixture.engine.store.setting(LEDGER).unwrap().unwrap();

    assert_eq!(serde_json::to_value(report).unwrap(), expected_report());
    assert_eq!(saved_report, expected_report());
    assert_eq!(
        ledger,
        json!({
            "credentials": ["legacy-host"], "servers": ["bad name", "no-command"], "config": true, "files": true,
        })
    );

    let again = fixture.import(&fixture.dir.0, &settings);

    assert_eq!(again, SettingsReport::default());
    assert_eq!(fixture.engine.store.setting::<Value>(LEDGER).unwrap(), Some(ledger));
}
