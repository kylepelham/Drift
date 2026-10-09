//! Who may do what: rules decide, and anything undecided is put to the user over the socket.

mod decision;
mod grants;
mod policy;

#[cfg(test)]
mod tests;

pub use grants::Grant;
pub use policy::{Compiled, Decision, Policies, Policy, Rule};

use crate::event::{Event, Hub};
use crate::id;
use crate::tool::Ask;
use grants::{always_grants, grants_for};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Mutex;
use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;
use utoipa::ToSchema;

/// How far up a chain of subagents approvals are looked for; delegation is one level deep today.
const MAX_LINEAGE: usize = 4;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = PermissionRequest)]
pub struct Request {
    pub id: String,
    pub session_id: String,
    pub message_id: String,
    pub call_id: String,
    pub tool: String,
    #[serde(flatten)]
    pub ask: Ask,
    pub created_at: i64,
    /// What answering "always" would allow from now on, for the client to show before it is chosen.
    #[serde(default)]
    pub always: Vec<Grant>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum Reply {
    Once,
    /// Allow this and matching calls in this workspace from now on, in every session; `pattern` widens the match.
    Always,
    /// Refuse this call; the turn goes on, and the model hears `message` if there is one.
    Deny,
    /// Refuse this call and end the turn.
    Stop,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct ReplyBody {
    pub reply: Reply,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pattern: Option<String>,
    /// What the user wants the model told when refusing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

/// Writes a workspace's "always" grants, all of them, when one is added.
pub type GrantSaver = Box<dyn Fn(&str, &[Grant]) + Send + Sync>;

pub struct Permissions {
    /// Rules that apply everywhere, set by the shell; workspace rules from drift.json are passed per check.
    policy: Mutex<Policy>,
    /// Grants of a session not bound to a workspace (tests, engine-made sessions); kept in memory only.
    session_rules: Mutex<HashMap<String, Vec<Grant>>>,
    /// "Always" grants by workspace id, loaded when a session of that workspace is bound.
    workspace_rules: Mutex<HashMap<String, Vec<Grant>>>,
    /// Each bound session's workspace.
    workspaces: Mutex<HashMap<String, String>>,
    saver: std::sync::OnceLock<GrantSaver>,
    /// A subagent's parent, whose session approvals it also has. One way: its own never reach the parent.
    parents: Mutex<HashMap<String, String>>,
    /// Each waiting ask with the workspace policy it was asked under, so a later "always" can settle it.
    pending: Mutex<Vec<(Request, Policy, oneshot::Sender<ReplyBody>)>>,
    /// Sessions answering their own asks (`Session::auto_accept`); their subagents too.
    auto: Mutex<std::collections::HashSet<String>>,
    /// Every session answers its own asks.
    auto_all: std::sync::atomic::AtomicBool,
}

#[derive(Debug, PartialEq)]
pub enum Outcome {
    Allowed,
    /// A rule forbids the call.
    Refused,
    /// The user refused it, perhaps saying why, perhaps ending the turn.
    Denied {
        feedback: Option<String>,
        stop: bool,
    },
    Aborted,
}

#[derive(Debug, PartialEq)]
pub struct NotPending;

impl Permissions {
    pub fn new(policy: Policy) -> Self {
        Self {
            policy: Mutex::new(policy),
            session_rules: Mutex::default(),
            workspace_rules: Mutex::default(),
            workspaces: Mutex::default(),
            saver: std::sync::OnceLock::new(),
            parents: Mutex::default(),
            pending: Mutex::default(),
            auto: Mutex::default(),
            auto_all: std::sync::atomic::AtomicBool::new(false),
        }
    }

    /// Turns auto-accept on or off for one session, or for all of them; turning it on answers the
    /// waiting asks it now covers, as "always" does.
    pub fn set_auto_accept(&self, hub: &Hub, session_id: Option<&str>, on: bool) {
        match session_id {
            Some(id) if on => drop(self.auto.lock().unwrap().insert(id.into())),
            Some(id) => drop(self.auto.lock().unwrap().remove(id)),
            None => self.auto_all.store(on, std::sync::atomic::Ordering::SeqCst),
        }

        if on {
            self.settle_covered(hub);
        }
    }

    /// Records a session's stored setting when it is planned, without settling anything.
    pub fn load_auto_accept(&self, session_id: &str, on: bool) {
        let mut auto = self.auto.lock().unwrap();
        if on {
            auto.insert(session_id.into());
        } else {
            auto.remove(session_id);
        }
    }

    fn auto_accepts(&self, session_id: &str) -> bool {
        if self.auto_all.load(std::sync::atomic::Ordering::SeqCst) {
            return true;
        }

        let auto = self.auto.lock().unwrap();
        self.lineage(session_id).iter().any(|session| auto.contains(session))
    }

    /// Where "always" grants are written; set once, when the engine opens.
    pub fn save_grants_with(&self, saver: GrantSaver) {
        let _ = self.saver.set(saver);
    }

    /// Ties `session_id` to its workspace, loading that workspace's stored grants the first time with `load`.
    pub fn bind(&self, session_id: &str, workspace_id: &str, load: impl FnOnce() -> Vec<Grant>) {
        self.workspaces
            .lock()
            .unwrap()
            .insert(session_id.into(), workspace_id.into());

        let mut rules = self.workspace_rules.lock().unwrap();
        if !rules.contains_key(workspace_id) {
            rules.insert(workspace_id.into(), load());
        }
    }

    /// A workspace's "always" grants, loading them with `load` if no session of it was bound yet.
    pub fn grants(&self, workspace_id: &str, load: impl FnOnce() -> Vec<Grant>) -> Vec<Grant> {
        self.workspace_rules
            .lock()
            .unwrap()
            .entry(workspace_id.into())
            .or_insert_with(load)
            .clone()
    }

    /// Takes back one grant, or every grant of the workspace when `grant` is `None`; returns whether anything went.
    pub fn revoke(&self, workspace_id: &str, grant: Option<&Grant>, load: impl FnOnce() -> Vec<Grant>) -> bool {
        let mut rules = self.workspace_rules.lock().unwrap();
        let kept = rules.entry(workspace_id.into()).or_insert_with(load);
        let before = kept.len();
        match grant {
            Some(grant) => kept.retain(|held| held != grant),
            None => kept.clear(),
        }

        let removed = kept.len() != before;
        if removed && let Some(saver) = self.saver.get() {
            saver(workspace_id, kept);
        }

        removed
    }

    fn workspace_of(&self, session_id: &str) -> Option<String> {
        self.workspaces.lock().unwrap().get(session_id).cloned()
    }

    /// Keeps `grants`: for the session's workspace (written through the saver), or the session alone when it has none.
    fn remember(&self, session_id: &str, grants: Vec<Grant>) {
        let Some(workspace) = self.workspace_of(session_id) else {
            let mut rules = self.session_rules.lock().unwrap();
            let kept = rules.entry(session_id.into()).or_default();
            let fresh = grants
                .into_iter()
                .filter(|grant| !kept.contains(grant))
                .collect::<Vec<_>>();
            kept.extend(fresh);
            return;
        };

        let mut rules = self.workspace_rules.lock().unwrap();
        let kept = rules.entry(workspace.clone()).or_default();
        // One command may occur twice in a shell line, but its grant is stored only once.
        let mut fresh = Vec::new();
        for grant in grants {
            if !kept.contains(&grant) && !fresh.contains(&grant) {
                fresh.push(grant);
            }
        }
        if fresh.is_empty() {
            return;
        }

        kept.extend(fresh);
        if let Some(saver) = self.saver.get() {
            saver(&workspace, kept);
        }
    }

    /// `child` (a subagent) also runs under `parent`'s session approvals.
    pub fn inherit(&self, child: &str, parent: &str) {
        self.parents.lock().unwrap().insert(child.into(), parent.into());
    }

    /// The session and the parents whose approvals it inherits, nearest first.
    pub(crate) fn lineage(&self, session_id: &str) -> Vec<String> {
        let parents = self.parents.lock().unwrap();
        let mut chain = vec![session_id.to_string()];

        while let Some(parent) = parents.get(chain.last().unwrap()).filter(|_| chain.len() < MAX_LINEAGE) {
            chain.push(parent.clone());
        }

        chain
    }

    pub fn set_policy(&self, policy: Policy) {
        *self.policy.lock().unwrap() = policy;
    }

    pub fn policy(&self) -> Policy {
        self.policy.lock().unwrap().clone()
    }

    pub async fn check_under(
        &self,
        hub: &Hub,
        policies: Policies<'_>,
        request: Request,
        abort: &CancellationToken,
    ) -> Outcome {
        let Policies { workspace, agent } = policies;
        if agent.explicit(&request.ask) == Some(Decision::Deny) {
            return Outcome::Refused;
        }

        let rules = agent.rules.iter().chain(&workspace.rules).cloned().collect();
        self.check(hub, &Policy { rules }, request, abort).await
    }

    /// Resolves immediately from rules, or publishes a request and waits for the user.
    pub async fn check(&self, hub: &Hub, workspace: &Policy, request: Request, abort: &CancellationToken) -> Outcome {
        match self.decide(&request.session_id, workspace, &request.ask) {
            Decision::Allow => return Outcome::Allowed,
            Decision::Deny => return Outcome::Refused,
            Decision::Ask => {}
        }

        let (response, answered) = oneshot::channel();
        self.pending
            .lock()
            .unwrap()
            .push((request.clone(), workspace.clone(), response));
        hub.publish(Event::PermissionAsked {
            request: request.clone(),
        });

        let reply = tokio::select! {
            reply = answered => reply.ok(),
            () = abort.cancelled() => None,
        };
        let Some(reply) = reply else {
            self.pending
                .lock()
                .unwrap()
                .retain(|(pending, _, _)| pending.id != request.id);
            return Outcome::Aborted;
        };

        self.apply(&request, &reply)
    }

    fn apply(&self, request: &Request, reply: &ReplyBody) -> Outcome {
        let feedback = reply
            .message
            .as_deref()
            .map(str::trim)
            .filter(|message| !message.is_empty())
            .map(str::to_string);

        match reply.reply {
            Reply::Once => Outcome::Allowed,
            Reply::Deny => Outcome::Denied { feedback, stop: false },
            Reply::Stop => Outcome::Denied { feedback, stop: true },
            Reply::Always => {
                self.remember(&request.session_id, grants_for(request, reply));
                Outcome::Allowed
            }
        }
    }

    /// Answers a waiting ask. "Always" is kept before the asker hears it, and every other waiting ask
    /// the new grant now covers is answered with it, as opencode settles them.
    pub fn reply(&self, hub: &Hub, request_id: &str, body: ReplyBody) -> Result<(), NotPending> {
        let mut pending = self.pending.lock().unwrap();
        let index = pending
            .iter()
            .position(|(request, _, _)| request.id == request_id)
            .ok_or(NotPending)?;
        let (request, _, response) = pending.remove(index);
        drop(pending);

        let decision = match body.reply {
            Reply::Deny | Reply::Stop => Decision::Deny,
            _ => Decision::Allow,
        };
        let always = body.reply == Reply::Always;
        if always {
            self.remember(&request.session_id, grants_for(&request, &body));
        }
        let _ = response.send(body);
        hub.publish(Event::PermissionReplied {
            request_id: request.id,
            session_id: request.session_id,
            decision,
        });

        if always {
            self.settle_covered(hub);
        }
        Ok(())
    }

    /// Answers "once" for each waiting ask the rules and grants now allow.
    fn settle_covered(&self, hub: &Hub) {
        let waiting: Vec<(String, String, Policy, Ask)> = self
            .pending
            .lock()
            .unwrap()
            .iter()
            .map(|(request, policy, _)| {
                (
                    request.id.clone(),
                    request.session_id.clone(),
                    policy.clone(),
                    request.ask.clone(),
                )
            })
            .collect();

        for (id, session_id, policy, ask) in waiting {
            if self.decide(&session_id, &policy, &ask) == Decision::Allow {
                let _ = self.reply(
                    hub,
                    &id,
                    ReplyBody {
                        reply: Reply::Once,
                        pattern: None,
                        message: None,
                    },
                );
            }
        }
    }

    pub fn pending(&self) -> Vec<Request> {
        self.pending
            .lock()
            .unwrap()
            .iter()
            .map(|(request, _, _)| request.clone())
            .collect()
    }

    /// A forgotten workspace's cached grants and its sessions' ties to it.
    pub fn forget_workspace(&self, workspace_id: &str) {
        self.workspace_rules.lock().unwrap().remove(workspace_id);
        self.workspaces
            .lock()
            .unwrap()
            .retain(|_, workspace| workspace != workspace_id);
    }

    pub fn forget_session(&self, session_id: &str) {
        self.session_rules.lock().unwrap().remove(session_id);
        self.workspaces.lock().unwrap().remove(session_id);
        self.parents.lock().unwrap().remove(session_id);
    }
}

pub fn new_request(session_id: &str, message_id: &str, call_id: &str, tool: &str, ask: Ask) -> Request {
    Request {
        id: id::new("perm"),
        session_id: session_id.into(),
        message_id: message_id.into(),
        call_id: call_id.into(),
        tool: tool.into(),
        always: always_grants(&ask),
        ask,
        created_at: id::now_ms(),
    }
}
