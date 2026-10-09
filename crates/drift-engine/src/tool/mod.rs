//! Tools the model can call. Each one declares its schema, its permission and how to run.

pub mod apply_patch;
mod ask;
pub mod bash;
pub mod command;
mod context;
pub mod edit;
pub mod glob;
pub mod grep;
pub mod image;
pub(crate) mod lock;
pub mod patch;
mod paths;
pub mod question;
pub mod read;
mod registry;
pub mod schema;
pub mod sensitive;
pub mod skill;
pub mod spool;
pub(crate) mod stage;
pub mod task;
mod text;
pub mod todo;
pub mod webfetch;
pub mod write;

#[cfg(test)]
pub(crate) mod tests;

pub use crate::session::types::ToolMetadata;
pub use ask::{Ask, MAX_ASK_DIFF, Reason};
pub use context::{Context, Progress, SessionFiles, read_ask};
pub use paths::{FileGlob, canonical, display, scratch_dir, walk, walker};
pub use registry::Registry;

use crate::llm::ToolSpec;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use utoipa::ToSchema;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct Output {
    pub title: String,
    pub output: String,
    #[serde(default = "ToolMetadata::null", skip_serializing_if = "ToolMetadata::is_null")]
    pub metadata: ToolMetadata,
}

/// Anything that goes back to the model as an error result. Text is written for the model.
#[derive(Debug, PartialEq)]
pub struct ToolError(pub String);

pub type RunFuture<'a> = Pin<Box<dyn Future<Output = Result<Output, ToolError>> + Send + 'a>>;

impl Output {
    pub fn new(title: impl Into<String>, output: impl Into<String>) -> Self {
        Self {
            title: title.into(),
            output: output.into(),
            metadata: ToolMetadata::null(),
        }
    }
}

impl<E: std::fmt::Display> From<E> for ToolError {
    fn from(error: E) -> Self {
        Self(error.to_string())
    }
}

/// Adds Drift's own remark about a call: after its output for the model, and in `metadata.notes` for the UI to show beneath.
pub fn add_note(output: &mut String, metadata: &mut ToolMetadata, note: &str) {
    if !output.is_empty() {
        output.push_str("\n\n");
    }
    output.push_str(note);

    if metadata.legacy.is_some() {
        *metadata = ToolMetadata::default();
    }
    metadata.extra.remove("notes");
    metadata.notes.get_or_insert_default().push(note.into());
}

pub trait Tool: Send + Sync {
    fn spec(&self) -> ToolSpec;

    /// The MCP server a tool comes from; built-ins have none.
    fn server(&self) -> Option<&str> {
        None
    }

    /// `None` means the call needs no permission at all.
    fn ask(&self, ctx: &Context, input: &Value) -> Option<Ask>;

    /// Everything the call must be allowed, each judged on its own; any refusal refuses the call.
    fn asks(&self, ctx: &Context, input: &Value) -> Vec<Ask> {
        let ask = self.ask(ctx, input).unwrap_or_else(|| {
            Ask::new(&self.spec().name, "*", format!("Use {}", self.spec().name)).allow_by_default()
        });

        vec![ask]
    }

    /// Whether this tool can write outside memory, so what its calls change is recorded for undo.
    fn mutates(&self) -> bool {
        false
    }

    /// Whether this particular call may write; a tool that can tell a call that only reads says so here.
    fn call_mutates(&self, _input: &Value) -> bool {
        self.mutates()
    }

    /// Whether a read-only agent may make this call: nothing it does, or sets going, changes anything.
    fn stays_read_only(&self, _ctx: &Context, input: &Value) -> bool {
        !self.call_mutates(input)
    }

    /// The files a writing call will change, when it can say up front. `None` means anything might
    /// change, so the workspace is compared before and after instead.
    fn touches(&self, _ctx: &Context, _input: &Value) -> Option<Vec<PathBuf>> {
        None
    }

    /// A result that still reports failure, for tools whose failures carry metadata the UI needs.
    fn failed(&self, _output: &Output) -> bool {
        false
    }

    /// Metadata to show while the call runs, before its result exists.
    fn running_metadata(&self, _ctx: &Context, _input: &Value) -> Option<ToolMetadata> {
        None
    }

    /// The call returns promptly by itself once `ctx.abort` fires, with a result worth keeping
    /// (partial output). Otherwise a stop drops the call and records it as aborted.
    fn stops_itself(&self) -> bool {
        false
    }

    /// May run speculatively while streaming; the ordering and result rules are in docs/engine-rewrite.md.
    fn starts_early(&self) -> bool {
        false
    }

    /// The permission kinds every call of this tool is judged under; none means its own name, as `asks` judges it.
    fn permissions(&self) -> &'static [&'static str] {
        &[]
    }

    /// Whether the rules refuse every call this tool could make, so it is not offered at all, as opencode leaves such a tool out.
    fn denied_outright(&self, rules: &crate::permission::Compiled) -> bool {
        match self.permissions() {
            [] => rules.denies_all(&self.spec().name),
            kinds => kinds.iter().any(|kind| rules.denies_all(kind)),
        }
    }

    fn run<'a>(&'a self, ctx: &'a Context, input: Value) -> RunFuture<'a>;
}

pub(crate) fn required_str<'a>(input: &'a Value, key: &str) -> Result<&'a str, ToolError> {
    input[key]
        .as_str()
        .filter(|value| !value.is_empty())
        .ok_or_else(|| ToolError(format!("`{key}` is required")))
}

/// Refuses, before anything is written, a file too large for undo to keep: it could never be put back.
fn fits_history(name: &str, bytes: usize) -> Result<(), ToolError> {
    let limit = crate::session::snapshot::MAX_RECORDED_BYTES;
    if bytes as u64 > limit {
        return Err(ToolError(format!(
            "{name} would be {} MB, over the {} MB undo can keep, so it was not written",
            bytes / 1024 / 1024,
            limit / 1024 / 1024
        )));
    }

    Ok(())
}
