use super::*;

const MAX_CONTINUATIONS: u32 = 3;

pub(super) struct BeforeTool<'a> {
    pub name: &'a str,
    pub schema: &'a serde_json::Value,
    pub input: serde_json::Value,
}

pub(super) struct AfterTool<'a> {
    pub name: &'a str,
    pub input: Option<serde_json::Value>,
    pub failed: bool,
    pub text: String,
    pub metadata: ToolMetadata,
}

fn text_parts<'a>(parts: impl Iterator<Item = &'a Part>) -> String {
    parts
        .filter_map(|part| match part {
            Part::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

impl Engine {
    /// Plugins see the user's own prompts before the model does: one may refuse it, rewrite its text or add context beside it.
    pub(super) async fn hook_prompt(
        &self,
        plan: &Plan,
        mut prompt: Prompt,
        how: &Admission<'_>,
    ) -> Result<Prompt, TurnError> {
        if self.hooks.is_empty() || plan.turn_only || !how.bootstrap.is_empty() || how.steer_only {
            return Ok(prompt);
        }

        let text = text_parts(prompt.parts.iter());
        if text.trim().is_empty() {
            return Ok(prompt);
        }

        let event = crate::hook::PromptEvent {
            session_id: plan.session.id.clone(),
            workspace: plan.workspace.to_string_lossy().into_owned(),
            agent: plan.session.agent.clone(),
            text: text.clone(),
        };
        let (replaced, context) = self.hooks.prompt_submit(event).await.map_err(|(plugin, reason)| {
            TurnError::Refused(format!("The {plugin} plugin refused this prompt: {reason}"))
        })?;

        if replaced != text {
            // One text part holds the replacement, so the model never reads it twice.
            let mut first = true;
            prompt.parts.retain_mut(|part| {
                let Part::Text { text } = part else { return true };
                if !first {
                    return false;
                }

                first = false;
                *text = replaced.clone();
                true
            });
        }
        prompt
            .parts
            .extend(context.into_iter().map(|(plugin, text)| Part::Context { plugin, text }));

        Ok(prompt)
    }

    /// A plugin reads the reply that would end the turn and may keep it going with a prompt of its own, a few times at most.
    pub(super) async fn hook_turn_end(&self, plan: &Plan, continued: &mut u32, abort: &CancellationToken) -> bool {
        if self.hooks.is_empty() || plan.turn_only || *continued >= MAX_CONTINUATIONS || abort.is_cancelled() {
            return false;
        }

        let Ok(Some(reply)) = self.store.last_reply(&plan.session.id) else {
            return false;
        };
        if reply.info.error.is_some() {
            return false;
        }

        let event = crate::hook::ReplyEvent {
            session_id: plan.session.id.clone(),
            workspace: plan.workspace.to_string_lossy().into_owned(),
            agent: plan.session.agent.clone(),
            text: text_parts(reply.parts.iter().map(|row| &row.part)),
        };
        let ended = self.hooks.turn_end(&event).await;
        for (plugin, note) in ended.notes {
            let part = Part::Context { plugin, text: note };
            if let Ok(row) = self.store.add_part(&reply.info.id, &plan.session.id, part) {
                self.hub.publish(Event::PartCreated { part: row });
            }
        }

        let Some((plugin, reason)) = ended.continued else {
            return false;
        };
        let pick = Pick {
            model: &plan.model_ref,
            variant: None,
            agent: None,
            sticky: true,
        };
        let parts = vec![Part::Context { plugin, text: reason }];
        match self.admit_fenced(&plan.session.id, pick, parts, None, Some(abort), None) {
            Ok(admitted) => {
                self.announce(&plan.session.id, admitted);
                *continued += 1;
                true
            }
            Err(_) => false,
        }
    }

    /// A plugin may refuse the call or change its input; a changed input must still fit the tool. Says whether it changed.
    pub(super) async fn hook_before(
        &self,
        scope: &CallScope<'_>,
        row: &mut PartRow,
        input: BeforeTool<'_>,
    ) -> Result<(serde_json::Value, bool), Outcome> {
        if self.hooks.is_empty() {
            return Ok((input.input, false));
        }

        let call = crate::hook::ToolCall {
            session_id: scope.plan.session.id.clone(),
            workspace: scope.plan.workspace.to_string_lossy().into_owned(),
            agent: scope.plan.session.agent.clone(),
            tool: input.name.to_owned(),
            input: input.input,
        };
        let (call, denied) = self.hooks.before_tool(call).await;
        if let Some((plugin, reason)) = denied {
            let reason = format!("The {plugin} plugin refused this call: {reason}");
            self.settle(row, ToolStatus::Error, None, reason, None);
            return Err(Outcome::Allowed);
        }

        let Part::ToolCall { input: stored, .. } = &mut row.part else {
            return Ok((call.input, false));
        };
        if *stored == call.input {
            return Ok((call.input, false));
        }

        let problems = crate::tool::schema::problems(input.schema, &call.input);
        if !problems.is_empty() {
            let reason = format!(
                "A plugin changed the call so it no longer fits the tool: {}.",
                problems.join("; ")
            );
            self.settle(row, ToolStatus::Error, None, reason, None);
            return Err(Outcome::Allowed);
        }
        stored.clone_from(&call.input);

        Ok((call.input, true))
    }

    /// A plugin may replace what the model reads or add a note under it.
    pub(super) async fn hook_after(&self, scope: &CallScope<'_>, output: AfterTool<'_>) -> (String, ToolMetadata) {
        let AfterTool {
            name,
            input,
            failed,
            text,
            mut metadata,
        } = output;
        let Some(input) = input else { return (text, metadata) };

        let result = crate::hook::ToolResult {
            session_id: scope.plan.session.id.clone(),
            workspace: scope.plan.workspace.to_string_lossy().into_owned(),
            agent: scope.plan.session.agent.clone(),
            tool: name.to_owned(),
            input,
            output: text,
            failed,
        };
        let (mut text, notes) = self.hooks.after_tool(result).await;
        for note in notes {
            crate::tool::add_note(&mut text, &mut metadata, &note);
        }

        (text, metadata)
    }
}
