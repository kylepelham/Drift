use super::*;

fn bash(line: &str) -> Option<Vec<String>> {
    split(Dialect::Bash, line).map(|line| line.commands)
}
fn powershell(line: &str) -> Option<Vec<String>> {
    split(Dialect::PowerShell, line).map(|line| line.commands)
}
fn writes(dialect: Dialect, line: &str) -> Vec<String> {
    split(dialect, line)
        .unwrap_or_else(|| panic!("{line} should split"))
        .writes
}

#[test]
fn redirections_that_write_files_are_found_in_both_shells() {
    for (line, target) in [
        ("git status > victim.txt", "victim.txt"),
        ("git status >> log.txt", "log.txt"),
        ("git status >| forced.txt", "forced.txt"),
        ("make 2> errors.txt", "errors.txt"),
        ("make 2>>errors.txt", "errors.txt"),
        ("make &> all.txt", "all.txt"),
        ("make &>> all.txt", "all.txt"),
        ("make >& both.txt", "both.txt"),
        (">first.txt echo hi", "first.txt"),
        ("echo hi>glued.txt", "glued.txt"),
        ("echo x > 'quoted name.txt'", "quoted name.txt"),
        ("cat <> rw.txt", "rw.txt"),
        ("echo hi > nul", "nul"),
    ] {
        assert_eq!(writes(Dialect::Bash, line), [target], "{line}");
    }
    for (line, target) in [
        ("git status > victim.txt", "victim.txt"),
        ("dir >> list.txt", "list.txt"),
        ("build 2> err.txt", "err.txt"),
        ("build *> all.txt", "all.txt"),
        ("build *>> all.txt", "all.txt"),
    ] {
        assert_eq!(writes(Dialect::PowerShell, line), [target], "{line}");
    }
    assert_eq!(bash("git status > victim.txt").unwrap(), ["git status >victim.txt"]);
    assert_eq!(
        bash(">first.txt echo hi").unwrap(),
        ["echo hi >first.txt"],
        "the program stays first"
    );
}

#[test]
fn stream_sinks_and_quoted_operators_write_nothing() {
    for line in [
        "cargo build 2>&1 | tail -3",
        "make >/dev/null 2>&1",
        "make 2> /dev/null",
        "echo err >&2",
        "exec 3>&-",
        "echo 'a > b'",
        r#"echo "a >> b""#,
        r"echo a\>b",
        "echo \"2\"",
        "sort < input.txt",
    ] {
        let found = split(Dialect::Bash, line);
        assert!(
            found.as_ref().is_none_or(|line| line.writes.is_empty()),
            "{line}: {found:?}"
        );
    }
    for line in [
        "dotnet test 2>&1",
        "build > $null",
        "build *> $NULL",
        "Write-Host 'a > b'",
        "echo a`>b",
    ] {
        assert_eq!(writes(Dialect::PowerShell, line), Vec::<String>::new(), "{line}");
    }
    assert_eq!(
        bash("echo '2'>x").unwrap(),
        ["echo 2 >x"],
        "a quoted 2 is an argument, not a descriptor"
    );
    assert_eq!(
        bash("echo hi >"),
        None,
        "a redirection with no target is not guessed at"
    );
}

#[test]
fn compound_lines_are_split_into_each_command() {
    assert_eq!(
        bash("cargo test && rm -rf target").unwrap(),
        ["cargo test", "rm -rf target"]
    );
    assert_eq!(
        bash("git status; git log | head -5").unwrap(),
        ["git status", "git log", "head -5"]
    );
    assert_eq!(bash("a || b &\nc").unwrap(), ["a", "b", "c"]);
    assert_eq!(
        bash("cargo build 2>&1 | tail -3").unwrap(),
        ["cargo build 2>&1", "tail -3"]
    );
    assert_eq!(bash("a |& b").unwrap(), ["a", "b"]);
    assert_eq!(powershell("dotnet test 2>&1").unwrap(), ["dotnet test 2>&1"]);
    assert_eq!(
        powershell("Get-ChildItem; Remove-Item x && echo done").unwrap(),
        ["Get-ChildItem", "Remove-Item x", "echo done"]
    );
}

