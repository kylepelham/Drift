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
        Some(Ask {
            kind: "edit".into(),
            pattern: path.to_string_lossy().into(),
            title: format!("Write {}", display(&path, &ctx.workspace)),
        })
    }

    fn mutates(&self) -> bool {
        true
    }

    fn run<'a>(&'a self, ctx: &'a Context, input: Value) -> RunFuture<'a> {
        Box::pin(async move {
            let path = ctx.resolve(required_str(&input, "path")?);
            let content = input["content"].as_str().ok_or(ToolError("`content` is required".into()))?;
            let existing = tokio::fs::read_to_string(&path).await.ok();
            if existing.is_some() && !ctx.files.was_read(&path) {
                return Err(ToolError(format!(
                    "{} exists and has not been read this session; read it before overwriting",
                    display(&path, &ctx.workspace)
                )));
            }
            if let Some(parent) = path.parent() {
                tokio::fs::create_dir_all(parent).await?;
            }
            let ending = existing.as_deref().map(LineEnding::detect).unwrap_or_default();
            tokio::fs::write(&path, ending.apply(content)).await?;
            ctx.files.mark_read(&path);
            let name = display(&path, &ctx.workspace);
            let before = existing.unwrap_or_default();
            Ok(Output {
                title: name.clone(),
                output: diff(&name, &before, content),
                metadata: json!({ "created": before.is_empty() }),
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
}
