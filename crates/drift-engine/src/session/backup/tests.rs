use super::*;
use crate::session::turn::tests::{harness, prompt, text, until_idle};
use crate::session::types::Revert;

#[tokio::test]
async fn a_backup_restores_into_another_workspace_once_and_again_after_a_delete() {
    let h = harness().await;
    h.provider.push(text("first answer")).push(text("second answer"));
    h.engine.submit(&h.session.id, prompt("one")).await.unwrap();
    until_idle(&h).await;
    h.engine.submit(&h.session.id, prompt("two")).await.unwrap();
    until_idle(&h).await;

    let backup = h.engine.export_sessions(std::slice::from_ref(&h.session.id)).unwrap();
    let json = serde_json::to_string(&backup).unwrap();
    let backup: Backup = serde_json::from_str(&json).unwrap();
    assert_eq!(backup.sessions[0].messages.len(), 4);

    let other = h
        .engine
        .store
        .add_workspace(&h._dir.join("other").to_string_lossy(), "other", "")
        .unwrap();
    let again = h.engine.import_sessions(backup.clone(), &other.id).unwrap();
    assert_eq!(
        again.skipped,
        vec![h.session.id.clone()],
        "a session already here is never overwritten"
    );

    h.engine.store.delete_session(&h.session.id).unwrap();
    let restored = h.engine.import_sessions(backup, &other.id).unwrap();
    assert_eq!(restored.imported, vec![h.session.id.clone()]);
    let session = h.engine.store.session(&h.session.id).unwrap().unwrap();
    assert_eq!(session.workspace_id, other.id);
    assert_eq!(h.engine.store.transcript(&h.session.id).unwrap().len(), 4);
}

#[tokio::test]
async fn an_undone_tail_stays_out_of_the_backup_and_other_files_are_refused() {
    let h = harness().await;
    h.provider.push(text("first answer")).push(text("second answer"));
    h.engine.submit(&h.session.id, prompt("one")).await.unwrap();
    until_idle(&h).await;
    h.engine.submit(&h.session.id, prompt("two")).await.unwrap();
    until_idle(&h).await;
    let second = h.engine.store.transcript(&h.session.id).unwrap()[2].info.id.clone();
    h.engine
        .store
        .set_revert(
            &h.session.id,
            Some(&Revert {
                message_id: second,
                kept: Vec::new(),
                files: None,
            }),
        )
        .unwrap();

    let mut backup = h.engine.export_sessions(std::slice::from_ref(&h.session.id)).unwrap();
    assert_eq!(backup.sessions[0].messages.len(), 2, "what undo hid is not saved");
    assert!(backup.sessions[0].session.revert.is_none());

    backup.format = "something-else".into();
    let refused = h.engine.import_sessions(backup, &h.session.workspace_id);
    assert!(matches!(refused, Err(BackupError::Unreadable(_))));
}

#[test]
fn a_restored_undo_record_names_only_files_inside_the_workspace() {
    assert!(inside_workspace("src/a.rs") && inside_workspace("./a.rs"));
    let outside: &[&str] = if cfg!(windows) {
        &["", "../x", r"..\x", r"C:\Windows\x", "C:x", "/etc/passwd", r"\x"]
    } else {
        &["", "../x", "a/../../x", "/etc/passwd"]
    };
    for path in outside {
        assert!(!inside_workspace(path), "{path}");
    }
}
