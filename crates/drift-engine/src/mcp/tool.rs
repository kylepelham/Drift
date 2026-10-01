use std::sync::Arc;

use serde_json::{json, Value};

use super::{CallError, Live, Slot, REPLACEMENT_WAIT};
use crate::llm::ToolSpec;
use crate::tool::{Ask, Context, Output, RunFuture, Tool, ToolError};

/// A server's tool as a turn was offered it: the client it was planned with, and the server's slot for what came after.
pub struct McpTool {
    server: String,
    tool: rmcp::model::Tool,
    pinned: Arc<Live>,
    slot: Arc<Slot>,
}

impl McpTool {
    pub(super) fn new(server: &str, tool: rmcp::model::Tool, pinned: Arc<Live>, slot: Arc<Slot>) -> Self {
        Self { server: server.into(), tool, pinned, slot }
    }

    fn read_only(&self) -> bool {
        self.tool.annotations.as_ref().and_then(|a| a.read_only_hint).unwrap_or(false)
    }

    fn closed(&self) -> ToolError {
        ToolError(format!("the {} MCP server was disabled or removed", self.server))
    }

    async fn call(&self, ctx: &Context, client: &Live, input: Value) -> Result<(String, bool), CallError> {
        tokio::select! {
            result = client.call(&self.tool.name, input) => result,
            () = ctx.abort.cancelled() => Err(CallError::Failed("aborted".into())),
            () = self.slot.closing() => Err(CallError::Failed(format!("the {} MCP server was disabled while the call ran; it may or may not have taken effect", self.server))),
        }
    }

    /// A call cut off by a lost connection: a read-only one is asked again once of the reconnected server, never one that may have changed something.
    async fn after_loss(&self, ctx: &Context, lost: &Arc<Live>, input: Value) -> Result<(String, bool), ToolError> {
        let uncertain = || ToolError(format!("the connection to {} closed during the call; it may or may not have taken effect and was not retried", self.server));
        if !self.read_only() {
            return Err(uncertain());
        }
        let lost_again = || ToolError(format!("the connection to {} closed during the call and it did not come back", self.server));
        let Some(next) = self.slot.replacement(lost, REPLACEMENT_WAIT).await else { return Err(lost_again()) };
        match self.call(ctx, &next, input).await {
            Ok(answer) => Ok(answer),
            Err(CallError::Lost) => Err(lost_again()),
            Err(CallError::Failed(error)) => Err(ToolError(error)),
        }
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
            if self.slot.is_closed() {
                return Err(self.closed());
            }
            let client = self.slot.client_for(&self.pinned);
            let (text, is_error) = match self.call(ctx, &client, input.clone()).await {
                Ok(answer) => answer,
                Err(CallError::Lost) => self.after_loss(ctx, &client, input).await?,
                Err(CallError::Failed(error)) => return Err(ToolError(error)),
            };
            if is_error {
                return Err(ToolError(text));
            }
            Ok(Output { title: format!("{}: {}", self.server, self.tool.name), output: text, metadata: json!({ "server": self.server }) })
        })
    }
}
