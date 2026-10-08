use std::sync::atomic::{AtomicUsize, Ordering};

use super::*;

/// One fake token endpoint at a time, since they share `oauth::TEST_TOKEN_URL`.
static TOKEN_ENDPOINT: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// Holds the fake endpoint for one test; dropping it points the engine back at the real one.
struct FakeTokenEndpoint {
    _turn: tokio::sync::MutexGuard<'static, ()>,
}

impl Drop for FakeTokenEndpoint {
    fn drop(&mut self) {
        *crate::llm::anthropic::oauth::TEST_TOKEN_URL.lock().unwrap() = None;
    }
}

/// A slow Anthropic token endpoint answering every refresh with `status` and `body`; counts the refreshes.
async fn token_endpoint(status: u16, body: serde_json::Value) -> (FakeTokenEndpoint, Arc<AtomicUsize>) {
    let held = TOKEN_ENDPOINT.lock().await;
    let hits = Arc::new(AtomicUsize::new(0));
    let counter = hits.clone();
    let app = axum::Router::new().route(
        "/token",
        axum::routing::post(move || {
            let (counter, body) = (counter.clone(), body.clone());
            async move {
                counter.fetch_add(1, Ordering::SeqCst);
                tokio::time::sleep(Duration::from_millis(100)).await;
                (axum::http::StatusCode::from_u16(status).unwrap(), axum::Json(body))
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    *crate::llm::anthropic::oauth::TEST_TOKEN_URL.lock().unwrap() =
        Some(format!("http://{}/token", listener.local_addr().unwrap()));
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

    (FakeTokenEndpoint { _turn: held }, hits)
}

fn signed_in(h: &Harness, access: &str) {
    let live = Credential::OAuth {
        access: access.into(),
        refresh: "r1".into(),
        expires_at: id::now_ms() + 3_600_000,
        account: None,
    };
    h.engine.credentials.set("anthropic", &live).unwrap();
}

#[tokio::test]
async fn a_refused_sign_in_is_renewed_once_and_the_request_sent_again() {
    let (_held, hits) = token_endpoint(
        200,
        json!({ "access_token": "fresh", "refresh_token": "r2", "expires_in": 3600 }),
    )
    .await;
    let h = harness().await;
    signed_in(&h, "revoked");
    h.provider
        .push_error(llm::Error::Unauthenticated("OAuth token has expired.".into()))
        .push(text("a"))
        .push(text("b"));
    h.engine.submit(&h.session.id, prompt("one")).await.await_ok();
    until_idle(&h).await;
    h.engine.submit(&h.session.id, prompt("two")).await.await_ok();
    until_idle(&h).await;

    let messages = transcript(&h);
    assert_eq!(messages.len(), 4, "the refusal leaves no failed reply behind");
    assert!(messages.iter().all(|message| message.info.error.is_none()));
    assert_eq!(
        hits.load(Ordering::SeqCst),
        1,
        "renewed once; the next turn uses the stored token"
    );
    assert!(
        matches!(h.engine.credentials.get("anthropic").unwrap(), Credential::OAuth { access, .. } if access == "fresh")
    );
}

#[tokio::test]
async fn a_sign_in_that_cannot_be_renewed_says_it_expired() {
    let (_held, _) = token_endpoint(
        400,
        json!({ "error": "invalid_grant", "error_description": "Refresh token revoked" }),
    )
    .await;
    let h = harness().await;
    signed_in(&h, "revoked");
    h.provider
        .push_error(llm::Error::Unauthenticated("OAuth token has expired.".into()));
    h.engine.submit(&h.session.id, prompt("one")).await.await_ok();
    until_idle(&h).await;

    let error = transcript(&h)[1].info.error.clone().unwrap();
    assert!(
        error.starts_with("the provider refused the credentials: OAuth token has expired."),
        "{error}"
    );
    assert!(
        error.contains("the sign-in has expired and could not be renewed; sign in again under Settings > Providers"),
        "{error}"
    );
    assert!(!error.contains("no credentials"), "{error}");
}

#[tokio::test]
async fn concurrent_turns_refresh_an_expired_token_once() {
    let (_held, hits) = token_endpoint(
        200,
        json!({ "access_token": "fresh", "refresh_token": "r2", "expires_in": 3600 }),
    )
    .await;
    let h = harness().await;
    h.engine
        .credentials
        .set(
            "anthropic",
            &Credential::OAuth {
                access: "stale".into(),
                refresh: "r1".into(),
                expires_at: 1,
                account: None,
            },
        )
        .unwrap();
    let other = sibling_session(&h, "Other");
    h.provider.push(text("a")).push(text("b"));
    let (first, second) = tokio::join!(
        h.engine.submit(&h.session.id, prompt("one")),
        h.engine.submit(&other.id, prompt("two"))
    );
    first.await_ok();
    second.await_ok();
    until_idle(&h).await;

    assert_eq!(hits.load(Ordering::SeqCst), 1, "one refresh for two turns");
    let stored = h.engine.credentials.get("anthropic").unwrap();
    assert!(matches!(stored, Credential::OAuth { access, refresh, .. } if access == "fresh" && refresh == "r2"));
}

#[tokio::test]
async fn a_subscription_sign_in_is_never_sent_to_a_route_the_user_re_pointed() {
    let h = harness().await;
    let live = Credential::OAuth {
        access: "a".into(),
        refresh: "r".into(),
        expires_at: id::now_ms() + 3_600_000,
        account: None,
    };
    h.engine.credentials.set("anthropic", &live).unwrap();
    h.engine
        .catalog
        .write()
        .unwrap()
        .providers
        .get_mut("anthropic")
        .unwrap()
        .api = Some("https://gateway.example".into());
    let refused = h.engine.submit(&h.session.id, prompt("hi")).await.err();
    assert!(
        matches!(&refused, Some(TurnError::Config(why)) if why.contains("subscription sign-in is only sent to anthropic")),
        "{refused:?}"
    );

    h.engine
        .credentials
        .set("anthropic", &Credential::ApiKey { key: "k".into() })
        .unwrap();
    h.provider.push(text("via the gateway"));
    h.engine.submit(&h.session.id, prompt("hi")).await.await_ok();
    until_idle(&h).await;
}
