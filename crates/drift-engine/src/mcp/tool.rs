use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use serde_json::{json, Value};

use super::{Answer, CallError, Live, Slot, REPLACEMENT_WAIT};
use crate::llm::ToolSpec;
use crate::tool::{Ask, Context, Output, RunFuture, Tool, ToolError};

/// A server's tool as a turn was offered it: the client it was planned with, and the server's slot for what came after.
pub struct McpTool {
    server: String,
    tool: rmcp::model::Tool,
    pinned: Arc<Live>,
    slot: Arc<Slot>,
    /// The name the model calls it by ([`wire_names`]), fixed when the turn's tools are made.
    name: String,
}

impl McpTool {
    pub(super) fn new(server: &str, tool: rmcp::model::Tool, pinned: Arc<Live>, slot: Arc<Slot>, name: String) -> Self {
        Self { server: server.into(), tool, pinned, slot, name }
    }

    fn read_only(&self) -> bool {
        self.tool.annotations.as_ref().and_then(|a| a.read_only_hint).unwrap_or(false)
    }

    fn closed(&self) -> ToolError {
        ToolError(format!("the {} MCP server was disabled or removed", self.server))
    }

    /// A reconnected or re-listed server may redefine the tool (no longer read-only, another schema); the turn was given this one.
    fn unchanged_on(&self, client: &Arc<Live>) -> Result<(), ToolError> {
        if client.tools().iter().any(|tool| behaves_alike(tool, &self.tool)) {
            return Ok(());
        }
        Err(ToolError(format!("{} changed its {} tool since this turn began, so it was not run; the next turn sees the new one", self.server, self.tool.name)))
    }

    async fn call(&self, ctx: &Context, client: &Live, input: Value) -> Result<Answer, CallError> {
        let limit = async {
            match client.timeout {
                Some(limit) => tokio::time::sleep(limit).await,
                None => std::future::pending().await,
            }
        };
        tokio::select! {
            result = client.call(&self.tool.name, input) => result,
            () = ctx.abort.cancelled() => Err(CallError::Failed("aborted".into())),
            () = self.slot.closing() => Err(CallError::Failed(format!("the {} MCP server was disabled while the call ran; it may or may not have taken effect", self.server))),
            () = limit => Err(CallError::Failed(format!("the {} MCP server did not answer within its {}s timeout; the call may or may not have taken effect", self.server, client.timeout.unwrap_or_default().as_secs()))),
        }
    }

    /// A call cut off by a lost connection: a read-only one is asked again once of the reconnected server, never one that may have changed something.
    async fn after_loss(&self, ctx: &Context, lost: &Arc<Live>, input: Value) -> Result<Answer, ToolError> {
        let uncertain = || ToolError(format!("the connection to {} closed during the call; it may or may not have taken effect and was not retried", self.server));
        if !self.read_only() {
            return Err(uncertain());
        }
        let lost_again = || ToolError(format!("the connection to {} closed during the call and it did not come back", self.server));
        let next = match lost.holds_nothing_open() && lost.is_open() {
            // Only the request failed; the client stands, so it is asked again there.
            true => lost.clone(),
            false => {
                if lost.holds_nothing_open() {
                    ctx.engine.recheck_mcp(&self.server, lost);
                }
                self.slot.replacement(lost, REPLACEMENT_WAIT).await.ok_or_else(lost_again)?
            }
        };
        self.unchanged_on(&next)?;
        match self.call(ctx, &next, input).await {
            Ok(answer) => Ok(answer),
            Err(CallError::Lost) => Err(lost_again()),
            Err(CallError::Failed(error)) => Err(ToolError(error)),
        }
    }
}

/// Same name, input and safety hints, judged by the defaults MCP gives missing ones; a new description or title changes nothing a call relies on.
fn behaves_alike(a: &rmcp::model::Tool, b: &rmcp::model::Tool) -> bool {
    let hints = |tool: &rmcp::model::Tool| tool.annotations.as_ref().map_or((false, true), |hint| (hint.read_only_hint.unwrap_or(false), hint.is_destructive()));
    a.name == b.name && a.input_schema == b.input_schema && hints(a) == hints(b)
}

