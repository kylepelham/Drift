use rmcp::model::ContentBlock;
use std::path::Path;
use std::sync::Arc;

use super::connect::{list_tools, within};
use super::response::{resource_text, take_resource};
use super::{Answer, Error, Given, Key, Live, McpTool, READY_WAIT, Servers, Slot, resources, wire_names};
use crate::event::{Event, Hub};
use crate::store::Store;

impl Servers {
    /// Offers each connection's tools for the workspace, using shared connections only for remote servers.
    /// Persistent `server_tool` names distinguish servers and stay stable across turns.
    /// The store owns those names ([`wire_names`]).
    pub fn tools(&self, store: &Store, workspace: Option<&Path>) -> Vec<Arc<dyn crate::tool::Tool>> {
        let shown = self.shown(workspace);
        let lives = self.lock().lives_for(workspace, &shown);
        let resources = lives.iter().any(|(_, _, live)| live.resources);
        let listed: Vec<(Key, rmcp::model::Tool, Arc<Live>, Arc<Slot>)> = lives
            .into_iter()
            .flat_map(|(key, slot, live)| {
                live.tools()
                    .into_iter()
                    .map(move |tool| (key.clone(), tool, live.clone(), slot.clone()))
            })
            .collect();

        let pairs: Vec<(&str, &str)> = listed
            .iter()
            .map(|(key, tool, ..)| (key.server.as_str(), tool.name.as_ref()))
            .collect();
        let names = store
            .name_mcp_tools(&pairs)
            .unwrap_or_else(|_| wire_names(&Given::new(), &pairs));
        let mut tools: Vec<Arc<dyn crate::tool::Tool>> = listed
            .iter()
            .zip(names)
            .map(|((key, tool, live, slot), name)| {
                Arc::new(McpTool::new(key, tool.clone(), live.clone(), slot.clone(), name))
                    as Arc<dyn crate::tool::Tool>
            })
            .collect();

        if resources {
            tools.push(Arc::new(resources::ListResources));
            tools.push(Arc::new(resources::ReadResource));
        }

        tools
    }

    /// Re-lists connections whose tools passed their `ttlMs`, so the next turn sees current definitions.
    pub async fn refresh_stale(&self, store: &Store, hub: &Hub) {
        let due: Vec<(String, Arc<Live>)> = self
            .lock()
            .servers
            .iter()
            .filter_map(|(key, slot)| Some((key.server.clone(), slot.current()?)))
            .filter(|(_, live)| live.listing_due())
            .collect();
        let relist = |name: String, live: Arc<Live>| async move {
            match tokio::time::timeout(READY_WAIT, list_tools(&live.service)).await {
                Ok(Ok((tools, ttl))) => live.relisted(tools, ttl).then_some(name),
                _ => {
                    live.relist_failed();
                    None
                }
            }
        };

        let changed = futures_util::future::join_all(due.into_iter().map(|(name, live)| relist(name, live))).await;
        for name in changed.into_iter().flatten() {
            if let Ok(Some(row)) = store.mcp_server(&name) {
                hub.publish(Event::McpUpdated {
                    server: self.status_of(row),
                });
            }
        }
    }

    /// The instructions of each server connected for `workspace`, by server name.
    pub fn instructions(&self, workspace: Option<&Path>) -> Vec<(String, String)> {
        let shown = self.shown(workspace);

        self.lock()
            .lives_for(workspace, &shown)
            .into_iter()
            .filter_map(|(key, _, live)| Some((key.server, live.instructions.clone()?)))
            .collect()
    }

    fn live(&self, server: &str, workspace: Option<&Path>) -> Result<Arc<Live>, Error> {
        let shown = self.shown(workspace);

        self.lock()
            .live_for(server, workspace, &shown)
            .map(|(_, live)| live)
            .ok_or_else(|| Error::NotConnected {
                server: server.to_owned(),
            })
    }

