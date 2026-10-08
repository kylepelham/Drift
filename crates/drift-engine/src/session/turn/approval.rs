use super::*;

impl Engine {
    /// Checks one of a call's asks. `None` lets the call go on; otherwise the call is settled as
    /// refused and the outcome says whether the turn goes on.
    pub(super) async fn permit(&self, scope: &CallScope<'_>, row: &mut PartRow, call: CallAsk<'_>) -> Option<Outcome> {
        let CallAsk { call_id, name, ask } = call;

        // Plugins may answer an Ask decision, but cannot override an explicit permission rule.
        if !self.hooks.is_empty()
            && self.permissions.decide_under(
                &scope.plan.session.id,
                &scope.plan.config.policy(),
                &scope.plan.config.agent_policy(&scope.plan.session.agent),
                &ask,
            ) == permission::Decision::Ask
        {
            let event = crate::hook::PermissionAsk {
                session_id: scope.plan.session.id.clone(),
                workspace: scope.plan.workspace.to_string_lossy().into_owned(),
                agent: scope.plan.session.agent.clone(),
                tool: name.to_owned(),
                kind: ask.kind.clone(),
                pattern: ask.pattern.clone(),
                title: ask.title.clone(),
                commands: ask.commands.clone(),
            };
            match self.hooks.permission(&event).await {
                Some((_, crate::hook::PermissionDecision::Allow)) => return None,
                Some((plugin, crate::hook::PermissionDecision::Deny(reason))) => {
                    self.settle(
                        row,
                        Settlement::denied(format!("The {plugin} plugin refused this call: {reason}")),
                    );
                    return Some(Outcome::Allowed);
                }
                _ => {}
            }
        }

        let request = permission::new_request(&scope.plan.session.id, &scope.message.id, call_id, name, ask);
        let outcome = self
            .permissions
            .check_under(
                &self.hub,
                permission::Policies {
                    workspace: &scope.plan.config.policy(),
                    agent: &scope.plan.config.agent_policy(&scope.plan.session.agent),
                },
                request,
                scope.abort,
            )
            .await;

        match outcome {
            Outcome::Allowed => None,
            Outcome::Refused => {
                self.settle(row, Settlement::denied("A permission rule forbids this call.".into()));
                Some(Outcome::Allowed)
            }
            Outcome::Denied { feedback, stop } => {
                self.settle(row, Settlement::denied(denial(feedback.as_deref(), stop)));
                if !stop {
                    return Some(Outcome::Allowed);
                }

                scope.abort.cancel();
                Some(Outcome::Aborted)
            }
            Outcome::Aborted => {
                self.settle(row, Settlement::error("Aborted while waiting for permission.".into()));
                Some(Outcome::Aborted)
            }
        }
    }
}

/// What the model is told about a call the user refused.
fn denial(feedback: Option<&str>, stop: bool) -> String {
    let refused = if stop {
        "The user denied permission for this call and stopped the turn."
    } else {
        "The user denied permission for this call."
    };

    match feedback {
        Some(said) => format!("{refused} They said: {said}"),
        None => refused.to_string(),
    }
}
