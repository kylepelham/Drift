//! A project may bring its own commands: checks and formatters its drift.json names, and formatter
//! programs installed inside it (`node_modules/.bin`). They run only once the user has said so.

use std::collections::HashMap;
use std::sync::Mutex;

use tokio_util::sync::CancellationToken;

use super::turn::Plan;
use crate::permission::{self, Decision, Outcome, Policy};
use crate::tool::Ask;
use crate::Engine;

/// What each session was told about each project command (one line naming it): allowed or not.
#[derive(Default)]
pub struct Answers(Mutex<HashMap<String, HashMap<String, bool>>>);

impl Answers {
    /// The nearest answer about `line` along `lineage` (the session, then the parents a subagent inherits approvals from).
    fn get(&self, lineage: &[String], line: &str) -> Option<bool> {
        let answers = self.0.lock().unwrap();
        lineage.iter().find_map(|id| answers.get(id)?.get(line).copied())
    }

    fn set(&self, session_id: &str, lines: &[String], allowed: bool) {
        let mut answers = self.0.lock().unwrap();
        let session = answers.entry(session_id.into()).or_default();
        for line in lines {
            session.insert(line.clone(), allowed);
        }
    }
}

#[cfg(test)]
#[test]
fn a_subagent_takes_its_parents_answer_and_a_new_command_is_asked_about() {
    let answers = Answers::default();
    answers.set("parent", &["check lint: eslint".into()], true);
    let child = ["child".to_string(), "parent".to_string()];
    assert_eq!(answers.get(&child, "check lint: eslint"), Some(true), "once in the parent holds for the subagent it delegates to");
    assert_eq!(answers.get(&child, "check lint: eslint --fix"), None, "a changed command is another line");
    answers.set("child", &["check lint: eslint".into()], false);
    assert_eq!(answers.get(&child, "check lint: eslint"), Some(false), "its own answer comes first");
    assert_eq!(answers.get(&["parent".to_string()], "check lint: eslint"), Some(true), "and never reaches the parent");
}

/// Where a workspace keeps the project commands the user said always to run, line by line.
fn key(workspace_id: &str) -> String {
    format!("trustedCommands:{workspace_id}")
}

/// The call that brought the commands to run, which the user's card is shown beside.
pub(super) struct Asker<'a> {
    pub message_id: &'a str,
    pub call_id: &'a str,
    pub abort: &'a CancellationToken,
}

impl Engine {
    /// Whether the project's own commands may run on `files`: asked only when one would. "Always"
    /// holds for the workspace, "once" and "deny" for the session and its subagents, each per command,
    /// so a changed or newly installed one is asked about again.
    pub(super) async fn project_commands_trusted(&self, plan: &Plan, asker: Asker<'_>, files: &[std::path::PathBuf]) -> bool {
        let lines = project_lines(plan, files);
        if lines.is_empty() {
            return true;
        }
        let session = &plan.session;
        let mut kept: Vec<String> = self.store.setting(&key(&session.workspace_id)).ok().flatten().unwrap_or_default();
        let lineage = self.permissions.lineage(&session.id);
        let unanswered: Vec<String> = lines.iter().filter(|line| !kept.contains(line)).filter(|line| self.turns.trust.get(&lineage, line).is_none()).cloned().collect();
        if lines.iter().any(|line| !kept.contains(line) && self.turns.trust.get(&lineage, line) == Some(false)) {
            return false;
        }
        if unanswered.is_empty() {
            return true;
        }
        let ask = Ask::new("project-commands", unanswered.join("; "), "Run commands this project brings (its drift.json, or programs installed in it)");
        let request = permission::new_request(&session.id, asker.message_id, asker.call_id, "drift.json", ask.clone());
        let allowed = match self.permissions.check(&self.hub, &plan.config.policy(), request, asker.abort).await {
            Outcome::Allowed => true,
            Outcome::Denied { stop: true, .. } => {
                asker.abort.cancel();
                false
            }
            Outcome::Aborted => return false,
            Outcome::Refused | Outcome::Denied { .. } => false,
        };
        // "Always" leaves a session grant behind; that, unlike "once", is kept for the workspace.
        if allowed && self.permissions.decide_now(&session.id, &Policy::default(), &ask) == Decision::Allow {
            kept.extend(unanswered.iter().cloned());
            let _ = self.store.set_setting(&key(&session.workspace_id), &kept);
        }
        self.turns.trust.set(&session.id, &unanswered, allowed);
        allowed
    }
}

/// The project's own commands that would run on `files`: its drift.json's checks and formatters
/// when one covers them, and any formatter program installed inside the project.
fn project_lines(plan: &Plan, files: &[std::path::PathBuf]) -> Vec<String> {
    let mut lines = if plan.config.project_commands_cover(files) { plan.config.project_command_lines() } else { Vec::new() };
    let formatters = crate::edit::format::resolve(&plan.config.formatters);
    lines.extend(crate::edit::format::project_programs(files, &plan.workspace, &formatters));
    lines
}
