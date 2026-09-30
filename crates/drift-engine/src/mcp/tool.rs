use std::sync::Arc;

use serde_json::{json, Value};

use super::Live;
use crate::llm::ToolSpec;
use crate::tool::{Ask, Context, Output, RunFuture, Tool, ToolError};

pub struct McpTool {
    server: String,
    tool: rmcp::model::Tool,
    live: Arc<Live>,
}

impl McpTool {
    pub(super) fn new(server: &str, tool: rmcp::model::Tool, live: Arc<Live>) -> Self {
        Self { server: server.into(), tool, live }
    }

    fn read_only(&self) -> bool {
        self.tool.annotations.as_ref().and_then(|a| a.read_only_hint).unwrap_or(false)
    }
}

impl Tool for McpTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: format!("{}_{}", self.server, self.tool.name),
            description: format!("[{} MCP server] {}", self.server, self.tool.description.clone().unwrap_or_default()),
            input_schema: Value::Object((*self.tool.input_schema).clone()),
        }
    }

    /// Every MCP call asks unless the server marks the tool read-only; "always" then covers the whole server.
    fn ask(&self, _ctx: &Context, _input: &Value) -> Option<Ask> {
        if self.read_only() {
            return None;
        }
        Some(Ask::new("mcp", format!("{}/{}", self.server, self.tool.name), format!("Call {} on {}", self.tool.name, self.server)))
    }

    fn mutates(&self) -> bool {
        !self.read_only()
    }

    fn run<'a>(&'a self, ctx: &'a Context, input: Value) -> RunFuture<'a> {
        Box::pin(async move {
            let called = tokio::select! {
                result = self.live.call(&self.tool.name, input) => result,
                () = ctx.abort.cancelled() => return Err(ToolError("aborted".into())),
            };
            let (text, is_error) = called.map_err(ToolError)?;
            if is_error {
                return Err(ToolError(text));
            }
            Ok(Output { title: format!("{}: {}", self.server, self.tool.name), output: text, metadata: json!({ "server": self.server }) })
        })
    }
}
