use super::*;

fn test_dir(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("drift-{name}-test-{}", std::process::id()));
    std::fs::remove_dir_all(&dir).ok();
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn store_roundtrip() {
    let dir = test_dir("store");
    let store = open(&dir).unwrap();

    assert!(!store.dictation_enabled().unwrap());
    store.save_dictation_enabled(true).unwrap();
    assert!(store.dictation_enabled().unwrap());
    store.save_dictation_enabled(false).unwrap();
    assert!(!store.dictation_enabled().unwrap());

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
    // Listing expired tombstones must not drop them: only the confirmed unarchive does.
    let expired_sessions = store.expired_archived(now() + 1000).unwrap();
    assert_eq!(expired_sessions, vec!["s2".to_string()]);
    assert_eq!(store.archived().unwrap().len(), 1);
    assert!(store.expired_archived(now() - 1000).unwrap().is_empty());
    store.unarchive_session("s2").unwrap();
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
    let expired = store.expired_removed_workspaces(now() + 1000).unwrap();
    assert_eq!(expired.len(), 1);
    assert_eq!(expired[0].path, "S:/moved");
    assert!(store
        .expired_removed_workspaces(now() - 1000)
        .unwrap()
        .is_empty());
    store.forget_workspace(&expired[0].id).unwrap();
    assert!(store.workspaces().unwrap().is_empty());
    assert!(store.removed_workspaces().unwrap().is_empty());
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
fn open_drops_legacy_recoverable_interruptions() {
    let dir = test_dir("legacy-interruption");
    let file = dir.join("drift.db");
    let raw = Connection::open(&file).unwrap();
    raw.execute_batch(
        "CREATE TABLE recoverable_interruption(id INTEGER PRIMARY KEY);\n\
         INSERT INTO recoverable_interruption(id) VALUES(1);",
    )
    .unwrap();
    drop(raw);

    let store = open_at(&file).unwrap();
    drop(store);
    let raw = Connection::open(&file).unwrap();
    let tables: i64 = raw
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'recoverable_interruption'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(tables, 0);
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn expired_duplicates_of_active_directories_are_collapsed_not_returned() {
    let dir = test_dir("dup");
    let file = dir.join("drift.db");
    let store = open_at(&file).unwrap();
    store
        .add_workspace("active", "S:\\proj\\app", "App", "icon")
        .unwrap();
    // Seed raw: add_workspace's canonical guard forbids creating a duplicate through the API.
    let raw = Connection::open(&file).unwrap();
    raw.execute(
        "INSERT INTO workspace(id, path, name, icon, last_used, removed_at) VALUES('dup', 'S:/proj/APP', 'App', '', 1, 1)",
        [],
    )
    .unwrap();
    raw.execute(
        "INSERT INTO session_meta(session_id, workspace_id, archived_at) VALUES('s-arch', 'dup', 5)",
        [],
    )
    .unwrap();
    drop(raw);

    // The duplicate must never be offered for session deletion: its directory is on the sidebar.
    assert!(store
        .expired_removed_workspaces(now() + 1000)
        .unwrap()
        .is_empty());
    assert_eq!(store.workspaces().unwrap().len(), 1);
    assert!(store.removed_workspaces().unwrap().is_empty());
    let archived = store.archived().unwrap();
    assert_eq!(archived.len(), 1);
    assert_eq!(archived[0].workspace_id, "active");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn saving_a_path_onto_another_workspace_merges_the_rows() {
    let dir = test_dir("save-merge");
    let store = open_at(&dir.join("drift.db")).unwrap();
    store.add_workspace("a", "S:/one", "One", "icon").unwrap();
    store.add_workspace("b", "S:/two", "Two", "").unwrap();
    store.archive_session("s1", "b").unwrap();
    store.save_workspace("a", "S:\\TWO", "One", "icon").unwrap();
    let workspaces = store.workspaces().unwrap();
    assert_eq!(workspaces.len(), 1);
    assert_eq!(workspaces[0].id, "a");
    assert_eq!(store.archived().unwrap()[0].workspace_id, "a");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn open_collapses_duplicate_workspace_paths() {
    let dir = test_dir("collapse");
    let file = dir.join("drift.db");
    {
        let store = open_at(&file).unwrap();
        store
            .add_workspace("user", "S:\\proj", "Proj", "icon")
            .unwrap();
        let raw = Connection::open(&file).unwrap();
        raw.execute(
            "INSERT INTO workspace(id, path, name, icon, last_used) VALUES('imported', 'S:/proj', 'Proj', '', 999)",
            [],
        )
        .unwrap();
        raw.execute(
            "INSERT INTO session_meta(session_id, workspace_id, archived_at) VALUES('s1', 'imported', 7)",
            [],
        )
        .unwrap();
    }
    let store = open_at(&file).unwrap();
    let workspaces = store.workspaces().unwrap();
    assert_eq!(workspaces.len(), 1);
    assert_eq!(workspaces[0].id, "user");
    let archived = store.archived().unwrap();
    assert_eq!(archived.len(), 1);
    assert_eq!(archived[0].workspace_id, "user");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn imports_opencode_projects_without_overwriting_drift_metadata() {
    let dir = test_dir("import");
    let source = dir.join("opencode.db");
    let conn = Connection::open(&source).unwrap();
    conn.execute_batch(
        "CREATE TABLE project(id TEXT PRIMARY KEY, worktree TEXT, name TEXT, time_updated INTEGER);
         CREATE TABLE session(id TEXT PRIMARY KEY, project_id TEXT, time_updated INTEGER);
         INSERT INTO project VALUES('p1', 'S:/one', 'One', 10);
         INSERT INTO project VALUES('p2', '/tmp/project-directories', 'Temporary', 15);
         INSERT INTO project VALUES('p3', '/tmp/manual', 'Manual project', 16);
         INSERT INTO project VALUES('p4', 'C:/Users/Example/AppData/Local/Temp/opencode-test-long', 'Windows temporary', 17);
         INSERT INTO project VALUES('global', '/', 'Global', 20);
         INSERT INTO session VALUES('s1', 'p1', 30);
         INSERT INTO session VALUES('s2', 'p2', 31);
         INSERT INTO session VALUES('s4', 'p4', 32);",
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
    store
        .add_workspace(
            "p4",
            "C:/Users/Example/AppData/Local/Temp/opencode-test-long",
            "Windows temporary",
            "",
        )
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
    assert!(!workspaces
        .iter()
        .any(|workspace| workspace.path.contains("AppData/Local/Temp")));
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

    // A slash/case variant of an existing workspace directory must not import as a duplicate,
    // and a removed workspace must stay removed instead of being resurrected by the import.
    let variants = dir.join("opencode-variants.db");
    let conn = Connection::open(&variants).unwrap();
    conn.execute_batch(
        "CREATE TABLE project(id TEXT PRIMARY KEY, worktree TEXT, name TEXT, time_updated INTEGER);
         CREATE TABLE session(id TEXT PRIMARY KEY, project_id TEXT, time_updated INTEGER);
         INSERT INTO project VALUES('p9', 'S:/ONE', 'Case variant', 40);
         INSERT INTO session VALUES('s9', 'p9', 41);",
    )
    .unwrap();
    drop(conn);
    assert_eq!(store.import_opencode_workspaces(&variants).unwrap(), 0);
    store.remove_workspace("p1").unwrap();
    assert_eq!(store.import_opencode_workspaces(&source).unwrap(), 0);
    assert!(store
        .workspaces()
        .unwrap()
        .iter()
        .all(|workspace| workspace.id != "p1"));
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn mcp_decisions_are_global_and_survive_definition_changes() {
    let dir = test_dir("mcp-store");
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
