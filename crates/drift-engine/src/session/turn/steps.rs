use super::*;

impl Engine {
    /// Runs a command's calls (a skill, a delegated task or its shell lines) before the model answers.
    /// They share one message, and each passes the same permission check as the model's own calls.
    pub(super) async fn run_bootstrap(self: &Arc<Self>, plan: &mut Plan, abort: &CancellationToken) -> bool {
        let bootstraps = std::mem::take(&mut plan.bootstrap);
        if bootstraps.is_empty() {
            return true;
        }

        let prepared = self.record_bootstrap(plan, bootstraps);
        let (message, rows) = match prepared {
            Ok(prepared) => prepared,
            Err(error) => {
                self.pause(plan, format!("could not record command execution: {error}"));
                return false;
            }
        };
        let batch = CallBatch {
            rows,
            early: early::Early::new(abort),
        };

        self.run_calls(plan, &message, batch, abort).await != Outcome::Aborted
    }

    fn record_bootstrap(
        &self,
        plan: &Plan,
        bootstraps: Vec<command::Bootstrap>,
    ) -> rusqlite::Result<(Message, Vec<PartRow>)> {
        let mut message = self
            .store
            .create_reply(&plan.session.id, &plan.model_ref, &plan.session.agent)?;
        self.hub.publish(Event::MessageCreated {
            message: message.clone(),
        });

        let mut rows = Vec::new();
        for bootstrap in bootstraps {
            let mut metadata = ToolMetadata {
                engine_command: Some(bootstrap.command),
                ..Default::default()
            };
            if let Some(model) = &bootstrap.model {
                metadata.command_model = Some(format!("{}/{}", model.provider, model.model));
            }

            let part = Part::ToolCall {
                call_id: id::new("call"),
                name: bootstrap.tool,
                input: bootstrap.input,
                status: ToolStatus::Pending,
                title: None,
                output: None,
                metadata: Some(Box::new(metadata)),
                started_at: None,
                finished_at: None,
            };

            let row = self.store.add_part(&message.id, &plan.session.id, part)?;
            self.hub.publish(Event::PartCreated { part: row.clone() });
            rows.push(row);
        }

        message.status = MessageStatus::Done;
        self.finish(&mut message)?;
        Ok((message, rows))
    }

    /// One run of model steps; returns the newest prompt the last request included.
    pub(super) async fn run_steps(
        self: &Arc<Self>,
        plan: &mut Plan,
        abort: &CancellationToken,
        started: Option<&str>,
    ) -> Option<String> {
        let mut attempts = 0;
        let mut recovered = false;
        let mut steps = 0;
        let mut repeats = Repeats::default();
        let mut answered = None;
        let mut wrapping = None;

        loop {
            if let Err(reason) = self.follow_session(plan).await {
                self.pause(plan, reason.to_string());
                break;
            }

            let limits = plan.config.limits_for(&plan.session.agent);
            if wrapping.is_none() && steps + 1 >= limits.steps {
                wrapping = Some(WrapUp::Steps(limits.steps));
            }
            let Some(transcript) = self.transcript_for_step(plan, abort).await else {
                break;
            };

            let wrap_up = wrapping.map(WrapUp::instruction);
            let request;
            (request, answered) = self.step_request(plan, transcript, started, wrap_up);
            let Ok(message) = self
                .store
                .create_reply(&plan.session.id, &plan.model_ref, &plan.session.agent)
            else {
                break;
            };

            self.hub.publish(Event::MessageCreated {
                message: message.clone(),
            });
            let reply = message.id.clone();

            match self.step(plan, message, &request, abort).await {
                Step::Done | Step::Continue if wrapping.is_some() => {
                    self.end_wrap_up(plan, wrapping);
                    break;
                }
                Step::Done => break,
                Step::Continue => {
                    attempts = 0;
                    steps += 1;
                    wrapping = repeats
                        .record(self.last_calls(&plan.session.id), &limits)
                        .map(WrapUp::Repeats);
                }
                Step::Retry(retry) if retry.allowed(attempts) => {
                    attempts += 1;
                    match self.wait_to_retry(&plan.session.id, attempts, &retry, abort).await {
                        Wait::Elapsed => {}
                        Wait::Switched(switch) => {
                            self.adopt(plan, *switch);
                            attempts = 0;
                        }
                        Wait::Stopped => break,
                    }
                }
                Step::Retry(_) => break,
                Step::Overflow if !recovered => {
                    recovered = true;
                    if !self.recover_from_overflow(&plan.session.id, reply, abort).await {
                        break;
                    }
                }
                Step::Overflow => break,
            }
        }

        answered
    }

    /// Compacts an overflowing request and discards an empty refused reply once recovery succeeds.
    async fn recover_from_overflow(
        self: &Arc<Self>,
        session_id: &str,
        reply: String,
        abort: &CancellationToken,
    ) -> bool {
        if self.compact(session_id, Trigger::Overflow, abort).await.is_err() {
            return false;
        }

        if self.store.discard_empty_reply(&reply).unwrap_or(false) {
            self.hub.publish(Event::MessageRemoved {
                session_id: session_id.into(),
                message_id: reply,
            });
        }

        true
    }

    /// The transcript for the next request, compacted first when the last reply left too little room.
    /// A failed compaction still lets the request go; if it is too long, the overflow path tries once more.
    async fn transcript_for_step(
        self: &Arc<Self>,
        plan: &Plan,
        abort: &CancellationToken,
    ) -> Option<Vec<MessageWithParts>> {
        let transcript = self.request_window(&plan.session.id)?;
        if !self.wants_compaction(&plan.session.id, &plan.model, &transcript) {
            return Some(transcript);
        }

        let _ = self.compact(&plan.session.id, Trigger::Auto, abort).await;
        if abort.is_cancelled() {
            return None;
        }

        self.request_window(&plan.session.id)
    }

