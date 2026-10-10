//! A signed-in account, a small catalog and a local stand-in for the Codex model list.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use axum::extract::Query;
use axum::http::{HeaderMap, StatusCode};
use serde_json::json;

use super::*;
use crate::llm::catalog::{Model, ModelMode};

mod catalog;
mod refresh;

fn credential(user: &str, account: &str) -> Credential {
    let claims = json!({ "sub": user, "chatgpt_compute_residency": "eu" }).to_string();
    let payload = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(claims);

    Credential::OAuth {
        access: format!("h.{payload}.s"),
        refresh: "refresh".into(),
        expires_at: i64::MAX,
        account: Some(account.into()),
    }
}

/// Three plain models, and a Fast and an Ultrafast mode of `gpt-6-sol`.
fn provider() -> ProviderInfo {
    let mut models: BTreeMap<String, Model> = ["gpt-6-sol", "gpt-6-astra", "gpt-5.6-cyber"]
        .map(|id| (id.to_string(), model(id)))
        .into_iter()
        .collect();

    for (name, tier) in [("fast", "priority"), ("ultrafast", "ultrafast")] {
        let mut speed = model("gpt-6-sol");
        speed.id = format!("gpt-6-sol-{name}");
        speed.mode = Some(ModelMode {
            name: name.into(),
            base: "gpt-6-sol".into(),
            body: json!({ "service_tier": tier }).as_object().unwrap().clone(),
            headers: BTreeMap::from([("x-test".into(), name.into())]),
        });
        models.insert(speed.id.clone(), speed);
    }

    ProviderInfo {
        id: "openai".into(),
        name: "OpenAI".into(),
        env: vec![],
        api: None,
        models,
    }
}

fn model(id: &str) -> Model {
    serde_json::from_value(json!({
        "id": id, "name": id, "reasoning": true, "attachment": true, "profile": "apply_patch",
        "limit": { "context": 400000, "output": 128000 },
        "cost": { "input": 2.0, "output": 10.0 },
        "variants": [{ "name": "high", "kind": "effort", "level": "high" }]
    }))
    .unwrap()
}

/// The list as the backend returns it for an account with Daybreak Blue.
fn list() -> Value {
    let fast = json!([{ "id": "priority", "name": "Fast" }]);

    json!({ "models": [
        { "slug": "gpt-6-sol", "service_tiers": fast, "available_access_programs": { "cyber": ["standard", "daybreak_blue"] } },
        { "slug": "gpt-6-astra", "service_tiers": fast, "available_access_programs": { "cyber": ["standard"] } },
        { "slug": "gpt-5.6-cyber", "available_access_programs": { "cyber": ["daybreak_red"] } },
        { "slug": "unknown", "available_access_programs": { "cyber": ["future_program"] } },
        { "slug": "no-programs" }
    ] })
}

/// Offers already cached for `credential`, as a fetch of `list()` leaves them.
fn cached(credential: &Credential) -> Offers {
    let offers = Offers::default();
    offers.state.write().unwrap().cached = Some(Cached {
        identity: identity(credential).unwrap(),
        until: Instant::now() + TTL,
        offers: list::read(list()),
    });

    offers
}

/// Serves `body` as the model list, checking each request the way the backend requires it.
async fn server(body: Value, status: StatusCode, calls: Arc<AtomicUsize>) -> (String, tokio::task::JoinHandle<()>) {
    let answer = move |headers: HeaderMap, query: Query<BTreeMap<String, String>>| {
        let (body, calls) = (body.clone(), calls.clone());
        async move {
            calls.fetch_add(1, Ordering::SeqCst);

            assert_eq!(query["client_version"], crate::VERSION);
            assert!(headers["authorization"].to_str().unwrap().starts_with("Bearer "));
            assert_eq!(headers["chatgpt-account-id"], "account");
            assert_eq!(headers["originator"], crate::llm::openai::CODEX_ORIGINATOR);
            assert_eq!(headers["x-openai-internal-codex-residency"], "eu");

            (status, axum::Json(body))
        }
    };

    let app = axum::Router::new().route("/models", axum::routing::get(answer));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let task = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

    (base, task)
}
