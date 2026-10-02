use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use super::edit::{diff, LineEnding};
use super::patch::{self, Op};
use super::{display, required_str, stage, Ask, Context, Output, RunFuture, Tool, ToolError};
use crate::llm::ToolSpec;
use crate::store::Store;

pub struct ApplyPatch;

impl Tool for ApplyPatch {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "apply_patch".into(),
            description: include_str!("prompts/apply_patch.txt").trim().into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "patch": { "type": "string", "description": "The full patch, from *** Begin Patch to *** End Patch." }
                },
                "required": ["patch"]
            }),
        }
    }

    /// The first of the patch's asks; `asks` has them all.
    fn ask(&self, ctx: &Context, input: &Value) -> Option<Ask> {
        self.asks(ctx, input).into_iter().next()
    }

    /// Every file the patch writes, removes or moves onto is judged on its own, so a rule for one
    /// path never covers another and a move destination is asked about like any write.
    fn asks(&self, ctx: &Context, input: &Value) -> Vec<Ask> {
        let mut unique: Vec<PathBuf> = Vec::new();
        for path in self.touches(ctx, input).unwrap_or_default() {
            if !unique.contains(&path) {
                unique.push(path);
            }
        }
        unique.into_iter().filter_map(|path| ctx.ask_to_write(&path, "Patch")).collect()
    }

    fn mutates(&self) -> bool {
        true
    }

    /// Every file the patch names, including where a moved file lands.
    fn touches(&self, ctx: &Context, input: &Value) -> Option<Vec<std::path::PathBuf>> {
        let ops = patch::parse(input["patch"].as_str()?).ok()?;
        let mut paths = Vec::new();
        for op in &ops {
            paths.push(ctx.resolve(op.path()));
            if let Op::Update { move_to: Some(to), .. } = op {
                paths.push(ctx.resolve(to));
            }
        }
        Some(paths)
    }

    /// The whole patch is checked before any file changes; a failed write puts back what went before.
    fn run<'a>(&'a self, ctx: &'a Context, input: Value) -> RunFuture<'a> {
        Box::pin(async move {
            let ops = patch::parse(required_str(&input, "patch")?)?;
            let mut plan = Plan::default();
            for op in &ops {
                let path = ctx.resolve(op.path());
                let name = display(&path, &ctx.workspace);
                plan.prepare(ctx, op, &path).await.map_err(|e| ToolError(format!("{name}: {}", e.0)))?;
                plan.touched.push(name);
            }
            for step in &plan.steps {
                super::fits_history(&display(&step.path, &ctx.workspace), step.after.as_ref().map_or(0, Vec::len))?;
            }
            plan.apply(&ctx.engine.store).await?;
            for step in plan.steps.iter().filter(|s| s.after.is_some()) {
                ctx.files.mark_read(&step.path);
            }
            let files: Vec<String> = plan.steps.iter().filter(|s| s.after.is_some()).map(|s| s.path.to_string_lossy().into_owned()).collect();
            Ok(Output { title: plan.touched.join(", "), output: plan.diffs.join("\n"), metadata: json!({ "files": files }) })
        })
    }
}

/// One file change the patch will make: its bytes before (`None`: no file) and after (`None`: removed).
struct Step {
    path: PathBuf,
    before: Option<Vec<u8>>,
    after: Option<Vec<u8>>,
}

#[derive(Default)]
struct Plan {
    steps: Vec<Step>,
    diffs: Vec<String>,
    touched: Vec<String>,
}

impl Plan {
    /// Reads and checks one operation and records what it will do. Nothing is written here.
    async fn prepare(&mut self, ctx: &Context, op: &Op, path: &Path) -> Result<(), ToolError> {
        if self.steps.iter().any(|s| s.path == path) {
            return Err(ToolError("appears twice in the patch".into()));
        }
        match op {
            Op::Add { content, .. } => {
                let existing = existing(ctx, path).await?;
                let before_text = existing.as_ref().map(|b| String::from_utf8_lossy(b).into_owned()).unwrap_or_default();
                self.diffs.push(diff(&display(path, &ctx.workspace), &before_text, content));
                self.steps.push(Step { path: path.to_path_buf(), before: existing, after: Some(content.clone().into_bytes()) });
            }
            Op::Delete { .. } => {
                let before = existing(ctx, path).await?.ok_or(ToolError("does not exist".into()))?;
                self.diffs.push(diff(&display(path, &ctx.workspace), &String::from_utf8_lossy(&before), ""));
                self.steps.push(Step { path: path.to_path_buf(), before: Some(before), after: None });
            }
            Op::Update { move_to, chunks, .. } => {
                let raw = existing(ctx, path).await?.ok_or(ToolError("does not exist".into()))?;
                // Decoding loosely and writing back would turn every byte that is not UTF-8 into U+FFFD, the whole file over.
                let text = String::from_utf8(raw.clone()).map_err(|_| ToolError("is not UTF-8 text (Windows-1252, for example), so patching it would corrupt it; it was not changed".into()))?;
                let ending = LineEnding::detect(&text);
                let before = ending.normalise(&text);
                let after = patch::apply_chunks(&before, chunks)?;
                let target: PathBuf = move_to.as_ref().map_or(path.to_path_buf(), |to| ctx.resolve(to));
                self.diffs.push(diff(&display(&target, &ctx.workspace), &before, &after));
                let written = ending.apply(&after).into_bytes();
                if target == path {
                    self.steps.push(Step { path: target, before: Some(raw), after: Some(written) });
                    return Ok(());
                }
                if self.steps.iter().any(|s| s.path == target) {
                    return Err(ToolError(format!("moves onto {}, which the patch also changes", display(&target, &ctx.workspace))));
                }
                let displaced = existing(ctx, &target).await?;
                self.steps.push(Step { path: target, before: displaced, after: Some(written) });
                self.steps.push(Step { path: path.to_path_buf(), before: Some(raw), after: None });
            }
        }
        Ok(())
    }

