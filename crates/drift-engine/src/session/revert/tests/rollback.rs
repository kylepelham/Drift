use super::*;
use crate::tool::stage::tests::{Fault, inject, leftovers};

#[tokio::test]
async fn an_undo_whose_write_fails_once_begun_leaves_the_file_whole_and_can_be_tried_again() {
    let h = harness().await;
    let (_, second) = two_writing_turns(&h).await;

    let workspace = crate::tool::canonical(&h._dir.join("ws"));
    inject(Fault::AfterStaging, &workspace.join("a.txt"));

    assert!(matches!(
        h.engine.revert(&h.session.id, &second).await,
        Err(RevertError::Files(_))
    ));
    assert_eq!(read(&h, "a.txt").as_deref(), Some("two"), "not cut short");
    assert!(leftovers(&workspace).is_empty() && h.engine.store.replacements().unwrap().is_empty());

    h.engine.revert(&h.session.id, &second).await.unwrap();
    assert_eq!(
        read(&h, "a.txt").as_deref(),
        Some("one"),
        "the conflict check still lets the untouched file go back"
    );
}

#[tokio::test]
async fn an_undo_that_fails_partway_puts_back_what_it_already_changed() {
    let h = harness().await;
    allow_writes(&h);
    h.provider
        .push(write("a.txt", "one"))
        .push(write("b.txt", "uno"))
        .push(text("first"));
    turn(&h, "first").await;
    h.provider
        .push(write("a.txt", "two"))
        .push(write("b.txt", "dos"))
        .push(text("second"));
    turn(&h, "second").await;
    let second = h
        .engine
        .store
        .transcript(&h.session.id)
        .unwrap()
        .iter()
        .filter(|message| message.info.role == Role::User)
        .nth(1)
        .unwrap()
        .info
        .id
        .clone();
    let workspace = crate::tool::canonical(&h._dir.join("ws"));
    inject(Fault::AfterStaging, &workspace.join("b.txt"));
    let Err(RevertError::Files(message)) = h.engine.revert(&h.session.id, &second).await else {
        panic!("the undo should fail")
    };
    assert!(
        message.contains("b.txt") && message.contains("no file was changed"),
        "{message}"
    );
    assert_eq!(
        (read(&h, "a.txt").as_deref(), read(&h, "b.txt").as_deref()),
        (Some("two"), Some("dos")),
        "not half undone"
    );
    assert!(h.engine.store.session(&h.session.id).unwrap().unwrap().revert.is_none());

    let undone = h.engine.revert(&h.session.id, &second).await.unwrap();
    assert!(
        undone.kept.is_empty(),
        "nothing the user did not touch is reported as kept: {:?}",
        undone.kept
    );
    assert_eq!(
        (read(&h, "a.txt").as_deref(), read(&h, "b.txt").as_deref()),
        (Some("one"), Some("uno"))
    );
}

#[tokio::test]
async fn an_undo_or_redo_whose_marker_cannot_be_saved_puts_the_files_back_and_can_be_tried_again() {
    let h = harness().await;
    let (_, second) = two_writing_turns(&h).await;
    refuse_marker(&h);
    let Err(RevertError::Files(message)) = h.engine.revert(&h.session.id, &second).await else {
        panic!("the undo should fail")
    };
    assert!(
        message.contains("undo point") && message.contains("no file was changed"),
        "{message}"
    );
    assert_eq!(
        (read(&h, "a.txt").as_deref(), read(&h, "b.txt").as_deref()),
        (Some("two"), Some("bee")),
        "files and history still agree"
    );
    allow_marker(&h);
    let undone = h.engine.revert(&h.session.id, &second).await.unwrap();
    assert!(
        undone.kept.is_empty(),
        "nothing is wrongly reported as edited elsewhere: {:?}",
        undone.kept
    );
    assert_eq!((read(&h, "a.txt").as_deref(), read(&h, "b.txt")), (Some("one"), None));

    refuse_marker(&h);
    assert!(matches!(
        h.engine.unrevert(&h.session.id).await,
        Err(RevertError::Files(_))
    ));
    assert_eq!(
        (read(&h, "a.txt").as_deref(), read(&h, "b.txt")),
        (Some("one"), None),
        "a failed redo leaves the undone files"
    );
    assert!(h.engine.store.session(&h.session.id).unwrap().unwrap().revert.is_some());
    allow_marker(&h);
    let redone = h.engine.unrevert(&h.session.id).await.unwrap();
    assert!(redone.kept.is_empty() && redone.session.revert.is_none());
    assert_eq!(
        (read(&h, "a.txt").as_deref(), read(&h, "b.txt").as_deref()),
        (Some("two"), Some("bee"))
    );
}

