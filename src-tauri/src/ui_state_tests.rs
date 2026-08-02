use super::*;

fn snapshot(workspace_id: Option<&str>) -> UiMirrorSnapshot {
    UiMirrorSnapshot {
        schema: 1,
        revision: 99,
        theme: UiTheme {
            name: "drift-dark".into(),
            custom: CustomTheme {
                background: "#111318".into(),
                surface: "#1b1e25".into(),
                text: "#e8eaf0".into(),
                accent: "#a78bfa".into(),
            },
            ui_font: "".into(),
            code_font: "".into(),
            custom_css: "".into(),
        },
        selection: UiSelection {
            workspace_id: workspace_id.map(str::to_string),
            session_id: None,
        },
    }
}

fn test_store(name: &str) -> (std::path::PathBuf, Store) {
    let dir = std::env::temp_dir().join(format!("drift-ui-state-{name}-{}", std::process::id()));
    std::fs::remove_dir_all(&dir).ok();
    std::fs::create_dir_all(&dir).unwrap();
    let store = crate::store::open(&dir).unwrap();
    (dir, store)
}

#[test]
fn snapshot_initialization_is_insert_only_and_survives_reopen() {
    let (dir, store) = test_store("persist");
    store.add_workspace("one", "S:/one", "One", "").unwrap();
    let authority = UiStateAuthority::load(&store).unwrap();
    let first = authority.initialize(&store, snapshot(Some("one"))).unwrap();
    assert_eq!(first.revision, 0);
    let mut replacement = snapshot(None);
    replacement.theme.name = "drift-light".into();
    assert_eq!(authority.initialize(&store, replacement).unwrap(), first);
    drop(authority);
    drop(store);
    let reopened = crate::store::open(&dir).unwrap();
    assert_eq!(
        UiStateAuthority::load(&reopened)
            .unwrap()
            .snapshot()
            .unwrap(),
        first
    );
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn mutations_increment_once_and_deduplicate_retries() {
    let (dir, store) = test_store("dedupe");
    let authority = UiStateAuthority::load(&store).unwrap();
    authority.initialize(&store, snapshot(None)).unwrap();
    let mutation = UiStateMutation {
        client_id: "desktop".into(),
        mutation_id: "m1".into(),
        theme: Some(snapshot(None).theme),
        selection: None,
    };
    let (first, changed) = authority.update(&store, mutation.clone()).unwrap();
    let (retry, retry_changed) = authority.update(&store, mutation).unwrap();
    assert!(changed);
    assert!(!retry_changed);
    assert_eq!(first.revision, 1);
    assert_eq!(retry, first);
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn validation_rejects_bad_themes_selection_and_timeouts() {
    let mut invalid = snapshot(None);
    invalid.theme.name = "unknown".into();
    assert!(validate_snapshot(&invalid).is_err());
    let mut invalid = snapshot(None);
    invalid.selection.session_id = Some("session".into());
    assert!(validate_snapshot(&invalid).is_err());
    let mut invalid = snapshot(None);
    invalid.theme.custom_css = "x".repeat(20_001);
    assert!(validate_snapshot(&invalid).is_err());
    assert!(validate_timeout(Some(59_999)).is_err());
    assert!(validate_timeout(Some(60_000)).is_ok());
    assert!(validate_timeout(None).is_ok());
}

#[test]
fn shell_timeout_is_insert_only_then_mutable_and_persistent() {
    let (dir, store) = test_store("timeout");
    let authority = ShellTimeoutAuthority::load(&store).unwrap();
    let none = ShellTimeoutPolicy { timeout_ms: None };
    assert_eq!(authority.initialize(&store, none.clone()).unwrap(), none);
    assert_eq!(
        authority
            .initialize(
                &store,
                ShellTimeoutPolicy {
                    timeout_ms: Some(60_000)
                }
            )
            .unwrap(),
        none
    );
    let five = ShellTimeoutPolicy {
        timeout_ms: Some(300_000),
    };
    authority.update(&store, five.clone()).unwrap();
    drop(authority);
    assert_eq!(
        ShellTimeoutAuthority::load(&store)
            .unwrap()
            .snapshot()
            .unwrap(),
        five
    );
    std::fs::remove_dir_all(dir).ok();
}