    /// One assistant message and the tool calls it makes.
    async fn step(
        self: &Arc<Self>,
        plan: &mut Plan,
        mut message: Message,
        request: &Request,
        abort: &CancellationToken,
    ) -> Step {
        let streamed = match self.stream(&message, plan, request, abort).await {
            Ok(streamed) => streamed,
            Err(error) => return self.failed_step(&mut message, error),
        };
        message.usage = streamed.usage;
        message.cost = cost(&plan.model, streamed.usage);
        message.status = MessageStatus::Done;

        // An incomplete reply cannot supply trustworthy tool arguments.
        let ending = match streamed.stop {
            StopReason::MaxTokens => Some((
                format!("{OUTPUT_LIMIT_ENDING} ({} tokens).", request.max_tokens),
                "the reply hit its output limit, so this call's input may be cut short.",
            )),
            StopReason::Refused => Some((
                REFUSED_ENDING.to_string(),
                "the provider's safety filter ended the reply before this call ran.",
            )),
            StopReason::ContextFull => {
                message.status = MessageStatus::Error;
                Some((
                    "The reply ran into the end of the context window.".to_string(),
                    "the reply ran out of context, so this call's input may be cut short.",
                ))
            }
            _ => None,
        };
        if let Some((error, _)) = &ending {
            message.error = Some(error.clone());
        }
        message.ending = match streamed.stop {
            StopReason::MaxTokens => Some(types::Ending::Length),
            StopReason::Refused => Some(types::Ending::Refused),
            _ if request.no_tool_calls => Some(types::Ending::Limit),
            _ => None,
        };
        if self.finish(&mut message).is_err() {
            return Step::Done;
        }

        if let Some((_, unrun)) = ending {
            self.settle_unrun(&message, unrun);
            return if streamed.stop == StopReason::ContextFull {
                Step::Overflow
            } else {
                Step::Done
            };
        }
        if streamed.calls.is_empty() {
            return Step::Done;
        }
        if request.no_tool_calls {
            self.settle_unrun(&message, "tools were off for this reply, so this call was not run.");
            return Step::Done;
        }

        let batch = CallBatch {
            rows: streamed.calls,
            early: streamed.early,
        };
        match self.run_calls(plan, &message, batch, abort).await {
            Outcome::Aborted => {
                self.settle_unrun(&message, "the turn was stopped before this call ran.");
                Step::Done
            }
            _ => Step::Continue,
        }
    }

    fn failed_step(&self, message: &mut Message, error: StreamError) -> Step {
        match error {
            StreamError::Aborted => {
                message.status = MessageStatus::Aborted;
                let _ = self.finish(message);
                self.settle_unrun(message, "the reply was stopped before this call ran.");
                Step::Done
            }
            StreamError::Provider(error) => {
                message.status = MessageStatus::Error;
                message.error = Some(error.to_string());
                let _ = self.finish(message);
                self.settle_unrun(
                    message,
                    "the reply failed before it finished, so its calls were not trusted to run.",
                );
                if error.is_context_overflow() {
                    return Step::Overflow;
                }

                Retry::from(&error).map_or(Step::Done, Step::Retry)
            }
        }
    }

    /// Opens the response. A refused sign-in is renewed once and the request sent again; the turn keeps the new token.
    async fn open_response(&self, plan: &mut Plan, request: &Request) -> Result<llm::ChunkStream, llm::Error> {
        let refused = match plan.provider.stream(request, &plan.credential).await {
            Err(llm::Error::Unauthenticated(words)) if matches!(plan.credential, Credential::OAuth { .. }) => words,
            opened => return opened,
        };
        match self.renew(&plan.model_ref.provider, plan.credential.clone()).await {
            Ok(fresh) => plan.credential = fresh,
            Err(error) => return Err(llm::Error::Unauthenticated(format!("{refused} ({error})"))),
        }

        plan.provider.stream(request, &plan.credential).await
    }

    async fn stream(
        self: &Arc<Self>,
        message: &Message,
        plan: &mut Plan,
        request: &Request,
        abort: &CancellationToken,
    ) -> Result<Streamed, StreamError> {
        let mut chunks = tokio::select! {
            opened = self.open_response(plan, request) => opened.map_err(StreamError::Provider)?,
            () = abort.cancelled() => return Err(StreamError::Aborted),
        };
        let mut assembler = Assembler::new(&self.store, &self.hub, message);
        let files = self.turns.files_for(&self.store, &plan.session.id);
        let mut early = early::Early::new(abort);
        let scope = early::ReadScope {
            engine: self,
            plan,
            message,
            files: &files,
        };

        loop {
            // Only leading reads may start before the reply finishes streaming.
            for row in assembler.calls[early.seen()..]
                .iter()
                .filter(|_| !request.no_tool_calls)
            {
                early.consider(&scope, row);
            }
            let next = tokio::select! {
                chunk = chunks.next() => chunk,
                () = abort.cancelled() => {
                    let _ = assembler.stop_block();
                    return Err(StreamError::Aborted);
                }
            };
            match next {
                Some(Ok(chunk)) => assembler
                    .apply(chunk)
                    .map_err(|error| StreamError::Provider(llm::Error::Transport(error.to_string())))?,
                Some(Err(error)) => {
                    let _ = assembler.stop_block();
                    return Err(StreamError::Provider(error));
                }
                None => break,
            }
        }

        let _ = assembler.stop_block();
        let Some(stop) = assembler.stop else {
            return Err(StreamError::Provider(llm::Error::Transport(
                "stream ended without a stop reason".into(),
            )));
        };

        Ok(Streamed {
            usage: assembler.usage,
            stop,
            calls: assembler.calls,
            early,
        })
    }
}
