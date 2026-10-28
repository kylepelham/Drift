use std::path::Path;
use std::process::Command;

use serde_json::json;

use super::*;
use crate::permission::{Decision, Policy, Rule};
use crate::session::turn::Prompt;
use crate::session::turn::tests::{harness, prompt, text, tool_call, until_idle};
use crate::session::types::{MessageWithParts, Part, Role};

fn git(root: &Path, args: &[&str]) {
    let status = Command::new("git")
        .current_dir(root)
        .args(["-c", "user.email=t@t", "-c", "user.name=t", "-c", "core.autocrlf=false"])
        .args(["-c", "commit.gpgsign=false"])
        .args(args)
        .status()
        .unwrap();
    assert!(status.success(), "git {args:?}");
}

fn repo() -> std::path::PathBuf {
    let root = crate::platform::temp::test_run().join(format!("drift-review-{}", crate::random_hex(4)));
    std::fs::create_dir_all(&root).unwrap();
    git(&root, &["init", "-q", "-b", "main"]);
    std::fs::write(root.join("kept.txt"), "one\ntwo\nthree\n").unwrap();
    std::fs::write(root.join("gone.txt"), "bye\n").unwrap();
    git(&root, &["add", "."]);
    git(&root, &["commit", "-q", "-m", "first"]);

    root
}

fn file<'a>(review: &'a Review, path: &str) -> &'a ReviewFile {
    review
        .files
        .iter()
        .find(|file| file.path == path)
        .unwrap_or_else(|| panic!("{path} in {review:?}"))
}

#[tokio::test]
async fn uncommitted_lists_edits_deletions_and_untracked_files_with_every_line() {
    let root = repo();
    std::fs::write(root.join("kept.txt"), "one\n2\nthree\n").unwrap();
    std::fs::remove_file(root.join("gone.txt")).unwrap();
    std::fs::write(root.join("new.txt"), "fresh\n").unwrap();

    let review = git_review(&root, GitSource::Uncommitted, None).await.unwrap();

    let kept = file(&review, "kept.txt");
    assert_eq!(kept.status, FileStatus::Modified);
    assert_eq!((kept.additions, kept.deletions), (1, 1));
    assert_eq!(kept.lines, [" one", "-two", "+2", " three"]);
    assert_eq!(file(&review, "gone.txt").status, FileStatus::Deleted);
    assert_eq!(file(&review, "new.txt").lines, ["+fresh"]);
    assert_eq!(review.files.len(), 3);
    let location = |path: &str| file(&review, path).location.clone().map(std::path::PathBuf::from);
    assert_eq!(location("kept.txt"), Some(root.join("kept.txt")), "a tracked file");
    assert_eq!(location("new.txt"), Some(root.join("new.txt")), "an untracked one");
    let _ = std::fs::remove_dir_all(root);
}

#[tokio::test]
async fn a_file_past_the_size_limit_is_listed_without_being_read_on_either_side() {
    let root = repo();
    let big = "x\n".repeat(MAX_DIFF_BYTES);
    std::fs::write(root.join("big.txt"), &big).unwrap();
    git(&root, &["add", "big.txt"]);
    git(&root, &["commit", "-q", "-m", "big"]);
    std::fs::write(root.join("big.txt"), big + "more\n").unwrap();
    std::fs::write(root.join("huge.bin"), vec![b'y'; MAX_DIFF_BYTES + 1]).unwrap();

    let review = git_review(&root, GitSource::Uncommitted, None).await.unwrap();
    assert_eq!(
        file(&review, "big.txt").preview,
        Preview::Large,
        "a committed blob past the limit"
    );
    assert_eq!(
        file(&review, "huge.bin").preview,
        Preview::Large,
        "an untracked file past the limit"
    );
    assert!(file(&review, "big.txt").lines.is_empty());
    let _ = std::fs::remove_dir_all(root);
}

