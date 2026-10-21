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
