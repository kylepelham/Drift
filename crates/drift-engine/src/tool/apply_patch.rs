use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use super::edit::{diff, LineEnding};
use super::patch::{self, Op};
use super::{display, required_str, Ask, Context, Output, RunFuture, Tool, ToolError};
use crate::llm::ToolSpec;

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

    /// One ask covers every file the patch touches; the title lists them.
    fn ask(&self, ctx: &Context, input: &Value) -> Option<Ask> {
        let ops = patch::parse(input["patch"].as_str()?).ok()?;
        let paths: Vec<String> = ops.iter().map(|op| display(&ctx.resolve(op.path()), &ctx.workspace)).collect();
        let pattern = ops.iter().map(|op| ctx.resolve(op.path()).to_string_lossy().into_owned()).collect::<Vec<_>>().join("\n");
        Some(Ask { kind: "edit".into(), pattern, title: format!("Patch {}", paths.join(", ")) })
    }

    fn mutates(&self) -> bool {
        true
    }

    fn run<'a>(&'a self, ctx: &'a Context, input: Value) -> RunFuture<'a> {
        Box::pin(async move {
            let ops = patch::parse(required_str(&input, "patch")?)?;
            let mut diffs = Vec::new();
            let mut touched = Vec::new();
            let mut files = Vec::new();
            for op in &ops {
                let path = ctx.resolve(op.path());
                let name = display(&path, &ctx.workspace);
                diffs.push(apply(ctx, op, &path, &name).await.map_err(|e| ToolError(format!("{name}: {}", e.0)))?);
                touched.push(name);
                if !matches!(op, Op::Delete { .. }) {
                    let written = match op {
                        Op::Update { move_to: Some(to), .. } => ctx.resolve(to),
                        _ => path,
                    };
                    files.push(written.to_string_lossy().into_owned());
                }
            }
            Ok(Output { title: touched.join(", "), output: diffs.join("\n"), metadata: json!({ "files": files }) })
        })
    }
}

async fn apply(ctx: &Context, op: &Op, path: &Path, name: &str) -> Result<String, ToolError> {
    match op {
        Op::Add { content, .. } => {
            if let Some(parent) = path.parent() {
                tokio::fs::create_dir_all(parent).await?;
            }
            tokio::fs::write(path, content).await?;
            ctx.files.mark_read(path);
            Ok(diff(name, "", content))
        }
        Op::Delete { .. } => {
            let before = tokio::fs::read_to_string(path).await.map_err(|_| ToolError("does not exist".into()))?;
            tokio::fs::remove_file(path).await?;
            Ok(diff(name, &before, ""))
        }
        Op::Update { move_to, chunks, .. } => {
            let raw = tokio::fs::read_to_string(path).await.map_err(|_| ToolError("does not exist".into()))?;
            let ending = LineEnding::detect(&raw);
            let before = ending.normalise(&raw);
            let after = patch::apply_chunks(&before, chunks)?;
            let target: PathBuf = move_to.as_ref().map_or(path.to_path_buf(), |to| ctx.resolve(to));
            if let Some(parent) = target.parent() {
                tokio::fs::create_dir_all(parent).await?;
            }
            tokio::fs::write(&target, ending.apply(&after)).await?;
            if target != path {
                tokio::fs::remove_file(path).await?;
            }
            ctx.files.mark_read(&target);
            Ok(diff(&display(&target, &ctx.workspace), &before, &after))
        }
    }
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
        let patch = "*** Begin Patch\n*** Add File: dir/new.txt\n+fresh\n*** Update File: a.txt\n*** Move to: b.txt\n-two\n+TWO\n*** Delete File: gone.txt\n*** End Patch\n";
        let out = ApplyPatch.run(&sandbox.ctx, json!({ "patch": patch })).await.unwrap();
        assert_eq!(out.title, "dir/new.txt, a.txt, gone.txt");
        assert_eq!(std::fs::read_to_string(sandbox.ctx.workspace.join("dir/new.txt")).unwrap(), "fresh\n");
        assert_eq!(std::fs::read(sandbox.ctx.workspace.join("b.txt")).unwrap(), b"one\r\nTWO\r\n");
        assert!(!sandbox.ctx.workspace.join("a.txt").exists());
        assert!(!sandbox.ctx.workspace.join("gone.txt").exists());
        assert!(out.output.contains("+TWO"));
        let ask = ApplyPatch.ask(&sandbox.ctx, &json!({ "patch": patch })).unwrap();
        assert_eq!(ask.title, "Patch dir/new.txt, a.txt, gone.txt");
    }

    #[tokio::test]
    async fn a_failed_hunk_names_the_file_and_changes_nothing() {
        let sandbox = Sandbox::new("apply-patch-miss");
        sandbox.file("a.txt", "one\n");
        let patch = "*** Begin Patch\n*** Update File: a.txt\n-nope\n+x\n*** End Patch\n";
        let err = ApplyPatch.run(&sandbox.ctx, json!({ "patch": patch })).await.unwrap_err();
        assert!(err.0.starts_with("a.txt: hunk 1"), "{}", err.0);
        assert_eq!(std::fs::read_to_string(sandbox.ctx.workspace.join("a.txt")).unwrap(), "one\n");
    }
}