    /// Servers connected for `workspace` that serve resources, by name.
    pub fn with_resources(&self, workspace: Option<&Path>) -> Vec<String> {
        let shown = self.shown(workspace);

        self.lock()
            .lives_for(workspace, &shown)
            .into_iter()
            .filter(|(_, _, live)| live.resources)
            .map(|(key, _, _)| key.server)
            .collect()
    }

    /// The prompts of every server connected for `workspace`, as `(server, prompt)`.
    pub fn prompts(&self, workspace: Option<&Path>) -> Vec<(String, rmcp::model::Prompt)> {
        let shown = self.shown(workspace);
        let mut prompts: Vec<(String, rmcp::model::Prompt)> = self
            .lock()
            .lives_for(workspace, &shown)
            .into_iter()
            .flat_map(|(key, _, live)| {
                live.prompts
                    .iter()
                    .map(|prompt| (key.server.clone(), prompt.clone()))
                    .collect::<Vec<_>>()
            })
            .collect();
        prompts.sort_by(|first, second| (&first.0, &first.1.name).cmp(&(&second.0, &second.1.name)));

        prompts
    }

    pub async fn list_resources(
        &self,
        server: &str,
        workspace: Option<&Path>,
    ) -> Result<Vec<rmcp::model::Resource>, Error> {
        let live = self.live(server, workspace)?;

        within("list its resources", async {
            live.service.list_all_resources().await.map_err(Error::Service)
        })
        .await
    }

    /// A resource's contents: text inline, images and PDFs as files, other binaries named.
    pub(crate) async fn read_resource(
        &self,
        server: &str,
        workspace: Option<&Path>,
        uri: &str,
    ) -> Result<Answer, Error> {
        let live = self.live(server, workspace)?;
        let read = within("read the resource", async {
            live.service
                .read_resource(rmcp::model::ReadResourceRequestParams::new(uri))
                .await
                .map_err(Error::Service)
        })
        .await?;

        let mut answer = Answer {
            text: String::new(),
            is_error: false,
            images: Vec::new(),
        };
        let mut lines = Vec::new();
        for content in &read.contents {
            take_resource(content, &mut answer.images, &mut lines);
        }
        answer.text = lines.join("\n");

        Ok(answer)
    }

    /// The prompts of every server connected for `workspace` as slash commands named `server:prompt`.
    pub fn prompt_commands(&self, workspace: Option<&Path>) -> Vec<crate::config::Command> {
        self.prompts(workspace)
            .into_iter()
            .map(|(server, prompt)| {
                let description = prompt
                    .description
                    .clone()
                    .unwrap_or_else(|| format!("A prompt from the {server} MCP server"));
                let name = format!("{server}:{}", prompt.name);
                let mut command = crate::config::Command::new(name, description, String::new());
                command.arguments = prompt
                    .arguments
                    .iter()
                    .flatten()
                    .map(|argument| argument.name.clone())
                    .collect();
                command.server = Some(server);

                command
            })
            .collect()
    }

    /// A prompt filled with `arguments`, as the text of its messages.
    pub async fn get_prompt(
        &self,
        server: &str,
        workspace: Option<&Path>,
        name: &str,
        arguments: serde_json::Map<String, serde_json::Value>,
    ) -> Result<String, Error> {
        let live = self.live(server, workspace)?;
        let mut params = rmcp::model::GetPromptRequestParams::new(name);
        params.arguments = Some(arguments);
        let filled = within("fill the prompt", async {
            live.service.get_prompt(params).await.map_err(Error::Service)
        })
        .await?;

        let texts: Vec<String> = filled
            .messages
            .iter()
            .map(|message| match &message.content {
                ContentBlock::Text(text) => text.text.clone(),
                ContentBlock::Resource(resource) => resource_text(&resource.resource),
                other => serde_json::to_string(other).unwrap_or_default(),
            })
            .collect();

        Ok(texts.join("\n\n"))
    }
}
