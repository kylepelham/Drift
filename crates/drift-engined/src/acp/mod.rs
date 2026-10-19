//! The Agent Client Protocol on stdio: an editor such as Zed drives Drift sessions through the in-process engine.

mod content;
mod history;
mod rpc;
mod tool;
mod turn;

#[cfg(test)]
mod tests;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use drift_engine::Engine;
use drift_engine::event::Event;
use drift_engine::session::types::{ModelRef, Session, Visibility};
use drift_engine::store::{NewSession, Workspace};
use drift_engine::tool::canonical;
use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, BufReader};

use rpc::{Peer, RpcError};

/// The one major version of the protocol this speaks.
const PROTOCOL_VERSION: u64 = 1;

/// How long a client that hung up waits for the turns it started to stop and their last words to be written.
const DRAIN: Duration = Duration::from_secs(5);

pub(crate) struct Options {
    /// The model new sessions start on; without it a turn needs `model` in drift.json.
    pub(crate) model: Option<ModelRef>,
}

/// Serves until the client closes `input`, then stops the turns it started.
pub(crate) async fn serve(
    engine: Arc<Engine>,
    options: Options,
    input: impl AsyncRead + Unpin,
    output: impl AsyncWrite + Unpin + Send + 'static,
) {
    let (peer, writer) = Peer::new(output);
    let agent = Arc::new(Agent {
        engine,
        peer,
        model: options.model,
        prompts: Mutex::default(),
    });
    let mut lines = BufReader::new(input).lines();

    while let Ok(Some(line)) = lines.next_line().await {
        if line.trim().is_empty() {
            continue;
        }
        match serde_json::from_str::<Value>(&line) {
            Ok(message) => agent.receive(message),
            Err(error) => agent.peer.respond(Value::Null, Err(RpcError::parse(error))),
        }
    }

    agent.hang_up();
    drop(agent);
    let _ = tokio::time::timeout(DRAIN, writer).await;
}