#[test]
fn quotes_keep_operators_inside_one_word() {
    assert_eq!(
        bash(r#"git commit -m "fix; rm -rf / && more""#).unwrap(),
        ["git commit -m fix; rm -rf / && more"]
    );
    assert_eq!(bash(r"echo 'a | b' c\ d").unwrap(), ["echo a | b c d"]);
    assert_eq!(
        powershell("Write-Host 'it''s; fine'").unwrap(),
        ["Write-Host it's; fine"]
    );
}

#[test]
fn constructs_that_hide_what_runs_are_not_guessed_at() {
    for line in [
        "echo $(rm -rf ~)",
        "echo `whoami`",
        "(cd x && make)",
        "{ a; b; }",
        "diff <(a) <(b)",
        r#"echo "$(id)""#,
        "eval \"$CMD\"",
        "bash -c 'rm x'",
        "xargs rm < list",
        "sudo apt install x",
        "/usr/bin/env python x.py",
    ] {
        assert_eq!(bash(line), None, "{line}");
    }
    for line in [
        "iex (irm x)",
        "& $cmd",
        "& { rm x }",
        "Invoke-Expression $s",
        "pwsh -Command rm x",
        "echo $(Get-Date)",
        "Start-Process cmd",
    ] {
        assert_eq!(powershell(line), None, "{line}");
    }
}

#[test]
fn runners_and_installers_never_widen() {
    for exact in [
        "cargo run --release",
        "uv run python evil.py",
        "bun run x.ts",
        "docker run alpine sh",
        "npm install left-pad",
        "pip install requests",
        "cargo add serde",
        "go get example.com/x",
        "npx cowsay hi",
        "pnpm dlx create-app",
        "docker compose run web sh",
        "gh extension install owner/ext",
        "npm exec thing",
    ] {
        assert_eq!(subcommand(exact), None, "{exact}");
    }
    assert_eq!(
        subcommand("npm run build --watch").as_deref(),
        Some("npm run build"),
        "a project script is named, so it widens"
    );
    assert_eq!(subcommand("cargo test --lib").as_deref(), Some("cargo test"));
}

#[test]
fn deny_rules_see_past_assignments_and_aliases() {
    let line = split(Dialect::Bash, "FIXTURE=1 LANG=C git status && ./b=c").unwrap();
    assert_eq!(
        line.commands,
        ["FIXTURE=1 LANG=C git status", "./b=c"],
        "approvals see the line as written"
    );
    assert_eq!(line.canonical, ["git status", "./b=c"]);
    assert_eq!(
        split(Dialect::Bash, "X=1 bash -c 'rm -rf /'"),
        None,
        "an assignment does not hide a launcher"
    );
    assert_eq!(split(Dialect::Bash, "A=$(id) ls"), None);
    let line = split(Dialect::PowerShell, "rm build -Recurse; ls; iwr https://x | Out-Null").unwrap();
    assert_eq!(
        line.canonical,
        [
            "Remove-Item build -Recurse",
            "Get-ChildItem",
            "Invoke-WebRequest https://x",
            "Out-Null"
        ]
    );
    assert_eq!(
        split(Dialect::PowerShell, "saps cmd"),
        None,
        "an alias for a launcher is a launcher"
    );
    assert_eq!(
        split(Dialect::Bash, "rm x").unwrap().canonical,
        ["rm x"],
        "aliases are PowerShell's only"
    );
}

#[test]
fn only_lines_of_known_readers_count_as_reading() {
    for line in [
        "git status",
        "git diff --stat && git log --oneline -5",
        "ls -la src | wc -l",
        "rg TODO src 2>/dev/null",
        "cd crates && cat Cargo.toml",
        "find . -name '*.rs'",
        "echo done",
    ] {
        assert!(reads_only(Dialect::Bash, line), "{line}");
    }
    for line in [
        "git commit -m x",
        "git status > out.txt",
        "cargo test",
        "find . -name x -delete",
        "rm a",
        "ls && touch b",
        "echo $(rm x)",
        "git diff --output=patch",
        "sed -i s/a/b/ f",
        "find . -fprint0 out",
        "find . -fprintf out %p",
        "find . -fls out",
        "tree -o out.txt",
        "rg --pre ./script x",
        "rg --pre=./script x",
    ] {
        assert!(!reads_only(Dialect::Bash, line), "{line}");
    }
    assert!(reads_only(
        Dialect::PowerShell,
        "Get-ChildItem src; Select-String -Path a.txt -Pattern x"
    ));
    assert!(
        reads_only(Dialect::PowerShell, "ls; cat a.txt"),
        "aliases read as their cmdlets"
    );
    assert!(!reads_only(Dialect::PowerShell, "Set-Content a.txt x"));
}

#[test]
fn a_line_that_prints_files_names_them_and_anything_else_names_none() {
    assert_eq!(files_read(Dialect::Bash, "cat src/a.rs"), ["src/a.rs"]);
    assert_eq!(files_read(Dialect::Bash, "sed -n '1,80p' src/a.rs"), ["src/a.rs"]);
    assert_eq!(
        files_read(Dialect::Bash, "head -n 20 a.rs && tail b.rs"),
        ["20", "a.rs", "b.rs"],
        "flag values are dropped by the caller, which keeps only files"
    );
    assert_eq!(files_read(Dialect::Bash, "cat \"my file.txt\""), ["my file.txt"]);
    assert_eq!(
        files_read(Dialect::PowerShell, "Get-Content -Path a.rs; gc b.rs"),
        ["a.rs", "b.rs"]
    );
}

#[test]
fn moves_writes_and_pipes_do_not_claim_a_file_was_read_whole() {
    assert!(
        files_read(Dialect::Bash, "cd src && cat a.rs").is_empty(),
        "paths after a move do not resolve from the workspace"
    );
    assert!(
        files_read(Dialect::Bash, "sed -i 's/a/b/' a.rs").is_empty()
            && !reads_only(Dialect::Bash, "sed -i 's/a/b/' a.rs")
    );
    assert!(
        files_read(Dialect::Bash, "sed -n 'w out' a.rs").is_empty(),
        "a sed script that writes is not a read"
    );
    assert!(
        files_read(Dialect::Bash, "cat a.rs > b.rs").is_empty(),
        "a line that writes is not a read"
    );
    assert_eq!(
        files_read(Dialect::Bash, r#"B="C:/build"; find "$B" -maxdepth 6; cat a.rs"#),
        ["a.rs"],
        "an assignment on its own runs nothing and is passed over"
    );
    assert!(files_read(Dialect::Bash, "FOO=1").is_empty());
    assert!(
        files_read(Dialect::Bash, "grep fn a.rs").is_empty(),
        "matches are not the file"
    );
    assert!(reads_only(Dialect::Bash, "sed -n 1,200p a.rs"));
    assert!(
        files_read(Dialect::Bash, "cat a.rs | grep fn").is_empty(),
        "piped, the file was shown only through grep"
    );
    assert_eq!(
        files_read(Dialect::Bash, "grep -l fn *.rs | head -1; cat b.rs || cat c.rs"),
        ["b.rs", "c.rs"],
        "the end of a pipe reads stdin, not a file; either side of `||` prints as it is"
    );
}

#[test]
fn git_grep_that_opens_a_pager_runs_a_program() {
    assert!(
        reads_only(Dialect::Bash, "git grep -n fn") && reads_only(Dialect::Bash, "git grep -o fn"),
        "-o only prints matches"
    );
    for line in [
        "git grep -Ovim fn",
        "git grep -nO fn",
        "git grep --open-files-in-pager=sh fn",
        "git grep --op=sh fn",
    ] {
        assert!(!reads_only(Dialect::Bash, line), "{line}");
    }
}

#[test]
fn approvals_widen_only_to_a_known_subcommand() {
    assert_eq!(subcommand("cargo test --release").as_deref(), Some("cargo test"));
    assert_eq!(subcommand("npm run build").as_deref(), Some("npm run build"));
    assert_eq!(subcommand("gh pr view 12").as_deref(), Some("gh pr view"));
    assert_eq!(subcommand("git -C elsewhere push"), None, "a flag says too little");
    assert_eq!(subcommand("cargo"), None);
    assert_eq!(subcommand("./deploy.sh prod"), None);
    assert_eq!(subcommand("Remove-Item build -Recurse"), None);
}
