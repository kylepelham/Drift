use super::*;

#[test]
fn store_roundtrip() {
    let dir = std::env::temp_dir().join(format!("drift-store-test-{}", std::process::id()));
    std::fs::remove_dir_all(&dir).ok();
    std::fs::create_dir_all(&dir).unwrap();
    let store = open(&dir).unwrap();

    let created = store.add_workspace("w1", "S:/proj", "Proj", "").unwrap();
    assert_eq!(created.id, "w1");
    store
        .save_workspace("w1", "S:/moved", "Renamed", "R")
        .unwrap();
    assert_eq!(store.workspaces().unwrap()[0].name, "Renamed");
    assert_eq!(store.workspaces().unwrap()[0].path, "S:/moved");

    store.archive_session("s1", "w1").unwrap();
    store.archive_session("s2", "w1").unwrap();
    assert_eq!(store.archived().unwrap().len(), 2);
    store.unarchive_session("s1").unwrap();
    assert_eq!(store.archived().unwrap().len(), 1);
    let purged = store.purge_archived(now() + 1000).unwrap();
    assert_eq!(purged.len(), 1);
    assert!(store.archived().unwrap().is_empty());

    store.remove_workspace("w1").unwrap();
    assert!(store.workspaces().unwrap().is_empty());
    assert_eq!(store.removed_workspaces().unwrap().len(), 1);

    let restored = store
        .add_workspace("w2", "S:/moved", "Ignored", "")
        .unwrap();
    assert_eq!(restored.id, "w1");
    assert_eq!(restored.name, "Renamed");
    assert_eq!(store.workspaces().unwrap().len(), 1);

    store.remove_workspace("w1").unwrap();
    let paths = store.purge_removed_workspaces(now() + 1000).unwrap();
    assert_eq!(paths, vec!["S:/moved".to_string()]);
    assert!(store.workspaces().unwrap().is_empty());
    assert!(
        store
            .add_workspace("w3", "S:/moved", "Fresh", "")
            .unwrap()
            .id
            == "w3"
    );
    let value = serde_json::json!({ "prompt": "Drift prompt" });
    let original = serde_json::json!({ "prompt": "Original prompt" });
    store
        .save_prompt_override("agent:build", &value, Some(&original))
        .unwrap();
    let prompts = store.prompt_overrides().unwrap();
    assert_eq!(prompts.len(), 1);
    assert_eq!(prompts[0].value, value);
    assert_eq!(prompts[0].original, Some(original));
    store.reset_prompt_override("agent:build").unwrap();
    assert!(store.prompt_overrides().unwrap().is_empty());
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn imports_opencode_projects_without_overwriting_drift_metadata() {
    let dir = std::env::temp_dir().join(format!("drift-import-test-{}", std::process::id()));
    std::fs::remove_dir_all(&dir).ok();
    std::fs::create_dir_all(&dir).unwrap();
    let source = dir.join("opencode.db");
    let conn = Connection::open(&source).unwrap();
    conn.execute_batch(
        "CREATE TABLE project(id TEXT PRIMARY KEY, worktree TEXT, name TEXT, time_updated INTEGER);
         CREATE TABLE session(id TEXT PRIMARY KEY, project_id TEXT, time_updated INTEGER);
         INSERT INTO project VALUES('p1', 'S:/one', 'One', 10);
         INSERT INTO project VALUES('p2', '/tmp/project-directories', 'Temporary', 15);
         INSERT INTO project VALUES('p3', '/tmp/manual', 'Manual project', 16);
         INSERT INTO project VALUES('global', '/', 'Global', 20);
         INSERT INTO session VALUES('s1', 'p1', 30);",
    )
    .unwrap();
    drop(conn);

    let store = open_at(&dir.join("drift.db")).unwrap();
    store
        .add_workspace("p2", "/tmp/project-directories", "Temporary", "")
        .unwrap();
    store
        .add_workspace("manual", "/tmp/manual", "Manual", "")
        .unwrap();
    assert_eq!(store.import_opencode_workspaces(&source).unwrap(), 1);
    let workspaces = store.workspaces().unwrap();
    assert_eq!(workspaces.len(), 2);
    assert!(workspaces
        .iter()
        .any(|workspace| workspace.path == "S:/one"));
    assert!(workspaces
        .iter()
        .any(|workspace| workspace.path == "/tmp/manual"));
    assert!(!workspaces
        .iter()
        .any(|workspace| workspace.path == "/tmp/project-directories"));
    store.save_workspace("p1", "S:/one", "Custom", "C").unwrap();
    assert_eq!(store.import_opencode_workspaces(&source).unwrap(), 0);
    assert_eq!(
        store
            .workspaces()
            .unwrap()
            .into_iter()
            .find(|workspace| workspace.id == "p1")
            .unwrap()
            .name,
        "Custom"
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn mcp_decisions_are_global_and_survive_definition_changes() {
    let dir = std::env::temp_dir().join(format!("drift-mcp-store-test-{}", std::process::id()));
    std::fs::remove_dir_all(&dir).ok();
    std::fs::create_dir_all(&dir).unwrap();
    let store = open_at(&dir.join("drift.db")).unwrap();
    let first = serde_json::json!({ "type": "local", "command": ["one"] });
    let second = serde_json::json!({ "type": "local", "command": ["two"] });

    assert_eq!(store.save_mcp_server("server", None, &first).unwrap(), 1);
    assert_eq!(
        store
            .decide_mcp(
                "server",
                "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                "approved"
            )
            .unwrap(),
        2
    );
    store.save_mcp_server("server", None, &second).unwrap();
    store.save_mcp_server("server", None, &first).unwrap();
    let state = store.mcp_state().unwrap();
    assert_eq!(state.decisions.len(), 1);
    assert_eq!(state.decisions[0].decision, "approved");

    store
        .decide_mcp(
            "server",
            "sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
            "rejected",
        )
        .unwrap();
    assert_eq!(store.mcp_state().unwrap().decisions.len(), 2);
    store
        .revoke_mcp("sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa")
        .unwrap();
    assert_eq!(store.mcp_state().unwrap().decisions[0].decision, "rejected");
    std::fs::remove_dir_all(dir).ok();
}
