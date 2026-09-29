//! Who may do what: rules decide, and anything undecided is put to the user over the socket.

use std::collections::HashMap;
use std::sync::Mutex;

use globset::GlobBuilder;
use serde::{Deserialize, Serialize};
use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;
use utoipa::ToSchema;

use crate::event::{Event, Hub};
use crate::id;
use crate::tool::Ask;

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum Decision {
    Allow,
    Deny,
    Ask,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct Rule {
    pub kind: String,
    pub pattern: String,
    pub decision: Decision,
}

impl Rule {
    fn matches(&self, ask: &Ask) -> bool {
        if self.kind != ask.kind {
            return false;
        }
        GlobBuilder::new(&self.pattern)
            .literal_separator(false)
            .build()
            .map(|glob| glob.compile_matcher().is_match(&ask.pattern))
            .unwrap_or(false)
    }
}

/// Ordered rules; the first match wins, and nothing matching means ask.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Policy {
    pub rules: Vec<Rule>,
}

impl Policy {
    pub fn decide(&self, ask: &Ask) -> Decision {
        self.rules.iter().find(|rule| rule.matches(ask)).map_or(Decision::Ask, |rule| rule.decision)
    }
}

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
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum Reply {
    Once,
    /// Allow this and matching calls for the rest of the session; `pattern` widens the match.
    Always,
    Deny,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct ReplyBody {
    pub reply: Reply,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pattern: Option<String>,
}

pub struct Permissions {
    /// Rules that apply everywhere, set by the shell; workspace rules from drift.json are passed per check.
    policy: Mutex<Policy>,
    session_rules: Mutex<HashMap<String, Vec<Rule>>>,
    pending: Mutex<Vec<(Request, oneshot::Sender<ReplyBody>)>>,
}

#[derive(Debug, PartialEq)]
pub enum Outcome {
    Allowed,
    Denied,
    Aborted,
}

impl Permissions {
    pub fn new(policy: Policy) -> Self {
        Self { policy: Mutex::new(policy), session_rules: Mutex::default(), pending: Mutex::default() }
    }

    pub fn set_policy(&self, policy: Policy) {
        *self.policy.lock().unwrap() = policy;
    }

    /// Session answers first, then the workspace's drift.json, then the global policy.
    fn decide(&self, session_id: &str, workspace: &Policy, ask: &Ask) -> Decision {
        let session = self.session_rules.lock().unwrap();
        let from_session = session.get(session_id).and_then(|rules| rules.iter().find(|rule| rule.matches(ask)));
        if let Some(rule) = from_session {
            return rule.decision;
        }
        match workspace.rules.iter().find(|rule| rule.matches(ask)) {
            Some(rule) => rule.decision,
            None => self.policy.lock().unwrap().decide(ask),
        }
    }

    /// Resolves immediately from rules, or publishes a request and waits for the user.
    pub async fn check(&self, hub: &Hub, workspace: &Policy, request: Request, abort: &CancellationToken) -> Outcome {
        match self.decide(&request.session_id, workspace, &request.ask) {
            Decision::Allow => return Outcome::Allowed,
            Decision::Deny => return Outcome::Denied,
            Decision::Ask => {}
        }
        let (tx, rx) = oneshot::channel();
        self.pending.lock().unwrap().push((request.clone(), tx));
        hub.publish(Event::PermissionAsked { request: request.clone() });
        let reply = tokio::select! {
            reply = rx => reply.ok(),
            () = abort.cancelled() => None,
        };
        let Some(reply) = reply else {
            self.pending.lock().unwrap().retain(|(pending, _)| pending.id != request.id);
            return Outcome::Aborted;
        };
        self.apply(&request, &reply)
    }

    fn apply(&self, request: &Request, reply: &ReplyBody) -> Outcome {
        match reply.reply {
            Reply::Once => Outcome::Allowed,
            Reply::Deny => Outcome::Denied,
            Reply::Always => {
                let pattern = reply.pattern.clone().unwrap_or_else(|| always_pattern(&request.ask));
                let rule = Rule { kind: request.ask.kind.clone(), pattern, decision: Decision::Allow };
                self.session_rules.lock().unwrap().entry(request.session_id.clone()).or_default().push(rule);
                Outcome::Allowed
            }
        }
    }

    pub fn reply(&self, hub: &Hub, request_id: &str, body: ReplyBody) -> Result<(), NotPending> {
        let mut pending = self.pending.lock().unwrap();
        let index = pending.iter().position(|(request, _)| request.id == request_id).ok_or(NotPending)?;
        let (request, tx) = pending.remove(index);
        drop(pending);
        let decision = match body.reply {
            Reply::Deny => Decision::Deny,
            _ => Decision::Allow,
        };
        let _ = tx.send(body);
        hub.publish(Event::PermissionReplied { request_id: request.id, session_id: request.session_id, decision });
        Ok(())
    }

    pub fn pending(&self) -> Vec<Request> {
        self.pending.lock().unwrap().iter().map(|(request, _)| request.clone()).collect()
    }

    pub fn forget_session(&self, session_id: &str) {
        self.session_rules.lock().unwrap().remove(session_id);
    }
}

/// What "always" widens to: the command's program for shells, the exact path otherwise.
fn always_pattern(ask: &Ask) -> String {
    if ask.kind == "bash" {
        let program = ask.pattern.split_whitespace().next().unwrap_or(&ask.pattern);
        return format!("{program}*");
    }
    ask.pattern.clone()
}

#[derive(Debug, PartialEq)]
pub struct NotPending;

pub fn new_request(session_id: &str, message_id: &str, call_id: &str, tool: &str, ask: Ask) -> Request {
    Request {
        id: id::new("perm"),
        session_id: session_id.into(),
        message_id: message_id.into(),
        call_id: call_id.into(),
        tool: tool.into(),
        ask,
        created_at: id::now_ms(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ask(kind: &str, pattern: &str) -> Ask {
        Ask { kind: kind.into(), pattern: pattern.into(), title: pattern.into() }
    }

    fn request(kind: &str, pattern: &str) -> Request {
        new_request("ses_1", "msg_1", "call_1", kind, ask(kind, pattern))
    }

    #[test]
    fn first_matching_rule_wins_and_no_match_asks() {
        let policy = Policy {
            rules: vec![
                Rule { kind: "bash".into(), pattern: "git push*".into(), decision: Decision::Deny },
                Rule { kind: "bash".into(), pattern: "git *".into(), decision: Decision::Allow },
                Rule { kind: "edit".into(), pattern: "C:/repo/**".into(), decision: Decision::Allow },
            ],
        };
        assert_eq!(policy.decide(&ask("bash", "git push origin")), Decision::Deny);
        assert_eq!(policy.decide(&ask("bash", "git status")), Decision::Allow);
        assert_eq!(policy.decide(&ask("bash", "rm -rf /")), Decision::Ask);
        assert_eq!(policy.decide(&ask("edit", "C:/repo/src/a.rs")), Decision::Allow);
        assert_eq!(policy.decide(&ask("edit", "C:/other/a.rs")), Decision::Ask);
    }

    #[tokio::test]
    async fn asks_over_the_hub_and_always_remembers_for_the_session() {
        let none = Policy::default();
        let hub = Hub::new(16);
        let permissions = Permissions::new(Policy::default());
        let abort = CancellationToken::new();
        let mut rx = hub.attach(None).rx;
        let first = request("bash", "cargo test");
        let waiting = permissions.check(&hub, &none, first.clone(), &abort);
        let replier = async {
            let asked = rx.recv().await.unwrap();
            let Event::PermissionAsked { request } = asked.event else { panic!("expected ask") };
            assert_eq!(request.id, first.id);
            assert_eq!(permissions.pending().len(), 1);
            permissions.reply(&hub, &request.id, ReplyBody { reply: Reply::Always, pattern: None }).unwrap();
        };
        let (outcome, ()) = tokio::join!(waiting, replier);
        assert_eq!(outcome, Outcome::Allowed);
        assert!(permissions.pending().is_empty());
        assert_eq!(permissions.check(&hub, &none, request("bash", "cargo build"), &abort).await, Outcome::Allowed);
        assert_eq!(permissions.decide("ses_2", &none, &ask("bash", "cargo build")), Decision::Ask);
        let replied = rx.recv().await.unwrap();
        assert!(matches!(replied.event, Event::PermissionReplied { .. }));
    }

    #[tokio::test]
    async fn deny_and_abort_resolve_the_wait() {
        let none = Policy::default();
        let hub = Hub::new(16);
        let permissions = Permissions::new(Policy::default());
        let abort = CancellationToken::new();
        let denied = request("edit", "a.rs");
        let (outcome, ()) = tokio::join!(permissions.check(&hub, &none, denied.clone(), &abort), async {
            tokio::task::yield_now().await;
            permissions.reply(&hub, &denied.id, ReplyBody { reply: Reply::Deny, pattern: None }).unwrap();
        });
        assert_eq!(outcome, Outcome::Denied);

        let aborted = request("edit", "b.rs");
        let (outcome, ()) = tokio::join!(permissions.check(&hub, &none, aborted, &abort), async {
            tokio::task::yield_now().await;
            abort.cancel();
        });
        assert_eq!(outcome, Outcome::Aborted);
        assert!(permissions.pending().is_empty());
        assert_eq!(permissions.reply(&hub, "perm_nope", ReplyBody { reply: Reply::Once, pattern: None }), Err(NotPending));
    }
}
