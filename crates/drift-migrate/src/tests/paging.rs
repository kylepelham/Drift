use super::*;

#[test]
fn messages_with_equal_timestamps_keep_their_order_across_read_and_write_pages() {
    let directory = dir();
    let source = directory.0.join("opencode.db");
    let conn = opencode(&source);
    session(&conn, "ses_a", None, "C:/repo", None);
    let conversation = Conversation::new(&conn, "ses_a");
    let count = READ_PAGE + 3;

    for index in 0..count {
        let text = format!("Please review section {} of the migration notes.", index + 1);
        conversation.message(
            &format!("msg_{index:03}"),
            2000,
            user(2000),
            &[json!({ "type": "text", "text": text })],
        );
    }
    drop(conn);
    let store = store_with(&directory.0, &["C:/repo"]);

    let report = run_import(&store, &source, &HashSet::new(), &mut Kept::default(), &mut |_| {}).unwrap();
    let transcript = store.transcript("ses_a").unwrap();

    assert_eq!(report.imported, 1);
    assert_eq!(transcript.len(), count);
    for (index, message) in transcript.iter().enumerate() {
        assert_eq!(message.info.created_at, 2000);
        assert_eq!(
            message.parts[0].part,
            Part::Text {
                text: format!("Please review section {} of the migration notes.", index + 1),
            }
        );
    }
    assert!(transcript.windows(2).all(|pair| pair[0].info.id < pair[1].info.id));

    let again = run_import(&store, &source, &HashSet::new(), &mut Kept::default(), &mut |_| {}).unwrap();

    assert_eq!(again.known, 1);
    assert_eq!(store.transcript("ses_a").unwrap(), transcript);
}
