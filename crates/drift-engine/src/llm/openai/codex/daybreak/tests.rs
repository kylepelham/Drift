use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use super::*;
use crate::llm::catalog::Model;

fn credential(user: &str, account: &str) -> Credential {
    let payload = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .encode(json!({ "sub": user, "chatgpt_compute_residency": "eu" }).to_string());
    Credential::OAuth {
        access: format!("h.{payload}.s"),
        refresh: "refresh".into(),
        expires_at: i64::MAX,
        account: Some(account.into()),
    }
}

fn provider() -> ProviderInfo {
    let models = ["gpt-6-sol", "gpt-6-astra", "gpt-5.6-cyber"]
        .map(|id| {
            let model: Model = serde_json::from_value(json!({
                "id": id, "name": id, "reasoning": true, "attachment": true,
                "profile": "apply_patch", "limit": { "context": 400000, "output": 128000 },
                "cost": { "input": 2.0, "output": 10.0 },
                "variants": [{ "name": "high", "kind": "effort", "level": "high" }]
            }))
            .unwrap();
            (id.into(), model)
        })
        .into_iter()
        .collect();
    ProviderInfo {
        id: "openai".into(),
        name: "OpenAI".into(),
        env: vec![],
        api: None,
        models,
    }
}

fn list() -> Value {
    json!({ "models": [
        { "slug": "gpt-6-sol", "available_access_programs": { "cyber": ["standard", "daybreak_blue"] } },
        { "slug": "gpt-6-astra", "available_access_programs": { "cyber": ["standard"] } },
        { "slug": "gpt-5.6-cyber", "available_access_programs": { "cyber": ["daybreak_red"] } },
        { "slug": "unknown", "available_access_programs": { "cyber": ["future_program"] } },
        { "slug": "no-programs" }
    ] })
}

fn cache(credential: &Credential) -> Daybreak {
    let daybreak = Daybreak::default();
    let list: ModelList = serde_json::from_value(list()).unwrap();
    daybreak.state.write().unwrap().cached = Some(Cached {
        identity: identity(credential).unwrap(),
        until: Instant::now() + TTL,
        programs: list.models.into_iter().filter_map(program).collect(),
    });
    daybreak
}

#[test]
fn daybreak_entries_keep_the_base_model_and_only_select_an_offered_program() {
    let credential = credential("user", "account");
    let daybreak = cache(&credential);
    let mut provider = provider();
    let ordinary = provider.models.clone();
    daybreak.apply(&mut provider, &credential);
    assert_eq!(provider.models.len(), ordinary.len() + 2);
    assert!(!provider.models.contains_key("gpt-6-astra-daybreak"));
    for (id, program) in [("gpt-6-sol", "daybreak_blue"), ("gpt-5.6-cyber", "daybreak_red")] {
        let base = &ordinary[id];
        let copy = &provider.models[&format!("{id}-daybreak")];
        assert_eq!(copy.wire(&copy.id), id);
        assert_eq!(copy.name, format!("{} Daybreak", base.name));
        assert_eq!(
            (&copy.limit, &copy.cost, &copy.variants, copy.profile),
            (&base.limit, &base.cost, &base.variants, base.profile)
        );
        assert_eq!(copy.mode.as_ref().unwrap().body["access_programs"]["cyber"], program);
        assert_eq!(provider.models[id], *base);
    }
    let once = provider.clone();
    daybreak.apply(&mut provider, &credential);
    assert_eq!(provider, once);
}

#[test]
fn speed_modes_keep_their_settings_when_daybreak_is_selected() {
    let credential = credential("user", "account");
    let mut provider = provider();
    let mut fast = provider.models["gpt-6-sol"].clone();
    fast.id = "gpt-6-sol-fast".into();
    fast.mode = Some(ModelMode {
        name: "fast".into(),
        base: "gpt-6-sol".into(),
        body: json!({ "service_tier": "priority" }).as_object().unwrap().clone(),
        headers: BTreeMap::from([("x-test".into(), "fast".into())]),
    });
    provider.models.insert(fast.id.clone(), fast);
    cache(&credential).apply(&mut provider, &credential);
    let copy = &provider.models["gpt-6-sol-fast-daybreak"];
    let mode = copy.mode.as_ref().unwrap();
    assert_eq!(copy.wire(&copy.id), "gpt-6-sol");
    assert_eq!(mode.body["service_tier"], "priority");
    assert_eq!(mode.headers["x-test"], "fast");
}

#[test]
fn entitlements_never_cross_users_accounts_api_keys_or_expiry() {
    let owner = credential("user", "account");
    let daybreak = cache(&owner);
    for credential in [
        credential("another", "account"),
        credential("user", "another"),
        Credential::ApiKey { key: "key".into() },
    ] {
        let mut provider = provider();
        daybreak.apply(&mut provider, &credential);
        assert_eq!(provider.models.len(), 3);
    }
    daybreak.state.write().unwrap().cached.as_mut().unwrap().until = Instant::now() - RETRY;
    let mut provider = provider();
    daybreak.apply(&mut provider, &owner);
    assert_eq!(provider.models.len(), 3);
    assert!(daybreak.clear());
    assert!(daybreak.state.read().unwrap().cached.is_none());
}