    /// Makes every change; on a failure, puts back every step through the failing one and names any file it could not.
    async fn apply(&self, store: &Store) -> Result<(), ToolError> {
        for (index, step) in self.steps.iter().enumerate() {
            if let Err(error) = set(store, &step.path, step.after.as_deref()).await {
                let unrestored = self.undo(store, index + 1).await;
                let state = if unrestored.is_empty() { "nothing was changed".to_string() } else { format!("these could not be put back: {}", unrestored.join("; ")) };
                return Err(ToolError(format!("could not write {}: {error}; {state}", step.path.display())));
            }
        }
        Ok(())
    }

    /// Puts back the first `count` steps, newest first; returns each file it could not, with why.
    async fn undo(&self, store: &Store, count: usize) -> Vec<String> {
        let mut unrestored = Vec::new();
        for step in self.steps[..count].iter().rev() {
            if let Err(error) = restore(store, &step.path, step.before.as_deref()).await {
                unrestored.push(format!("{} ({error})", step.path.display()));
            }
        }
        unrestored
    }
}

/// A file's bytes if it exists and was read this session; only a missing file is absent, any other read error stops the patch.
async fn existing(ctx: &Context, path: &Path) -> Result<Option<Vec<u8>>, ToolError> {
    let bytes = match tokio::fs::read(path).await {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(ToolError(format!("could not read it: {error}"))),
    };
    if !ctx.files.was_read(path) {
        return Err(ToolError("exists and has not been read this session; read it before patching it".into()));
    }
    Ok(Some(bytes))
}

/// Puts a file back as it was; a directory where no file was is not this patch's, so it is left alone.
async fn restore(store: &Store, path: &Path, before: Option<&[u8]>) -> std::io::Result<()> {
    match before {
        Some(bytes) if tokio::fs::read(path).await.is_ok_and(|now| now == bytes) => Ok(()),
        Some(bytes) => set(store, path, Some(bytes)).await,
        None => match tokio::fs::symlink_metadata(path).await {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Ok(meta) if meta.is_dir() => Ok(()),
            _ => set(store, path, None).await,
        },
    }
}

