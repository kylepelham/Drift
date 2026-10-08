use super::super::tests::Sandbox;
use super::*;

#[tokio::test]
async fn finds_lines_with_numbers_and_filters_by_include() {
    let sandbox = Sandbox::new("grep");
    sandbox.file("src/a.rs", "fn alpha() {}\nfn beta() {}\n");
    sandbox.file("notes.md", "alpha in prose\n");
    let out = Grep.run(&sandbox.ctx, json!({ "pattern": "alpha" })).await.unwrap();
    let mut lines: Vec<&str> = out.output.lines().collect();
    lines.sort();
    assert_eq!(lines, ["notes.md:1: alpha in prose", "src/a.rs:1: fn alpha() {}"]);
    let only = Grep
        .run(&sandbox.ctx, json!({ "pattern": "alpha", "include": "*.rs" }))
        .await
        .unwrap();
    assert_eq!(only.output, "src/a.rs:1: fn alpha() {}");
    let none = Grep.run(&sandbox.ctx, json!({ "pattern": "gamma" })).await.unwrap();
    assert_eq!(none.output, "No matches");
}

#[tokio::test]
async fn skips_secrets_git_internals_and_binaries() {
    let sandbox = Sandbox::new("grep-skip");
    sandbox.file("src/a.rs", "token = 1\n");
    sandbox.file(".env", "token = hunter2\n");
    sandbox.file(".env.example", "token = changeme\n");
    sandbox.file(".git/config", "token = internal\n");
    std::fs::write(sandbox.ctx.workspace.join("blob.bin"), b"token = 1\0\x01\x02").unwrap();
    let out = Grep.run(&sandbox.ctx, json!({ "pattern": "token" })).await.unwrap();
    let mut lines: Vec<&str> = out.output.lines().filter(|line| !line.starts_with('(')).collect();
    lines.sort();
    assert_eq!(lines, [".env.example:1: token = changeme", "src/a.rs:1: token = 1"]);
    assert!(
        out.output.contains("1 files that may hold secrets were not searched"),
        "{}",
        out.output
    );
    assert!(
        !out.output.contains("hunter2") && !out.output.contains("internal") && !out.output.contains("blob.bin"),
        "{}",
        out.output
    );
    assert_eq!(out.metadata.withheld, Some(1));

    let named = json!({ "pattern": "token", "path": ".env" });
    assert!(
        Grep.ask(&sandbox.ctx, &named)
            .is_some_and(|ask| ask.title.contains("may hold secrets"))
    );
    let direct = Grep.run(&sandbox.ctx, named).await.unwrap();
    assert_eq!(
        direct.output, ".env:1: token = hunter2",
        "a secret named directly is searched once approved"
    );
}

#[tokio::test]
async fn many_files_searched_together_list_in_order_and_stop_past_the_limit() {
    let sandbox = Sandbox::new("grep-many");
    for i in 0..60 {
        sandbox.file(&format!("f{i:02}.txt"), "hit\nmiss\nhit\n");
    }
    let few = Grep
        .run(&sandbox.ctx, json!({ "pattern": "hit", "include": "f0*.txt" }))
        .await
        .unwrap();
    let lines: Vec<&str> = few.output.lines().collect();
    assert_eq!(lines.len(), 20);
    assert!(
        lines.windows(2).all(|pair| pair[0] < pair[1]) && lines[0] == "f00.txt:1: hit",
        "by file then line: {lines:?}"
    );
    let all = Grep.run(&sandbox.ctx, json!({ "pattern": "hit" })).await.unwrap();
    assert_eq!(
        (all.metadata.count, all.metadata.truncated),
        (Some(120), Some(false)),
        "120 matches fit"
    );
    for i in 60..110 {
        sandbox.file(&format!("g{i}.txt"), "hit\nhit\n");
    }
    let past = Grep.run(&sandbox.ctx, json!({ "pattern": "hit" })).await.unwrap();
    assert_eq!(
        (past.metadata.count, past.metadata.total, past.metadata.truncated),
        (Some(MAX_MATCHES), Some(220), Some(true))
    );
    let lines: Vec<&str> = past.output.lines().collect();
    assert_eq!(
        (lines[0], lines[MAX_MATCHES - 1]),
        ("f00.txt:1: hit", "g89.txt:2: hit"),
        "the first by file and line, not the first found"
    );
    assert!(
        past.output
            .contains("220 matches; these are the first 200 by file and line"),
        "{}",
        past.output
    );
}

