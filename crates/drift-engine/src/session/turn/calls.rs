use crate::session::changes::Capture;
use crate::tool::{Output, Tool, ToolError};
use serde_json::Value;

use super::*;

struct PreparedCall {
    context: Context,
    name: String,
    tool: Arc<dyn Tool>,
    input: Value,
    rewritten: bool,
}

struct CallResult {
    status: ToolStatus,
    title: Option<String>,
    text: String,
    metadata: ToolMetadata,
}

impl Engine {
    pub(super) async fn run_call(self: &Arc<Self>, scope: &CallScope<'_>, mut row: PartRow) -> Outcome {
        let mut call = match self.prepare_call(scope, &mut row).await {
            Ok(call) => call,
            Err(outcome) => return outcome,
        };
        let hooked = (!self.hooks.is_empty()).then(|| call.input.clone());
        let writes = call.tool.call_mutates(&call.input);
        let touches = writes.then(|| call.tool.touches(&call.context, &call.input));

        let paths = touches.as_ref().and_then(Option::as_deref);
        let _turn = match self.lock_call(scope, &mut row, paths).await {
            Ok(turn) => turn,
            Err(outcome) => return outcome,
        };
        for ask in call.tool.asks(&call.context, &call.input) {
            if let Some(refused) = self
                .permit(scope, &mut row, &call.context.call_id, &call.name, ask)
                .await
            {
                return refused;
            }
        }
        let capture = match self.before_write(scope, &mut row, touches).await {
            Ok(capture) => capture,
            Err(outcome) => return outcome,
        };

        if !self.record_running_call(&mut row, &mut call) {
            return Outcome::Allowed;
        }

        let result = self.invoke_call(scope, &call).await;
        let result = self.call_result(scope, &call, result, writes).await;
        let failed = result.status == ToolStatus::Error || result.metadata.exit.is_some_and(|code| code != 0);
        let (text, metadata) = self
            .hook_after(
                scope,
                AfterTool {
                    name: &call.name,
                    input: hooked,
                    failed,
                    text: result.text,
                    metadata: result.metadata,
                },
            )
            .await;
        let result = CallResult {
            text,
            metadata,
            ..result
        };
        self.finish_call(scope, &mut row, result, capture).await;

        if writes {
            scope.wrote.lock().unwrap().note(&row);
        }

        if scope.abort.is_cancelled() {
            Outcome::Aborted
        } else {
            Outcome::Allowed
        }
    }

    async fn prepare_call(self: &Arc<Self>, scope: &CallScope<'_>, row: &mut PartRow) -> Result<PreparedCall, Outcome> {
        let Part::ToolCall {
            call_id,
            name,
            input,
            metadata,
            ..
        } = row.part.clone()
        else {
            return Err(Outcome::Allowed);
        };
        let command_model = metadata
            .as_ref()
            .filter(|metadata| metadata.engine_command.is_some())
            .and_then(|metadata| metadata.command_model.as_deref())
            .and_then(crate::config::parse_model);
        let context = Context {
            workspace: scope.plan.workspace.clone(),
            session_id: scope.plan.session.id.clone(),
            agent: scope.plan.session.agent.clone(),
            message_id: scope.message.id.clone(),
            call_id,
            files: scope.files.clone(),
            abort: scope.abort.clone(),
            engine: self.clone(),
            config: scope.plan.config.clone(),
            progress: Default::default(),
            command_model,
        };

        // Only the tools offered to this turn may run; a unique case-insensitive name is accepted.
        let Some((name, tool)) = scope.plan.offer.tool_named(&name) else {
            let reason = format!("`{name}` is not available in this session; use only the tools you were given");
            self.settle(row, ToolStatus::Error, None, reason, None);
            return Err(Outcome::Allowed);
        };
        if let Part::ToolCall { name: stored, .. } = &mut row.part {
            stored.clone_from(&name);
        }
        if let Some(reason) = invalid_input(tool.as_ref(), &input) {
            self.settle(row, ToolStatus::Error, None, reason, None);
            return Err(Outcome::Allowed);
        }

        // Read-only agents are checked before plugins rewrite arguments, as before permission asks.
        let agent = &scope.plan.session.agent;
        let read_only = scope.plan.config.agent(agent).is_some_and(|agent| agent.read_only);
        if read_only && !tool.stays_read_only(&context, &input) {
            let reason = format!(
                "The {agent} agent only reads, so this call was not run: it would change something. Use read-only commands and tools, or hand the work to a read-only subagent such as explore."
            );
            self.settle(row, ToolStatus::Error, None, reason, None);
            return Err(Outcome::Allowed);
        }

        let (input, rewritten) = self
            .hook_before(
                scope,
                row,
                BeforeTool {
                    name: &name,
                    schema: &tool.spec().input_schema,
                    input,
                },
            )
            .await?;

        Ok(PreparedCall {
            context,
            name,
            tool,
            input,
            rewritten,
        })
    }

