use super::*;

fn fixture() -> (PathBuf, Store, McpRuntime) {
    let root = std::env::temp_dir().join(format!(
        "drift-mcp-runtime-test-{}-{}",
        std::process::id(),
        TEMP_ID.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::remove_dir_all(&root).ok();
    let extensions = root.join("extensions");
    std::fs::create_dir_all(extensions.join("plugin")).unwrap();
    std::fs::write(extensions.join("opencode.json"), r#"{"plugin":["auth@1"]}"#).unwrap();
    std::fs::write(extensions.join("plugin/spawn-thread.ts"), "export {}\n").unwrap();
    std::fs::write(extensions.join("plugin/mcp-approval.ts"), "export {}\n").unwrap();
    std::fs::write(extensions.join("plugin/prompt-overrides.ts"), "export {}\n").unwrap();
    std::fs::write(
        extensions.join("prompt-catalog.json"),
        r#"{"version":1,"families":[],"agents":[]}"#,
    )
    .unwrap();
    let store = crate::store::open(&root.join("data")).unwrap();
    let runtime = McpRuntime::new(&root.join("data"), extensions);
    (root, store, runtime)
}

#[test]
fn validates_the_current_schema_and_preserves_top_level_extensions() {
    assert!(validate_config(&json!({
        "type": "local",
        "command": ["npx", "server"],
        "cwd": "./tools",
        "environment": { "TOKEN": "{env:TOKEN}" },
        "enabled": false,
        "timeout": 7_200_000,
        "vendor": { "mode": "fast" }
    }))
    .is_ok());
    assert!(validate_config(&json!({
        "type": "remote",
        "url": "https://example.com/mcp",
        "headers": { "Authorization": "Bearer {env:TOKEN}" },
        "oauth": { "clientId": "id", "callbackPort": 29418, "vendorOAuth": true }
    }))
    .is_ok());
    assert!(validate_config(&json!({ "type": "local", "command": [] })).is_err());
    assert!(validate_config(&json!({
        "type": "remote", "url": "http://192.0.2.10:8765/mcp"
    }))
    .is_ok());
    assert!(
        validate_config(&json!({ "type": "remote", "url": "http://127.0.0.2:3000" })).is_ok()
    );
    assert!(validate_config(&json!({ "type": "remote", "url": "http://[::1]:3000" })).is_ok());
    assert!(
        validate_config(&json!({ "type": "remote", "url": "ftp://example.com" })).is_err()
    );
    assert!(validate_config(&json!({
        "type": "remote", "url": "https://example.com", "oauth": { "callbackPort": 65536 }
    }))
    .is_err());
}

#[test]
fn materialization_replaces_files_without_touching_extensions() {
    let (root, store, runtime) = fixture();
    store
        .save_mcp_server(
            "docs",
            None,
            &json!({ "type": "remote", "url": "https://example.com", "unknown": true }),
        )
        .unwrap();
    runtime.materialize(&store).unwrap();
    runtime.materialize(&store).unwrap();

    let generated: Value = serde_json::from_str(
        &std::fs::read_to_string(runtime.config_dir().join("opencode.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(generated["mcp"]["docs"]["unknown"], true);
    assert_eq!(generated["plugin"][0], "auth@1");
    assert_eq!(
        std::fs::read_to_string(root.join("extensions/opencode.json")).unwrap(),
        r#"{"plugin":["auth@1"]}"#
    );
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn decisions_require_the_exact_pending_fingerprint_and_generation() {
    let (root, store, runtime) = fixture();
    runtime.materialize(&store).unwrap();
    let directory = "S:/repo";
    let fingerprint = "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    write_json(
        &report_path(&runtime.root.join("pending"), directory),
        &json!({
            "version": 3,
            "generation": 0,
            "directory": directory,
            "servers": [{ "name": "docs", "type": "remote", "fingerprint": fingerprint, "decision": "pending" }]
        }),
    )
    .unwrap();

    assert!(runtime
        .decide(
            &store,
            directory,
            "other",
            fingerprint,
            0,
            McpDecision::Approved,
            || Ok(())
        )
        .is_err());
    runtime
        .decide(
            &store,
            directory,
            "docs",
            fingerprint,
            0,
            McpDecision::Rejected,
            || Ok(()),
        )
        .unwrap();
    assert_eq!(store.mcp_state().unwrap().decisions[0].decision, "rejected");
    assert!(runtime
        .decide(
            &store,
            directory,
            "docs",
            fingerprint,
            0,
            McpDecision::Approved,
            || Ok(())
        )
        .is_err());
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn invalid_observations_are_visible_but_cannot_be_decided() {
    let (root, store, runtime) = fixture();
    runtime.materialize(&store).unwrap();
    let directory = "S:/repo";
    let fingerprint = "sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
    write_json(
        &report_path(&runtime.root.join("pending"), directory),
        &json!({
            "version": 3,
            "generation": 0,
            "directory": directory,
            "servers": [{ "name": "insecure", "type": "remote", "fingerprint": fingerprint, "decision": "invalid" }]
        }),
    )
    .unwrap();

    let snapshot = runtime.snapshot(&store, directory).unwrap();
    assert_eq!(snapshot.observed.len(), 1);
    assert_eq!(snapshot.observed[0].decision, McpDecision::Invalid);
    assert!(runtime
        .decide(
            &store,
            directory,
            "insecure",
            fingerprint,
            0,
            McpDecision::Approved,
            || Ok(())
        )
        .is_err());
    assert!(runtime
        .revoke(&store, directory, "insecure", fingerprint, 0, || Ok(()))
        .is_err());
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn failed_materialization_restores_registry_content() {
    let (root, store, runtime) = fixture();
    let first = json!({ "type": "local", "command": ["first"] });
    store.save_mcp_server("server", None, &first).unwrap();
    runtime.materialize(&store).unwrap();
    let config_path = runtime.config_dir().join("opencode.json");
    std::fs::remove_file(&config_path).unwrap();
    std::fs::create_dir(&config_path).unwrap();

    let stops = std::cell::Cell::new(0);
    assert!(runtime
        .save(
            &store,
            "server",
            Some("server"),
            json!({ "type": "local", "command": ["second"] }),
            1,
            || {
                stops.set(stops.get() + 1);
                Ok(())
            }
        )
        .is_err());
    assert_eq!(store.mcp_state().unwrap().servers[0].config, first);
    assert_eq!(stops.get(), 2);
    assert!(runtime.config_dir().join(SENTINEL_FILE).is_file());
    let policy: Value = serde_json::from_str(
        &std::fs::read_to_string(runtime.config_dir().join("mcp-approvals.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(policy["version"], 0);
    assert!(std::fs::read_dir(runtime.config_dir())
        .unwrap()
        .all(|entry| !entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .ends_with(".tmp")));

    std::fs::remove_dir(&config_path).unwrap();
    runtime.materialize(&store).unwrap();
    assert!(!runtime.config_dir().join(SENTINEL_FILE).exists());
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn prompt_changes_preserve_pending_approval_reports() {
    let (root, store, runtime) = fixture();
    runtime.materialize(&store).unwrap();
    let directory = "S:/repo";
    let report = report_path(&runtime.root.join("pending"), directory);
    write_json(
        &report,
        &json!({ "version": 3, "generation": 0, "directory": directory, "servers": [] }),
    )
    .unwrap();

    runtime
        .save_prompt(
            &store,
            "family:gpt",
            json!("Custom prompt"),
            Some(json!("Original prompt")),
        )
        .unwrap();
    assert!(report.is_file());
    let generated: Value = serde_json::from_str(
        &std::fs::read_to_string(runtime.config_dir().join("prompt-overrides.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(generated["families"]["gpt"], "Custom prompt");

    runtime.reset_prompt(&store, "family:gpt").unwrap();
    assert!(report.is_file());
    assert!(store.prompt_overrides().unwrap().is_empty());
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn prompt_validation_matches_agent_configuration_shapes() {
    let (root, store, runtime) = fixture();
    runtime
        .save_prompt(
            &store,
            "agent:build",
            json!({ "permission": "deny", "color": "#a1B2c3", "tools": { "bash": false } }),
            None,
        )
        .unwrap();
    assert!(runtime
        .save_prompt(
            &store,
            "agent:build",
            json!({ "permission": { "bash": "sometimes" } }),
            None,
        )
        .is_err());
    assert!(runtime
        .save_prompt(&store, "agent:build", json!({ "color": "purple" }), None)
        .is_err());
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn windows_directory_normalization_is_ascii_only() {
    let input = r"S:\Ünicode\İ\Repo\";
    let expected = if cfg!(windows) {
        "s:/Ünicode/İ/repo"
    } else {
        "S:/Ünicode/İ/Repo"
    };
    assert_eq!(normalize_directory(input), expected);
}
