//! Speculative leading reads; their ordering and cancellation contract is in docs/engine-rewrite.md.

use std::collections::HashMap;
use std::sync::Arc;

use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use super::turn::Plan;
use super::types::{Message, Part, PartRow};
use crate::Engine;
use crate::tool::{Context, Output, SessionFiles, ToolError};

pub(super) struct Early {
    started: HashMap<String, Started>,
    /// How many of the reply's calls have been considered.
    seen: usize,
    /// Cancelled when the step is done with them: a result nobody takes stops being worked out.
    stop: CancellationToken,
    /// A call that may change something has closed in this reply; nothing after it starts early.
    closed: bool,
}

pub(super) struct Started {
    run: JoinHandle<Result<Output, ToolError>>,
    files: Arc<SessionFiles>,
}

pub(super) struct ReadScope<'a> {
    pub engine: &'a Arc<Engine>,
    pub plan: &'a Plan,
    pub message: &'a Message,
    pub files: &'a SessionFiles,
}

impl Started {
    /// The call's result; what it read now counts as read in the session.
    pub(super) async fn finish(self, files: &SessionFiles) -> Result<Output, ToolError> {
        let result = self
            .run
            .await
            .unwrap_or_else(|failure| Err(ToolError(failure.to_string())));
        if result.is_ok() {
            files.absorb(&self.files);
        }
        result
    }
}

impl Early {
    pub(super) fn new(abort: &CancellationToken) -> Self {
        Self {
            started: HashMap::new(),
            seen: 0,
            stop: abort.child_token(),
            closed: false,
        }
    }

    pub(super) fn seen(&self) -> usize {
        self.seen
    }

    /// Starts `row` now if it may, else leaves it for the step.
    pub(super) fn consider(&mut self, scope: &ReadScope<'_>, row: &PartRow) {
        let ReadScope {
            engine,
            plan,
            message,
            files,
        } = *scope;
        self.seen += 1;
        let Part::ToolCall {
            call_id, name, input, ..
        } = &row.part
        else {
            return;
        };
        let tool = plan.offered(name).filter(|tool| tool.starts_early());
        let Some(tool) = tool.filter(|_| !self.closed) else {
            self.closed = true;
            return;
        };
        if !input.is_object() || !crate::tool::schema::problems(&tool.spec().input_schema, input).is_empty() {
            return;
        }
        let scratch = Arc::new(files.scratch());
        let ctx = Context {
            workspace: plan.workspace.clone(),
            session_id: plan.session.id.clone(),
            agent: plan.session.agent.clone(),
            message_id: message.id.clone(),
            call_id: call_id.clone(),
            files: scratch.clone(),
            abort: self.stop.clone(),
            engine: engine.clone(),
            config: plan.config.clone(),
            progress: Default::default(),
            command_model: None,
        };
        let policy = plan.config.policy();
        let agent_policy = plan.config.agent_policy(&plan.session.agent);
        let allowed = tool.asks(&ctx, input).iter().all(|ask| {
            engine
                .permissions
                .decide_under(&plan.session.id, &policy, &agent_policy, ask)
                == crate::permission::Decision::Allow
        });
        if !allowed {
            return;
        }
        let input = input.clone();
        let stop = self.stop.clone();
        let run = tokio::spawn(async move {
            tokio::select! {
                result = tool.run(&ctx, input) => result,
                () = stop.cancelled() => Err(ToolError("Aborted.".into())),
            }
        });
        self.started.insert(call_id.clone(), Started { run, files: scratch });
    }

    pub(super) fn take(&mut self, call_id: &str) -> Option<Started> {
        self.started.remove(call_id)
    }
}

impl Drop for Early {
    fn drop(&mut self) {
        self.stop.cancel();
    }
}
