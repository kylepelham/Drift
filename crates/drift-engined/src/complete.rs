//! `--complete`: one text request per stdin line, `{"id", "system", "prompt", "maxTokens"}`, each answered on stdout
//! as `{"id", "text"}` or `{"id", "error"}` when it finishes. Requests run side by side; the process ends after stdin
//! closes and the last answer is written. The engine sends them with the user's credentials, which never leave it.

use std::sync::Arc;

use drift_engine::Engine;
use drift_engine::session::types::ModelRef;
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::sync::Mutex;
use tokio::task::JoinSet;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Request {
    id: String,
    system: String,
    prompt: String,
    max_tokens: u32,
}

#[derive(Debug, PartialEq, Serialize)]
struct Reply {
    id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

pub(crate) async fn serve<R, W>(engine: Arc<Engine>, model: ModelRef, input: R, output: W)
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin + Send + 'static,
{
    let output = Arc::new(Mutex::new(output));
    let mut lines = BufReader::new(input).lines();
    let mut running = JoinSet::new();

    while let Ok(Some(line)) = lines.next_line().await {
        if line.trim().is_empty() {
            continue;
        }
        let (engine, model, output) = (engine.clone(), model.clone(), output.clone());
        running.spawn(async move {
            let reply = answer(&engine, &model, &line).await;
            let mut text = serde_json::to_string(&reply).unwrap_or_default();
            text.push('\n');

            let mut output = output.lock().await;
            let _ = output.write_all(text.as_bytes()).await;
            let _ = output.flush().await;
        });
    }

    while running.join_next().await.is_some() {}
}

async fn answer(engine: &Engine, model: &ModelRef, line: &str) -> Reply {
    let request: Request = match serde_json::from_str(line) {
        Ok(request) => request,
        Err(error) => return failed(String::new(), format!("bad request: {error}")),
    };

    match engine
        .complete_text(model, request.system, request.prompt, request.max_tokens)
        .await
    {
        Ok(text) => Reply {
            id: request.id,
            text: Some(text),
            error: None,
        },
        Err(error) => failed(request.id, error),
    }
}

fn failed(id: String, error: String) -> Reply {
    Reply {
        id,
        text: None,
        error: Some(error),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn each_line_is_answered_with_its_id_and_a_bad_one_says_why() {
        let _ = rustls::crypto::ring::default_provider().install_default();
        let data = drift_engine::platform::temp::test_run().join(format!("drift-complete-{}", std::process::id()));
        let options = drift_engine::Options {
            file_credentials: true,
            ..Default::default()
        };
        let engine = Engine::open_with(&data, options).unwrap();
        let model = ModelRef {
            provider: "google".into(),
            model: "a-model-the-catalog-lacks".into(),
        };
        let input = "not json\n\n{\"id\":\"greeting\",\"system\":\"\",\"prompt\":\"Hello\",\"maxTokens\":16}\n";
        let (written, mut read) = tokio::io::duplex(4096);

        serve(engine, model, input.as_bytes(), written).await;
        let mut replies = String::new();
        tokio::io::AsyncReadExt::read_to_string(&mut read, &mut replies)
            .await
            .unwrap();

        let replies: Vec<serde_json::Value> = replies
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        assert_eq!(replies.len(), 2, "the blank line is skipped");
        let bad = replies.iter().find(|reply| reply["id"] == "").unwrap();
        assert!(bad["error"].as_str().unwrap().starts_with("bad request"));
        let unknown = replies.iter().find(|reply| reply["id"] == "greeting").unwrap();
        assert!(
            unknown["error"].is_string() && unknown.get("text").is_none(),
            "{unknown}"
        );
        let _ = std::fs::remove_dir_all(&data);
    }
}
