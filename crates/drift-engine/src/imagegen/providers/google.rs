//! Gemini's `generateContent` on an image model, the references as inline parts before the prompt.

use serde_json::{Map, Value, json};

use super::{Call, Failure, Made, decoded, made, post, priced, tokens};
use crate::imagegen::ImageRequest;
use crate::imagegen::size::{GOOGLE_RATIOS, GOOGLE_TIERS};

pub(super) const MODELS: &[&str] = &["gemini-3.1-flash-image", "gemini-3-pro-image"];

/// Per million tokens: input, text and thinking out, image out.
const PRICES: &[(&str, [f64; 3])] = &[
    ("gemini-3.1-flash-image", [0.5, 3.0, 60.0]),
    ("gemini-3-pro-image", [2.0, 12.0, 120.0]),
];

pub(super) async fn generate(call: &Call<'_>, request: &ImageRequest<'_>) -> Result<Made, Failure> {
    let root = call.root("https://generativelanguage.googleapis.com/v1beta");
    let mut parts: Vec<Value> = request
        .references
        .iter()
        .map(|image| json!({ "inlineData": { "mimeType": image.mime, "data": image.base64 } }))
        .collect();
    parts.push(json!({ "text": request.prompt }));
    let mut config = Map::new();
    if let Some(ratio) = request.size.ratio_among(&GOOGLE_RATIOS) {
        config.insert("aspectRatio".into(), ratio.into());
    }
    if let Some(tier) = request.size.tier(&GOOGLE_TIERS) {
        config.insert("imageSize".into(), tier.into());
    }
    let body = json!({
        "contents": [{ "role": "user", "parts": parts }],
        "generationConfig": { "responseModalities": ["TEXT", "IMAGE"], "imageConfig": config }
    });
    let url = format!("{root}/models/{}:generateContent", call.model);
    let answer = post(call.http.post(url).header("x-goog-api-key", call.key()), &body).await?;

    let parts = answer["candidates"][0]["content"]["parts"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    let said: Vec<&str> = parts.iter().filter_map(|part| part["text"].as_str()).collect();
    let Some(data) = parts.iter().find_map(|part| part["inlineData"]["data"].as_str()) else {
        let why = said.join(" ");
        let finish = answer["candidates"][0]["finishReason"]
            .as_str()
            .unwrap_or("no reason given");
        return Err(format!(
            "the answer has no image ({})",
            if why.is_empty() { finish } else { &why }
        )
        .into());
    };

    Ok(made(
        decoded(data)?,
        cost(call.model, &answer["usageMetadata"]),
        Some(said.join("\n")),
    ))
}

pub(super) fn cost(model: &str, usage: &Value) -> Option<f64> {
    let [input, text_out, image_out] = priced(PRICES, model)?;
    if !usage.is_object() {
        return None;
    }
    let images: f64 = usage["candidatesTokensDetails"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|detail| detail["modality"] == "IMAGE")
        .map(|detail| tokens(&detail["tokenCount"]))
        .sum();
    let texts = (tokens(&usage["candidatesTokenCount"]) - images).max(0.0) + tokens(&usage["thoughtsTokenCount"]);

    Some((tokens(&usage["promptTokenCount"]) * input + texts * text_out + images * image_out) / 1e6)
}