async fn set(store: &Store, path: &Path, content: Option<&[u8]>) -> std::io::Result<()> {
    match content {
        Some(bytes) => stage::replace(store, path, bytes).await?,
        None => match tokio::fs::remove_file(path).await {
            Err(error) if error.kind() != std::io::ErrorKind::NotFound => return Err(error),
            _ => {}
        },
    }
    #[cfg(test)]
    tests::injected_failure(path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
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
        assert_eq!(std::fs::read_to_string(sandbox.ctx.workspace.join("dir/new.txt")).unwrap(), "fresh\n");
        assert_eq!(std::fs::read(sandbox.ctx.workspace.join("b.txt")).unwrap(), b"one\r\nTWO\r\n");
        assert!(!sandbox.ctx.workspace.join("a.txt").exists());
        assert!(!sandbox.ctx.workspace.join("gone.txt").exists());
        assert!(out.output.contains("+TWO"));
        let titles: Vec<String> = ApplyPatch.asks(&sandbox.ctx, &json!({ "patch": patch })).into_iter().map(|a| a.title).collect();
        assert_eq!(titles, ["Patch dir/new.txt", "Patch a.txt", "Patch b.txt", "Patch gone.txt"], "each path, the move destination included, asked on its own");
    }

    #[tokio::test]
    async fn a_file_that_is_not_utf8_is_refused_untouched_not_rewritten() {
        let sandbox = Sandbox::new("apply-patch-1252");
        let path = sandbox.ctx.workspace.join("page.asp");
        let bytes = b"<% caf\xe9 %>\r\nline two\r\n".to_vec();
        std::fs::write(&path, &bytes).unwrap();
        read_all(&sandbox, &["page.asp"]);
        let patch = "*** Begin Patch\n*** Update File: page.asp\n-line two\n+line 2\n*** End Patch\n";
        let refused = ApplyPatch.run(&sandbox.ctx, json!({ "patch": patch })).await.unwrap_err();
        assert!(refused.0.contains("not UTF-8"), "{}", refused.0);
        assert_eq!(std::fs::read(&path).unwrap(), bytes, "every byte as it was");
    }

    #[test]
    fn a_rule_for_the_move_destination_decides_for_it() {
        use crate::permission::{Decision, Permissions, Policy, Rule};
        let sandbox = Sandbox::new("apply-patch-asks");
        let policy = Policy {
            rules: vec![
                Rule { kind: "edit".into(), pattern: "**/denied.txt".into(), decision: Decision::Deny },
                Rule { kind: "edit".into(), pattern: "**".into(), decision: Decision::Allow },
            ],
        };
        let permissions = Permissions::new(Policy::default());
        let patch = "*** Begin Patch\n*** Update File: source.txt\n*** Move to: denied.txt\n-a\n+b\n*** End Patch\n";
        let decisions: Vec<Decision> = ApplyPatch.asks(&sandbox.ctx, &json!({ "patch": patch })).iter().map(|ask| permissions.decide_now("s", &policy, ask)).collect();
        assert_eq!(decisions, [Decision::Allow, Decision::Deny], "the source is allowed, the destination is not, so the call is refused");
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
            let err = ApplyPatch.run(&sandbox.ctx, json!({ "patch": patch })).await.unwrap_err();
            assert!(err.0.contains("has not been read"), "{patch}: {}", err.0);
            assert!(!err.0.contains("mine"), "an unread file's content never reaches the result");
        }
        assert_eq!(std::fs::read_to_string(sandbox.ctx.workspace.join("kept.txt")).unwrap(), "mine\n");
        assert_eq!(std::fs::read_to_string(sandbox.ctx.workspace.join("source.txt")).unwrap(), "s\n");
    }

    #[tokio::test]
    async fn a_bad_hunk_late_in_the_patch_leaves_every_file_as_it_was() {
        let sandbox = Sandbox::new("apply-patch-atomic");
        sandbox.file("a.txt", "one\n");
        read_all(&sandbox, &["a.txt"]);
        let patch = "*** Begin Patch\n*** Add File: first.txt\n+new\n*** Update File: a.txt\n-nope\n+x\n*** End Patch\n";
        let err = ApplyPatch.run(&sandbox.ctx, json!({ "patch": patch })).await.unwrap_err();
        assert!(err.0.starts_with("a.txt: hunk 1"), "{}", err.0);
        assert!(!sandbox.ctx.workspace.join("first.txt").exists(), "the earlier add did not happen");
    }

    /// Paths whose next write fails after it has already changed the file.
    static FAIL_AFTER_WRITE: std::sync::Mutex<Vec<PathBuf>> = std::sync::Mutex::new(Vec::new());

    pub(super) fn injected_failure(path: &Path) -> std::io::Result<()> {
        let mut failing = FAIL_AFTER_WRITE.lock().unwrap();
        match failing.iter().position(|p| p == path) {
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
        let err = ApplyPatch.run(&sandbox.ctx, json!({ "patch": patch })).await.unwrap_err();
        assert!(err.0.contains("injected") && err.0.contains("nothing was changed"), "{}", err.0);
        assert!(!sandbox.ctx.workspace.join("first.txt").exists());
        assert_eq!(std::fs::read_to_string(sandbox.ctx.workspace.join("a.txt")).unwrap(), "one\n");
        assert_eq!(std::fs::read_to_string(sandbox.ctx.workspace.join("b.txt")).unwrap(), "keep me\n", "the failing step's own file is restored");
        let leftovers: Vec<_> = std::fs::read_dir(&sandbox.ctx.workspace).unwrap().flatten().filter(|e| e.file_name().to_string_lossy().ends_with(".tmp")).collect();
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
        let err = ApplyPatch.run(&sandbox.ctx, json!({ "patch": patch })).await.unwrap_err();
        assert!(err.0.contains("could not be put back") && err.0.contains("a.txt"), "{}", err.0);
        assert!(!err.0.contains("nothing was changed"));
    }

    #[tokio::test]
    async fn only_a_missing_file_counts_as_absent() {
        let sandbox = Sandbox::new("apply-patch-unreadable");
        sandbox.file("taken/inside.txt", "");
        read_all(&sandbox, &["taken"]);
        let patch = "*** Begin Patch\n*** Add File: taken\n+over a directory\n*** End Patch\n";
        let err = ApplyPatch.run(&sandbox.ctx, json!({ "patch": patch })).await.unwrap_err();
        assert!(err.0.starts_with("taken: could not read it"), "a read error stops preparation instead of reading as no file: {}", err.0);
        assert!(sandbox.ctx.workspace.join("taken/inside.txt").exists());
    }

    #[tokio::test]
    async fn a_failed_hunk_names_the_file_and_changes_nothing() {
        let sandbox = Sandbox::new("apply-patch-miss");
        sandbox.file("a.txt", "one\n");
        read_all(&sandbox, &["a.txt"]);
        let patch = "*** Begin Patch\n*** Update File: a.txt\n-nope\n+x\n*** End Patch\n";
        let err = ApplyPatch.run(&sandbox.ctx, json!({ "patch": patch })).await.unwrap_err();
        assert!(err.0.starts_with("a.txt: hunk 1"), "{}", err.0);
        assert_eq!(std::fs::read_to_string(sandbox.ctx.workspace.join("a.txt")).unwrap(), "one\n");
    }
}
