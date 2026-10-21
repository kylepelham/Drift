//! Models a local server (LM Studio, Ollama) actually has, asked of the server itself rather than
//! taken from models.dev, which lists only a few and knows nothing of what is installed.

use std::collections::BTreeMap;
use std::sync::Mutex;
use std::time::Duration;

use serde_json::Value;

use super::catalog::{Limit, Model, Reasoning, ToolProfile, Variant};

/// A local server that is not running refuses at once; one that is answers well inside this.
const ASK_WITHIN: Duration = Duration::from_secs(2);

/// The local routes and where they listen unless the user's drift.json says otherwise.
pub const LOCAL: [(&str, &str, &str); 2] = [
    ("lmstudio", "LM Studio", "http://127.0.0.1:1234/v1"),
    ("ollama", "Ollama", super::ollama::LOCAL_ROOT),
];

/// What `/api/show` says of each Ollama model, by the digest of what is installed: asked once per
/// build of a model, and a model re-created under the same name has a new digest, so it is asked again.
#[derive(Default)]
pub struct Shown(Mutex<BTreeMap<String, Showing>>);

/// One Ollama model as `/api/show` describes it.
#[derive(Clone, Debug, Default, PartialEq)]
struct Showing {
    /// The window it was trained for, else the `num_ctx` its Modelfile sets; `None` when neither is known.
    window: Option<u64>,
    /// `false` only when Ollama lists its capabilities without `tools`.
    tools: bool,
    vision: bool,
    thinking: bool,
    /// The thinking levels it takes (`low`, `medium`, `high`); empty when it only turns thinking on or off.
    levels: Vec<String>,
}

/// The chat models the server at `base` offers, each window at most `limit`; `None` when it does not answer.
/// LM Studio's `base` is its OpenAI-compatible `/v1` root; Ollama's is its root, or `/v1` as older drift.json files say.
pub async fn discover(
    client: &reqwest::Client,
    provider: &str,
    base: &str,
    shown: &Shown,
    limit: Option<u64>,
) -> Option<Vec<Model>> {
    if provider == "ollama" {
        return ollama_models(client, &super::ollama::root(base), shown, limit).await;
    }

    let base = base.trim_end_matches('/');
    let listed = get(client, &format!("{base}/models")).await?;
    let details = lm_studio_details(client, base).await;

    let mut models: Vec<Model> = listed["data"]
        .as_array()?
        .iter()
        .filter_map(|entry| entry["id"].as_str())
        .filter_map(|id| {
            let description = details
                .as_ref()
                .and_then(|details| details.iter().find(|model| model["id"] == id));
            model(id, description)
        })
        .collect();
    for model in &mut models {
        cap(model, limit);
    }

    Some(models)
}

/// Ollama's installed chat models, from Ollama itself: each at the window it was trained for, which every
/// request asks for (`num_ctx`), with what it can do. `None` unless what answers at `root` is Ollama.
async fn ollama_models(client: &reqwest::Client, root: &str, shown: &Shown, limit: Option<u64>) -> Option<Vec<Model>> {
    // Another server on Ollama's port answers `/api/version` with something other than a version number.
    let version = get(client, &format!("{root}/api/version")).await?;
    if !is_ollama_version(version["version"].as_str()?) {
        return None;
    }

    let installed = get(client, &format!("{root}/api/tags")).await?;
    let mut models = Vec::new();
    for entry in installed["models"].as_array()? {
        // An embedding model is left out before anything more is asked of it.
        let Some(listed) = entry["name"].as_str().and_then(|name| model(name, None)) else {
            continue;
        };
        let name = listed.id.as_str();
        let digest = entry["digest"]
            .as_str()
            .map_or_else(|| format!("{root}|{name}"), str::to_string);

        let showing = shown.describe(client, root, name, &digest).await;
        if showing.tools {
            models.push(ollama_model(listed, &showing, limit));
        }
    }

    Some(models)
}

