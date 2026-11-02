use serde_json::{Value, json};

use super::{Failure, google, openai, priced};

#[test]
fn prices_come_from_token_counts_and_a_snapshot_is_priced_as_its_model() {
    let usage = json!({
        "input_tokens": 50,
        "input_tokens_details": { "text_tokens": 10, "image_tokens": 40 },
        "output_tokens": 1000,
        "output_tokens_details": { "image_tokens": 1000, "text_tokens": 0 }
    });
    let cost = openai::cost("gpt-image-2-2026-04-21", &usage).unwrap();
    assert!((cost - (10.0 * 5.0 + 40.0 * 8.0 + 1000.0 * 30.0) / 1e6).abs() < 1e-12);
    assert_eq!(
        openai::cost("dall-e-3", &usage),
        None,
        "an unknown model is not guessed at"
    );
    assert_eq!(openai::cost("gpt-image-2", &Value::Null), None);
    assert_eq!(
        priced(&[("recraftv4", [0.04]), ("recraftv4_1", [0.035])], "recraftv4_1_pro"),
        Some([0.035]),
        "the longest matching name wins"
    );

    let usage = json!({
        "promptTokenCount": 100,
        "candidatesTokenCount": 1130,
        "candidatesTokensDetails": [{ "modality": "IMAGE", "tokenCount": 1120 }, { "modality": "TEXT", "tokenCount": 10 }]
    });
    let cost = google::cost("gemini-3.1-flash-image", &usage).unwrap();
    assert!((cost - (100.0 * 0.5 + 10.0 * 3.0 + 1120.0 * 60.0) / 1e6).abs() < 1e-12);
}

#[test]
fn only_a_refused_or_spent_payer_hands_over() {
    let failure = |status| Failure {
        status: Some(status),
        message: String::new(),
    };
    for status in [401, 402, 403, 429] {
        assert!(failure(status).moves_on(), "{status}");
    }
    for status in [400, 422, 500, 503] {
        assert!(
            !failure(status).moves_on(),
            "{status} would fail the same way, or may already be billed"
        );
    }
    assert!(!Failure::from("timed out").moves_on());
}

#[test]
fn a_private_image_url_is_refused_unless_it_is_the_provider() {
    let blocked = |url: &str| super::fetchable(&reqwest::Url::parse(url).unwrap(), None).is_err();
    assert!(blocked("http://169.254.169.254/latest/meta-data"));
    assert!(blocked("http://127.0.0.1:9/secret"));
    assert!(blocked("http://localhost/x"));
    assert!(blocked("http://printer.local/x"));
    assert!(blocked("http://intranet/x"));
    assert!(blocked("file:///etc/passwd"));
    assert!(super::fetchable(&reqwest::Url::parse("https://cdn.example/a.png").unwrap(), None).is_ok());
    let local = reqwest::Url::parse("http://127.0.0.1:8188/view").unwrap();
    assert!(super::fetchable(&local, Some("http://127.0.0.1:8188")).is_ok());
    assert!(super::fetchable(&local, Some("http://127.0.0.1:9999")).is_err());
}

#[tokio::test]
async fn a_redirect_to_a_private_host_is_not_followed() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut buf = [0; 512];
        let _ = tokio::io::AsyncReadExt::read(&mut socket, &mut buf).await;
        let body =
            "HTTP/1.1 302 Found\r\nLocation: http://169.254.169.254/latest/meta-data\r\nContent-Length: 0\r\n\r\n";
        let _ = tokio::io::AsyncWriteExt::write_all(&mut socket, body.as_bytes()).await;
    });

    let error = super::download(&format!("http://{addr}/image.png"), Some(&format!("http://{addr}")))
        .await
        .unwrap_err();
    assert!(error.message.contains("not one that can be fetched"), "{error}");
}
