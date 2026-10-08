use super::*;

pub(in crate::session) struct Settlement {
    pub status: ToolStatus,
    pub title: Option<String>,
    pub text: String,
    pub metadata: Option<ToolMetadata>,
}

impl Settlement {
    pub(in crate::session) fn new(
        status: ToolStatus,
        title: Option<String>,
        text: String,
        metadata: Option<ToolMetadata>,
    ) -> Self {
        Self {
            status,
            title,
            text,
            metadata,
        }
    }

    pub(super) fn error(text: String) -> Self {
        Self::new(ToolStatus::Error, None, text, None)
    }

    pub(super) fn denied(text: String) -> Self {
        Self::new(ToolStatus::Denied, None, text, None)
    }
}

impl Engine {
    /// Persists the message's terminal state. On failure the published state is an error, and the caller stops.
    pub(super) fn finish(&self, message: &mut Message) -> rusqlite::Result<()> {
        message.finished_at = Some(id::now_ms());
        let saved = self
            .store
            .save_message(message)
            .and_then(|()| self.store.touch_session(&message.session_id));

        // The sidebar orders by last activity, so a reply landing moves its conversation up.
        if let (Ok(()), Ok(Some(session))) = (&saved, self.store.session(&message.session_id)) {
            self.hub.publish(Event::SessionUpdated { session });
        }
        if let Err(error) = &saved {
            message.status = MessageStatus::Error;
            message.error = Some(format!("response was not persisted ({error})"));
            let _ = self.store.save_message(message);
        }

        self.hub.publish(Event::MessageUpdated {
            message: message.clone(),
        });
        saved
    }

    /// Publishes a running call's part with what it reports merged into its metadata; nothing is stored.
    pub(super) fn progress_for(self: &Arc<Self>, row: &PartRow) -> crate::tool::Progress {
        let engine = Arc::downgrade(self);
        let running = Mutex::new(row.clone());

        crate::tool::Progress::new(move |patch| {
            let Some(engine) = engine.upgrade() else { return };
            let mut row = running.lock().unwrap();
            if let Part::ToolCall { metadata, .. } = &mut row.part {
                let current = metadata.take().unwrap_or_default();
                *metadata = current.merged(Some(patch)).map(Box::new);
            }

            engine.hub.publish_transient(Event::PartUpdated { part: row.clone() });
        })
    }

    /// Marks the call running in storage before it does anything; a call that cannot be recorded does not run.
    pub(super) fn start_call(&self, row: &mut PartRow) -> rusqlite::Result<()> {
        if let Part::ToolCall { status, started_at, .. } = &mut row.part {
            *status = ToolStatus::Running;
            *started_at = Some(id::now_ms());
        }

        self.store.save_part(row)?;
        self.hub.publish(Event::PartUpdated { part: row.clone() });
        Ok(())
    }

    /// Ends the turn by itself, visibly: a reply-less message whose `error` is the reason.
    /// After the wrap-up reply a conversation pauses with the reason; a subagent's reply is its result.
    pub(super) fn end_wrap_up(&self, plan: &Plan, wrapping: Option<WrapUp>) {
        if let Some(wrap_up) = wrapping.filter(|_| plan.session.visibility != Visibility::Hidden) {
            self.pause(plan, wrap_up.pause_reason());
        }
    }

    pub(super) fn pause(&self, plan: &Plan, reason: String) {
        let Ok(mut message) = self
            .store
            .create_reply(&plan.session.id, &plan.model_ref, &plan.session.agent)
        else {
            return;
        };

        self.hub.publish(Event::MessageCreated {
            message: message.clone(),
        });
        message.status = MessageStatus::Paused;
        message.error = Some(reason);
        let _ = self.finish(&mut message);
    }

    /// The calls the session's latest reply made, with their inputs and results.
    pub(super) fn last_calls(&self, session_id: &str) -> Vec<CallTrace> {
        let Ok(Some(last)) = self.store.last_reply(session_id) else {
            return Vec::new();
        };

        last.parts
            .iter()
            .filter_map(|row| match &row.part {
                Part::ToolCall {
                    name, input, output, ..
                } => Some(CallTrace {
                    name: name.clone(),
                    input: input.to_string(),
                    output: output.clone().unwrap_or_default(),
                }),
                _ => None,
            })
            .collect()
    }

    /// Closes the calls a message made that will never run, with the reason, so none stays pending.
    pub(super) fn settle_unrun(&self, message: &Message, reason: &str) {
        let Ok(Some(found)) = self.store.with_parts(&message.id) else {
            return;
        };

        for mut row in found.parts {
            if matches!(
                row.part,
                Part::ToolCall {
                    status: ToolStatus::Pending,
                    ..
                }
            ) {
                self.settle(&mut row, Settlement::error(format!("Not run: {reason}")));
            }
        }
    }

    /// Writes the outcome. If that write fails, what is published is the failure, never a success the store lacks.
    pub(super) fn settle(&self, row: &mut PartRow, result: Settlement) {
        self.settle_delivering(row, result, None);
    }

    /// [`Self::settle`] that also marks `delivers` handed over in the same write; a failed write leaves it owed.
    pub(in crate::session) fn settle_delivering(&self, row: &mut PartRow, result: Settlement, delivers: Option<&str>) {
        let Settlement {
            status: new_status,
            title: new_title,
            text,
            metadata: meta,
        } = result;

        if let Part::ToolCall {
            status,
            title,
            output,
            metadata,
            finished_at,
            ..
        } = &mut row.part
        {
            *status = new_status;
            *title = new_title.or(title.take());
            *output = Some(text);

            let command = metadata
                .as_ref()
                .and_then(|meta| meta.engine_command.as_deref())
                .map(str::to_string);
            let mut value = meta.unwrap_or_else(ToolMetadata::null);
            value.engine_command = None;
            value.extra.remove("engineCommand");
            if let Some(command) = command {
                if value.legacy.is_some() {
                    value = ToolMetadata::default();
                }
                value.engine_command = Some(command);
            }
            *metadata = (!value.is_null()).then(|| Box::new(value));
            *finished_at = Some(id::now_ms());
        }

        let saved = match delivers {
            Some(task) => self.store.save_part_delivering(row, task).map(|_| ()),
            None => self.store.save_part(row),
        };
        if let Err(error) = &saved {
            if let Part::ToolCall { status, output, .. } = &mut row.part {
                *status = ToolStatus::Error;
                *output = Some(format!("result was not persisted ({error}); treat this call as failed"));
            }
            let _ = self.store.save_part(row);
        }

        self.hub.publish(Event::PartUpdated { part: row.clone() });
        if let (Some(task), Ok(())) = (delivers, saved) {
            self.publish_task(task);
        }
    }
}
