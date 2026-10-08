use super::{Location, compact, quote_list, stats};
use drift_engine::session::types::{Part, Role, Visibility};
use drift_engine::store::NewSession;

struct Dir(std::path::PathBuf);

impl Drop for Dir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn stats_count_conversations_and_size_the_database_and_the_engines_folders() {
    let dir = Dir(std::env::temp_dir().join(format!("drift-storage-{}", drift_engine::id::new("t"))));
    let store = drift_engine::store::open(&dir.0).unwrap();
    let ws = store.add_workspace("C:/w", "w", "").unwrap();
    let new = |title: &str, parent: Option<&str>, visibility| {
        store
            .create_session(NewSession {
                workspace_id: &ws.id,
                parent_id: parent,
                visibility,
                title,
                agent: "build",
                model: None,
            })
            .unwrap()
    };
    let parent = new("parent", None, Visibility::Sibling);
    new("child", Some(&parent.id), Visibility::Hidden);
    let archived = new("archived", None, Visibility::Sibling);
    store.set_session_archived(&archived.id, true).unwrap();
    let listed = new("listed in Drift's archive", None, Visibility::Sibling);
    let message = store.create_message(&parent.id, Role::User, None).unwrap();
    store
        .add_part(
            &message.id,
            &parent.id,
            Part::Text {
                text: "x".repeat(10_000),
            },
        )
        .unwrap();
    std::fs::create_dir_all(dir.0.join("snapshots/ws-a/objects")).unwrap();
    std::fs::write(dir.0.join("snapshots/ws-a/objects/blob"), vec![0u8; 4096]).unwrap();
    std::fs::create_dir_all(dir.0.join("tool-output/ses_1")).unwrap();
    std::fs::write(dir.0.join("tool-output/ses_1/out.txt"), vec![0u8; 1000]).unwrap();

    let location = Location {
        data_dir: dir.0.clone(),
    };
    let found = stats(&location, &[listed.id.clone(), "bad'id".into()]).unwrap();
    assert_eq!(
        (
            found.sessions.total,
            found.sessions.top_level,
            found.sessions.subagent,
            found.sessions.archived
        ),
        (4, 3, 1, 2)
    );
    let size = |name: &str| found.tables.iter().find(|table| table.table == name).unwrap().bytes;
    assert!(size("part") >= 10_000, "{}", size("part"));
    assert_eq!((size("undo"), size("output")), (4096, 1000));
    assert!(found.total_bytes >= 5096 + size("part"));

    drop(store);
    let compacted = compact(&location).unwrap();
    assert_eq!(compacted.free_bytes, 0, "a compacted file holds no free pages");
}

#[test]
fn quote_list_drops_ids_that_are_not_plain_identifiers() {
    let quoted = quote_list(&[
        "ses_ok-1".into(),
        "bad'; DROP TABLE session; --".into(),
        "also_ok".into(),
    ]);
    assert_eq!(quoted, "'ses_ok-1','also_ok'");
    assert!(!quoted.contains("DROP"));
}