#[tokio::test]
async fn a_touched_but_unchanged_file_is_not_listed() {
    let root = repo();
    std::fs::write(root.join("kept.txt"), "one\ntwo\nthree\n").unwrap();

    let review = git_review(&root, GitSource::Uncommitted, None).await.unwrap();

    assert!(review.files.is_empty(), "{review:?}");
    let _ = std::fs::remove_dir_all(root);
}

#[tokio::test]
async fn a_branch_review_starts_where_the_branches_split_and_a_commit_review_shows_only_it() {
    let root = repo();
    git(&root, &["checkout", "-q", "-b", "work"]);
    std::fs::write(root.join("kept.txt"), "one\ntwo\nthree\nfour\n").unwrap();
    git(&root, &["commit", "-q", "-am", "four"]);
    git(&root, &["checkout", "-q", "main"]);
    std::fs::write(root.join("main.txt"), "main only\n").unwrap();
    git(&root, &["add", "."]);
    git(&root, &["commit", "-q", "-m", "main"]);
    git(&root, &["checkout", "-q", "work"]);

    let branch = git_review(&root, GitSource::Branch, Some("main")).await.unwrap();
    assert_eq!(branch.files.len(), 1, "main's own commit is not part of it: {branch:?}");
    assert_eq!(file(&branch, "kept.txt").additions, 1);

    let commit = git_review(&root, GitSource::Commit, Some("HEAD")).await.unwrap();
    assert_eq!(commit.files.len(), 1);
    assert_eq!(file(&commit, "kept.txt").lines.last().unwrap(), "+four");

    let refs = git_refs(&root).await;
    assert!(refs.repo);
    assert_eq!(refs.branch.as_deref(), Some("work"));
    assert_eq!(refs.branches, ["main"]);
    assert_eq!(refs.commits[0].subject, "four");
    let _ = std::fs::remove_dir_all(root);
}

#[tokio::test]
async fn a_target_that_is_an_option_or_unknown_is_refused() {
    let root = repo();

    for target in ["--output=x", "nope"] {
        let error = git_review(&root, GitSource::Commit, Some(target)).await.unwrap_err();
        assert!(matches!(error, ReviewError::UnknownTarget(_)), "{error:?}");
    }
    let _ = std::fs::remove_dir_all(root);
}

#[tokio::test]
async fn outside_a_repository_there_is_nothing_to_pick() {
    let root = crate::platform::temp::test_run().join(format!("drift-review-plain-{}", crate::random_hex(4)));
    std::fs::create_dir_all(&root).unwrap();

    assert!(!git_refs(&root).await.repo);
    assert!(matches!(
        git_review(&root, GitSource::Uncommitted, None).await,
        Err(ReviewError::NotARepo)
    ));
    let _ = std::fs::remove_dir_all(root);
}

#[tokio::test]
async fn named_files_come_back_as_they_are_with_or_without_a_repository() {
    let root = crate::platform::temp::test_run().join(format!("drift-review-files-{}", crate::random_hex(4)));
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("src/app.rs"), "fn main() {}\r\nfn other() {}\n").unwrap();
    std::fs::write(root.join("logo.png"), [0u8, 1, 2]).unwrap();

    let listed = "src/app.rs\n\nmissing.rs\nlogo.png\n";
    let review = git_review(&root, GitSource::Files, Some(listed)).await.unwrap();
    assert_eq!(review.files.len(), 2, "a missing file is left out: {review:?}");
    let app = file(&review, "src/app.rs");
    assert_eq!(app.lines, [" fn main() {}", " fn other() {}"]);
    assert_eq!(
        (app.status, app.additions, app.deletions),
        (FileStatus::Unchanged, 0, 0)
    );
    assert_eq!(
        app.location.as_deref().map(std::path::PathBuf::from),
        Some(root.join("src").join("app.rs"))
    );
    assert_eq!(file(&review, "logo.png").preview, Preview::Binary);

    let outside = std::env::temp_dir().join("x.rs");
    for path in ["../x.rs", "src/../../x.rs", outside.to_str().unwrap(), "/x.rs", "."] {
        let error = git_review(&root, GitSource::Files, Some(path)).await.unwrap_err();
        assert!(matches!(error, ReviewError::OutsidePath(_)), "{path}: {error:?}");
    }
    let many: String = (0..=MAX_FILES).map(|_| "src/app.rs\n").collect();
    let capped = git_review(&root, GitSource::Files, Some(&many)).await.unwrap();
    assert_eq!((capped.files.len(), capped.omitted), (MAX_FILES, 1));
    let _ = std::fs::remove_dir_all(root);
}

