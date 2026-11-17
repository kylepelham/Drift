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

#[tokio::test]
async fn a_subagent_whose_conversation_is_nowhere_is_left_out_of_a_restore() {
    let h = harness().await;
    let backup = h.engine.export_sessions(std::slice::from_ref(&h.session.id)).unwrap();
    let entry = |id: &str, parent: &str, visibility: Visibility| {
        let mut entry = backup.sessions[0].clone();
        entry.session.id = id.into();
        entry.session.parent_id = Some(parent.into());
        entry.session.visibility = visibility;
        entry.messages.clear();
        entry
    };
    let sessions = vec![
        entry("ses_orphan", "ses_gone", Visibility::Hidden),
        entry("ses_orphan_child", "ses_orphan", Visibility::Hidden),
        entry("ses_thread", "ses_gone", Visibility::Sibling),
        entry("ses_kept", &h.session.id, Visibility::Hidden),
    ];

    let restored = h
        .engine
        .import_sessions(Backup { sessions, ..backup }, &h.session.workspace_id)
        .unwrap();

    assert_eq!(restored.skipped, ["ses_orphan", "ses_orphan_child"]);
    assert_eq!(restored.imported, ["ses_thread", "ses_kept"]);
    assert!(h.engine.store.session("ses_orphan").unwrap().is_none());
    let thread = h.engine.store.session("ses_thread").unwrap().unwrap();
    assert_eq!(thread.parent_id, None, "a spawned thread stands on its own");
    let kept = h.engine.store.session("ses_kept").unwrap().unwrap();
    assert_eq!(kept.parent_id.as_deref(), Some(h.session.id.as_str()));
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
