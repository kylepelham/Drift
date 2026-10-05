use serde_json::{json, Value};

use super::{Ask, Context, Output, RunFuture, Tool, ToolError};
use crate::event::Event;
use crate::llm::ToolSpec;
use crate::session::types::{Todo, TodoStatus};

pub struct TodoWrite;

impl Tool for TodoWrite {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "todowrite".into(),
            description: include_str!("prompts/todowrite.txt").trim().into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "todos": {
                        "type": "array",
                        "description": "The complete list; it replaces the previous one.",
                        "items": {
                            "type": "object",
                            "properties": {
                                "content": { "type": "string" },
                                "status": { "type": "string", "enum": ["pending", "in_progress", "completed", "cancelled"] },
                                "priority": { "type": "string", "enum": ["high", "medium", "low"] }
                            },
                            "required": ["content", "status"]
                        }
                    }
                },
                "required": ["todos"]
            }),
        }
    }

    fn ask(&self, _ctx: &Context, _input: &Value) -> Option<Ask> {
        None
    }

    fn run<'a>(&'a self, ctx: &'a Context, input: Value) -> RunFuture<'a> {
        Box::pin(async move {
            let todos: Vec<Todo> = serde_json::from_value(input["todos"].clone()).map_err(|e| ToolError(format!("invalid todos: {e}")))?;
            ctx.engine.store.set_todos(&ctx.session_id, &todos)?;
            ctx.engine.hub.publish(Event::TodoUpdated { session_id: ctx.session_id.clone(), todos: todos.clone() });
            let open = todos.iter().filter(|t| !matches!(t.status, TodoStatus::Completed | TodoStatus::Cancelled)).count();
            Ok(Output {
                title: format!("{open} of {} remaining", todos.len()),
                output: serde_json::to_string_pretty(&todos).unwrap(),
                metadata: json!({ "count": todos.len(), "open": open }),
            })
        })
    }
}

#[cfg(test)]
mod tests {
    use super::super::tests::Sandbox;
    use super::*;
    use crate::session::types::Visibility;
    use crate::store::NewSession;

    #[tokio::test]
    async fn saves_and_publishes_the_list() {
        let sandbox = Sandbox::new("todo");
        let session = sandbox
            .ctx
            .engine
            .store
            .create_session(NewSession { workspace_id: "w", parent_id: None, visibility: Visibility::Sibling, title: "", agent: "build", model: None })
            .unwrap();
        let ctx = Context { session_id: session.id.clone(), ..sandbox.ctx_clone() };
        let mut rx = ctx.engine.hub.attach(None).rx;
        let out = TodoWrite
            .run(&ctx, json!({ "todos": [{ "content": "a", "status": "completed" }, { "content": "b", "status": "in_progress", "priority": "high" }] }))
            .await
            .unwrap();
        assert_eq!(out.title, "1 of 2 remaining");
        assert_eq!(ctx.engine.store.todos(&session.id).unwrap().len(), 2);
        assert!(matches!(rx.try_recv().unwrap().event, Event::TodoUpdated { .. }));
        assert!(TodoWrite.run(&ctx, json!({ "todos": [{ "status": "nope" }] })).await.is_err());
    }
}
