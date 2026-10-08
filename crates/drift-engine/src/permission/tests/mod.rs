use super::grants::outside_folder;
use super::*;

mod grants;
mod rules;

fn ask(kind: &str, target: &str) -> Ask {
    Ask::new(kind, target, target)
}

fn request(kind: &str, target: &str) -> Request {
    new_request("ses_1", "msg_1", "call_1", kind, ask(kind, target))
}

fn rule(kind: &str, pattern: &str, decision: Decision) -> Rule {
    Rule {
        kind: kind.into(),
        pattern: pattern.into(),
        decision,
    }
}

/// A shell ask as the bash tool makes it: the line plus the commands it runs.
fn shell(line: &str) -> Ask {
    Ask::shell(crate::tool::command::Dialect::Bash, line, line)
}

fn powershell(line: &str) -> Ask {
    Ask::shell(crate::tool::command::Dialect::PowerShell, line, line)
}

fn approve_always(permissions: &Permissions, ask: Ask) {
    let request = new_request("ses_1", "msg_1", "call_1", "bash", ask);
    permissions.apply(
        &request,
        &ReplyBody {
            reply: Reply::Always,
            pattern: None,
            message: None,
        },
    );
}

#[test]
fn a_subagent_has_its_parents_approvals_but_not_the_other_way() {
    let none = Policy::default();
    let permissions = Permissions::new(Policy::default());
    approve_always(&permissions, shell("cargo test"));
    permissions.inherit("ses_child", "ses_1");

    assert_eq!(
        permissions.decide("ses_child", &none, &shell("cargo test --lib")),
        Decision::Allow,
        "inherited from the parent"
    );
    assert_eq!(
        permissions.decide("ses_other", &none, &shell("cargo test")),
        Decision::Ask,
        "only for its own children"
    );
    let child = new_request("ses_child", "m", "c", "bash", shell("npm run build"));
    permissions.apply(
        &child,
        &ReplyBody {
            reply: Reply::Always,
            pattern: None,
            message: None,
        },
    );
    assert_eq!(
        permissions.decide("ses_child", &none, &shell("npm run build")),
        Decision::Allow
    );
    assert_eq!(
        permissions.decide("ses_1", &none, &shell("npm run build")),
        Decision::Ask,
        "a worker's approval stays with the worker"
    );

    permissions.forget_session("ses_child");
    assert_eq!(
        permissions.decide("ses_child", &none, &shell("cargo test")),
        Decision::Ask
    );
}

#[test]
fn refusals_carry_what_the_user_said_and_whether_to_stop() {
    let permissions = Permissions::new(Policy::default());
    let request = request("bash", "rm -rf build");
    let said = |reply, message: Option<&str>| {
        permissions.apply(
            &request,
            &ReplyBody {
                reply,
                pattern: None,
                message: message.map(str::to_string),
            },
        )
    };

    assert_eq!(
        said(Reply::Deny, Some(" use cargo clean ")),
        Outcome::Denied {
            feedback: Some("use cargo clean".into()),
            stop: false
        }
    );
    assert_eq!(
        said(Reply::Deny, Some("  ")),
        Outcome::Denied {
            feedback: None,
            stop: false
        },
        "blank feedback is none"
    );
    assert_eq!(
        said(Reply::Stop, None),
        Outcome::Denied {
            feedback: None,
            stop: true
        }
    );
}

#[tokio::test]
async fn asks_over_the_hub_and_always_remembers_for_the_session() {
    let none = Policy::default();
    let hub = Hub::new(16);
    let permissions = Permissions::new(Policy::default());
    let abort = CancellationToken::new();
    let mut events = hub.attach(None).rx;
    let first = new_request("ses_1", "msg_1", "call_1", "bash", shell("cargo test"));

    let waiting = permissions.check(&hub, &none, first.clone(), &abort);
    let replier = async {
        let asked = events.recv().await.unwrap();
        let Event::PermissionAsked { request } = asked.event else {
            panic!("expected ask")
        };
        assert_eq!(request.id, first.id);
        assert_eq!(permissions.pending().len(), 1);
        permissions
            .reply(
                &hub,
                &request.id,
                ReplyBody {
                    reply: Reply::Always,
                    pattern: None,
                    message: None,
                },
            )
            .unwrap();
    };
    let (outcome, ()) = tokio::join!(waiting, replier);

    assert_eq!(outcome, Outcome::Allowed);
    assert!(permissions.pending().is_empty());
    let again = new_request("ses_1", "msg_1", "call_2", "bash", shell("cargo test --lib"));
    assert_eq!(permissions.check(&hub, &none, again, &abort).await, Outcome::Allowed);
    assert_eq!(
        permissions.decide("ses_2", &none, &shell("cargo test --lib")),
        Decision::Ask
    );
    let replied = events.recv().await.unwrap();
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
        permissions
            .reply(
                &hub,
                &denied.id,
                ReplyBody {
                    reply: Reply::Deny,
                    pattern: None,
                    message: None,
                },
            )
            .unwrap();
    });
    assert_eq!(
        outcome,
        Outcome::Denied {
            feedback: None,
            stop: false
        }
    );

    let aborted = request("edit", "b.rs");
    let (outcome, ()) = tokio::join!(permissions.check(&hub, &none, aborted, &abort), async {
        tokio::task::yield_now().await;
        abort.cancel();
    });
    assert_eq!(outcome, Outcome::Aborted);
    assert!(permissions.pending().is_empty());
    assert_eq!(
        permissions.reply(
            &hub,
            "perm_nope",
            ReplyBody {
                reply: Reply::Once,
                pattern: None,
                message: None
            }
        ),
        Err(NotPending)
    );
}
