use super::*;

/// The columns this search actually reads, matching the engine's `session` and `part` tables.
fn fixture() -> Connection {
    let conn = Connection::open_in_memory().unwrap();
    conn.execute_batch(
        "CREATE TABLE session(
            id TEXT PRIMARY KEY,
            parent_id TEXT,
            directory TEXT,
            title TEXT,
            time_updated INTEGER
        );
        CREATE TABLE part(
            id TEXT PRIMARY KEY,
            message_id TEXT,
            session_id TEXT,
            time_created INTEGER,
            data TEXT
        );",
    )
    .unwrap();
    conn
}

fn add_session(conn: &Connection, id: &str, directory: &str, title: &str, updated: i64) {
    conn.execute(
        "INSERT INTO session(id, parent_id, directory, title, time_updated) VALUES(?1, NULL, ?2, ?3, ?4)",
        rusqlite::params![id, directory, title, updated],
    )
    .unwrap();
}

fn add_part(conn: &Connection, id: &str, session: &str, message: &str, created: i64, data: &str) {
    conn.execute(
        "INSERT INTO part(id, message_id, session_id, time_created, data) VALUES(?1, ?2, ?3, ?4, ?5)",
        rusqlite::params![id, message, session, created, data],
    )
    .unwrap();
}

fn text_part(text: &str) -> String {
    serde_json::json!({ "type": "text", "text": text }).to_string()
}

#[test]
fn finds_the_newest_session_per_match_and_reports_the_matching_message() {
    let conn = fixture();
    add_session(&conn, "old", "C:\\work\\app", "Older thread", 100);
    add_session(&conn, "new", "C:\\work\\app", "Newer thread", 200);
    add_part(&conn, "p1", "old", "m1", 1, &text_part("the vulkan swapchain resize path"));
    add_part(&conn, "p2", "new", "m2", 1, &text_part("unrelated"));
    add_part(&conn, "p3", "new", "m3", 2, &text_part("checking the Vulkan swapchain again"));

    let found = search_in(&conn, "vulkan swapchain", "C:/work/app").unwrap();
    assert_eq!(found.len(), 2);
    // Newest session first, and the match points at the message that actually contains the text.
    assert_eq!(found[0].session_id, "new");
    assert_eq!(found[0].message_id, "m3");
    assert_eq!(found[1].session_id, "old");
    assert_eq!(found[1].message_id, "m1");
    assert!(found[0].excerpt.to_lowercase().contains("vulkan swapchain"));
}

#[test]
fn scopes_to_one_workspace_regardless_of_slash_direction_or_case() {
    let conn = fixture();
    add_session(&conn, "here", "C:\\Work\\App", "Here", 100);
    add_session(&conn, "elsewhere", "D:\\other", "Elsewhere", 200);
    add_part(&conn, "p1", "here", "m1", 1, &text_part("shared keyword"));
    add_part(&conn, "p2", "elsewhere", "m2", 1, &text_part("shared keyword"));

    let scoped = search_in(&conn, "shared keyword", "c:/work/app").unwrap();
    assert_eq!(scoped.len(), 1);
    assert_eq!(scoped[0].session_id, "here");

    // An empty scope searches every workspace.
    let everywhere = search_in(&conn, "shared keyword", "").unwrap();
    assert_eq!(everywhere.len(), 2);
}

#[test]
fn ignores_subagent_sessions_and_payloads_without_readable_text() {
    let conn = fixture();
    add_session(&conn, "parent", "C:\\work", "Parent", 100);
    conn.execute(
        "INSERT INTO session(id, parent_id, directory, title, time_updated) VALUES('child', 'parent', 'C:\\work', 'Child', 150)",
        [],
    )
    .unwrap();
    add_part(&conn, "p1", "child", "m1", 1, &text_part("delegated finding"));
    // A tool call carrying the term in its arguments is not a conversation match.
    add_part(
        &conn,
        "p2",
        "parent",
        "m2",
        1,
        &serde_json::json!({ "type": "tool", "state": { "input": { "pattern": "delegated finding" } } }).to_string(),
    );

    let found = search_in(&conn, "delegated finding", "C:/work").unwrap();
    assert!(found.is_empty());

    // The same words in an assistant message are a match, including reasoning parts.
    add_part(
        &conn,
        "p3",
        "parent",
        "m3",
        2,
        &serde_json::json!({ "type": "reasoning", "text": "the delegated finding held up" }).to_string(),
    );
    let reasoned = search_in(&conn, "delegated finding", "C:/work").unwrap();
    assert_eq!(reasoned.len(), 1);
    assert_eq!(reasoned[0].message_id, "m3");
}

#[test]
fn treats_wildcards_as_literal_text() {
    let conn = fixture();
    add_session(&conn, "s", "C:\\work", "Literal", 100);
    add_part(&conn, "p1", "s", "m1", 1, &text_part("progress was 50% done"));
    add_part(&conn, "p2", "s", "m2", 2, &text_part("nothing relevant"));

    assert_eq!(search_in(&conn, "50%", "C:/work").unwrap().len(), 1);
    // Bare wildcards must not match everything.
    assert!(search_in(&conn, "%%", "C:/work").unwrap().is_empty());
    assert!(search_in(&conn, "_o", "C:/work").unwrap().is_empty());
}

#[test]
fn requires_a_meaningful_query() {
    let conn = fixture();
    add_session(&conn, "s", "C:\\work", "Short", 100);
    add_part(&conn, "p1", "s", "m1", 1, &text_part("a b c"));

    assert!(search_in(&conn, "", "C:/work").unwrap().is_empty());
    assert!(search_in(&conn, " a ", "C:/work").unwrap().is_empty());
}

#[test]
fn excerpts_center_the_match_and_stay_on_one_line() {
    let long = format!("{}needle{}", "before ".repeat(40), " after".repeat(40));
    let window = excerpt(&long, "needle");
    assert!(window.contains("needle"));
    assert!(window.starts_with('…') && window.ends_with('…'));
    assert!(!window.contains('\n'));
    assert!(window.chars().count() < long.chars().count());

    // Multi-line input collapses, and a match near the start keeps its leading text.
    let short = excerpt("first line\n\nsecond needle here", "needle");
    assert_eq!(short, "first line second needle here");
}

#[test]
fn part_text_only_reads_prose_payloads() {
    assert_eq!(part_text(&text_part("hello")).as_deref(), Some("hello"));
    assert_eq!(
        part_text(&serde_json::json!({ "type": "reasoning", "text": "why" }).to_string()).as_deref(),
        Some("why"),
    );
    assert!(part_text(&serde_json::json!({ "type": "tool", "text": "x" }).to_string()).is_none());
    assert!(part_text("not json").is_none());
}

#[test]
fn normalizes_directories_and_escapes_like_wildcards() {
    assert_eq!(normalize_directory("C:\\Work\\App\\"), "c:/work/app");
    assert_eq!(normalize_directory("C:/Work/App"), "c:/work/app");
    assert_eq!(escape_like("100%_\\x"), "100\\%\\_\\\\x");
}
