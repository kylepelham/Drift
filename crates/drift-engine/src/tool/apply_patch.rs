use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use super::ToolMetadata;
use super::edit::Change;
use super::patch::{self, Chunk, Op};
use super::text::TextFormat;
use super::{Ask, Context, Output, RunFuture, Tool, ToolError, display, required_str, stage};
use crate::llm::ToolSpec;
use crate::session::types::MetadataFile;
use crate::store::Store;

pub struct ApplyPatch;

impl Tool for ApplyPatch {
    fn permissions(&self) -> &'static [&'static str] {
        &["edit"]
    }

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

        // The patch is itself the change, so each file's ask shows it whole.
        let patch = input["patch"].as_str().map(str::to_string);
        unique
            .into_iter()
            .filter_map(|path| ctx.ask_to_write(&path, "Patch"))
            .map(|ask| ask.with_diff(patch.clone()))
            .collect()
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
                plan.prepare(ctx, op, &path)
                    .await
                    .map_err(|error| ToolError(format!("{name}: {}", error.0)))?;
                plan.touched.push(name);
            }

            for step in &plan.steps {
                super::fits_history(
                    &display(&step.path, &ctx.workspace),
                    step.after.as_ref().map_or(0, Vec::len),
                )?;
            }

            plan.apply(&ctx.engine.store).await?;
            let written: Vec<&Step> = plan.steps.iter().filter(|step| step.after.is_some()).collect();
            for step in &written {
                ctx.files.mark_read(&step.path);
            }

            let files = written
                .iter()
                .map(|step| MetadataFile::Path(step.path.to_string_lossy().into_owned()))
                .collect();
            Ok(Output {
                title: plan.touched.join(", "),
                output: plan.summary(),
                metadata: ToolMetadata {
                    files: Some(files),
                    file_changes: Some(plan.changes.iter().map(Change::metadata).collect()),
                    ..Default::default()
                },
            })
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
    changes: Vec<Change>,
    touched: Vec<String>,
}

impl Plan {
    /// Reads and checks one operation and records what it will do. Nothing is written here.
    async fn prepare(&mut self, ctx: &Context, op: &Op, path: &Path) -> Result<(), ToolError> {
        if self.steps.iter().any(|step| step.path == path) {
            return Err(ToolError("appears twice in the patch".into()));
        }

        match op {
            Op::Add { content, .. } => self.prepare_add(ctx, path, content).await,
            Op::Delete { .. } => self.prepare_delete(ctx, path).await,
            Op::Update { move_to, chunks, .. } => {
                let target = move_to.as_ref().map_or(path.to_path_buf(), |to| ctx.resolve(to));
                self.prepare_update(ctx, path, &target, chunks).await
            }
        }
    }

    /// Records an update in place, or a move when `target` is not `path`.
    async fn prepare_update(
        &mut self,
        ctx: &Context,
        path: &Path,
        target: &Path,
        chunks: &[Chunk],
    ) -> Result<(), ToolError> {
        let raw = existing(ctx, path).await?.ok_or(ToolError("does not exist".into()))?;
        // Lossy decoding would write every byte that is not UTF-8 back as U+FFFD.
        let text = String::from_utf8(raw.clone()).map_err(|_| {
            ToolError(
                "is not UTF-8 text (Windows-1252, for example), so patching it would corrupt it; it was not changed"
                    .into(),
            )
        })?;
        let ending = TextFormat::detect(&text);
        let before = ending.normalise(&text);
        let after = patch::apply_chunks(&before, chunks)?;

        let kind = if target == path { "update" } else { "move" };
        self.changes.push(Change::new(
            target,
            &display(target, &ctx.workspace),
            kind,
            &before,
            &after,
        ));
        let written = ending.apply(&after).into_bytes();
        if target == path {
            self.steps.push(Step {
                path: target.to_path_buf(),
                before: Some(raw),
                after: Some(written),
            });
            return Ok(());
        }

        if self.steps.iter().any(|step| step.path == target) {
            return Err(ToolError(format!(
                "moves onto {}, which the patch also changes",
                display(target, &ctx.workspace)
            )));
        }
        let displaced = existing(ctx, target).await?;
        self.steps.push(Step {
            path: target.to_path_buf(),
            before: displaced,
            after: Some(written),
        });
        self.steps.push(Step {
            path: path.to_path_buf(),
            before: Some(raw),
            after: None,
        });

        Ok(())
    }

    async fn prepare_add(&mut self, ctx: &Context, path: &Path, content: &str) -> Result<(), ToolError> {
        let existing = existing(ctx, path).await?;
        let before_text = existing
            .as_ref()
            .map(|bytes| String::from_utf8_lossy(bytes).into_owned())
            .unwrap_or_default();
        let format = TextFormat::detect(&before_text);
        let kind = if existing.is_some() { "update" } else { "add" };

        self.changes.push(Change::new(
            path,
            &display(path, &ctx.workspace),
            kind,
            &format.normalise(&before_text),
            &format.normalise(content),
        ));
        self.steps.push(Step {
            path: path.to_path_buf(),
            before: existing,
            after: Some(format.apply(content).into_bytes()),
        });

        Ok(())
    }

    async fn prepare_delete(&mut self, ctx: &Context, path: &Path) -> Result<(), ToolError> {
        let before = existing(ctx, path).await?.ok_or(ToolError("does not exist".into()))?;

        self.changes.push(Change::new(
            path,
            &display(path, &ctx.workspace),
            "delete",
            &String::from_utf8_lossy(&before),
            "",
        ));
        self.steps.push(Step {
            path: path.to_path_buf(),
            before: Some(before),
            after: None,
        });

        Ok(())
    }

    /// A line per file, as opencode answers; the diffs are in the metadata for the UI.
    fn summary(&self) -> String {
        let letter = |kind: &str| match kind {
            "add" => "A",
            "delete" => "D",
            "move" => "R",
            _ => "M",
        };
        let listed: Vec<String> = self
            .changes
            .iter()
            .map(|change| format!("{} {}", letter(change.kind), change.summary()))
            .collect();
        let plural = if listed.len() == 1 { "" } else { "s" };

        format!("Patched {} file{plural}:\n{}", listed.len(), listed.join("\n"))
    }

    /// Makes every change; on a failure, puts back every step through the failing one and names any file it could not.
    async fn apply(&self, store: &Store) -> Result<(), ToolError> {
        for (index, step) in self.steps.iter().enumerate() {
            if let Err(error) = set(store, &step.path, step.after.as_deref()).await {
                let unrestored = self.undo(store, index + 1).await;
                let state = if unrestored.is_empty() {
                    "nothing was changed".to_string()
                } else {
                    format!("these could not be put back: {}", unrestored.join("; "))
                };
                return Err(ToolError(format!(
                    "could not write {}: {error}; {state}",
                    step.path.display()
                )));
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
        return Err(ToolError(
            "exists and has not been read this session; read it before patching it".into(),
        ));
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
mod tests;
