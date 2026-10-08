use super::*;

#[tokio::test]
async fn a_server_on_the_older_sse_transport_connects_and_answers() {
    let base = legacy_sse_server().await;
    let engine = engine();
    let config = ServerConfig::Sse {
        url: format!("{base}/sse"),
        headers: Default::default(),
        oauth: None,
        timeout_seconds: None,
    };
    let row = saved(&engine, "legacy", &config).await;
    engine.connect_mcp_in("legacy", Some(&here())).await.unwrap();

    let output = tool(&engine, "legacy_echo")
        .run(&context(&engine), json!({ "text": "over sse" }))
        .await
        .unwrap();
    assert_eq!(output.output, "over sse");
    assert_eq!(engine.mcp.status_of(row).transport, Transport::Sse);
}

/// Serves the 2024-11-05 HTTP+SSE transport: GET names the message endpoint,
/// and responses to POST requests return through the same event stream.
async fn legacy_sse_server() -> String {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, legacy_sse_routes()).await.unwrap() });

    base
}

/// The HTTP+SSE transport's two routes: `/sse` streams replies, `/messages` takes requests.
pub(super) fn legacy_sse_routes() -> axum::Router {
    use axum::routing::{get, post};

    let (sender, receiver) = tokio::sync::mpsc::unbounded_channel::<String>();
    let receiver = Arc::new(tokio::sync::Mutex::new(Some(receiver)));
    let stream = get(move || {
        let receiver = receiver.clone();
        async move {
            let receiver = receiver.lock().await.take().expect("one stream");
            let first = futures_util::stream::once(async {
                Ok::<_, std::convert::Infallible>("event: endpoint\ndata: /messages?session=1\n\n".to_string())
            });
            let rest = futures_util::stream::unfold(receiver, |mut receiver| async move {
                receiver
                    .recv()
                    .await
                    .map(|message| (Ok(format!("event: message\ndata: {message}\n\n")), receiver))
            });

            (
                [("content-type", "text/event-stream")],
                axum::body::Body::from_stream(futures_util::StreamExt::chain(first, rest)),
            )
        }
    });

    let messages = post(move |axum::Json(message): axum::Json<serde_json::Value>| {
        let sender = sender.clone();
        async move {
            let result = match message["method"].as_str() {
                Some("initialize") => json!({
                    "protocolVersion": "2024-11-05", "capabilities": { "tools": {} },
                    "serverInfo": { "name": "legacy", "version": "0" },
                }),
                Some("tools/list") => json!({ "tools": [{
                    "name": "echo", "inputSchema": { "type": "object", "properties": { "text": { "type": "string" } } },
                    "annotations": { "readOnlyHint": true },
                }] }),
                Some("tools/call") => {
                    json!({ "content": [{ "type": "text", "text": message["params"]["arguments"]["text"] }] })
                }
                _ => serde_json::Value::Null,
            };
            if !message["id"].is_null() {
                let reply = json!({ "jsonrpc": "2.0", "id": message["id"], "result": result }).to_string();
                let _ = sender.send(reply);
            }

            axum::http::StatusCode::ACCEPTED
        }
    });

    axum::Router::new().route("/sse", stream).route("/messages", messages)
}
