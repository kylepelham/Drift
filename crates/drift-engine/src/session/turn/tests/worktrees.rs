use super::*;
use std::path::Path;

fn git(directory: &Path, args: &[&str]) -> String {
    let output = std::process::Command::new("git")
        .current_dir(directory)
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

/// The harness workspace as a git repository with one commit on `main`.
fn repository(h: &Harness) -> PathBuf {
    let root = h._dir.join("ws");
    git(&root, &["init", "-q", "-b", "main"]);
    for (key, value) in [
        ("user.email", "t@example.com"),
        ("user.name", "T"),
        ("core.autocrlf", "false"),
    ] {
        git(&root, &["config", key, value]);
    }
    std::fs::write(root.join("a.txt"), "one\n").unwrap();
    git(&root, &["add", "."]);
    git(&root, &["commit", "-q", "-m", "first"]);

    root
}

fn workspace(h: &Harness) -> crate::store::Workspace {
    h.engine.store.workspace(&h.session.workspace_id).unwrap().unwrap()
}

fn checkout(h: &Harness, name: &str) -> PathBuf {
    h.engine.worktree_path(&h.session.workspace_id, name)
}

async fn turn(h: &Harness, text: &str) {
    h.engine.submit(&h.session.id, prompt(text)).await.await_ok();
    until_idle(h).await;
}

fn outputs(h: &Harness) -> Vec<String> {
    transcript(h)
        .iter()
        .flat_map(|message| &message.parts)
        .filter_map(|row| match &row.part {
            Part::ToolCall { output, .. } => output.clone(),
            _ => None,
        })
        .collect()
}

#[tokio::test]
async fn a_move_takes_effect_when_the_step_ends() {
    let h = harness().await;
    let root = repository(&h);
    let both = [
        call_block("toolu_move", "session_move", r#"{"directory":"feature"}"#),
        call_block("toolu_same", "write", r#"{"path":"same-step.txt","content":"x"}"#),
        vec![Chunk::Stop(StopReason::ToolUse)],
    ]
    .concat();
    h.provider
        .push(tool_call("worktree_create", r#"{"name":"feature"}"#))
        .push(both)
        .push(tool_call("write", r#"{"path":"next-step.txt","content":"y"}"#))
        .push(text("done"));
    turn(&h, "work in a worktree").await;

    let worktree = checkout(&h, "feature");
    assert_eq!(session(&h).worktree.as_deref(), Some("feature"));
    assert!(
        root.join("same-step.txt").exists(),
        "a call beside the move ran where the session was"
    );
    assert!(
        worktree.join("next-step.txt").exists(),
        "the next step works in the worktree"
    );
    assert!(!root.join("next-step.txt").exists());
    assert_eq!(
        git(
            &root,
            &["branch", "--list", "--format=%(refname:short)", "drift/feature"]
        ),
        "drift/feature"
    );
    let requests = h.provider.requests.lock().unwrap();
    assert!(requests[0].tools.iter().any(|tool| tool.name == "worktree_create"));
    let worktree = crate::tool::canonical(&worktree).to_string_lossy().into_owned();
    let reminded = |request: &Request| {
        request
            .messages
            .iter()
            .flat_map(|message| &message.blocks)
            .any(|block| matches!(block, llm::Block::Reminder(text) if text.contains(&worktree)))
    };
    assert!(
        !reminded(&requests[1]),
        "the step with the move still works where it was"
    );
    assert!(reminded(&requests[2]), "the step after the move hears where it works");
}

#[tokio::test]
async fn list_shows_each_worktree_and_remove_keeps_an_unmerged_branch() {
    let h = harness().await;
    let root = repository(&h);
    h.engine.create_worktree(&workspace(&h), &root, "kept").await.unwrap();
    let worktree = checkout(&h, "kept");
    std::fs::write(worktree.join("b.txt"), "two\n").unwrap();
    git(&worktree, &["add", "."]);
    git(&worktree, &["commit", "-q", "-m", "second"]);

    h.provider
        .push(tool_call("worktree_list", "{}"))
        .push(tool_call("worktree_remove", r#"{"name":"kept"}"#))
        .push(text("removed"));
    turn(&h, "tidy up").await;

    let outputs = outputs(&h);
    assert!(
        outputs[0].contains("- kept: ") && outputs[0].contains("drift/kept"),
        "{}",
        outputs[0]
    );
    assert!(outputs[0].contains("not merged"), "{}", outputs[0]);
    assert!(outputs[1].contains("not merged, so it was kept"), "{}", outputs[1]);
    assert!(!worktree.exists());
    assert_eq!(
        git(&root, &["branch", "--list", "--format=%(refname:short)", "drift/kept"]),
        "drift/kept"
    );
}

#[tokio::test]
async fn uncommitted_changes_refuse_removal_and_force_asks_first() {
    let h = harness().await;
    let root = repository(&h);
    h.engine.create_worktree(&workspace(&h), &root, "dirty").await.unwrap();
    let worktree = checkout(&h, "dirty");
    std::fs::write(worktree.join("scratch.txt"), "unsaved\n").unwrap();

    h.provider
        .push(tool_call("worktree_remove", r#"{"name":"dirty"}"#))
        .push(text("refused"));
    turn(&h, "remove it").await;
    assert!(outputs(&h)[0].contains("uncommitted change"), "{}", outputs(&h)[0]);
    assert!(worktree.exists());

    let mut events = h.engine.hub.attach(None).rx;
    h.provider
        .push(tool_call("worktree_remove", r#"{"name":"dirty","force":true}"#))
        .push(text("gone"));
    h.engine.submit(&h.session.id, prompt("drop it")).await.await_ok();
    let ask = next_ask(&mut events).await;
    assert_eq!(
        (ask.ask.kind.as_str(), ask.ask.pattern.as_str()),
        ("worktree", "remove --force dirty")
    );
    reply_permission(&h, &ask.id, Reply::Once);
    until_idle(&h).await;

    assert!(!worktree.exists());
    assert_eq!(
        git(&root, &["branch", "--list", "--format=%(refname:short)", "drift/dirty"]),
        "",
        "a merged branch goes with it"
    );
}

#[tokio::test]
async fn setup_commands_ask_like_bash_and_run_in_the_new_worktree() {
    let h = harness().await;
    let root = repository(&h);
    let setup = r#"{ "worktree": { "setup": ["echo ready > setup.txt"] } }"#;
    std::fs::write(root.join("drift.json"), setup).unwrap();

    let mut events = h.engine.hub.attach(None).rx;
    h.provider
        .push(tool_call("worktree_create", r#"{"name":"prepared"}"#))
        .push(text("made"));
    h.engine.submit(&h.session.id, prompt("make one")).await.await_ok();
    let ask = next_ask(&mut events).await;
    assert_eq!(
        (ask.ask.kind.as_str(), ask.ask.pattern.as_str()),
        ("bash", "echo ready > setup.txt")
    );
    reply_permission(&h, &ask.id, Reply::Once);
    until_idle(&h).await;

    assert!(checkout(&h, "prepared").join("setup.txt").exists());
    assert!(!root.join("setup.txt").exists());
    let output = &outputs(&h)[0];
    assert!(output.contains("Setup `echo ready > setup.txt`"), "{output}");
    assert!(
        output.contains("were not copied"),
        "the uncommitted drift.json stays behind: {output}"
    );
}

#[tokio::test]
async fn a_prompt_can_start_its_thread_in_a_new_worktree() {
    let h = harness().await;
    repository(&h);
    h.provider.push(text("on it"));
    let start = Prompt {
        worktree: true,
        ..prompt("Fix the parser")
    };
    h.engine.submit(&h.session.id, start).await.await_ok();
    until_idle(&h).await;

    let name = session(&h).worktree.expect("the thread works in a worktree");
    assert!(name.starts_with("fix-the-parser-"), "{name}");
    let directory = crate::tool::canonical(&checkout(&h, &name));
    let requests = h.provider.requests.lock().unwrap();
    assert!(
        requests[0].system.contains(&*directory.to_string_lossy()),
        "the first step already works there"
    );
}

#[tokio::test]
async fn a_purged_thread_takes_its_merged_clean_worktree_with_it() {
    let h = harness().await;
    let root = repository(&h);
    let workspace = workspace(&h);
    h.engine.create_worktree(&workspace, &root, "done").await.unwrap();
    h.engine.create_worktree(&workspace, &root, "open").await.unwrap();
    std::fs::write(checkout(&h, "open").join("wip.txt"), "wip\n").unwrap();
    for name in ["done", "open"] {
        let thread = sibling_session(&h, name);
        h.engine.store.set_session_worktree(&thread.id, Some(name)).unwrap();
        h.engine.store.delete_session(&thread.id).unwrap();
        h.engine.retire_worktree(&workspace.id, name).await;
    }

    assert!(!checkout(&h, "done").exists(), "merged and clean: removed");
    assert!(checkout(&h, "open").exists(), "uncommitted work stays");
}

#[tokio::test]
async fn a_workspace_outside_git_is_not_offered_worktrees() {
    let h = harness().await;
    h.provider.push(text("hi"));
    turn(&h, "hello").await;

    let requests = h.provider.requests.lock().unwrap();
    assert!(
        !requests[0]
            .tools
            .iter()
            .any(|tool| crate::worktree::TOOLS.contains(&tool.name.as_str()))
    );
}

#[tokio::test]
async fn a_spawned_thread_branches_its_own_worktree_from_its_parents() {
    let h = harness().await;
    let root = repository(&h);
    h.provider.push(text("hi"));
    turn(&h, "hello").await;
    h.engine.create_worktree(&workspace(&h), &root, "parent").await.unwrap();
    let parent = checkout(&h, "parent");
    std::fs::write(parent.join("parent.txt"), "from the parent\n").unwrap();
    git(&parent, &["add", "."]);
    git(&parent, &["commit", "-q", "-m", "parent work"]);
    h.engine
        .store
        .set_session_worktree(&h.session.id, Some("parent"))
        .unwrap();

    h.provider.push(text("spawned"));
    let thread = h.engine.spawn(&h.session.id, "Carry on elsewhere").await.unwrap();
    until_session_idle(&h, &thread.id).await;

    let name = h.engine.store.session(&thread.id).unwrap().unwrap().worktree.unwrap();
    assert_ne!(name, "parent");
    assert!(
        checkout(&h, &name).join("parent.txt").exists(),
        "branched from the parent's worktree"
    );
    assert_eq!(session(&h).worktree.as_deref(), Some("parent"));
}