#[tokio::test]
async fn a_broad_search_stops_past_the_count_cap() {
    let sandbox = Sandbox::new("grep-cap");
    for i in 0..30 {
        sandbox.file(&format!("f{i:02}.txt"), &"hit\n".repeat(100));
    }
    let out = Grep.run(&sandbox.ctx, json!({ "pattern": "hit" })).await.unwrap();
    assert_eq!(
        (out.metadata.capped, out.metadata.total, out.metadata.count),
        (Some(true), Some(MAX_COUNTED), Some(MAX_MATCHES))
    );
    assert!(
        out.output.contains("more than 2000 matches, so the search stopped"),
        "{}",
        out.output
    );
}

#[tokio::test]
async fn a_stop_ends_the_search() {
    let sandbox = Sandbox::new("grep-stop");
    sandbox.file("a.txt", "hit\n");
    sandbox.ctx.abort.cancel();
    let err = Grep.run(&sandbox.ctx, json!({ "pattern": "hit" })).await.unwrap_err();
    assert_eq!(err.0, "stopped");
}

#[tokio::test]
async fn a_directory_search_does_not_bypass_file_read_rules() {
    let sandbox = Sandbox::new("grep-policy");
    sandbox.file("allowed.txt", "hit public");
    sandbox.file("blocked.txt", "hit restricted");
    sandbox.ctx.engine.permissions.set_policy(crate::permission::Policy {
        rules: vec![crate::permission::Rule {
            kind: "read".into(),
            pattern: "blocked.txt".into(),
            decision: crate::permission::Decision::Deny,
        }],
    });
    let out = Grep.run(&sandbox.ctx, json!({ "pattern": "hit" })).await.unwrap();
    assert!(out.output.contains("hit public") && !out.output.contains("hit restricted"));
    assert_eq!(out.metadata.restricted, Some(1));
}

#[tokio::test]
async fn an_approved_search_outside_the_workspace_covers_its_files_unless_a_rule_says_otherwise() {
    let sandbox = Sandbox::new("grep-outside");
    let outside = sandbox.ctx.workspace.parent().unwrap().join("outside");
    std::fs::create_dir_all(outside.join("deep")).unwrap();
    std::fs::write(outside.join("deep/a.txt"), "hit outside").unwrap();
    std::fs::write(outside.join("held.txt"), "hit held").unwrap();
    let input = json!({ "pattern": "hit", "path": outside.to_string_lossy() });
    assert!(
        Grep.ask(&sandbox.ctx, &input).is_some_and(|ask| !ask.default_allow),
        "searching outside asks first"
    );
    let out = Grep.run(&sandbox.ctx, input.clone()).await.unwrap();
    assert!(
        out.output.contains("hit outside") && out.output.contains("hit held"),
        "{}",
        out.output
    );
    sandbox.ctx.engine.permissions.set_policy(crate::permission::Policy {
        rules: vec![crate::permission::Rule {
            kind: "read".into(),
            pattern: "*held.txt".into(),
            decision: crate::permission::Decision::Ask,
        }],
    });
    let ruled = Grep.run(&sandbox.ctx, input).await.unwrap();
    assert!(
        ruled.output.contains("hit outside") && !ruled.output.contains("hit held"),
        "{}",
        ruled.output
    );
    assert_eq!(ruled.metadata.restricted, Some(1));
}

#[tokio::test]
async fn the_scratch_directory_is_searched_without_asking() {
    let sandbox = Sandbox::new("grep-scratch");
    let scratch = super::super::scratch_dir().join(format!("grep-{}", crate::random_hex(4)));
    std::fs::create_dir_all(&scratch).unwrap();
    std::fs::write(scratch.join("notes.txt"), "hit scratch").unwrap();
    let input = json!({ "pattern": "hit", "path": scratch.to_string_lossy() });
    assert!(Grep.ask(&sandbox.ctx, &input).is_some_and(|ask| ask.default_allow));
    let out = Grep.run(&sandbox.ctx, input).await.unwrap();
    let _ = std::fs::remove_dir_all(&scratch);
    assert!(out.output.contains("hit scratch"), "{}", out.output);
}

#[tokio::test]
async fn bad_regex_is_reported() {
    let sandbox = Sandbox::new("grep-bad");
    let err = Grep.run(&sandbox.ctx, json!({ "pattern": "(" })).await.unwrap_err();
    assert!(err.0.starts_with("invalid regex"));
}
