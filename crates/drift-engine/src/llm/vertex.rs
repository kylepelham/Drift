//! Vertex AI: Claude through Anthropic's publisher endpoint and Gemini through Google's, with a Google Cloud access token.

use super::{anthropic, gemini, google, ChunkStream, Credential, Error, Request};

const VERSION: &str = "vertex-2023-10-16";

#[derive(Clone, Debug)]
pub struct Vertex {
    /// `DRIFT_GOOGLE_VERTEX_BASE_URL` for recorded runs; otherwise the location's endpoint.
    base_url: Option<String>,
    client: reqwest::Client,
    pub timeouts: super::http::Timeouts,
}

impl Vertex {
    pub fn new(base_url: Option<String>) -> Self {
        Self { base_url, client: super::http::client(), timeouts: super::http::Timeouts::default() }
    }

    pub async fn stream(&self, request: &Request, credential: &Credential) -> Result<ChunkStream, Error> {
        let token = match credential {
            Credential::OAuth { access, .. } | Credential::ApiKey { key: access } => access.clone(),
            Credential::Ambient { .. } => google::token(&self.client, &self.timeouts).await?,
        };
        self.send(request, &token, &google::target()?).await
    }

    async fn send(&self, request: &Request, token: &str, target: &google::Target) -> Result<ChunkStream, Error> {
        let models = format!("{}/v1/projects/{}/locations/{}/publishers", self.base(&target.location), target.project, target.location);
        if is_claude(&request.model) {
            let url = format!("{models}/anthropic/models/{}:streamRawPredict", request.model);
            let mut http = self.client.post(url).bearer_auth(token).header("accept", "text/event-stream").json(&anthropic::cloud_body(request, VERSION, true));
            if anthropic::interleaves(request) {
                http = http.header("anthropic-beta", anthropic::INTERLEAVED_THINKING);
            }
            return anthropic::stream_from(http, &self.timeouts, false).await;
        }
        let url = format!("{models}/google/models/{}:streamGenerateContent?alt=sse", request.model);
        gemini::stream_from(self.client.post(url).bearer_auth(token), request, &self.timeouts).await
    }

    /// `global` has no region in its host name; every other location does.
    fn base(&self, location: &str) -> String {
        match &self.base_url {
            Some(base) => base.clone(),
            None if location == "global" => "https://aiplatform.googleapis.com".into(),
            None => format!("https://{location}-aiplatform.googleapis.com"),
        }
    }
}

/// Claude models speak Anthropic's wire on Vertex; everything else offered here is Gemini.
pub fn is_claude(model: &str) -> bool {
    model.starts_with("claude")
}

#[cfg(test)]
mod tests {
    use futures_util::StreamExt;

    use super::*;
    use crate::llm::Chunk;

    /// Each request: its path, its authorization and its `anthropic-beta`.
    type Seen = std::sync::Arc<std::sync::Mutex<Vec<(String, String, String)>>>;

    fn seen_beta(seen: &Seen) -> String {
        seen.lock().unwrap().last().unwrap().2.clone()
    }

    /// A local stand-in for Vertex that answers each publisher with its own SSE and records the paths.
    async fn fake() -> (String, Seen) {
        use axum::extract::{OriginalUri, State};
        let seen: Seen = Default::default();
        let handler = |State(seen): State<Seen>, OriginalUri(uri): OriginalUri, headers: axum::http::HeaderMap| async move {
            let auth = headers.get("authorization").and_then(|v| v.to_str().ok()).unwrap_or_default().to_string();
            let beta = headers.get("anthropic-beta").and_then(|v| v.to_str().ok()).unwrap_or_default().to_string();
            seen.lock().unwrap().push((uri.to_string(), auth, beta));
            let body = if uri.path().contains("/publishers/anthropic/") {
                "event: content_block_start\ndata: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"text\",\"text\":\"\"}}\n\nevent: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"claude on vertex\"}}\n\nevent: message_delta\ndata: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\"},\"usage\":{\"output_tokens\":3}}\n\n"
            } else {
                "data: {\"candidates\":[{\"content\":{\"parts\":[{\"text\":\"gemini on vertex\"}]},\"finishReason\":\"STOP\"}]}\n\n"
            };
            ([("content-type", "text/event-stream")], body)
        };
        let app = axum::Router::new().fallback(axum::routing::post(handler)).with_state(seen.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        (url, seen)
    }

    #[tokio::test]
    async fn claude_and_gemini_each_reach_their_publisher_with_the_token() {
        let _ = rustls::crypto::ring::default_provider().install_default();
        let (url, recorded) = fake().await;
        let vertex = Vertex::new(Some(url));
        let target = google::Target { project: "proj".into(), location: "us-east5".into() };
        let texts = |chunks: Vec<Chunk>| chunks.into_iter().filter_map(|c| if let Chunk::TextDelta(t) = c { Some(t) } else { None }).collect::<String>();
        let mut request = crate::llm::tests::request();
        request.model = "claude-sonnet-4-5@20250929".into();
        let claude: Vec<Chunk> = vertex.send(&request, "tok", &target).await.unwrap().map(Result::unwrap).collect().await;
        assert_eq!(texts(claude), "claude on vertex");
        request.model = "gemini-3.6-flash".into();
        let gemini: Vec<Chunk> = vertex.send(&request, "tok", &target).await.unwrap().map(Result::unwrap).collect().await;
        assert_eq!(texts(gemini), "gemini on vertex");
        let seen = recorded.lock().unwrap().clone();
        assert_eq!(seen[0].0, "/v1/projects/proj/locations/us-east5/publishers/anthropic/models/claude-sonnet-4-5@20250929:streamRawPredict");
        assert_eq!(seen[1].0, "/v1/projects/proj/locations/us-east5/publishers/google/models/gemini-3.6-flash:streamGenerateContent?alt=sse");
        assert!(seen.iter().all(|(_, auth, _)| auth == "Bearer tok"));
        assert!(seen.iter().all(|(_, _, beta)| beta.is_empty()), "no budget, no beta");
        request.model = "claude-sonnet-4-5@20250929".into();
        request.tools = vec![crate::llm::ToolSpec { name: "read".into(), description: "r".into(), input_schema: serde_json::json!({}) }];
        request.reasoning = Some(crate::llm::catalog::Reasoning::Budget { tokens: 4096 });
        vertex.send(&request, "tok", &target).await.unwrap().collect::<Vec<_>>().await;
        assert_eq!(seen_beta(&recorded), anthropic::INTERLEAVED_THINKING, "a budget with tools asks for interleaved thinking");
    }

    #[test]
    fn locations_and_publishers_pick_the_endpoint() {
        let vertex = Vertex::new(None);
        assert_eq!(vertex.base("global"), "https://aiplatform.googleapis.com");
        assert_eq!(vertex.base("us-east5"), "https://us-east5-aiplatform.googleapis.com");
        assert!(is_claude("claude-sonnet-4-5@20250929") && !is_claude("gemini-3.6-flash"));
        let body = anthropic::cloud_body(&crate::llm::tests::request(), VERSION, true);
        assert_eq!((body["anthropic_version"].as_str(), body["stream"].as_bool()), (Some(VERSION), Some(true)));
        assert!(body.get("model").is_none());
    }
}
