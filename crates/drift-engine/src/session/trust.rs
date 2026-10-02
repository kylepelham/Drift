//! A project's drift.json may name check and formatter commands; they run only once the user has said so for that workspace.

use std::collections::HashMap;
use std::sync::Mutex;

use tokio_util::sync::CancellationToken;

use super::turn::Plan;
use crate::permission::{self, Decision, Outcome, Policy};
use crate::tool::Ask;
use crate::Engine;

/// What each session was told for the project commands it was asked about, by their hash: allowed or not.
#[derive(Default)]
pub struct Answers(Mutex<HashMap<String, (String, bool)>>);

impl Answers {
    /// The nearest answer along `lineage` (the session, then the parents a subagent inherits approvals from).
    fn get(&self, lineage: &[String], hash: &str) -> Option<bool> {
        let answers = self.0.lock().unwrap();
        lineage.iter().find_map(|id| answers.get(id).filter(|(asked, _)| asked == hash).map(|(_, allowed)| *allowed))
    }

    fn set(&self, session_id: &str, hash: &str, allowed: bool) {
        self.0.lock().unwrap().insert(session_id.into(), (hash.into(), allowed));
    }
}

#[cfg(test)]
#[test]
fn a_subagent_takes_its_parents_answer_and_a_changed_command_set_is_asked_again() {
    let answers = Answers::default();
    answers.set("parent", "h1", true);
    let child = ["child".to_string(), "parent".to_string()];
    assert_eq!(answers.get(&child, "h1"), Some(true), "once in the parent holds for the subagent it delegates to");
    assert_eq!(answers.get(&child, "h2"), None);
    answers.set("child", "h1", false);
    assert_eq!(answers.get(&child, "h1"), Some(false), "its own answer comes first");
    assert_eq!(answers.get(&["parent".to_string()], "h1"), Some(true), "and never reaches the parent");
}

/// Where a workspace's trusted command set is kept: the hash of the commands the user said always to run.
fn key(workspace_id: &str) -> String {
    format!("trustedCommands:{workspace_id}")
}

fn hash(lines: &[String]) -> String {
    use sha2::Digest;
    sha2::Sha256::digest(lines.join("\n").as_bytes()).iter().take(12).map(|b| format!("{b:02x}")).collect()
}

/// The call that brought the commands to run, which the user's card is shown beside.
pub(super) struct Asker<'a> {
    pub message_id: &'a str,
    pub call_id: &'a str,
    pub abort: &'a CancellationToken,
}

impl Engine {
    /// Whether the project's own check and formatter commands may run on `files`: asked only when one covers them; "always" holds for the workspace, "once" and "deny" for the session and its subagents.
    pub(super) async fn project_commands_trusted(&self, plan: &Plan, asker: Asker<'_>, files: &[std::path::PathBuf]) -> bool {
        // Nothing of the project's would run on these files, so nothing untrusted runs either way.
        if !plan.config.project_commands_cover(files) {
            return true;
        }
        let lines = plan.config.project_command_lines();
        let hash = hash(&lines);
        let session = &plan.session;
        if self.store.setting::<String>(&key(&session.workspace_id)).ok().flatten().as_deref() == Some(hash.as_str()) {
            return true;
        }
        if let Some(allowed) = self.turns.trust.get(&self.permissions.lineage(&session.id), &hash) {
            return allowed;
        }
        let ask = Ask::new("project-commands", lines.join("; "), "Run the commands this project's drift.json names");
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
            let _ = self.store.set_setting(&key(&session.workspace_id), &hash);
        }
        self.turns.trust.set(&session.id, &hash, allowed);
        allowed
    }
}
