//! `mcp_resources` and `mcp_read_resource`: what connected servers publish as resources, listed and
//! read. Offered only while a connected server serves any; reading changes nothing, so neither asks.

use serde_json::{Value, json};

use crate::llm::ToolSpec;
use crate::tool::ToolMetadata;
use crate::tool::{Ask, Context, Output, RunFuture, Tool, ToolError, required_str};

pub(super) struct ListResources;
pub(super) struct ReadResource;

impl Tool for ListResources {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "mcp_resources".into(),
            description: concat!(
                "Lists the resources connected MCP servers publish (files, records, documents), ",
                "with the uri to read each by. Read one with mcp_read_resource.",
            )
            .into(),
            input_schema: json!({
                "type": "object",
                "properties": { "server": {
                    "type": "string",
                    "description": "Only this server's resources. Default: every server that has any."
                } }
            }),
        }
    }

    fn ask(&self, _ctx: &Context, _input: &Value) -> Option<Ask> {
        None
    }

    fn run<'a>(&'a self, ctx: &'a Context, input: Value) -> RunFuture<'a> {
        Box::pin(async move {
            let servers = match input["server"].as_str() {
                Some(server) => vec![server.to_string()],
                None => ctx.engine.mcp.with_resources(Some(&ctx.workspace)),
            };

            let mut lines = Vec::new();
            for server in &servers {
                let resources = ctx
                    .engine
                    .mcp
                    .list_resources(server, Some(&ctx.workspace))
                    .await
                    .map_err(|error| ToolError(error.to_string()))?;

                for resource in resources {
                    let about = resource
                        .description
                        .as_deref()
                        .map(|d| format!(": {d}"))
                        .unwrap_or_default();
                    let kind = resource
                        .mime_type
                        .as_deref()
                        .map(|m| format!(" ({m})"))
                        .unwrap_or_default();
                    lines.push(format!("{server} {} {}{kind}{about}", resource.uri, resource.name));
                }
            }

            let output = if lines.is_empty() {
                "No resources.".to_string()
            } else {
                lines.join("\n")
            };

            Ok(Output::new(format!("Resources of {}", servers.join(", ")), output))
        })
    }
}

impl Tool for ReadResource {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "mcp_read_resource".into(),
            description: concat!(
                "Reads one MCP resource by its server and uri, as mcp_resources lists them. ",
                "Text comes back as text; images and PDFs come back for you to look at.",
            )
            .into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "server": { "type": "string", "description": "The MCP server's name." },
                    "uri": { "type": "string", "description": "The resource's uri." }
                },
                "required": ["server", "uri"]
            }),
        }
    }

    fn ask(&self, _ctx: &Context, _input: &Value) -> Option<Ask> {
        None
    }

    fn run<'a>(&'a self, ctx: &'a Context, input: Value) -> RunFuture<'a> {
        Box::pin(async move {
            let (server, uri) = (required_str(&input, "server")?, required_str(&input, "uri")?);
            let answer = ctx
                .engine
                .mcp
                .read_resource(server, Some(&ctx.workspace), uri)
                .await
                .map_err(|error| ToolError(error.to_string()))?;

            let mut metadata = ToolMetadata {
                server: Some(server.into()),
                uri: Some(uri.into()),
                ..Default::default()
            };
            if !answer.images.is_empty() {
                metadata.images = Some(crate::tool::image::metadata(&answer.images));
            }

            Ok(Output {
                title: format!("{server}: {uri}"),
                output: answer.text,
                metadata,
            })
        })
    }
}
