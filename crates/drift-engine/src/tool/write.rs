use serde_json::{json, Value};

use super::edit::{diff, Change};
use super::text::TextFormat;
use super::{display, required_str, Ask, Context, Output, RunFuture, Tool, ToolError};
use crate::llm::ToolSpec;

pub struct Write;

impl Tool for Write {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "write".into(),
            description: include_str!("prompts/write.txt").trim().into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "File to create or overwrite. Parent directories are created." },
                    "content": { "type": "string", "description": "The complete new contents of the file." }
                },
                "required": ["path", "content"]
            }),
        }
    }

    /// Carries the change the write would make: from what the file holds now, or from nothing.
    fn ask(&self, ctx: &Context, input: &Value) -> Option<Ask> {
        let path = ctx.resolve(input["path"].as_str()?);
        let ask = ctx.ask_to_write(&path, "Write")?;
        let before = std::fs::read(&path).map(|bytes| String::from_utf8_lossy(&bytes).replace("\r\n", "\n")).unwrap_or_default();
        let format = TextFormat::detect(&before);
        let proposed = input["content"].as_str().map(|content| diff(&display(&path, &ctx.workspace), &format.normalise(&before), &format.normalise(content)));
        Some(ask.with_diff(proposed))
    }

    fn mutates(&self) -> bool {
        true
    }

    fn touches(&self, ctx: &Context, input: &Value) -> Option<Vec<std::path::PathBuf>> {
        Some(vec![ctx.resolve(input["path"].as_str()?)])
    }

    fn run<'a>(&'a self, ctx: &'a Context, input: Value) -> RunFuture<'a> {
        Box::pin(async move {
            let path = ctx.resolve(required_str(&input, "path")?);
            let content = input["content"].as_str().ok_or(ToolError("`content` is required".into()))?;
            let name = display(&path, &ctx.workspace);
            // Only a missing file is new; one that cannot be read or decoded still exists.
            let existing = match tokio::fs::read(&path).await {
                Ok(bytes) => Some(String::from_utf8_lossy(&bytes).into_owned()),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
                Err(error) => return Err(ToolError(format!("{name} could not be read ({error}), so it was not overwritten"))),
            };
            if existing.is_some() && !ctx.files.was_read(&path) {
                return Err(ToolError(format!("{name} exists and has not been read this session; read it before overwriting")));
            }
            let ending = existing.as_deref().map(TextFormat::detect).unwrap_or_default();
            let written = ending.apply(content);
            super::fits_history(&name, written.len())?;
            super::stage::replace(&ctx.engine.store, &path, written.as_bytes()).await?;
            ctx.files.mark_read(&path);
            let created = existing.is_none();
            let before = existing.unwrap_or_default();
            let change = Change::new(&path, &name, if created { "add" } else { "update" }, &ending.normalise(&before), &ending.normalise(content));
            let lines = |count: usize| if count == 1 { "1 line".to_string() } else { format!("{count} lines") };
            let output = if created { format!("Created {name} ({}).", lines(change.additions)) } else { format!("Wrote {}.", change.summary()) };
            Ok(Output {
                title: name.clone(),
                output,
                metadata: json!({ "created": created, "files": [path.to_string_lossy()], "diff": change.patch, "changes": [change.json()] }),
            })
        })
    }
}

#[cfg(test)]
mod tests {
    use super::super::tests::Sandbox;
    use super::*;

    #[tokio::test]
    async fn creates_parents_and_reports_a_diff() {
        let sandbox = Sandbox::new("write");
        let out = Write.run(&sandbox.ctx, json!({ "path": "a/b/c.txt", "content": "hello\n" })).await.unwrap();
        assert_eq!(std::fs::read_to_string(sandbox.ctx.workspace.join("a/b/c.txt")).unwrap(), "hello\n");
        assert_eq!(out.output, "Created a/b/c.txt (1 line).", "a new file is not echoed back to the model");
        assert!(out.metadata["diff"].as_str().unwrap().contains("+hello"));
        assert_eq!((out.metadata["created"].as_bool(), out.metadata["changes"][0]["type"].as_str()), (Some(true), Some("add")));
    }

    #[tokio::test]
    async fn refuses_to_overwrite_unread_files_and_keeps_crlf() {
        let sandbox = Sandbox::new("write-unread");
        let path = sandbox.file("x.txt", "a\r\nb\r\n");
        let err = Write.run(&sandbox.ctx, json!({ "path": "x.txt", "content": "c\n" })).await.unwrap_err();
        assert!(err.0.contains("has not been read"));
        sandbox.ctx.files.mark_read(&path);
        Write.run(&sandbox.ctx, json!({ "path": "x.txt", "content": "c\nd\n" })).await.unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"c\r\nd\r\n");
    }

    #[tokio::test]
    async fn a_file_that_is_not_text_still_exists() {
        let sandbox = Sandbox::new("write-binary");
        let path = sandbox.ctx.resolve("blob.bin");
        std::fs::write(&path, [0xff, 0xfe, 0x00, 0x9f]).unwrap();
        let err = Write.run(&sandbox.ctx, json!({ "path": "blob.bin", "content": "text" })).await.unwrap_err();
        assert!(err.0.contains("has not been read"), "invalid UTF-8 is not absence: {}", err.0);
        assert_eq!(std::fs::read(&path).unwrap(), [0xff, 0xfe, 0x00, 0x9f]);
        sandbox.ctx.files.mark_read(&path);
        let out = Write.run(&sandbox.ctx, json!({ "path": "blob.bin", "content": "text" })).await.unwrap();
        assert_eq!(out.metadata["created"], false);
        std::fs::create_dir_all(sandbox.ctx.resolve("dir")).unwrap();
        let err = Write.run(&sandbox.ctx, json!({ "path": "dir", "content": "x" })).await.unwrap_err();
        assert!(err.0.contains("could not be read"), "any other read error stops it: {}", err.0);
    }

    #[tokio::test]
    async fn a_write_that_fails_once_begun_or_is_interrupted_leaves_the_file_whole() {
        use crate::tool::stage::tests::{inject, leftovers, stranded, Fault};
        let sandbox = Sandbox::new("write-fails");
        let path = sandbox.file("x.txt", "kept\n");
        sandbox.ctx.files.mark_read(&path);
        inject(Fault::AfterStaging, &path);
        assert!(Write.run(&sandbox.ctx, json!({ "path": "x.txt", "content": "new\n" })).await.is_err());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "kept\n");
        assert!(leftovers(&sandbox.ctx.workspace).is_empty());

        // A crash mid-swap leaves the original only in the backup; the next start puts it back.
        let store = &sandbox.ctx.engine.store;
        std::fs::remove_file(&path).unwrap();
        stranded(store, &path, "kept\n");
        assert_eq!(crate::tool::stage::recover_leftovers(store), 1);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "kept\n");
        assert!(leftovers(&sandbox.ctx.workspace).is_empty());
    }
}