#[tokio::test]
async fn an_undo_takes_every_files_turn_before_changing_any() {
    let h = harness().await;
    let (_, second) = two_writing_turns(&h).await;

    let workspace = crate::tool::canonical(&h._dir.join("ws"));
    let held = crate::tool::lock::files(&[workspace.join("b.txt")]).await;
    let (engine, id) = (h.engine.clone(), h.session.id.clone());
    let undo = tokio::spawn(async move { engine.revert(&id, &second).await.map(|_| ()) });

    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(!undo.is_finished());
    assert_eq!(
        read(&h, "a.txt").as_deref(),
        Some("two"),
        "a.txt waits too, so a rollback never lands over another writer"
    );

    drop(held);
    tokio::time::timeout(Duration::from_secs(5), undo)
        .await
        .unwrap()
        .unwrap()
        .unwrap();

    assert_eq!((read(&h, "a.txt").as_deref(), read(&h, "b.txt")), (Some("one"), None));
}

#[tokio::test]
async fn rollback_keeps_competing_writers_out_until_the_marker_failure_is_repaired() {
    let h = harness().await;
    let (_, second) = two_writing_turns(&h).await;
    let session = h.engine.store.session(&h.session.id).unwrap().unwrap();
    let shifted = h.engine.shift(&session, &second, None, Direction::Back).await.unwrap();
    assert_eq!(read(&h, "a.txt").as_deref(), Some("one"));

    let workspace = h._dir.join("ws");
    let competing = tokio::spawn(async move {
        let file = workspace.join("a.txt");
        let _held = crate::tool::lock::files(std::slice::from_ref(&file)).await;
        tokio::fs::write(file, "another session").await.unwrap();
    });

    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(
        !competing.is_finished(),
        "undo still holds the files while its marker is uncommitted"
    );

    refuse_marker(&h);
    let marker = Revert::new(&second, Vec::new(), Some(&second));
    assert!(
        h.engine
            .mark_or_put_back(&h.session.id, Some(&marker), shifted)
            .await
            .is_err()
    );
    tokio::time::timeout(Duration::from_secs(5), competing)
        .await
        .unwrap()
        .unwrap();

    assert_eq!(
        read(&h, "a.txt").as_deref(),
        Some("another session"),
        "rollback completed before the competing writer ran"
    );
    assert!(h.engine.store.session(&h.session.id).unwrap().unwrap().revert.is_none());
}

#[tokio::test]
async fn cross_workspace_undo_rolls_back_from_the_endpoint_that_owns_the_previous_bytes() {
    let (h, first, _, file) = overlapping_writes(false).await;

    refuse_marker(&h);
    assert!(matches!(
        h.engine.revert(&h.session.id, &first).await,
        Err(RevertError::Files(_))
    ));
    assert_eq!(
        std::fs::read_to_string(&file).unwrap(),
        "C",
        "C is stored only in the nested workspace's snapshot repository"
    );
    assert!(h.engine.store.session(&h.session.id).unwrap().unwrap().revert.is_none());

    allow_marker(&h);
    h.engine.revert(&h.session.id, &first).await.unwrap();
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "A");

    refuse_marker(&h);
    assert!(matches!(
        h.engine.unrevert(&h.session.id).await,
        Err(RevertError::Files(_))
    ));
    assert_eq!(
        std::fs::read_to_string(&file).unwrap(),
        "A",
        "redo rollback reads A from the original workspace's repository"
    );
    assert!(h.engine.store.session(&h.session.id).unwrap().unwrap().revert.is_some());
}

#[tokio::test]
async fn stopping_an_undo_waiting_for_files_leaves_them_unchanged() {
    let h = harness().await;
    let (_, second) = two_writing_turns(&h).await;

    let held = crate::tool::lock::files(&[h._dir.join("ws/a.txt")]).await;
    let (engine, id) = (h.engine.clone(), h.session.id.clone());
    let undo = tokio::spawn(async move { engine.revert(&id, &second).await });

    for _ in 0..100 {
        if h.engine.turns.is_running(&h.session.id) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }

    assert!(h.engine.abort(&h.session.id));
    assert!(matches!(
        tokio::time::timeout(Duration::from_secs(2), undo)
            .await
            .unwrap()
            .unwrap(),
        Err(RevertError::Stopped)
    ));
    assert_eq!(read(&h, "a.txt").as_deref(), Some("two"));
    assert!(h.engine.store.session(&h.session.id).unwrap().unwrap().revert.is_none());

    drop(held);
}
