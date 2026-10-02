//! Models a local server (LM Studio, Ollama) actually has, asked of the server itself rather than
//! taken from models.dev, which lists only a few and knows nothing of what is installed.

use std::time::Duration;

use serde_json::Value;

use super::catalog::{Limit, Model, ToolProfile};

/// A local server that is not running refuses at once; one that is answers well inside this.
const ASK_WITHIN: Duration = Duration::from_secs(2);

/// The local routes and where they listen unless the user's drift.json says otherwise.
pub const LOCAL: [(&str, &str, &str); 2] = [("lmstudio", "LM Studio", "http://127.0.0.1:1234/v1"), ("ollama", "Ollama", "http://127.0.0.1:11434/v1")];

/// The chat models the server at `base` (an OpenAI-compatible `/v1` root) offers; `None` when it does not answer.
pub async fn discover(client: &reqwest::Client, base: &str) -> Option<Vec<Model>> {
    let base = base.trim_end_matches('/');
    let listed = get(client, &format!("{base}/models")).await?;
    let details = lm_studio_details(client, base).await;
    let models = listed["data"]
        .as_array()?
        .iter()
        .filter_map(|entry| entry["id"].as_str())
        .filter_map(|id| model(id, details.as_ref().and_then(|d| d.iter().find(|m| m["id"] == id))))
        .collect();
    Some(models)
}

async fn get(client: &reqwest::Client, url: &str) -> Option<Value> {
    let response = tokio::time::timeout(ASK_WITHIN, client.get(url).send()).await.ok()?.ok()?;
    if !response.status().is_success() {
        return None;
    }
    tokio::time::timeout(ASK_WITHIN, response.json::<Value>()).await.ok()?.ok()
}

/// LM Studio's own listing says each model's kind, context and tool support; other servers lack it.
async fn lm_studio_details(client: &reqwest::Client, base: &str) -> Option<Vec<Value>> {
    let root = base.strip_suffix("/v1").unwrap_or(base);
    get(client, &format!("{root}/api/v0/models")).await?["data"].as_array().cloned()
}

/// A model as the catalog holds it; embedding models are left out, since they cannot hold a conversation.
fn model(id: &str, details: Option<&Value>) -> Option<Model> {
    let kind = details.and_then(|d| d["type"].as_str()).unwrap_or("llm");
    if kind == "embeddings" || id.contains("embed") {
        return None;
    }
    let context = details.and_then(|d| d["loaded_context_length"].as_u64().or(d["max_context_length"].as_u64())).unwrap_or(0);
    let capabilities = details.map(|d| d["capabilities"].as_array().cloned().unwrap_or_default()).unwrap_or_default();
    Some(Model {
        id: id.into(),
        name: id.into(),
        family: String::new(),
        reasoning: false,
        attachment: kind == "vlm",
        temperature: true,
        release_date: String::new(),
        limit: Limit { context, output: 0 },
        cost: Default::default(),
        profile: ToolProfile::Edit,
        variants: Vec::new(),
    })
    .filter(|_| details.is_none() || capabilities.is_empty() || capabilities.iter().any(|c| c == "tool_use"))
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[tokio::test]
    async fn a_running_server_lists_its_chat_models_and_a_stopped_one_lists_nothing() {
        let app = axum::Router::new()
            .route("/v1/models", axum::routing::get(|| async { axum::Json(json!({ "data": [{ "id": "qwen3-coder" }, { "id": "text-embedding-nomic" }, { "id": "llava" }, { "id": "no-tools" }] })) }))
            .route(
                "/api/v0/models",
                axum::routing::get(|| async {
                    axum::Json(json!({ "data": [
                        { "id": "qwen3-coder", "type": "llm", "loaded_context_length": 65536, "max_context_length": 262144, "capabilities": ["tool_use"] },
                        { "id": "llava", "type": "vlm", "max_context_length": 4096, "capabilities": ["tool_use"] },
                        { "id": "no-tools", "type": "llm", "max_context_length": 8192, "capabilities": ["vision"] }
                    ] }))
                }),
            );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}/v1", listener.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let _ = rustls::crypto::ring::default_provider().install_default();
        let client = crate::llm::http::client();
        let models = discover(&client, &base).await.unwrap();
        let found: Vec<(&str, u64, bool)> = models.iter().map(|m| (m.id.as_str(), m.limit.context, m.attachment)).collect();
        assert_eq!(found, [("qwen3-coder", 65536, false), ("llava", 4096, true)], "loaded context wins; embeddings and tool-less models stay out");
        let stopped = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap().local_addr().unwrap();
        assert!(discover(&client, &format!("http://{stopped}/v1")).await.is_none());
    }
}
