use std::path::Path;
use std::process::Command;

use super::scope::{Changes, count, resolve};
use super::*;

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

fn changes(lines: usize) -> Scope {
    Scope::Changes(Changes {
        base: "abc123".into(),
        base_name: "main".into(),
        files: 3,
        lines,
        untracked: vec!["notes/plan.md".into()],
    })
}

#[test]
fn arguments_name_a_level_flags_and_a_target() {
    assert_eq!(parse(""), Request::default());
    assert_eq!(
        parse("HIGH --fix 'feature/login form' --comment --max-findings"),
        Request {
            level: Level::High,
            target: Some("feature/login form".into()),
            fix: true,
            comment: true,
        }
    );
    assert_eq!(parse("xhigh").level, Level::Max);
    assert_eq!(
        parse("low high").target.as_deref(),
        Some("high"),
        "only the first level is one"
    );
    assert_eq!(parse("1432").target.as_deref(), Some("1432"));
}

#[test]
fn levels_shape_the_method_and_the_cap() {
    let low = render(&parse("low"), Some(&changes(900)));
    assert!(!low.contains("`task`") && low.contains("at most 5 findings"), "{low}");
    assert!(!low.contains("`verdict`"));

    let medium = render(&parse(""), Some(&changes(40)));
    assert!(
        medium.contains("across 2 finder subagents"),
        "the fewest for a small diff: {medium}"
    );
    assert!(medium.contains("precision matters more") && medium.contains("at most 8 findings"));
    assert!(!medium.contains("Sweep") && !medium.contains("Language pitfalls"));

    let high = render(&parse("high"), Some(&changes(700)));
    assert!(high.contains("across 5 finder subagents"), "{high}");

    let max = render(&parse("max"), Some(&changes(5000)));
    assert!(max.contains("across 8 finder subagents") && max.contains("### 3. Sweep"));
    assert!(max.contains("Language pitfalls") && max.contains("at most 15 findings"));
    assert!(!max.contains('{'), "every placeholder filled: {max}");
}

#[test]
fn scope_flags_and_targets_change_the_instructions() {
    let local = render(&parse(""), Some(&changes(120)));
    assert!(local.contains("`git diff abc123`") && local.contains("`notes/plan.md`"));
    assert!(local.contains("Do not edit") && local.contains("## Afterwards") && !local.contains("## Fix"));

    let fixing = render(&parse("--fix --comment"), Some(&changes(120)));
    assert!(fixing.contains("## Fix") && !fixing.contains("## Afterwards"));
    assert!(
        fixing.contains("repos/{owner}/{repo}/pulls"),
        "gh's own placeholders survive"
    );

    let target = render(&parse("medium 1432"), None);
    assert!(
        target.contains("`gh pr diff 1432`") && target.contains("about one per 150 changed lines and 2 to 4 in all")
    );

    let clean = render(
        &parse(""),
        Some(&Scope::Clean {
            base_name: "main".into(),
        }),
    );
    assert!(clean.contains("nothing to review") && !clean.contains("report_findings` once"));
    assert!(render(&parse("high"), Some(&Scope::NotRepo)).contains("not in a git repository"));
    let failed = render(&parse(""), Some(&Scope::Unlisted("git diff")));
    assert!(failed.contains("`git diff` failed") && !failed.contains("nothing to review"));
}

#[test]
fn numstat_counts_text_and_binary_files() {
    assert_eq!(count("3\t1\tsrc/a.rs\n-\t-\tlogo.png\n10\t0\tdocs/b.md\n"), (3, 14));
    assert_eq!(count(""), (0, 0));
}

#[tokio::test]
async fn the_scope_is_the_branch_work_and_uncommitted_files() {
    let root = crate::platform::temp::test_run().join(format!("drift-code-review-{}", crate::random_hex(4)));
    std::fs::create_dir_all(&root).unwrap();
    assert_eq!(resolve(&root).await, Scope::NotRepo);

    git(&root, &["init", "-q", "-b", "main"]);
    std::fs::write(root.join("app.txt"), "one\ntwo\n").unwrap();
    git(&root, &["add", "."]);
    git(&root, &["commit", "-q", "-m", "first"]);
    assert_eq!(
        resolve(&root).await,
        Scope::Clean {
            base_name: "HEAD".into()
        }
    );

    git(&root, &["checkout", "-q", "-b", "feature"]);
    std::fs::write(root.join("app.txt"), "one\ntwo\nthree\n").unwrap();
    git(&root, &["commit", "-q", "-am", "second"]);
    std::fs::write(root.join("app.txt"), "one\n2\nthree\n").unwrap();
    std::fs::write(root.join("new.txt"), "a\nb").unwrap();
    let Scope::Changes(found) = resolve(&root).await else {
        panic!("changes expected");
    };
    assert_eq!(found.base_name, "main");
    assert_eq!(
        (found.files, found.lines),
        (2, 5),
        "three lines in the diff, two untracked"
    );
    assert_eq!(found.untracked, ["new.txt"]);

    let _ = std::fs::remove_dir_all(&root);
}