/// `0.12.3` and the like; `0.0.0` from a development build counts too.
fn is_ollama_version(version: &str) -> bool {
    let mut parts = version.trim_start_matches('v').split('.');
    let numbered = |part: Option<&str>| part.is_some_and(|part| part.starts_with(|c: char| c.is_ascii_digit()));

    numbered(parts.next()) && numbered(parts.next())
}

/// An installed model with what `/api/show` says it can do.
fn ollama_model(mut model: Model, showing: &Showing, limit: Option<u64>) -> Model {
    model.attachment = showing.vision;
    model.reasoning = showing.thinking;
    model.limit.context = showing.window.unwrap_or(0);
    model.variants = showing
        .levels
        .iter()
        .map(|level| Variant {
            name: level.clone(),
            reasoning: Reasoning::Effort { level: level.clone() },
        })
        .collect();
    cap(&mut model, limit);

    model
}

/// Holds a model's window to the user's limit, from drift.json's `contextLimit`.
fn cap(model: &mut Model, limit: Option<u64>) {
    if let Some(limit) = limit.filter(|limit| *limit > 0)
        && (model.limit.context == 0 || model.limit.context > limit)
    {
        model.limit.context = limit;
    }
}

async fn get(client: &reqwest::Client, url: &str) -> Option<Value> {
    let response = tokio::time::timeout(ASK_WITHIN, client.get(url).send())
        .await
        .ok()?
        .ok()?;

    read(response).await
}

async fn read(response: reqwest::Response) -> Option<Value> {
    if !response.status().is_success() {
        return None;
    }

    tokio::time::timeout(ASK_WITHIN, response.json::<Value>())
        .await
        .ok()?
        .ok()
}

impl Shown {
    /// Caches `/api/show` by model digest. An unanswered request assumes tool support and is retried on the
    /// next discovery.
    async fn describe(&self, client: &reqwest::Client, root: &str, model: &str, digest: &str) -> Showing {
        if let Some(known) = self.0.lock().unwrap().get(digest) {
            return known.clone();
        }

        let shown = async {
            read(
                client
                    .post(format!("{root}/api/show"))
                    .json(&serde_json::json!({ "model": model }))
                    .send()
                    .await
                    .ok()?,
            )
            .await
        };
        let Some(shown) = tokio::time::timeout(ASK_WITHIN, shown).await.ok().flatten() else {
            return Showing {
                tools: true,
                ..Showing::default()
            };
        };

        let set = shown["parameters"].as_str().and_then(|parameters| {
            parameters.lines().find_map(|line| {
                line.trim()
                    .strip_prefix("num_ctx")
                    .and_then(|value| value.trim().parse::<u64>().ok())
            })
        });
        let trained = shown["model_info"].as_object().and_then(|info| {
            info.iter()
                .find(|(key, _)| key.ends_with(".context_length"))
                .and_then(|(_, value)| value.as_u64())
        });
        let capabilities: Vec<&str> = shown["capabilities"]
            .as_array()
            .map(|c| c.iter().filter_map(Value::as_str).collect())
            .unwrap_or_default();
        let levels = shown["thinking"]["values"]
            .as_array()
            .map(|values| values.iter().filter_map(Value::as_str).map(str::to_string).collect())
            .unwrap_or_default();
        let showing = Showing {
            window: trained.or(set),
            tools: capabilities.is_empty() || capabilities.contains(&"tools"),
            vision: capabilities.contains(&"vision"),
            thinking: capabilities.contains(&"thinking"),
            levels,
        };
        self.0.lock().unwrap().insert(digest.into(), showing.clone());

        showing
    }
}

/// LM Studio's own listing says each model's kind, context and tool support; other servers lack it.
async fn lm_studio_details(client: &reqwest::Client, base: &str) -> Option<Vec<Value>> {
    let root = base.strip_suffix("/v1").unwrap_or(base);
    get(client, &format!("{root}/api/v0/models")).await?["data"]
        .as_array()
        .cloned()
}