#[tokio::test]
async fn a_link_leading_out_of_the_workspace_is_never_read() {
    let base = crate::platform::temp::test_run().join(format!("drift-review-link-{}", crate::random_hex(4)));
    let root = base.join("workspace");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(base.join("secret.txt"), "private").unwrap();
    #[cfg(unix)]
    let linked = std::os::unix::fs::symlink(base.join("secret.txt"), root.join("link.txt"));
    #[cfg(windows)]
    let linked = std::os::windows::fs::symlink_file(base.join("secret.txt"), root.join("link.txt"));
    if linked.is_err() {
        let _ = std::fs::remove_dir_all(base);
        return;
    }

    let review = git_review(&root, GitSource::Files, Some("link.txt")).await.unwrap();
    let link = file(&review, "link.txt");
    assert_eq!((link.preview, link.lines.len()), (Preview::Unavailable, 0));
    assert_eq!(link.location, None, "nor opened from the review");
    let _ = std::fs::remove_dir_all(base);
}

#[test]
fn binary_and_oversized_sides_are_listed_without_lines() {
    let binary = review_file("a.bin".into(), &Side::Absent, &Side::Present(vec![0, 1, 2]));
    assert_eq!((binary.preview, binary.lines.len()), (Preview::Binary, 0));

    let large = review_file(
        "a.txt".into(),
        &Side::Present(vec![b'a'; MAX_DIFF_BYTES + 1]),
        &Side::Absent,
    );
    assert_eq!((large.preview, large.status), (Preview::Large, FileStatus::Deleted));
}

#[tokio::test]
async fn a_turn_review_spans_the_turn_from_before_its_first_change_to_after_its_last() {
    let h = harness().await;
    h.engine.permissions.set_policy(Policy {
        rules: vec![Rule {
            kind: "edit".into(),
            pattern: "*".into(),
            decision: Decision::Allow,
        }],
        ..Default::default()
    });
    let write =
        |path: &str, content: &str| tool_call("write", &json!({ "path": path, "content": content }).to_string());
    h.provider.push(write("a.txt", "one\n")).push(text("wrote"));
    h.engine.submit(&h.session.id, prompt("first")).await.unwrap();
    until_idle(&h).await;
    h.provider
        .push(write("a.txt", "two\n"))
        .push(write("a.txt", "three\n"))
        .push(text("rewrote"));
    h.engine.submit(&h.session.id, prompt("second")).await.unwrap();
    until_idle(&h).await;

    let prompts: Vec<String> = h
        .engine
        .store
        .transcript(&h.session.id)
        .unwrap()
        .iter()
        .filter(|message| is_prompt(message))
        .map(|message| message.info.id.clone())
        .collect();

    let first = h.engine.turn_review(&h.session.id, &prompts[0]).await.unwrap();
    assert_eq!(file(&first, "a.txt").lines, ["+one"]);
    assert_eq!(file(&first, "a.txt").status, FileStatus::Added);
    assert_eq!(
        file(&first, "a.txt").location.as_deref().map(std::path::Path::new),
        Some(crate::tool::canonical(&h._dir.join("ws").join("a.txt")).as_path()),
        "where the turn wrote it"
    );

    let second = h.engine.turn_review(&h.session.id, &prompts[1]).await.unwrap();
    assert_eq!(file(&second, "a.txt").lines, ["-one", "+three"]);

    let reply = h
        .engine
        .store
        .transcript(&h.session.id)
        .unwrap()
        .last()
        .unwrap()
        .info
        .id
        .clone();
    assert!(matches!(
        h.engine.turn_review(&h.session.id, &reply).await,
        Err(ReviewError::NotAPrompt)
    ));
}