    fn record_running_call(self: &Arc<Self>, row: &mut PartRow, call: &mut PreparedCall) -> bool {
        let running = call.tool.running_metadata(&call.context, &call.input);
        if let (Some(running), Part::ToolCall { metadata, .. }) = (running, &mut row.part) {
            let previous = metadata.take().map(|metadata| *metadata);
            *metadata = running.merged(previous).map(Box::new);
        }

        if let Err(error) = self.start_call(row) {
            let reason = format!("refused to run: could not record the call ({error})");
            self.settle(row, ToolStatus::Error, None, reason, None);
            return false;
        }

        call.context.progress = self.progress_for(row);
        true
    }

    async fn invoke_call(&self, scope: &CallScope<'_>, call: &PreparedCall) -> Result<Output, ToolError> {
        // A streamed read used the original arguments, so a plugin rewrite cannot reuse it.
        let started = scope
            .early
            .lock()
            .unwrap()
            .take(&call.context.call_id)
            .filter(|_| !call.rewritten);

        match started {
            Some(started) => tokio::select! {
                result = started.finish(scope.files) => result,
                () = scope.abort.cancelled() => Err(ToolError("Aborted.".into())),
            },
            None if call.tool.stops_itself() => call.tool.run(&call.context, call.input.clone()).await,
            None => tokio::select! {
                result = call.tool.run(&call.context, call.input.clone()) => result,
                () = scope.abort.cancelled() => Err(ToolError("Aborted.".into())),
            },
        }
    }

    async fn call_result(
        &self,
        scope: &CallScope<'_>,
        call: &PreparedCall,
        result: Result<Output, ToolError>,
        writes: bool,
    ) -> CallResult {
        let output = match result {
            Ok(output) => output,
            Err(error) => {
                return CallResult {
                    status: ToolStatus::Error,
                    title: None,
                    text: error.0,
                    metadata: ToolMetadata::null(),
                };
            }
        };
        let status = if call.tool.failed(&output) {
            ToolStatus::Error
        } else {
            ToolStatus::Done
        };
        let title = Some(output.title);
        let (text, metadata) = if writes {
            self.after_write(scope, &call.context.call_id, output.output, output.metadata)
                .await
        } else {
            (output.output, output.metadata)
        };

        CallResult {
            status,
            title,
            text,
            metadata,
        }
    }

    async fn finish_call(
        self: &Arc<Self>,
        scope: &CallScope<'_>,
        row: &mut PartRow,
        result: CallResult,
        capture: Option<Capture>,
    ) {
        let call_id = call_id_of(row);
        let (mut metadata, text) = self.keep_images(&scope.message.id, result.metadata, result.text).await;
        let spill = self
            .data_dir
            .join("tool-output")
            .join(&scope.plan.session.id)
            .join(format!("{call_id}.result.log"));
        let (text, spilled) = crate::tool::spool::bound(text, spill);
        if let Some(file) = spilled {
            let spill_metadata = ToolMetadata {
                result_file: Some(file.to_string_lossy().into_owned()),
                ..Default::default()
            };
            metadata = metadata.merged(Some(spill_metadata)).unwrap_or_default();
        }

        // Failed and stopped calls may still have written, so history is recorded after formatting too.
        let (status, mut text, changes) = match capture {
            Some(capture) => self.history_of(scope, capture, result.status, text).await,
            None => (result.status, text, None),
        };
        if let Some(note) = changes.as_ref().and_then(|history| history.history_error.as_deref()) {
            crate::tool::add_note(&mut text, &mut metadata, note);
        }

        // Delivery is acknowledged in the same write as the result, only while this call holds its claim.
        let claimant = Claimant::call(&scope.plan.session.id, call_id);
        let delivers = metadata
            .delivers
            .as_deref()
            .filter(|task| self.workers.holds(task, &claimant))
            .map(str::to_owned);
        self.settle_delivering(
            row,
            status,
            result.title,
            text,
            metadata.merged(changes),
            delivers.as_deref(),
        );
        self.release_claims(&claimant);
    }
}

fn invalid_input(tool: &dyn Tool, input: &Value) -> Option<String> {
    if !input.is_object() {
        return Some(unparsed(input));
    }

    let problems = crate::tool::schema::problems(&tool.spec().input_schema, input);
    if problems.is_empty() {
        return None;
    }

    Some(format!(
        "The call did not run: {}. Send it again with arguments that fit the tool's schema.",
        problems.join("; ")
    ))
}
