//! Models a local server (LM Studio, Ollama) actually has, asked of the server itself rather than
//! taken from models.dev, which lists only a few and knows nothing of what is installed.

use std::collections::BTreeMap;
use std::sync::Mutex;
use std::time::Duration;

use serde_json::Value;

use super::catalog::{Limit, Model, ToolProfile};

/// A local server that is not running refuses at once; one that is answers well inside this.
const ASK_WITHIN: Duration = Duration::from_secs(2);

/// The local routes and where they listen unless the user's drift.json says otherwise.
pub const LOCAL: [(&str, &str, &str); 2] = [("lmstudio", "LM Studio", "http://127.0.0.1:1234/v1"), ("ollama", "Ollama", "http://127.0.0.1:11434/v1")];

/// Each Ollama model's own `num_ctx` (capped at its trained length) by server and model, asked once:
/// `/api/show` describes the installed model, which changes only when the list does.
static OLLAMA_SET: Mutex<BTreeMap<String, Option<u64>>> = Mutex::new(BTreeMap::new());

/// The chat models the server at `base` (an OpenAI-compatible `/v1` root) offers; `None` when it does not answer.
pub async fn discover(client: &reqwest::Client, provider: &str, base: &str) -> Option<Vec<Model>> {
    let base = base.trim_end_matches('/');
    let listed = get(client, &format!("{base}/models")).await?;
    let details = lm_studio_details(client, base).await;
    let mut models: Vec<Model> = listed["data"]
        .as_array()?
        .iter()
        .filter_map(|entry| entry["id"].as_str())
        .filter_map(|id| model(id, details.as_ref().and_then(|d| d.iter().find(|m| m["id"] == id))))
        .collect();
    if provider == "ollama" {
        forget_if_changed(base, &models);
        let running = ollama_running(client, base).await;
        for model in &mut models {
            model.limit.context = match running.get(&model.id) {
                Some(window) => *window,
                None => ollama_set(client, base, &model.id).await.unwrap_or(0),
            };
        }
    }
    Some(models)
}

async fn get(client: &reqwest::Client, url: &str) -> Option<Value> {
    read(tokio::time::timeout(ASK_WITHIN, client.get(url).send()).await.ok()?.ok()?).await
}

async fn read(response: reqwest::Response) -> Option<Value> {
    if !response.status().is_success() {
        return None;
    }
    tokio::time::timeout(ASK_WITHIN, response.json::<Value>()).await.ok()?.ok()
}

/// A model gone from the list may come back re-created with other settings: ask about every model again.
/// A newly pulled one is asked about anyway, having no answer yet.
fn forget_if_changed(base: &str, models: &[Model]) {
    let mut set = OLLAMA_SET.lock().unwrap();
    let prefix = format!("{base}|");
    let gone = set.keys().filter_map(|key| key.strip_prefix(&prefix)).any(|id| !models.iter().any(|m| m.id == id));
    if gone {
        set.retain(|key, _| !key.starts_with(&prefix));
    }
}

/// The window each loaded model really runs with, as Ollama allocated it (`/api/ps`), by model id.
async fn ollama_running(client: &reqwest::Client, base: &str) -> BTreeMap<String, u64> {
    let root = base.strip_suffix("/v1").unwrap_or(base);
    let Some(listed) = get(client, &format!("{root}/api/ps")).await else { return BTreeMap::new() };
    let models = listed["models"].as_array().cloned().unwrap_or_default();
    let mut windows = BTreeMap::new();
    for model in models {
        let Some(window) = model["context_length"].as_u64() else { continue };
        for name in [&model["name"], &model["model"]].into_iter().filter_map(Value::as_str) {
            windows.insert(name.to_string(), window);
            windows.entry(format!("{name}:latest")).or_insert(window);
        }
    }
    windows
}

