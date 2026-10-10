use super::*;
use crate::llm::credentials::Profile;
use crate::llm::limits::{Limits, Window};

/// Two Claude sign-ins in use order; returns their account keys.
fn two_accounts(h: &Harness) -> (String, String) {
    let sign_in = |access: &str| Credential::OAuth {
        access: access.into(),
        refresh: format!("{access}-refresh"),
        expires_at: id::now_ms() + 3_600_000,
        account: None,
    };
    let person = |name: &str| Profile {
        identity: Some(name.into()),
        email: Some(format!("{name}@example.com")),
    };

    let credentials = &h.engine.credentials;
    let first = credentials
        .add_account("anthropic", &sign_in("first"), &person("ann"))
        .unwrap();
    let second = credentials
        .add_account("anthropic", &sign_in("second"), &person("bob"))
        .unwrap();

    (first, second)
}

fn weekly(used_percent: f64) -> Limits {
    Limits {
        weekly: Some(Window {
            used_percent,
            resets_at: Some(id::now_ms() + 3_600_000),
        }),
        ..Limits::default()
    }
}

#[tokio::test]
async fn the_limits_a_response_reports_are_kept_for_the_account_that_sent_it() {
    let h = harness().await;
    let (first, second) = two_accounts(&h);
    let mut reply = vec![Chunk::Limits(weekly(39.0))];
    reply.extend(text("a"));
    h.provider.push(reply);

    h.engine.submit(&h.session.id, prompt("one")).await.await_ok();
    until_idle(&h).await;

    let reported = h.engine.limits.get(&first).unwrap();
    assert_eq!(reported.weekly.unwrap().used_percent, 39.0);
    assert!(
        h.engine.limits.get(&second).is_none(),
        "the idle account reported nothing"
    );
    assert!(transcript(&h).iter().all(|message| message.info.error.is_none()));
}
