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
async fn a_file_the_search_cannot_read_is_said_to_be_missing() {
    let sandbox = Sandbox::new("grep-unreadable");
    sandbox.file("a.rs", "needle\n");
    std::fs::write(sandbox.ctx.workspace.join("latin1.txt"), b"needle caf\xe9\n").unwrap();
    let out = Grep.run(&sandbox.ctx, json!({ "pattern": "needle" })).await.unwrap();

    assert!(out.output.starts_with("a.rs:1: needle"), "{}", out.output);
    assert!(
        out.output.contains("1 files or folders could not be read"),
        "{}",
        out.output
    );
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
async fn many_files_list_in_order_and_page_with_offset() {
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
    assert_eq!((all.metadata.count, all.metadata.truncated), (Some(120), Some(false)));

    let page = Grep
        .run(
            &sandbox.ctx,
            json!({ "pattern": "hit", "head_limit": 10, "offset": 10 }),
        )
        .await
        .unwrap();
    let lines: Vec<&str> = page.output.lines().collect();
    assert_eq!(
        (lines[0], lines[9]),
        ("f05.txt:1: hit", "f09.txt:3: hit"),
        "{}",
        page.output
    );
    assert!(
        page.output.contains("11 to 20 of 120; pass `offset: 20`"),
        "{}",
        page.output
    );
}

#[tokio::test]
async fn a_broad_search_stops_once_it_has_enough_to_show() {
    let sandbox = Sandbox::new("grep-enough");
    for i in 0..30 {
        sandbox.file(&format!("f{i:02}.txt"), &"hit\n".repeat(100));
    }
    let out = Grep.run(&sandbox.ctx, json!({ "pattern": "hit" })).await.unwrap();
    assert_eq!(
        (out.metadata.count, out.metadata.capped, out.metadata.truncated),
        (Some(DEFAULT_LIMIT), Some(true), Some(true))
    );
    assert!(out.metadata.total.unwrap() < 3000, "it did not search everything");
    assert!(out.output.contains("more matches exist"), "{}", out.output);

    let counted = Grep
        .run(&sandbox.ctx, json!({ "pattern": "hit", "output_mode": "count" }))
        .await
        .unwrap();
    assert_eq!(counted.metadata.count, Some(30), "a count searches every file");
    assert!(counted.output.starts_with("f00.txt:100\n"), "{}", counted.output);
}

#[tokio::test]
async fn takes_claude_codes_options() {
    let sandbox = Sandbox::new("grep-options");
    sandbox.file("src/a.rs", "one\nTwo\nthree\nfour\nfive\n");
    sandbox.file("src/b.rs", "nothing\n");
    sandbox.file("notes.md", "two\n");
    let run = |input: serde_json::Value| {
        let ctx = &sandbox.ctx;
        async move { Grep.run(ctx, input).await.unwrap().output }
    };

    assert_eq!(
        run(json!({ "pattern": "two", "-i": true, "output_mode": "files_with_matches" })).await,
        "notes.md\nsrc/a.rs"
    );
    assert_eq!(
        run(json!({ "pattern": "Two", "-C": 1 })).await,
        "src/a.rs-1- one\nsrc/a.rs:2: Two\nsrc/a.rs-3- three"
    );
    assert_eq!(
        run(json!({ "pattern": "Two", "-A": 1, "-n": false })).await,
        "src/a.rs: Two\nsrc/a.rs- three"
    );
    assert_eq!(
        run(json!({ "pattern": "one.two", "multiline": true, "-i": true })).await,
        "src/a.rs:1: one\nsrc/a.rs:2: Two"
    );
    assert_eq!(
        run(json!({ "pattern": "two", "-i": true, "type": "rust" })).await,
        "src/a.rs:2: Two"
    );
    let unknown = Grep
        .run(&sandbox.ctx, json!({ "pattern": "x", "type": "klingon" }))
        .await
        .unwrap_err();
    assert!(unknown.0.contains("unknown file type"), "{}", unknown.0);
}

#[tokio::test]
async fn heavy_folders_and_large_files_are_skipped_unless_named_or_asked_for() {
    let sandbox = Sandbox::new("grep-heavy");
    sandbox.file("src/a.rs", "needle\n");
    sandbox.file("node_modules/lib/index.js", "needle\n");
    sandbox.file("ignored/b.rs", "needle\n");
    sandbox.file(".gitignore", "ignored/\n");
    let big = format!("needle\n{}", "x".repeat(LARGE_FILE_BYTES as usize));
    sandbox.file("big.log", &big);

    let out = Grep.run(&sandbox.ctx, json!({ "pattern": "needle" })).await.unwrap();
    assert!(out.output.starts_with("src/a.rs:1: needle\n("), "{}", out.output);
    assert!(out.output.contains("not searched: node_modules"), "{}", out.output);
    assert!(
        out.output.contains("1 files over 10 MB were not searched"),
        "{}",
        out.output
    );

    let named = Grep
        .run(&sandbox.ctx, json!({ "pattern": "needle", "path": "node_modules" }))
        .await
        .unwrap();
    assert_eq!(named.output, "node_modules/lib/index.js:1: needle");
    let big = Grep
        .run(&sandbox.ctx, json!({ "pattern": "needle", "path": "big.log" }))
        .await
        .unwrap();
    assert_eq!(big.output, "big.log:1: needle", "a large file named is searched");

    let everything = Grep
        .run(
            &sandbox.ctx,
            json!({ "pattern": "needle", "include_ignored": true, "output_mode": "files_with_matches" }),
        )
        .await
        .unwrap();
    assert!(
        everything.output.contains("ignored/b.rs") && everything.output.contains("node_modules/lib/index.js"),
        "{}",
        everything.output
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
        ..Default::default()
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
        ..Default::default()
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

#[tokio::test]
async fn a_pattern_naming_a_newline_matches_across_lines_without_multiline() {
    let sandbox = Sandbox::new("grep-newline");
    sandbox.file("a.h", "typedef struct\n{\n  int x;\n} INFO;\nint a; // x.y\n");
    let found = Grep
        .run(
            &sandbox.ctx,
            json!({ "pattern": r"typedef struct[^\n]*\n\{", "output_mode": "count" }),
        )
        .await
        .unwrap();
    assert!(found.output.contains("a.h:1"), "{}", found.output);
    // An escaped backslash before `n` is a backslash and a letter, and `.` still stops at a line's end.
    assert!(!names_newline(r"C:\\new") && names_newline(r"a\nb") && names_newline("a\nb"));
    let one_line = Grep
        .run(
            &sandbox.ctx,
            json!({ "pattern": r"int.*\n?x", "output_mode": "content" }),
        )
        .await
        .unwrap();
    assert!(!one_line.output.contains("INFO"), "{}", one_line.output);
}