/// A model's own `num_ctx`, never more than it was trained for; `None` when it sets none, since
/// then Ollama decides when it loads (its default depends on the server's memory).
async fn ollama_set(client: &reqwest::Client, base: &str, model: &str) -> Option<u64> {
    let key = format!("{base}|{model}");
    if let Some(known) = OLLAMA_SET.lock().unwrap().get(&key) {
        return *known;
    }
    let root = base.strip_suffix("/v1").unwrap_or(base);
    let shown = async { read(client.post(format!("{root}/api/show")).json(&serde_json::json!({ "model": model })).send().await.ok()?).await };
    let shown = tokio::time::timeout(ASK_WITHIN, shown).await.ok().flatten()?;
    let set = shown["parameters"].as_str().and_then(|parameters| {
        parameters.lines().find_map(|line| line.trim().strip_prefix("num_ctx").and_then(|value| value.trim().parse::<u64>().ok()))
    });
    let trained = shown["model_info"].as_object().and_then(|info| info.iter().find(|(key, _)| key.ends_with(".context_length")).and_then(|(_, value)| value.as_u64()));
    let window = set.map(|set| trained.map_or(set, |trained| set.min(trained)));
    OLLAMA_SET.lock().unwrap().insert(key, window);
    window
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
    // A model LM Studio has not loaded gets its own default window when it loads, not its maximum: unknown.
    let context = details.filter(|d| d["state"] == "loaded").and_then(|d| d["loaded_context_length"].as_u64()).unwrap_or(0);
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
                        { "id": "qwen3-coder", "type": "llm", "state": "loaded", "loaded_context_length": 65536, "max_context_length": 262144, "capabilities": ["tool_use"] },
                        { "id": "llava", "type": "vlm", "state": "not-loaded", "max_context_length": 4096, "capabilities": ["tool_use"] },
                        { "id": "no-tools", "type": "llm", "max_context_length": 8192, "capabilities": ["vision"] }
                    ] }))
                }),
            );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}/v1", listener.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let _ = rustls::crypto::ring::default_provider().install_default();
        let client = crate::llm::http::client();
        let models = discover(&client, "lmstudio", &base).await.unwrap();
        let found: Vec<(&str, u64, bool)> = models.iter().map(|m| (m.id.as_str(), m.limit.context, m.attachment)).collect();
        assert_eq!(found, [("qwen3-coder", 65536, false), ("llava", 0, true)], "a loaded model's window; an unloaded one's is unknown; embeddings and tool-less models stay out");
        let stopped = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap().local_addr().unwrap();
        assert!(discover(&client, "lmstudio", &format!("http://{stopped}/v1")).await.is_none());
    }

    #[tokio::test]
    async fn ollama_models_get_the_window_ollama_runs_them_with() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let shows = std::sync::Arc::new(AtomicUsize::new(0));
        let counted = shows.clone();
        let app = axum::Router::new()
            .route("/v1/models", axum::routing::get(|| async { axum::Json(json!({ "data": [{ "id": "loaded:latest" }, { "id": "set" }, { "id": "unset" }, { "id": "tiny" }] })) }))
            .route("/api/ps", axum::routing::get(|| async { axum::Json(json!({ "models": [{ "name": "loaded:latest", "model": "loaded:latest", "context_length": 262144 }] })) }))
            .route(
                "/api/show",
                axum::routing::post(move |axum::Json(body): axum::Json<Value>| {
                    counted.fetch_add(1, Ordering::SeqCst);
                    async move {
                        let info = |n: u64| json!({ "llama.context_length": n });
                        axum::Json(match body["model"].as_str() {
                            Some("set") => json!({ "parameters": "temperature 0.7\nnum_ctx 32768", "model_info": info(131072) }),
                            Some("tiny") => json!({ "parameters": "num_ctx 8192", "model_info": info(2048) }),
                            _ => json!({ "model_info": info(131072) }),
                        })
                    }
                }),
            );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}/v1", listener.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let _ = rustls::crypto::ring::default_provider().install_default();
        let client = crate::llm::http::client();
        let models = discover(&client, "ollama", &base).await.unwrap();
        let windows: Vec<(&str, u64)> = models.iter().map(|m| (m.id.as_str(), m.limit.context)).collect();
        assert_eq!(windows, [("loaded:latest", 262144), ("set", 32768), ("unset", 0), ("tiny", 2048)], "loaded as allocated; else num_ctx within the trained length; else unknown");
        discover(&client, "ollama", &base).await.unwrap();
        assert_eq!(shows.load(Ordering::SeqCst), 3, "each installed model is shown once, not every poll");
    }
}
