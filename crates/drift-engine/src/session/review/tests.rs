use std::path::Path;
use std::process::Command;

use serde_json::json;

use super::*;
use crate::permission::{Decision, Policy, Rule};
use crate::session::turn::tests::{harness, prompt, text, tool_call, until_idle};

fn git(root: &Path, args: &[&str]) {
    let status = Command::new("git")
        .current_dir(root)
        .args(["-c", "user.email=t@t", "-c", "user.name=t", "-c", "core.autocrlf=false"])
        .args(args)
        .status()
        .unwrap();
    assert!(status.success(), "git {args:?}");
}

fn repo() -> std::path::PathBuf {
    let root = std::env::temp_dir().join(format!("drift-review-{}", crate::random_hex(4)));
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
    let root = std::env::temp_dir().join(format!("drift-review-plain-{}", crate::random_hex(4)));
    std::fs::create_dir_all(&root).unwrap();

    assert!(!git_refs(&root).await.repo);
    assert!(matches!(
        git_review(&root, GitSource::Uncommitted, None).await,
        Err(ReviewError::NotARepo)
    ));
    let _ = std::fs::remove_dir_all(root);
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
