//! Tools the model can call. Each one declares its schema, its permission and how to run.

pub mod bash;
pub mod edit;
pub mod glob;
pub mod grep;
pub mod question;
pub mod read;
pub mod todo;
pub mod webfetch;
pub mod write;

use std::collections::HashSet;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio_util::sync::CancellationToken;
use utoipa::ToSchema;

use crate::llm::ToolSpec;

/// Shared across one session: which files the model has read, so edits are never blind.
#[derive(Default)]
pub struct SessionFiles {
    read: Mutex<HashSet<PathBuf>>,
}

impl SessionFiles {
    pub fn mark_read(&self, path: &Path) {
        self.read.lock().unwrap().insert(path.to_path_buf());
    }

    pub fn was_read(&self, path: &Path) -> bool {
        self.read.lock().unwrap().contains(path)
    }
}

pub struct Context {
    pub workspace: PathBuf,
    pub session_id: String,
    pub message_id: String,
    pub call_id: String,
    pub files: Arc<SessionFiles>,
    pub abort: CancellationToken,
    /// Session-level tools (todos, questions) read and write through the engine.
    pub engine: Arc<crate::Engine>,
}

impl Context {
    /// Relative paths are relative to the workspace; the result is absolute but not canonicalised.
    pub fn resolve(&self, path: &str) -> PathBuf {
        let path = Path::new(path);
        if path.is_absolute() {
            path.to_path_buf()
        } else {
            self.workspace.join(path)
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct Output {
    pub title: String,
    pub output: String,
    #[serde(default, skip_serializing_if = "Value::is_null")]
    pub metadata: Value,
}

impl Output {
    pub fn new(title: impl Into<String>, output: impl Into<String>) -> Self {
        Self { title: title.into(), output: output.into(), metadata: Value::Null }
    }
}

/// Anything that goes back to the model as an error result. Text is written for the model.
#[derive(Debug, PartialEq)]
pub struct ToolError(pub String);

impl<E: std::fmt::Display> From<E> for ToolError {
    fn from(error: E) -> Self {
        Self(error.to_string())
    }
}

/// What a call wants to do, for the permission service to judge before it runs.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Ask {
    /// `read`, `edit`, `bash`, ...: the rule namespace.
    pub kind: String,
    /// The thing being touched: a path, a command. Rules match it with globs.
    pub pattern: String,
    pub title: String,
}

pub type RunFuture<'a> = Pin<Box<dyn Future<Output = Result<Output, ToolError>> + Send + 'a>>;

pub trait Tool: Send + Sync {
    fn spec(&self) -> ToolSpec;
    /// `None` means the call needs no permission at all.
    fn ask(&self, ctx: &Context, input: &Value) -> Option<Ask>;
    /// Whether this call writes outside memory, so the session is snapshotted first.
    fn mutates(&self) -> bool {
        false
    }
    fn run<'a>(&'a self, ctx: &'a Context, input: Value) -> RunFuture<'a>;
}

pub struct Registry {
    tools: Vec<Box<dyn Tool>>,
}

impl Registry {
    pub fn builtin() -> Self {
        Self {
            tools: vec![
                Box::new(read::Read),
                Box::new(write::Write),
                Box::new(edit::Edit),
                Box::new(bash::Bash::detect()),
                Box::new(glob::Glob),
                Box::new(grep::Grep),
                Box::new(webfetch::WebFetch),
                Box::new(todo::TodoWrite),
                Box::new(question::Question),
            ],
        }
    }

    pub fn specs(&self) -> Vec<ToolSpec> {
        self.tools.iter().map(|tool| tool.spec()).collect()
    }

    pub fn get(&self, name: &str) -> Option<&dyn Tool> {
        self.tools.iter().find(|tool| tool.spec().name == name).map(|tool| tool.as_ref())
    }
}

fn required_str<'a>(input: &'a Value, key: &str) -> Result<&'a str, ToolError> {
    input[key]
        .as_str()
        .filter(|value| !value.is_empty())
        .ok_or_else(|| ToolError(format!("`{key}` is required")))
}

fn display(path: &Path, workspace: &Path) -> String {
    path.strip_prefix(workspace)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub(crate) struct Sandbox {
        pub ctx: Context,
    }

    impl Sandbox {
        pub(crate) fn new(name: &str) -> Self {
            let _ = rustls::crypto::ring::default_provider().install_default();
            let root = std::env::temp_dir().join(format!("drift-tool-{name}-{}", crate::random_hex(4)));
            let workspace = root.join("ws");
            std::fs::create_dir_all(&workspace).unwrap();
            let engine = crate::Engine::open_with(&root.join("data"), crate::Options { file_credentials: true, ..Default::default() }).unwrap();
            Self {
                ctx: Context {
                    workspace,
                    session_id: "ses_test".into(),
                    message_id: "msg_test".into(),
                    call_id: "call_test".into(),
                    files: Arc::new(SessionFiles::default()),
                    abort: CancellationToken::new(),
                    engine,
                },
            }
        }

        pub(crate) fn ctx_clone(&self) -> Context {
            Context {
                workspace: self.ctx.workspace.clone(),
                session_id: self.ctx.session_id.clone(),
                message_id: self.ctx.message_id.clone(),
                call_id: self.ctx.call_id.clone(),
                files: self.ctx.files.clone(),
                abort: self.ctx.abort.clone(),
                engine: self.ctx.engine.clone(),
            }
        }

        pub(crate) fn file(&self, path: &str, content: &str) -> PathBuf {
            let full = self.ctx.workspace.join(path);
            std::fs::create_dir_all(full.parent().unwrap()).unwrap();
            std::fs::write(&full, content).unwrap();
            full
        }
    }

    impl Drop for Sandbox {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(self.ctx.workspace.parent().unwrap());
        }
    }

    #[test]
    fn registry_exposes_every_builtin_with_a_schema() {
        let registry = Registry::builtin();
        let names: Vec<String> = registry.specs().into_iter().map(|spec| spec.name).collect();
        assert_eq!(names, ["read", "write", "edit", "bash", "glob", "grep", "webfetch", "todowrite", "question"]);
        for spec in registry.specs() {
            assert_eq!(spec.input_schema["type"], "object", "{}", spec.name);
            assert!(!spec.description.is_empty(), "{}", spec.name);
        }
        assert!(registry.get("read").is_some());
        assert!(registry.get("nope").is_none());
    }
}
