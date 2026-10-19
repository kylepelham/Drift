//! Web search through a SearXNG instance: answers the `websearch` tool as the provider `searxng`, asking
//! `config.url` (the instance, as `https://search.example.org`) for JSON results over the `http` host
//! interface. The instance must allow the `json` format in its `search.formats` setting.

wit_bindgen::generate!({ world: "searching", path: "../../crates/drift-engine/wit" });

use drift::plugin::host::config;
use drift::plugin::http::fetch;

struct Searxng;

type Hits = Vec<(String, String, String)>;

impl Guest for Searxng {
    fn name() -> String {
        "searxng".into()
    }

    fn search_providers() -> Vec<(String, String)> {
        vec![("searxng".into(), "SearXNG".into())]
    }

    fn search(_provider: String, query: String, limit: u32) -> Result<Hits, (String, Option<u32>)> {
        let config: serde_json::Value = serde_json::from_str(&config()).unwrap_or_default();
        let Some(base) = config["url"].as_str() else {
            return Err(("set config.url to your SearXNG instance".into(), None));
        };
        let url = format!(
            "{}/search?format=json&q={}",
            base.trim_end_matches('/'),
            encoded(&query)
        );
        let response = fetch("GET", &url, &[], None).map_err(|error| (error, None))?;
        match response.status {
            200 => Ok(hits(&response.body, limit)),
            429 => Err(("SearXNG is rate limiting".into(), Some(60))),
            403 => Err((
                "the instance does not serve JSON; allow it in search.formats".into(),
                None,
            )),
            status => Err((format!("SearXNG answered {status}"), None)),
        }
    }

    fn before_tool(_call: ToolCall) -> BeforeTool {
        BeforeTool::Allow
    }

    fn after_tool(_outcome: ToolResult) -> AfterTool {
        AfterTool::Keep
    }

    fn prompt_submit(_prompt: Prompt) -> PromptSubmit {
        PromptSubmit::Keep
    }

    fn turn_end(_reply: Reply) -> TurnEnd {
        TurnEnd::Accept
    }

    fn permission(_ask: PermissionAsk) -> Permission {
        Permission::Pass
    }

    fn compaction(_compaction: Compaction) -> Compacting {
        Compacting::Proceed
    }

    fn session(_session: Session, _kind: SessionKind) {}
}

fn hits(body: &str, limit: u32) -> Hits {
    let answer: serde_json::Value = serde_json::from_str(body).unwrap_or_default();
    let text = |value: &serde_json::Value, key: &str| value[key].as_str().unwrap_or_default().to_owned();

    answer["results"]
        .as_array()
        .into_iter()
        .flatten()
        .take(limit as usize)
        .map(|result| (text(result, "url"), text(result, "title"), text(result, "content")))
        .collect()
}

/// The query as a URL's query string carries it.
fn encoded(query: &str) -> String {
    query
        .bytes()
        .map(|byte| match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => (byte as char).to_string(),
            b' ' => "+".into(),
            _ => format!("%{byte:02X}"),
        })
        .collect()
}

export!(Searxng);