#[tokio::test]
async fn a_turn_review_keeps_edits_made_after_the_engine_carried_the_turn_on() {
    let h = harness().await;
    h.engine.permissions.set_policy(Policy {
        rules: vec![Rule {
            kind: "edit".into(),
            pattern: "*".into(),
            decision: Decision::Allow,
        }],
        ..Default::default()
    });
    let write =
        |path: &str, content: &str| tool_call("write", &json!({ "path": path, "content": content }).to_string());
    h.provider.push(write("plan.md", "draft\n")).push(text("drafted"));
    h.engine.submit(&h.session.id, prompt("write the plan")).await.unwrap();
    until_idle(&h).await;

    // What a worker's result or a drive nudge looks like: a user message the engine wrote.
    h.provider.push(write("notes.md", "follow up\n")).push(text("noted"));
    let carry_on = Prompt {
        parts: vec![Part::Nudge {
            text: "keep going".into(),
        }],
        ..prompt("")
    };
    h.engine.submit(&h.session.id, carry_on).await.unwrap();
    until_idle(&h).await;

    let transcript = h.engine.store.transcript(&h.session.id).unwrap();
    let users: Vec<&MessageWithParts> = transcript
        .iter()
        .filter(|message| message.info.role == Role::User)
        .collect();
    assert!(!is_prompt(users[1]), "a nudge carries the turn on");

    let review = h.engine.turn_review(&h.session.id, &users[0].info.id).await.unwrap();
    assert_eq!(file(&review, "plan.md").lines, ["+draft"]);
    assert_eq!(file(&review, "notes.md").lines, ["+follow up"]);
}

#[test]
fn a_batch_reply_is_read_object_by_object_even_when_content_holds_newlines() {
    let output = b"aaa blob 6\none\ntw\nbbb missing\nccc blob 0\n\nddd blob 3\nxyz\n";
    let found = super::parse_batch(output);
    assert_eq!(found["aaa"], b"one\ntw");
    assert!(!found.contains_key("bbb"), "a missing object is unavailable");
    assert_eq!(found["ccc"], b"");
    assert_eq!(found["ddd"], b"xyz");
    assert!(
        super::parse_batch(b"eee blob 99\nshort").is_empty(),
        "a cut-off reply is not trusted"
    );
}

#[tokio::test]
async fn files_touched_but_unchanged_never_leave_a_later_change_unread() {
    let root = repo();
    for index in 0..MAX_FILES {
        std::fs::write(root.join(format!("a{index:03}.txt")), "same\n").unwrap();
    }
    std::fs::write(root.join("z.txt"), "old\n").unwrap();
    git(&root, &["add", "."]);
    git(&root, &["commit", "-q", "-m", "many"]);
    std::thread::sleep(std::time::Duration::from_millis(20));
    for index in 0..MAX_FILES {
        std::fs::write(root.join(format!("a{index:03}.txt")), "same\n").unwrap();
    }
    std::fs::write(root.join("z.txt"), "new\n").unwrap();

    let review = git_review(&root, GitSource::Uncommitted, None).await.unwrap();
    assert_eq!(file(&review, "z.txt").lines, ["-old", "+new"]);
    assert_eq!(review.files.len(), 1);
    let _ = std::fs::remove_dir_all(root);
}

#[tokio::test]
async fn a_commit_that_only_changes_a_files_mode_lists_nothing() {
    let root = repo();
    std::fs::write(root.join("big.txt"), "x\n".repeat(MAX_DIFF_BYTES)).unwrap();
    git(&root, &["add", "big.txt"]);
    git(&root, &["commit", "-q", "-m", "big"]);
    git(&root, &["update-index", "--chmod=+x", "big.txt", "kept.txt"]);
    git(&root, &["commit", "-q", "-m", "mode"]);

    let review = git_review(&root, GitSource::Commit, Some("HEAD")).await.unwrap();
    assert!(review.files.is_empty(), "{review:?}");
    let _ = std::fs::remove_dir_all(root);
}
