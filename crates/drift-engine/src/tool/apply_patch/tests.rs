use super::super::tests::Sandbox;
use super::*;

#[tokio::test]
async fn adds_updates_moves_and_deletes() {
    let sandbox = Sandbox::new("apply-patch");
    sandbox.file("a.txt", "one\r\ntwo\r\n");
    sandbox.file("gone.txt", "bye\n");
    read_all(&sandbox, &["a.txt", "gone.txt"]);
    let patch = "*** Begin Patch\n*** Add File: dir/new.txt\n+fresh\n*** Update File: a.txt\n*** Move to: b.txt\n-two\n+TWO\n*** Delete File: gone.txt\n*** End Patch\n";
    let out = ApplyPatch.run(&sandbox.ctx, json!({ "patch": patch })).await.unwrap();
    assert_eq!(out.title, "dir/new.txt, a.txt, gone.txt");
    assert_eq!(
        std::fs::read_to_string(sandbox.ctx.workspace.join("dir/new.txt")).unwrap(),
        "fresh\n"
    );
    assert_eq!(
        std::fs::read(sandbox.ctx.workspace.join("b.txt")).unwrap(),
        b"one\r\nTWO\r\n"
    );
    assert!(!sandbox.ctx.workspace.join("a.txt").exists());
    assert!(!sandbox.ctx.workspace.join("gone.txt").exists());
    assert_eq!(
        out.output, "Patched 3 files:\nA dir/new.txt (+1 -0)\nR b.txt (+1 -1)\nD gone.txt (+0 -1)",
        "one line per file, as opencode answers"
    );
    let kinds: Vec<&str> = out
        .metadata
        .file_changes
        .as_ref()
        .unwrap()
        .iter()
        .map(|change| change.kind.as_str())
        .collect();
    assert_eq!(kinds, ["add", "move", "delete"]);
    assert!(
        out.metadata.file_changes.as_ref().unwrap()[1].patch.contains("+TWO"),
        "the diff is in the metadata"
    );
    let titles: Vec<String> = ApplyPatch
        .asks(&sandbox.ctx, &json!({ "patch": patch }))
        .into_iter()
        .map(|ask| ask.title)
        .collect();
    assert_eq!(
        titles,
        ["Patch dir/new.txt", "Patch a.txt", "Patch b.txt", "Patch gone.txt"],
        "each path, the move destination included, asked on its own"
    );
}

#[tokio::test]
async fn a_file_that_is_not_utf8_is_refused_untouched_not_rewritten() {
    let sandbox = Sandbox::new("apply-patch-1252");
    let path = sandbox.ctx.workspace.join("page.asp");
    let bytes = b"<% caf\xe9 %>\r\nline two\r\n".to_vec();
    std::fs::write(&path, &bytes).unwrap();
    read_all(&sandbox, &["page.asp"]);
    let patch = "*** Begin Patch\n*** Update File: page.asp\n-line two\n+line 2\n*** End Patch\n";
    let refused = ApplyPatch
        .run(&sandbox.ctx, json!({ "patch": patch }))
        .await
        .unwrap_err();
    assert!(refused.0.contains("not UTF-8"), "{}", refused.0);
    assert_eq!(std::fs::read(&path).unwrap(), bytes, "every byte as it was");
}

#[test]
fn a_rule_for_the_move_destination_decides_for_it() {
    use crate::permission::{Decision, Permissions, Policy, Rule};
    let sandbox = Sandbox::new("apply-patch-asks");
    let policy = Policy {
        rules: vec![
            Rule {
                kind: "edit".into(),
                pattern: "**/denied.txt".into(),
                decision: Decision::Deny,
            },
            Rule {
                kind: "edit".into(),
                pattern: "**".into(),
                decision: Decision::Allow,
            },
        ],
    };
    let permissions = Permissions::new(Policy::default());
    let patch = "*** Begin Patch\n*** Update File: source.txt\n*** Move to: denied.txt\n-a\n+b\n*** End Patch\n";
    let decisions: Vec<Decision> = ApplyPatch
        .asks(&sandbox.ctx, &json!({ "patch": patch }))
        .iter()
        .map(|ask| permissions.decide_now("s", &policy, ask))
        .collect();
    assert_eq!(
        decisions,
        [Decision::Allow, Decision::Deny],
        "the source is allowed, the destination is not, so the call is refused"
    );
}

fn read_all(sandbox: &Sandbox, names: &[&str]) {
    for name in names {
        sandbox.ctx.files.mark_read(&sandbox.ctx.resolve(name));
    }
}

#[tokio::test]
async fn an_unread_existing_file_is_never_patched() {
    let sandbox = Sandbox::new("apply-patch-unread");
    sandbox.file("kept.txt", "mine\n");
    sandbox.file("source.txt", "s\n");
    read_all(&sandbox, &["source.txt"]);
    for patch in [
        "*** Begin Patch\n*** Add File: kept.txt\n+replaced\n*** End Patch\n",
        "*** Begin Patch\n*** Update File: kept.txt\n-mine\n+theirs\n*** End Patch\n",
        "*** Begin Patch\n*** Delete File: kept.txt\n*** End Patch\n",
        "*** Begin Patch\n*** Update File: source.txt\n*** Move to: kept.txt\n-s\n+t\n*** End Patch\n",
    ] {
        let err = ApplyPatch
            .run(&sandbox.ctx, json!({ "patch": patch }))
            .await
            .unwrap_err();
        assert!(err.0.contains("has not been read"), "{patch}: {}", err.0);
        assert!(
            !err.0.contains("mine"),
            "an unread file's content never reaches the result"
        );
    }
    assert_eq!(
        std::fs::read_to_string(sandbox.ctx.workspace.join("kept.txt")).unwrap(),
        "mine\n"
    );
    assert_eq!(
        std::fs::read_to_string(sandbox.ctx.workspace.join("source.txt")).unwrap(),
        "s\n"
    );
}

