use std::path::Path;
use std::process::Command;

use super::survey::{Area, Counted, balance, run};
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

fn survey(files: &[(&str, usize)]) -> Survey {
    Survey {
        files: files
            .iter()
            .map(|(path, lines)| Counted {
                path: (*path).into(),
                lines: *lines,
            })
            .collect(),
        unmeasured: 0,
        churn: vec![("src/hot.rs".into(), 7)],
    }
}

/// A hundred folders of 3000 lines each: 300000 lines in a hundred areas.
fn wide() -> Survey {
    let paths: Vec<String> = (0..100).map(|at| format!("pkg{at:02}/lib.rs")).collect();
    let files: Vec<(&str, usize)> = paths.iter().map(|path| (path.as_str(), 3000)).collect();
    survey(&files)
}

fn assert_clean(text: &str) {
    assert!(
        !text.contains('{') && !text.contains('}'),
        "every placeholder filled: {text}"
    );
    assert!(!text.contains('\u{2014}'), "no em dashes: {text}");
}

#[test]
fn levels_set_the_finders_the_angles_and_the_cap() {
    let surveyed = Surveyed::Files(wide());

    let low = render(&parse("low"), &surveyed);
    assert!(!low.contains("`task`") && low.contains("at most 8 findings") && !low.contains("`verdict`"));

    let medium = render(&parse(""), &surveyed);
    assert!(
        medium.contains("Run 6 finder subagents") && medium.contains("into 100 areas"),
        "{medium}"
    );
    assert!(medium.contains("precision matters more") && medium.contains("at most 15 findings"));
    assert!(!medium.contains("Sweep") && !medium.contains("Architecture"));

    let high = render(&parse("high"), &surveyed);
    assert!(
        high.contains("Run 10 finder subagents") && high.contains("at most 25 findings"),
        "{high}"
    );

    let max = render(&parse("max --comment"), &surveyed);
    assert!(
        max.contains("Run 16 finder subagents") && max.contains("### 3. Sweep"),
        "{max}"
    );
    assert!(max.contains("Architecture") && max.contains("Tests:") && max.contains("at most 32 findings"));
    assert!(max.contains("`scope` `codebase`") && max.contains("the state of the codebase"));
    assert!(!max.contains("Pull request comments"), "--comment does not apply");
    for text in [&low, &medium, &high, &max] {
        assert_clean(text);
    }
}

#[test]
fn finders_never_outnumber_the_areas_and_split_the_lines_evenly() {
    let small = Surveyed::Files(survey(&[("a/x.rs", 900), ("b/y.rs", 300)]));
    let text = render(&parse("max"), &small);
    assert!(text.contains("Run 2 finder subagents"), "{text}");
    assert!(text.contains("- Finder 1, about 900 lines: `a/` (900)"), "{text}");

    let areas = wide().areas();
    let parts = balance(&areas, 16);
    let loads: Vec<usize> = parts.iter().map(|(lines, _)| *lines).collect();
    assert_eq!(loads.iter().sum::<usize>(), 300_000);
    assert!(loads.iter().all(|lines| (18_000..=21_000).contains(lines)), "{loads:?}");
}

#[test]
fn a_large_folder_splits_into_its_children_and_its_own_files() {
    let split = survey(&[
        ("README.rs", 10),
        ("src/main.rs", 500),
        ("src/engine/a.rs", 3000),
        ("src/engine/b.rs", 900),
        ("src/ui/c.ts", 800),
        ("docs/x.rs", 40),
    ]);
    let labels: Vec<String> = split.areas().iter().map(Area::label).collect();
    assert_eq!(
        labels,
        [
            "`docs/`",
            "`src/engine/`",
            "`src/ui/`",
            "files directly in `src/`",
            "files at the top level",
        ]
    );

    let target = survey(&[("src/engine/a.rs", 3000), ("src/engine/deep/b.rs", 20)]);
    let labels: Vec<String> = target.areas().iter().map(Area::label).collect();
    assert_eq!(
        labels,
        ["`src/engine/deep/`", "files directly in `src/engine/`"],
        "from the files' own folder"
    );
}

#[test]
fn the_scope_names_hot_spots_and_an_empty_or_unlisted_workspace_says_so() {
    let text = render(
        &parse("medium crates"),
        &Surveyed::Files(survey(&[("crates/a.rs", 50), ("crates/b.rs", 10)])),
    );
    assert!(
        text.contains("`crates` as it is now") && text.contains("about 60 lines in 2 files"),
        "{text}"
    );
    assert!(text.contains("- `src/hot.rs` (7)") && text.contains("- `crates/a.rs` (50 lines)"));
    assert!(text.contains("Do not edit") && text.contains("## Afterwards"));
    assert!(
        text.contains("`scope` `codebase`, `path` `crates`"),
        "a rerun keeps the path"
    );
    assert!(render(&parse("--fix"), &Surveyed::Files(wide())).contains("the code clearly intends"));

    let empty = render(&parse("nowhere"), &Surveyed::Empty);
    assert!(empty.contains("`nowhere` holds no source files") && !empty.contains("report_findings` once"));

    let unlisted = render(&parse("high"), &Surveyed::NotRepo);
    assert!(unlisted.contains("not in a git repository") && unlisted.contains("`glob`, leaving out"));
    assert!(
        unlisted.contains("about one per 15000 lines and 5 to 10 in all"),
        "{unlisted}"
    );
    for text in [&text, &empty, &unlisted] {
        assert_clean(text);
    }
}

#[tokio::test]
async fn the_survey_counts_source_files_and_the_ones_changed_most() {
    let root = crate::platform::temp::test_run().join(format!("drift-audit-{}", crate::random_hex(4)));
    std::fs::create_dir_all(root.join("src")).unwrap();
    assert_eq!(run(&root, None).await, Surveyed::NotRepo);

    git(&root, &["init", "-q", "-b", "main"]);
    std::fs::write(root.join("src/app.rs"), "one\ntwo\nthree").unwrap();
    std::fs::write(root.join("src/util.rs"), "a\n").unwrap();
    std::fs::write(root.join("Cargo.lock"), "lock\n").unwrap();
    std::fs::write(root.join("notes.md"), "prose\n").unwrap();
    std::fs::write(root.join("logo.png"), [0u8, 1, 2]).unwrap();
    git(&root, &["add", "."]);
    git(&root, &["commit", "-q", "-m", "first"]);
    std::fs::write(root.join("src/app.rs"), "one\ntwo\nthree\nfour\n").unwrap();
    git(&root, &["commit", "-q", "-am", "second"]);
    std::fs::write(root.join("new.ts"), "x\ny\n").unwrap();

    let Surveyed::Files(found) = run(&root, None).await else {
        panic!("files expected");
    };
    let paths: Vec<&str> = found.files.iter().map(|file| file.path.as_str()).collect();
    assert_eq!(
        paths,
        ["new.ts", "src/app.rs", "src/util.rs"],
        "tracked and untracked, by path"
    );
    assert_eq!(found.lines(), 7);
    assert_eq!(found.churn[0], ("src/app.rs".to_string(), 2));

    let Surveyed::Files(target) = run(&root, Some("src/util.rs")).await else {
        panic!("one file expected");
    };
    assert_eq!(target.files.len(), 1);
    assert_eq!(run(&root, Some("nothing-here")).await, Surveyed::Empty);

    let _ = std::fs::remove_dir_all(&root);
}