struct Agent {
    engine: Arc<Engine>,
    peer: Peer,
    model: Option<ModelRef>,
    /// The prompt running in each session, by whether the client has cancelled it.
    prompts: Mutex<HashMap<String, Arc<AtomicBool>>>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct NewSessionParams {
    cwd: String,
    #[serde(default)]
    mcp_servers: Vec<Value>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct LoadSessionParams {
    session_id: String,
    cwd: String,
    #[serde(default)]
    mcp_servers: Vec<Value>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PromptParams {
    session_id: String,
    prompt: Vec<Value>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CancelParams {
    session_id: String,
}

impl Agent {
    fn receive(self: &Arc<Self>, message: Value) {
        let Some(method) = message.get("method").and_then(Value::as_str) else {
            self.peer.settle(&message);
            return;
        };
        let params = message.get("params").cloned().unwrap_or(Value::Null);

        match message.get("id").cloned() {
            Some(id) if method == "session/prompt" => self.prompt(id, params),
            Some(id) => self.peer.respond(id, self.call(method, params)),
            None if method == "session/cancel" => {
                if let Ok(params) = parse::<CancelParams>(params) {
                    self.cancel(&params.session_id);
                }
            }
            None => {}
        }
    }

    fn call(&self, method: &str, params: Value) -> Result<Value, RpcError> {
        match method {
            "initialize" => Ok(initialized()),
            // Drift signs in from its app or a provider's environment variable; there is nothing to do here.
            "authenticate" => Ok(json!({})),
            "session/new" => self.new_session(parse(params)?),
            "session/load" => self.load_session(parse(params)?),
            _ => Err(RpcError::method_not_found(method)),
        }
    }

    /// Registers the prompt before it runs, so a cancel read right after it is never missed.
    fn prompt(self: &Arc<Self>, id: Value, params: Value) {
        let started = parse::<PromptParams>(params).and_then(|params| {
            let cancelled = self.begin(&params.session_id)?;
            Ok((params, cancelled))
        });
        let agent = self.clone();

        tokio::spawn(async move {
            let result = match started {
                Ok((params, cancelled)) => {
                    let result = agent.run(&params, &cancelled).await;
                    agent.prompts.lock().unwrap().remove(&params.session_id);
                    result
                }
                Err(error) => Err(error),
            };
            agent.peer.respond(id, result);
        });
    }

    fn begin(&self, session_id: &str) -> Result<Arc<AtomicBool>, RpcError> {
        let mut prompts = self.prompts.lock().unwrap();
        if prompts.contains_key(session_id) {
            return Err(RpcError::invalid_request("the session is already running a prompt"));
        }

        let cancelled = Arc::new(AtomicBool::new(false));
        prompts.insert(session_id.into(), cancelled.clone());
        Ok(cancelled)
    }

    async fn run(&self, params: &PromptParams, cancelled: &AtomicBool) -> Result<Value, RpcError> {
        let (session, cwd) = self.session(&params.session_id)?;
        let turn = turn::Turn::new(self.engine.clone(), self.peer.clone(), &session.id, cwd);
        turn.run(&params.prompt, cancelled).await
    }

    fn cancel(&self, session_id: &str) {
        if let Some(cancelled) = self.prompts.lock().unwrap().get(session_id) {
            cancelled.store(true, Ordering::SeqCst);
            self.engine.abort(session_id);
        }
    }

    /// The client went away: its turns stop, and asks waiting on it are refused.
    fn hang_up(&self) {
        let running: Vec<String> = self.prompts.lock().unwrap().keys().cloned().collect();
        for session_id in running {
            self.cancel(&session_id);
        }
        self.peer.hang_up();
    }

    /// The working directory is a Drift workspace, added when Drift does not have it yet.
    fn new_session(&self, params: NewSessionParams) -> Result<Value, RpcError> {
        let cwd = directory(&params.cwd)?;
        unused_servers(&params.mcp_servers);
        let workspace = self.workspace(&params.cwd, &cwd)?;
        let config = self.engine.workspace_config(&canonical(&cwd));
        let session = self
            .engine
            .store
            .create_session(NewSession {
                workspace_id: &workspace.id,
                parent_id: None,
                visibility: Visibility::Sibling,
                title: "",
                agent: config.default_agent(),
                model: self.model.as_ref(),
            })
            .map_err(RpcError::internal)?;
        self.engine.hub.publish(Event::SessionCreated {
            session: session.clone(),
        });

        Ok(json!({ "sessionId": session.id }))
    }

    fn workspace(&self, path: &str, cwd: &Path) -> Result<Workspace, RpcError> {
        let name = cwd
            .file_name()
            .map_or_else(|| path.to_string(), |name| name.to_string_lossy().into_owned());
        let workspace = self
            .engine
            .store
            .add_workspace(path, &name, "")
            .map_err(RpcError::internal)?;
        self.engine.hub.publish(Event::WorkspaceCreated {
            workspace: workspace.clone(),
        });

        Ok(workspace)
    }

    fn load_session(&self, params: LoadSessionParams) -> Result<Value, RpcError> {
        let cwd = directory(&params.cwd)?;
        unused_servers(&params.mcp_servers);
        let (session, root) = self.session(&params.session_id)?;
        if root != canonical(&cwd) {
            return Err(RpcError::invalid_params(format!(
                "the session belongs to {}",
                root.display()
            )));
        }

        history::replay(&self.engine, &self.peer, &session.id, &root)?;
        Ok(json!({}))
    }

    /// The session and its workspace's directory.
    fn session(&self, session_id: &str) -> Result<(Session, PathBuf), RpcError> {
        let store = &self.engine.store;
        let session = store
            .session(session_id)
            .map_err(RpcError::internal)?
            .ok_or_else(|| RpcError::invalid_params("no session by that id"))?;
        let workspace = store
            .workspace(&session.workspace_id)
            .map_err(RpcError::internal)?
            .ok_or_else(|| RpcError::invalid_params("the session's workspace is gone"))?;

        Ok((session, canonical(Path::new(&workspace.path))))
    }
}

fn initialized() -> Value {
    json!({
        "protocolVersion": PROTOCOL_VERSION,
        "agentCapabilities": {
            "loadSession": true,
            "promptCapabilities": { "image": true, "audio": false, "embeddedContext": true },
            "mcpCapabilities": { "http": false, "sse": false },
        },
        "agentInfo": { "name": "drift", "title": "Drift", "version": drift_engine::VERSION },
        "authMethods": [],
    })
}

fn parse<T: DeserializeOwned>(params: Value) -> Result<T, RpcError> {
    serde_json::from_value(params).map_err(|error| RpcError::invalid_params(error.to_string()))
}

/// ACP paths are absolute; a session's directory must exist.
fn directory(path: &str) -> Result<PathBuf, RpcError> {
    let path = PathBuf::from(path);
    if !path.is_absolute() || !path.is_dir() {
        return Err(RpcError::invalid_params(format!(
            "cwd must be an absolute path to a directory: {}",
            path.display()
        )));
    }

    Ok(path)
}

/// The editor's MCP servers are not connected; Drift's own config says which servers a workspace has.
fn unused_servers(servers: &[Value]) {
    if !servers.is_empty() {
        eprintln!(
            "drift: acp ignores the {} MCP servers the editor offered; add them to drift.json",
            servers.len()
        );
    }
}

fn chunk(kind: &str, message_id: &str, text: &str) -> Value {
    json!({ "sessionUpdate": kind, "messageId": message_id, "content": { "type": "text", "text": text } })
}

/// Reasoning is its own message beside the reply it belongs to.
fn thought(message_id: &str, text: &str) -> Value {
    chunk("agent_thought_chunk", &format!("{message_id}:thought"), text)
}
