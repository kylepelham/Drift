use super::*;
use drift_engine::session::types::{Part, Role, Visibility};
use drift_engine::store::{NewSession, Store};

/// A real engine store in a temporary directory, searched through its own read-only connection as the app does.
struct Fixture {
    dir: std::path::PathBuf,
    store: Store,
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

impl Fixture {
    fn new() -> Self {
        let dir = std::env::temp_dir().join(format!("drift-search-{}", drift_engine::id::new("t")));
        let store = drift_engine::store::open(&dir).unwrap();
        Self { dir, store }
    }

    fn search(&self, query: &str, directory: &str) -> Vec<SessionMatch> {
        search(&self.dir.join("drift.db"), query, directory).unwrap()
    }

    fn workspace(&self, path: &str) -> String {
        self.store.add_workspace(path, "w", "").unwrap().id
    }

    fn session(&self, workspace: &str, title: &str, updated: i64, parent: Option<&str>) -> String {
        let visibility = if parent.is_some() { Visibility::Hidden } else { Visibility::Sibling };
        let session = self.store.create_session(NewSession { workspace_id: workspace, parent_id: parent, visibility, title, agent: "build", model: None }).unwrap();
        self.store.lock().execute("UPDATE session SET updated_at = ?2 WHERE id = ?1", rusqlite::params![session.id, updated]).unwrap();
        session.id
    }

    /// A message holding one part; its id.
    fn said(&self, session: &str, part: Part) -> String {
        let message = self.store.create_message(session, Role::User, None).unwrap();
        self.store.add_part(&message.id, session, part).unwrap();
        message.id
    }

    fn text(&self, session: &str, text: &str) -> String {
        self.said(session, Part::Text { text: text.into() })
    }
}

#[test]
fn excerpts_map_lowercase_offsets_back_to_original_unicode_characters() {
    for character in ['\u{212a}', '\u{130}', '\u{23a}'] {
        let short = format!("{character} needle");
        assert_eq!(excerpt(&short, "needle"), short);
        let prefix = character.to_string().repeat(100);
        let text = format!("{prefix} needle");
        assert_eq!(excerpt(&text, "needle"), format!("\u{2026}{} needle", character.to_string().repeat(69)));
    }
    let f = Fixture::new();
    let ws = f.workspace("C:/work");
    let session = f.session(&ws, "Unicode", 1, None);
    f.text(&session, "\u{212a} needle");
    assert_eq!(f.search("needle", "C:/work")[0].excerpt, "\u{212a} needle");
}

#[test]
fn finds_the_newest_session_per_match_and_reports_the_matching_message() {
    let f = Fixture::new();
    let ws = f.workspace("C:\\work\\app");
    let old = f.session(&ws, "Older thread", 100, None);
    let new = f.session(&ws, "Newer thread", 200, None);
    let first = f.text(&old, "the vulkan swapchain resize path");
    f.text(&new, "unrelated");
    let match_new = f.text(&new, "checking the Vulkan swapchain again");

    let found = f.search("vulkan swapchain", "C:/work/app");
    assert_eq!(found.len(), 2);
    assert_eq!((found[0].session_id.as_str(), found[0].message_id.as_str()), (new.as_str(), match_new.as_str()));
    assert_eq!((found[1].session_id.as_str(), found[1].message_id.as_str()), (old.as_str(), first.as_str()));
    assert_eq!((found[0].title.as_str(), found[0].directory.as_str(), found[0].updated_at), ("Newer thread", "C:\\work\\app", 200));
    assert!(found[0].excerpt.to_lowercase().contains("vulkan swapchain"));
}

#[test]
fn scopes_to_one_workspace_regardless_of_slash_direction_or_case_and_skips_removed_ones() {
    let f = Fixture::new();
    let here = f.workspace("C:\\Work\\App\\");
    let elsewhere = f.workspace("D:\\other");
    let gone = f.workspace("E:\\gone");
    for (workspace, title) in [(&here, "Here"), (&elsewhere, "Elsewhere"), (&gone, "Gone")] {
        let session = f.session(workspace, title, 100, None);
        f.text(&session, "shared keyword");
    }
    f.store.lock().execute("UPDATE workspace SET removed_at = 1 WHERE id = ?1", [&gone]).unwrap();

    let scoped = f.search("shared keyword", "c:/work/app");
    assert_eq!(scoped.iter().map(|m| m.title.as_str()).collect::<Vec<_>>(), ["Here"]);
    let everywhere = f.search("shared keyword", "");
    assert_eq!(everywhere.len(), 2, "every workspace on the sidebar, not one removed from it");
}

#[test]
fn ignores_subagent_sessions_and_parts_without_readable_text() {
    let f = Fixture::new();
    let ws = f.workspace("C:\\work");
    let parent = f.session(&ws, "Parent", 100, None);
    let child = f.session(&ws, "Child", 150, Some(&parent));
    f.text(&child, "delegated finding");
    let call = Part::ToolCall { call_id: "c".into(), name: "grep".into(), input: serde_json::json!({ "pattern": "delegated finding" }), status: drift_engine::session::types::ToolStatus::Done, title: None, output: None, metadata: None, started_at: None, finished_at: None };
    f.said(&parent, call);
    assert!(f.search("delegated finding", "C:/work").is_empty(), "a tool's arguments are not the conversation");

    let reasoned = f.said(&parent, Part::Reasoning { text: "the delegated finding held up".into(), signature: None, redacted: None });
    let found = f.search("delegated finding", "C:/work");
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].message_id, reasoned);
}