#[tokio::test]
async fn a_bad_hunk_late_in_the_patch_leaves_every_file_as_it_was() {
    let sandbox = Sandbox::new("apply-patch-atomic");
    sandbox.file("a.txt", "one\n");
    read_all(&sandbox, &["a.txt"]);
    let patch = "*** Begin Patch\n*** Add File: first.txt\n+new\n*** Update File: a.txt\n-nope\n+x\n*** End Patch\n";
    let err = ApplyPatch
        .run(&sandbox.ctx, json!({ "patch": patch }))
        .await
        .unwrap_err();
    assert!(err.0.starts_with("a.txt: hunk 1"), "{}", err.0);
    assert!(
        !sandbox.ctx.workspace.join("first.txt").exists(),
        "the earlier add did not happen"
    );
}

/// Paths whose next write fails after it has already changed the file.
static FAIL_AFTER_WRITE: std::sync::Mutex<Vec<PathBuf>> = std::sync::Mutex::new(Vec::new());

pub(super) fn injected_failure(path: &Path) -> std::io::Result<()> {
    let mut failing = FAIL_AFTER_WRITE.lock().unwrap();
    match failing.iter().position(|queued| queued == path) {
        Some(index) => {
            failing.remove(index);
            Err(std::io::Error::other("injected failure after the write"))
        }
        None => Ok(()),
    }
}

#[tokio::test]
async fn a_write_that_fails_after_changing_its_file_puts_that_file_back_too() {
    let sandbox = Sandbox::new("apply-patch-rollback");
    sandbox.file("a.txt", "one\n");
    sandbox.file("b.txt", "keep me\n");
    read_all(&sandbox, &["a.txt", "b.txt"]);
    FAIL_AFTER_WRITE.lock().unwrap().push(sandbox.ctx.resolve("b.txt"));
    let patch = "*** Begin Patch\n*** Add File: first.txt\n+new\n*** Update File: a.txt\n-one\n+two\n*** Update File: b.txt\n-keep me\n+changed\n*** End Patch\n";
    let err = ApplyPatch
        .run(&sandbox.ctx, json!({ "patch": patch }))
        .await
        .unwrap_err();
    assert!(
        err.0.contains("injected") && err.0.contains("nothing was changed"),
        "{}",
        err.0
    );
    assert!(!sandbox.ctx.workspace.join("first.txt").exists());
    assert_eq!(
        std::fs::read_to_string(sandbox.ctx.workspace.join("a.txt")).unwrap(),
        "one\n"
    );
    assert_eq!(
        std::fs::read_to_string(sandbox.ctx.workspace.join("b.txt")).unwrap(),
        "keep me\n",
        "the failing step's own file is restored"
    );
    let leftovers: Vec<_> = std::fs::read_dir(&sandbox.ctx.workspace)
        .unwrap()
        .flatten()
        .filter(|entry| entry.file_name().to_string_lossy().ends_with(".tmp"))
        .collect();
    assert!(leftovers.is_empty(), "no staged copies left behind");
}

#[tokio::test]
async fn a_file_that_cannot_be_put_back_is_named() {
    let sandbox = Sandbox::new("apply-patch-unrestorable");
    sandbox.file("a.txt", "one\n");
    read_all(&sandbox, &["a.txt"]);
    let a = sandbox.ctx.resolve("a.txt");
    // The write lands, then fails; the put-back write fails too.
    FAIL_AFTER_WRITE.lock().unwrap().extend([a.clone(), a.clone()]);
    let patch = "*** Begin Patch\n*** Update File: a.txt\n-one\n+two\n*** End Patch\n";
    let err = ApplyPatch
        .run(&sandbox.ctx, json!({ "patch": patch }))
        .await
        .unwrap_err();
    assert!(
        err.0.contains("could not be put back") && err.0.contains("a.txt"),
        "{}",
        err.0
    );
    assert!(!err.0.contains("nothing was changed"));
}

#[tokio::test]
async fn only_a_missing_file_counts_as_absent() {
    let sandbox = Sandbox::new("apply-patch-unreadable");
    sandbox.file("taken/inside.txt", "");
    read_all(&sandbox, &["taken"]);
    let patch = "*** Begin Patch\n*** Add File: taken\n+over a directory\n*** End Patch\n";
    let err = ApplyPatch
        .run(&sandbox.ctx, json!({ "patch": patch }))
        .await
        .unwrap_err();
    assert!(
        err.0.starts_with("taken: could not read it"),
        "a read error stops preparation instead of reading as no file: {}",
        err.0
    );
    assert!(sandbox.ctx.workspace.join("taken/inside.txt").exists());
}

#[tokio::test]
async fn a_failed_hunk_names_the_file_and_changes_nothing() {
    let sandbox = Sandbox::new("apply-patch-miss");
    sandbox.file("a.txt", "one\n");
    read_all(&sandbox, &["a.txt"]);
    let patch = "*** Begin Patch\n*** Update File: a.txt\n-nope\n+x\n*** End Patch\n";
    let err = ApplyPatch
        .run(&sandbox.ctx, json!({ "patch": patch }))
        .await
        .unwrap_err();
    assert!(err.0.starts_with("a.txt: hunk 1"), "{}", err.0);
    assert_eq!(
        std::fs::read_to_string(sandbox.ctx.workspace.join("a.txt")).unwrap(),
        "one\n"
    );
}
