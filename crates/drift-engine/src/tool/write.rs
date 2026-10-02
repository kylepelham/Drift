use serde_json::{json, Value};

use super::edit::{diff, LineEnding};
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

    fn ask(&self, ctx: &Context, input: &Value) -> Option<Ask> {
        let path = ctx.resolve(input["path"].as_str()?);
        ctx.ask_to_write(&path, "Write")
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
            let ending = existing.as_deref().map(LineEnding::detect).unwrap_or_default();
            let written = ending.apply(content);
            super::fits_history(&name, written.len())?;
            super::stage::replace(&ctx.engine.store, &path, written.as_bytes()).await?;
            ctx.files.mark_read(&path);
            let created = existing.is_none();
            let before = existing.unwrap_or_default();
            Ok(Output {
                title: name.clone(),
                output: diff(&name, &before, content),
                metadata: json!({ "created": created, "files": [path.to_string_lossy()] }),
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
        assert!(out.output.contains("+hello"));
        assert_eq!(out.metadata["created"], true);
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