/// The longest name a tool is given: providers take 64, and the subscription route adds `mcp_`.
const MAX_NAME: usize = 60;

/// Built-in tool names a server's `<server>_<tool>` could spell; providers refuse two tools of one name.
pub(crate) const RESERVED: [&str; 6] = ["apply_patch", "task_output", "task_stop", "read_thread", "mcp_resources", "mcp_read_resource"];

/// Names already given, by `(server, tool)`; a name once given is never given to another tool.
pub type Given = HashMap<(String, String), String>;

/// The names the model calls a set of servers' tools (`(server, tool)` pairs) by. A tool named
/// before keeps its name, so connecting another server never renames one a transcript already
/// calls. A new tool gets `<server>_<tool>` as written where no tool has or is getting that name,
/// and a hashed name ([`wire_name`] with `clashes`) where one does (`a_b` + `c` and `a` + `b_c`).
pub fn wire_names(given: &Given, tools: &[(&str, &str)]) -> Vec<String> {
    let taken: HashSet<&str> = given.values().map(String::as_str).collect();
    let known = |server: &str, tool: &str| given.get(&(server.to_string(), tool.to_string()));
    let plain: Vec<String> = tools.iter().map(|(server, tool)| wire_name(server, tool, false)).collect();
    let fresh = |name: &String| tools.iter().zip(&plain).filter(|((server, tool), other)| *other == name && known(server, tool).is_none()).count();
    let name = |(server, tool): &(&str, &str), plain: &String| match known(server, tool) {
        Some(name) => name.clone(),
        None if taken.contains(plain.as_str()) || fresh(plain) > 1 => wire_name(server, tool, true),
        None => plain.clone(),
    };
    tools.iter().zip(&plain).map(|(pair, plain)| name(pair, plain)).collect()
}

/// The name the model calls a server's tool by, in the characters every provider accepts
/// (`[a-zA-Z0-9_-]`, at most 64). A name that had to change (a character replaced, or cut to fit),
/// that spells a built-in tool's, or that `clashes` with another server's, ends in a hash of the
/// original, keeping every name apart.
pub fn wire_name(server: &str, tool: &str, clashes: bool) -> String {
    let raw = format!("{server}_{tool}");
    let clean: String = raw.chars().map(|c| if c.is_ascii_alphanumeric() || c == '_' || c == '-' { c } else { '_' }).collect();
    if clean == raw && clean.len() <= MAX_NAME && !RESERVED.contains(&clean.as_str()) && !clashes {
        return clean;
    }
    use sha2::Digest;
    // Server and tool apart, so `a_b` + `c` and `a` + `b_c` hash differently.
    let hash: String = sha2::Sha256::digest(format!("{server}\0{tool}").as_bytes()).iter().take(4).map(|b| format!("{b:02x}")).collect();
    let keep = clean.len().min(MAX_NAME - hash.len() - 1);
    format!("{}_{hash}", &clean[..keep])
}

impl Tool for McpTool {
    fn server(&self) -> Option<&str> {
        Some(&self.server)
    }

    /// A server's read-only mark is its own claim, enough to skip an ask; a read-only agent relies on
    /// it only for a server the user vouched for in their own drift.json (`readOnlyMcp`).
    fn stays_read_only(&self, ctx: &Context, _input: &Value) -> bool {
        self.read_only() && ctx.config.read_only_mcp.contains(&self.server)
    }

    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: self.name.clone(),
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
            self.unchanged_on(&client)?;
            let answer = match self.call(ctx, &client, input.clone()).await {
                Ok(answer) => answer,
                Err(CallError::Lost) => self.after_loss(ctx, &client, input).await?,
                Err(CallError::Failed(error)) => return Err(ToolError(error)),
            };
            if answer.is_error {
                return Err(ToolError(answer.text));
            }
            let mut metadata = json!({ "server": self.server });
            if !answer.images.is_empty() {
                metadata["images"] = crate::tool::image::metadata(&answer.images);
            }
            Ok(Output { title: format!("{}: {}", self.server, self.tool.name), output: answer.text, metadata })
        })
    }
}

