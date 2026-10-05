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

/// What `/api/show` says of each Ollama model, by the digest of what is installed: asked once per
/// build of a model, and a model re-created under the same name has a new digest, so it is asked again.
#[derive(Default)]
pub struct Shown(Mutex<BTreeMap<String, Showing>>);

/// One Ollama model as `/api/show` describes it.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Showing {
    /// Its own `num_ctx`, never more than it was trained for; `None` when it sets none.
    window: Option<u64>,
    /// `false` only when Ollama lists its capabilities without `tools`.
    tools: bool,
    vision: bool,
}

/// The chat models the server at `base` (an OpenAI-compatible `/v1` root) offers; `None` when it does not answer.
pub async fn discover(client: &reqwest::Client, provider: &str, base: &str, shown: &Shown) -> Option<Vec<Model>> {
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
        models = ollama_details(client, base, shown, models).await;
    }
    Some(models)
}

/// Ollama's own answers for each model: a loaded one's allocated window (`/api/ps`), else its own
/// `num_ctx`; whether it reads images; and models that cannot call tools are left out.
async fn ollama_details(client: &reqwest::Client, base: &str, shown: &Shown, models: Vec<Model>) -> Vec<Model> {
    let running = ollama_listing(client, base, "ps", "context_length").await;
    let digests = ollama_listing(client, base, "tags", "digest").await;
    let mut kept = Vec::new();
    for mut model in models {
        let digest = digests.get(&model.id).and_then(Value::as_str).map_or_else(|| format!("{base}|{}", model.id), str::to_string);
        let showing = shown.describe(client, base, &model.id, &digest).await;
        if !showing.tools {
            continue;
        }
        model.attachment = showing.vision;
        model.limit.context = running.get(&model.id).and_then(Value::as_u64).or(showing.window).unwrap_or(0);
        kept.push(model);
    }
    kept
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

/// One field of each model Ollama lists at `/api/<what>` (`ps` for loaded models' windows, `tags`
/// for installed models' digests), by every name the model goes by.
async fn ollama_listing(client: &reqwest::Client, base: &str, what: &str, field: &str) -> BTreeMap<String, Value> {
    let root = base.strip_suffix("/v1").unwrap_or(base);
    let Some(listed) = get(client, &format!("{root}/api/{what}")).await else { return BTreeMap::new() };
    let mut by_name = BTreeMap::new();
    for model in listed["models"].as_array().cloned().unwrap_or_default() {
        let value = model[field].clone();
        if value.is_null() {
            continue;
        }
        for name in [&model["name"], &model["model"]].into_iter().filter_map(Value::as_str) {
            by_name.insert(name.to_string(), value.clone());
            by_name.entry(format!("{name}:latest")).or_insert(value.clone());
        }
    }
    by_name
}

impl Shown {
    /// What `/api/show` says of a model. A model whose window Ollama sets none of gets `None`, since
    /// then Ollama decides when it loads (its default depends on the server's memory). One that does
    /// not answer is taken as able to call tools, and asked again next time.
    async fn describe(&self, client: &reqwest::Client, base: &str, model: &str, digest: &str) -> Showing {
        if let Some(known) = self.0.lock().unwrap().get(digest) {
            return *known;
        }
        let root = base.strip_suffix("/v1").unwrap_or(base);
        let shown = async { read(client.post(format!("{root}/api/show")).json(&serde_json::json!({ "model": model })).send().await.ok()?).await };
        let Some(shown) = tokio::time::timeout(ASK_WITHIN, shown).await.ok().flatten() else { return Showing { tools: true, ..Showing::default() } };
        let set = shown["parameters"].as_str().and_then(|parameters| {
            parameters.lines().find_map(|line| line.trim().strip_prefix("num_ctx").and_then(|value| value.trim().parse::<u64>().ok()))
        });
        let trained = shown["model_info"].as_object().and_then(|info| info.iter().find(|(key, _)| key.ends_with(".context_length")).and_then(|(_, value)| value.as_u64()));
        let capabilities: Vec<&str> = shown["capabilities"].as_array().map(|c| c.iter().filter_map(Value::as_str).collect()).unwrap_or_default();
        let showing = Showing {
            window: set.map(|set| trained.map_or(set, |trained| set.min(trained))),
            tools: capabilities.is_empty() || capabilities.contains(&"tools"),
            vision: capabilities.contains(&"vision"),
        };
        self.0.lock().unwrap().insert(digest.into(), showing);
        showing
    }
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
    if details.is_some() && !capabilities.is_empty() && !capabilities.iter().any(|c| c == "tool_use") {
        return None;
    }
    Some(Model {
        id: id.into(),
        name: id.into(),
        family: String::new(),
        reasoning: false,
        attachment: kind == "vlm",
        pdf: false,
        temperature: true,
        release_date: String::new(),
        limit: Limit { context, output: 0, input: 0 },
        cost: Default::default(),
        profile: ToolProfile::Edit,
        prompt: Default::default(),
        variants: Vec::new(),
        mode: None,
    })
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
        let models = discover(&client, "lmstudio", &base, &Shown::default()).await.unwrap();
        let found: Vec<(&str, u64, bool)> = models.iter().map(|m| (m.id.as_str(), m.limit.context, m.attachment)).collect();
        assert_eq!(found, [("qwen3-coder", 65536, false), ("llava", 0, true)], "a loaded model's window; an unloaded one's is unknown; embeddings and tool-less models stay out");
        let stopped = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap().local_addr().unwrap();
        assert!(discover(&client, "lmstudio", &format!("http://{stopped}/v1"), &Shown::default()).await.is_none());
    }

    #[tokio::test]
    async fn ollama_models_get_the_window_ollama_runs_them_with() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let shows = std::sync::Arc::new(AtomicUsize::new(0));
        let counted = shows.clone();
        let build = std::sync::Arc::new(AtomicUsize::new(1));
        let rebuilt = build.clone();
        let app = axum::Router::new()
            .route("/v1/models", axum::routing::get(|| async { axum::Json(json!({ "data": [{ "id": "loaded:latest" }, { "id": "set" }, { "id": "unset" }, { "id": "tiny" }, { "id": "chatty" }] })) }))
            .route("/api/ps", axum::routing::get(|| async { axum::Json(json!({ "models": [{ "name": "loaded:latest", "model": "loaded:latest", "context_length": 262144 }] })) }))
            .route(
                "/api/tags",
                axum::routing::get(move || {
                    let build = rebuilt.load(Ordering::SeqCst);
                    let tags = json!({ "models": [{ "name": "loaded:latest", "digest": "l" }, { "name": "set", "digest": format!("set-{build}") }, { "name": "unset", "digest": "u" }, { "name": "tiny", "digest": "t" }, { "name": "chatty", "digest": "c" }] });
                    async move { axum::Json(tags) }
                }),
            )
            .route(
                "/api/show",
                axum::routing::post(move |axum::Json(body): axum::Json<Value>| {
                    counted.fetch_add(1, Ordering::SeqCst);
                    async move {
                        let info = |n: u64| json!({ "llama.context_length": n });
                        axum::Json(match body["model"].as_str() {
                            Some("set") => json!({ "parameters": "temperature 0.7\nnum_ctx 32768", "model_info": info(131072), "capabilities": ["completion", "tools", "vision"] }),
                            Some("tiny") => json!({ "parameters": "num_ctx 8192", "model_info": info(2048), "capabilities": ["completion", "tools"] }),
                            Some("chatty") => json!({ "model_info": info(8192), "capabilities": ["completion"] }),
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
        let shown = Shown::default();
        let models = discover(&client, "ollama", &base, &shown).await.unwrap();
        let found: Vec<(&str, u64, bool)> = models.iter().map(|m| (m.id.as_str(), m.limit.context, m.attachment)).collect();
        assert_eq!(
            found,
            [("loaded:latest", 262144, false), ("set", 32768, true), ("unset", 0, false), ("tiny", 2048, false)],
            "loaded as allocated; else num_ctx within the trained length; else unknown; vision read from its capabilities; a model without tools left out"
        );
        discover(&client, "ollama", &base, &shown).await.unwrap();
        assert_eq!(shows.load(Ordering::SeqCst), 5, "each installed model is shown once, not every poll");
        build.store(2, Ordering::SeqCst);
        discover(&client, "ollama", &base, &shown).await.unwrap();
        assert_eq!(shows.load(Ordering::SeqCst), 6, "a model re-created under its name has a new digest, so it is asked again");
    }
}