#[test]
fn token_refresh_keeps_the_same_account_cache_identity() {
    let before = credential("user", "account");
    let mut after = before.clone();
    if let Credential::OAuth { access, refresh, .. } = &mut after {
        access.push_str("new-signature");
        *refresh = "new-refresh".into();
    }
    assert_eq!(identity(&before), identity(&after));
}

async fn server(
    body: Value,
    status: axum::http::StatusCode,
    calls: Arc<AtomicUsize>,
) -> (String, tokio::task::JoinHandle<()>) {
    let app = axum::Router::new().route(
        "/models",
        axum::routing::get(
            move |headers: axum::http::HeaderMap, query: axum::extract::Query<BTreeMap<String, String>>| {
                let body = body.clone();
                let calls = calls.clone();
                async move {
                    calls.fetch_add(1, Ordering::SeqCst);
                    assert_eq!(query["client_version"], crate::VERSION);
                    assert_eq!(headers["chatgpt-account-id"], "account");
                    assert_eq!(headers["originator"], super::super::super::CODEX_ORIGINATOR);
                    assert_eq!(headers["x-openai-internal-codex-residency"], "eu");
                    assert!(headers["authorization"].to_str().unwrap().starts_with("Bearer "));
                    (status, axum::Json(body))
                }
            },
        ),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let task = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (base, task)
}

#[tokio::test]
async fn discovery_is_versioned_authenticated_and_single_flight() {
    let calls = Arc::new(AtomicUsize::new(0));
    let (base, task) = server(list(), axum::http::StatusCode::OK, calls.clone()).await;
    let daybreak = Daybreak::default();
    let credential = credential("user", "account");
    let client = crate::llm::http::client();
    let (first, second) = tokio::join!(
        daybreak.refresh(&client, &base, &credential),
        daybreak.refresh(&client, &base, &credential)
    );
    assert!(first || second);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        daybreak.state.read().unwrap().cached.as_ref().unwrap().programs.len(),
        2
    );
    assert!(!daybreak.refresh(&client, &base, &credential).await);
    task.abort();
}

#[tokio::test]
async fn failed_or_malformed_discovery_never_offers_daybreak_and_backs_off() {
    for (body, status) in [
        (list(), axum::http::StatusCode::FORBIDDEN),
        (json!({ "unexpected": [] }), axum::http::StatusCode::OK),
    ] {
        let calls = Arc::new(AtomicUsize::new(0));
        let (base, task) = server(body, status, calls.clone()).await;
        let credential = credential("user", "account");
        let daybreak = cache(&credential);
        daybreak.state.write().unwrap().cached.as_mut().unwrap().until = Instant::now() - RETRY;
        let client = crate::llm::http::client();
        assert!(daybreak.refresh(&client, &base, &credential).await);
        assert!(!daybreak.refresh(&client, &base, &credential).await);
        let mut provider = provider();
        daybreak.apply(&mut provider, &credential);
        assert_eq!(provider.models.len(), 3);
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        task.abort();
    }
}

#[tokio::test]
async fn standard_only_accounts_and_api_keys_have_no_daybreak_entries() {
    let calls = Arc::new(AtomicUsize::new(0));
    let body = json!({ "models": [{ "slug": "gpt-6-sol", "available_access_programs": { "cyber": ["standard"] } }] });
    let (base, task) = server(body, axum::http::StatusCode::OK, calls.clone()).await;
    let credential = credential("user", "account");
    let daybreak = Daybreak::default();
    let client = crate::llm::http::client();
    daybreak.refresh(&client, &base, &credential).await;
    let mut provider = provider();
    daybreak.apply(&mut provider, &credential);
    assert_eq!(provider.models.len(), 3);
    daybreak
        .refresh(&client, &base, &Credential::ApiKey { key: "key".into() })
        .await;
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    task.abort();
}

#[tokio::test]
async fn logout_during_discovery_cannot_repopulate_the_cache() {
    let entered = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(tokio::sync::Notify::new());
    let signals = (entered.clone(), release.clone());
    let app = axum::Router::new().route(
        "/models",
        axum::routing::get(move || {
            let (entered, release) = signals.clone();
            async move {
                entered.notify_one();
                release.notified().await;
                axum::Json(list())
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let task = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let daybreak = Daybreak::default();
    let client = crate::llm::http::client();
    let credential = credential("user", "account");
    let (changed, ()) = tokio::join!(daybreak.refresh(&client, &base, &credential), async {
        entered.notified().await;
        daybreak.clear();
        release.notify_one();
    });
    assert!(!changed);
    assert!(daybreak.state.read().unwrap().cached.is_none());
    task.abort();
}
