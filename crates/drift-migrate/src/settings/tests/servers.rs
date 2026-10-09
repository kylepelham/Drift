use super::*;
use drift_engine::mcp::{OAuthClient, ServerConfig};
use std::collections::BTreeMap;

fn server(name: &str, definition: Value, approved: bool) -> OcServer {
    OcServer {
        name: name.into(),
        definition,
        approved,
    }
}

fn servers() -> Vec<OcServer> {
    vec![
        server(
            "local",
            json!({
                "type": "local", "command": ["npx", "-y", "tool"],
                "environment": { "KEY": "{env:PATH}" }, "enabled": true,
            }),
            true,
        ),
        server(
            "remote",
            json!({
                "type": "remote", "url": "https://r.example/mcp",
                "headers": { "Authorization": "Bearer {file:token.txt}" },
                "oauth": { "clientId": "app", "scope": "a b" },
            }),
            true,
        ),
        server(
            "unapproved",
            json!({ "type": "remote", "url": "https://u.example" }),
            false,
        ),
        server(
            "off",
            json!({ "type": "local", "command": ["x"], "enabled": false }),
            true,
        ),
        server("mine", json!({ "type": "local", "command": ["other"] }), true),
        server("bad name", json!({ "type": "local", "command": ["x"] }), true),
        server(
            "missing",
            json!({
                "type": "local", "command": ["x"], "environment": { "K": "{env:DRIFT_MIGRATE_UNSET_VAR}" },
            }),
            true,
        ),
        server("local", json!({ "type": "local", "command": ["duplicate"] }), true),
    ]
}

fn assert_server_configs(saved: &BTreeMap<String, (ServerConfig, bool)>, path_value: String) {
    assert_eq!(
        saved["local"],
        (
            ServerConfig::Stdio {
                command: "npx".into(),
                args: vec!["-y".into(), "tool".into()],
                env: BTreeMap::from([("KEY".into(), path_value)]),
                cwd: None,
                timeout_seconds: None,
            },
            true
        )
    );

    let oauth = Some(OAuthClient {
        client_id: "app".into(),
        client_secret: None,
        scopes: vec!["a".into(), "b".into()],
    });
    assert_eq!(
        saved["remote"],
        (
            ServerConfig::Http {
                url: "https://r.example/mcp".into(),
                headers: BTreeMap::from([("Authorization".into(), "Bearer file-secret".into())]),
                oauth,
                timeout_seconds: None,
            },
            true
        )
    );
}

#[test]
fn servers_come_in_switched_on_only_when_they_were_approved_and_enabled() {
    let fixture = Fixture::new();
    std::fs::write(fixture.dir.0.join("token.txt"), "file-secret\n").unwrap();
    // PATH exercises substitution without changing the process environment.
    let path_value = std::env::var("PATH").unwrap();
    fixture
        .engine
        .store
        .save_mcp_server(
            "mine",
            &ServerConfig::Http {
                url: "https://mine.example".into(),
                headers: BTreeMap::new(),
                oauth: None,
                timeout_seconds: None,
            },
        )
        .unwrap();
    let settings = settings(&fixture.dir.0, json!({}), json!({}), servers());

    let report = fixture.import(&fixture.dir.0, &settings);
    let saved = fixture
        .engine
        .store
        .mcp_servers()
        .unwrap()
        .into_iter()
        .map(|row| (row.name, (row.config, row.enabled)))
        .collect();

    assert_server_configs(&saved, path_value);
    assert_eq!(
        (report.servers, report.disabled_servers),
        (
            vec!["local".to_string(), "remote".into(), "unapproved".into(), "off".into()],
            vec!["unapproved".to_string(), "off".into()],
        )
    );
    assert!(
        matches!(&saved["mine"].0, ServerConfig::Http { url, .. } if url == "https://mine.example"),
        "Drift's own server is kept"
    );
    assert!(!saved.contains_key("missing") && !saved.contains_key("bad name"));
    assert_eq!(report.left_out.servers, ["bad name", "missing"]);
    let log = report.skipped.join("\n");
    assert!(
        log.contains("DRIFT_MIGRATE_UNSET_VAR is not set")
            && log.contains("bad name: its name")
            && log.contains("mine: Drift already has"),
        "{log}"
    );
}

#[test]
fn conversion_errors_keep_their_report_messages() {
    let cases = [
        (
            json!({ "type": "local" }),
            McpConfigError::MissingCommand,
            "it has no command",
        ),
        (json!({ "type": "remote" }), McpConfigError::MissingUrl, "it has no url"),
        (
            json!({ "type": "other" }),
            McpConfigError::UnsupportedType,
            "it is neither a local nor a remote server",
        ),
        (
            json!({ "type": "local", "command": ["{env:DRIFT_MIGRATE_UNSET_VAR}"] }),
            McpConfigError::MissingEnvironment {
                name: "DRIFT_MIGRATE_UNSET_VAR".into(),
            },
            "the environment variable DRIFT_MIGRATE_UNSET_VAR is not set",
        ),
    ];

    for (definition, expected, message) in cases {
        let error = mcp_config(&definition, Path::new(".")).unwrap_err();

        assert_eq!(error, expected);
        assert_eq!(error.to_string(), message);
    }

    let fixture = Fixture::new();
    let path = fixture.dir.0.join("missing.txt");
    let error = mcp_config(
        &json!({ "type": "local", "command": ["{file:missing.txt}"] }),
        &fixture.dir.0,
    )
    .unwrap_err();

    assert_eq!(error.to_string(), format!("{} could not be read", path.display()));
    assert_eq!(error, McpConfigError::UnreadableFile { path });
}