#[cfg(test)]
mod tests {
    use rmcp::model::{Tool, ToolAnnotations};
    use serde_json::json;

    use super::{behaves_alike, wire_names, Given};

    fn wire_name(server: &str, tool: &str) -> String {
        super::wire_name(server, tool, false)
    }

    #[test]
    fn a_name_once_given_stays_when_a_clashing_server_connects() {
        let given = Given::from([(("a".to_string(), "b_c".to_string()), "a_b_c".to_string())]);
        let names = wire_names(&given, &[("a", "b_c"), ("a_b", "c")]);
        assert_eq!(names[0], "a_b_c", "the tool the transcript calls keeps its name");
        assert!(names[1].starts_with("a_b_c_"), "the newcomer gets the hash: {names:?}");
        let alone = wire_names(&given, &[("a_b", "c")]);
        assert!(alone[0].starts_with("a_b_c_"), "a name stays taken while its tool is away: {alone:?}");
    }

    #[test]
    fn tool_names_are_what_providers_accept() {
        assert_eq!(wire_name("echo", "shout"), "echo_shout");
        assert!(wire_name("gh", "repos/list.all").starts_with("gh_repos_list_all_"), "a changed name carries a hash");
        assert_ne!(wire_name("s", "a.b"), wire_name("s", "a_b"), "names that clean to the same string stay apart");
        assert_eq!(wire_name("s", "a_b"), "s_a_b");
        assert_ne!(wire_name("task", "output"), "task_output", "never a built-in tool's name");
        assert_eq!(wire_name("my_server", "search"), "my_server_search", "a `_` in a server's name alone changes nothing");
        let names = wire_names(&Given::new(), &[("a_b", "c"), ("a", "b_c"), ("a", "d")]);
        assert!(names[0] != names[1] && names[0].starts_with("a_b_c_") && names[1].starts_with("a_b_c_"), "only names that meet get a hash: {names:?}");
        assert_eq!(names[2], "a_d");
        let builtin: Vec<String> = crate::tool::Registry::builtin().specs(crate::llm::catalog::ToolProfile::Edit).into_iter().chain(crate::tool::Registry::builtin().specs(crate::llm::catalog::ToolProfile::ApplyPatch)).map(|s| s.name).chain(["mcp_resources".into(), "mcp_read_resource".into()]).filter(|n| n.contains('_')).collect();
        assert!(builtin.iter().all(|name| super::RESERVED.contains(&name.as_str())), "every built-in name an MCP tool could spell is reserved: {builtin:?}");
        let long = wire_name("server", &"x".repeat(80));
        assert_eq!(long.len(), 60, "room left for the subscription route's mcp_ prefix");
        assert_ne!(long, wire_name("server", &"x".repeat(81)), "cut names stay apart");
        assert!(long.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-'));
    }

    fn tool(description: &'static str, schema: serde_json::Value) -> Tool {
        Tool::new("search", description, std::sync::Arc::new(schema.as_object().unwrap().clone()))
    }

    #[test]
    fn a_reworded_tool_is_the_same_tool_but_new_input_or_hints_are_not() {
        let schema = json!({ "type": "object", "properties": { "q": { "type": "string" } } });
        let read_only = tool("Searches", schema.clone()).annotate(ToolAnnotations::new().read_only(true));
        let reworded = tool("Searches the docs", schema.clone()).annotate(ToolAnnotations::with_title("Search").read_only(true));
        assert!(behaves_alike(&read_only, &reworded));
        assert!(!behaves_alike(&read_only, &tool("Searches", schema.clone())), "no longer read-only");
        assert!(!behaves_alike(&read_only, &tool("Searches", json!({ "type": "object" })).annotate(ToolAnnotations::new().read_only(true))), "another input");
        let plain = tool("Searches", schema.clone());
        assert!(behaves_alike(&plain, &plain.clone().annotate(ToolAnnotations::new().destructive(true))), "a hint stated as its default");
        assert!(!behaves_alike(&plain, &plain.clone().annotate(ToolAnnotations::new().destructive(false))));
    }
}