/// A model as the catalog holds it; embedding models are left out, since they cannot hold a conversation.
fn model(id: &str, details: Option<&Value>) -> Option<Model> {
    let kind = details.and_then(|d| d["type"].as_str()).unwrap_or("llm");
    if kind == "embeddings" || id.contains("embed") {
        return None;
    }

    // LM Studio chooses an unloaded model's default window at load time, not from its maximum.
    let context = details
        .filter(|d| d["state"] == "loaded")
        .and_then(|d| d["loaded_context_length"].as_u64())
        .unwrap_or(0);
    let capabilities = details
        .map(|d| d["capabilities"].as_array().cloned().unwrap_or_default())
        .unwrap_or_default();
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
        limit: Limit {
            context,
            output: 0,
            input: 0,
            compact: 0,
        },
        cost: Default::default(),
        profile: ToolProfile::Edit,
        prompt: Default::default(),
        variants: Vec::new(),
        mode: None,
        effort_updates: false,
        claude: Default::default(),
        tool_search: false,
    })
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use serde_json::json;

    use super::*;

    #[tokio::test]
    async fn a_running_server_lists_its_chat_models_and_a_stopped_one_lists_nothing() {
        let listed = json!({
            "data": [{ "id": "qwen3-coder" }, { "id": "text-embedding-nomic" }, { "id": "llava" }, { "id": "no-tools" }]
        });
        let detailed = json!({ "data": [
            {
                "id": "qwen3-coder",
                "type": "llm",
                "state": "loaded",
                "loaded_context_length": 65536,
                "max_context_length": 262144,
                "capabilities": ["tool_use"]
            },
            {
                "id": "llava",
                "type": "vlm",
                "state": "not-loaded",
                "max_context_length": 4096,
                "capabilities": ["tool_use"]
            },
            { "id": "no-tools", "type": "llm", "max_context_length": 8192, "capabilities": ["vision"] }
        ] });

        let app = axum::Router::new()
            .route(
                "/v1/models",
                axum::routing::get(move || async move { axum::Json(listed) }),
            )
            .route(
                "/api/v0/models",
                axum::routing::get(move || async move { axum::Json(detailed) }),
            );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}/v1", listener.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let _ = rustls::crypto::ring::default_provider().install_default();
        let client = crate::llm::http::client();

        let models = discover(&client, "lmstudio", &base, &Shown::default(), None)
            .await
            .unwrap();
        let found: Vec<(&str, u64, bool)> = models
            .iter()
            .map(|m| (m.id.as_str(), m.limit.context, m.attachment))
            .collect();
        assert_eq!(
            found,
            [("qwen3-coder", 65536, false), ("llava", 0, true)],
            "a loaded model's window; an unloaded one's is unknown; embeddings and tool-less models stay out"
        );

        // A port nothing listens on.
        let stopped = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .unwrap()
            .local_addr()
            .unwrap();
        assert!(
            discover(
                &client,
                "lmstudio",
                &format!("http://{stopped}/v1"),
                &Shown::default(),
                None
            )
            .await
            .is_none()
        );
    }

    /// Ollama's `/api/tags`: the installed models, `long` carrying a digest that changes with each `build`.
    fn installed(build: usize) -> Value {
        json!({ "models": [
            { "name": "long", "digest": format!("long-{build}") },
            { "name": "set", "digest": "s" },
            { "name": "unknown", "digest": "u" },
            { "name": "chatty", "digest": "c" },
            { "name": "nomic-embed-text", "digest": "e" }
        ] })
    }

    /// What Ollama's `/api/show` answers for each model the test installs.
    fn shown_model(model: Option<&str>) -> Value {
        let info = |n: u64| json!({ "llama.context_length": n });
        match model {
            Some("long") => json!({
                "parameters": "temperature 0.7\nnum_ctx 4096",
                "model_info": info(262_144),
                "capabilities": ["completion", "tools", "vision", "thinking"],
                "thinking": { "values": ["low", "medium", "high"], "default": "medium" }
            }),
            Some("set") => json!({ "parameters": "num_ctx 8192", "capabilities": ["completion", "tools"] }),
            Some("chatty") => json!({ "model_info": info(8192), "capabilities": ["completion"] }),
            _ => json!({}),
        }
    }

    /// A fake Ollama answering `/api/version` with `version`; counts `/api/show` calls.
    async fn ollama(version: &'static str) -> (String, Arc<AtomicUsize>, Arc<AtomicUsize>) {
        let shows = Arc::new(AtomicUsize::new(0));
        let counted = shows.clone();
        let build = Arc::new(AtomicUsize::new(1));
        let rebuilt = build.clone();
        let app = axum::Router::new()
            .route(
                "/api/version",
                axum::routing::get(move || async move { axum::Json(json!({ "version": version })) }),
            )
            .route(
                "/api/tags",
                axum::routing::get(move || {
                    let tags = installed(rebuilt.load(Ordering::SeqCst));
                    async move { axum::Json(tags) }
                }),
            )
            .route(
                "/api/show",
                axum::routing::post(move |axum::Json(body): axum::Json<Value>| {
                    counted.fetch_add(1, Ordering::SeqCst);
                    async move { axum::Json(shown_model(body["model"].as_str())) }
                }),
            );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let root = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let _ = rustls::crypto::ring::default_provider().install_default();

        (root, shows, build)
    }

    #[tokio::test]
    async fn ollama_models_run_at_the_window_they_were_trained_for_with_what_they_can_do() {
        let (root, shows, build) = ollama("0.12.6").await;
        let client = crate::llm::http::client();
        let shown = Shown::default();

        // An older drift.json names Ollama by its `/v1` root; either form reaches it.
        let models = discover(&client, "ollama", &format!("{root}/v1"), &shown, None)
            .await
            .unwrap();
        let found: Vec<(&str, u64, bool, bool)> = models
            .iter()
            .map(|m| (m.id.as_str(), m.limit.context, m.attachment, m.reasoning))
            .collect();
        assert_eq!(
            found,
            [
                ("long", 262_144, true, true),
                ("set", 8192, false, false),
                ("unknown", 0, false, false)
            ],
            "the trained window over a smaller Modelfile num_ctx; num_ctx when nothing else is known; \
             a model without tools and an embedding model left out"
        );
        let levels: Vec<&str> = models[0].variants.iter().map(|variant| variant.name.as_str()).collect();
        assert_eq!(
            levels,
            ["low", "medium", "high"],
            "its thinking levels become reasoning variants"
        );

        discover(&client, "ollama", &root, &shown, None).await.unwrap();
        assert_eq!(
            shows.load(Ordering::SeqCst),
            4,
            "each installed model is shown once, not every poll"
        );
        build.store(2, Ordering::SeqCst);
        discover(&client, "ollama", &root, &shown, None).await.unwrap();
        assert_eq!(
            shows.load(Ordering::SeqCst),
            5,
            "a model re-created under its name has a new digest, so it is asked again"
        );
    }

    #[tokio::test]
    async fn the_users_limit_holds_every_window_and_fills_an_unknown_one() {
        let (root, _, _) = ollama("0.12.6").await;
        let client = crate::llm::http::client();

        let models = discover(&client, "ollama", &root, &Shown::default(), Some(65_536))
            .await
            .unwrap();
        let windows: Vec<u64> = models.iter().map(|model| model.limit.context).collect();

        assert_eq!(windows, [65_536, 8192, 65_536]);
    }

    #[tokio::test]
    async fn another_server_on_ollamas_port_is_not_taken_for_ollama() {
        let (root, shows, _) = ollama("llama-fim shim").await;
        let client = crate::llm::http::client();

        assert!(
            discover(&client, "ollama", &root, &Shown::default(), None)
                .await
                .is_none()
        );
        assert_eq!(shows.load(Ordering::SeqCst), 0, "nothing more is asked of it");
        assert!(is_ollama_version("0.0.0") && is_ollama_version("v0.13.1"));
        assert!(!is_ollama_version("") && !is_ollama_version("1") && !is_ollama_version("llama-fim shim"));
    }
}