#[test]
fn spawned_threads_are_searched_like_any_conversation() {
    let f = Fixture::new();
    let ws = f.workspace("C:/work");
    let source = f.session(&ws, "Source", 100, None);
    let thread = f.store.create_session(NewSession { workspace_id: &ws, parent_id: Some(&source), visibility: Visibility::Sibling, title: "Thread", agent: "build", model: None }).unwrap();
    f.text(&thread.id, "branch topic");
    assert_eq!(f.search("branch topic", "C:/work")[0].session_id, thread.id);
}

#[test]
fn treats_wildcards_as_literal_text() {
    let f = Fixture::new();
    let ws = f.workspace("C:\\work");
    let session = f.session(&ws, "Literal", 100, None);
    f.text(&session, "progress was 50% done");
    f.text(&session, "nothing relevant");

    assert_eq!(f.search("50%", "C:/work").len(), 1);
    assert!(f.search("%%", "C:/work").is_empty());
    assert!(f.search("_o", "C:/work").is_empty());
}

#[test]
fn requires_a_meaningful_query() {
    let f = Fixture::new();
    let ws = f.workspace("C:\\work");
    let session = f.session(&ws, "Short", 100, None);
    f.text(&session, "a b c");

    assert!(f.search("", "C:/work").is_empty());
    assert!(f.search(" a ", "C:/work").is_empty());
}

#[test]
fn excerpts_center_the_match_and_stay_on_one_line() {
    let long = format!("{}needle{}", "before ".repeat(40), " after".repeat(40));
    let window = excerpt(&long, "needle");
    assert!(window.contains("needle"));
    assert!(window.starts_with('…') && window.ends_with('…'));
    assert!(!window.contains('\n'));
    assert!(window.chars().count() < long.chars().count());

    let short = excerpt("first line\n\nsecond needle here", "needle");
    assert_eq!(short, "first line second needle here");
}

#[test]
fn part_text_only_reads_prose_payloads() {
    assert_eq!(part_text(&serde_json::json!({ "type": "text", "text": "hello" }).to_string()).as_deref(), Some("hello"));
    assert_eq!(part_text(&serde_json::json!({ "type": "reasoning", "text": "why" }).to_string()).as_deref(), Some("why"));
    assert!(part_text(&serde_json::json!({ "type": "tool_call", "text": "x" }).to_string()).is_none());
    assert!(part_text("not json").is_none());
}

#[test]
fn normalizes_directories_and_escapes_like_wildcards() {
    assert_eq!(normalize_directory("C:\\Work\\App\\"), "c:/work/app");
    assert_eq!(normalize_directory("C:/Work/App"), "c:/work/app");
    assert_eq!(escape_like("100%_\\x"), "100\\%\\_\\\\x");
}
