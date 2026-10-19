//! The three image APIs' wire shapes and what each model costs. Prices are US dollars per million
//! tokens (per image for xAI), from each provider's pricing page on 2026-10-19.

use std::time::Duration;

use base64::Engine as _;
use serde_json::{Map, Value, json};

use super::size::{GOOGLE_RATIOS, GOOGLE_TIERS, XAI_RATIOS, XAI_TIERS};
use super::{Generated, ImageRequest};
use crate::tool::image::Image;

pub struct Provider {
    pub id: &'static str,
    pub name: &'static str,
    /// Its image models, the default first.
    pub models: &'static [&'static str],
}

pub const PROVIDERS: &[Provider] = &[
    Provider {
        id: "openai",
        name: "OpenAI",
        models: &[
            "gpt-image-2",
            "gpt-image-2.5-sunburst",
            "gpt-image-2.5-flare",
            "gpt-image-1.5",
            "gpt-image-1-mini",
            "gpt-image-1",
        ],
    },
    Provider {
        id: "google",
        name: "Google",
        models: &["gemini-3.1-flash-image", "gemini-3-pro-image"],
    },
    Provider {
        id: "xai",
        name: "xAI",
        models: &[
            "grok-imagine-image-2.0",
            "grok-imagine-image",
            "grok-imagine-image-quality",
        ],
    },
];

/// Text in, image in, image out, text out.
const OPENAI_PRICES: &[(&str, [f64; 4])] = &[
    ("gpt-image-2", [5.0, 8.0, 30.0, 0.0]),
    ("gpt-image-1.5", [5.0, 8.0, 32.0, 10.0]),
    ("chatgpt-image-latest", [5.0, 8.0, 32.0, 10.0]),
    ("gpt-image-1-mini", [2.0, 2.5, 8.0, 0.0]),
    ("gpt-image-1", [5.0, 10.0, 40.0, 0.0]),
];
/// Input, text and thinking out, image out.
const GOOGLE_PRICES: &[(&str, [f64; 3])] = &[
    ("gemini-3.1-flash-image", [0.5, 3.0, 60.0]),
    ("gemini-3-pro-image", [2.0, 12.0, 120.0]),
];
/// Per image out at the lowest resolution, per reference image in; only used when an answer names no cost.
const XAI_PRICES: &[(&str, [f64; 2])] = &[
    ("grok-imagine-image-2.0", [0.04, 0.01]),
    ("grok-imagine-image-quality", [0.05, 0.01]),
    ("grok-imagine-image", [0.02, 0.002]),
];
/// xAI's cost unit: a ten-billionth of a dollar.
const XAI_TICKS_PER_DOLLAR: f64 = 1e10;
/// Image models can take a minute or more on a large, high-quality picture.
const TIMEOUT: Duration = Duration::from_secs(300);

pub(super) fn find(id: &str) -> Option<&'static Provider> {
    PROVIDERS.iter().find(|provider| provider.id == id)
}

/// The prices of the longest entry `model` starts with, so a dated snapshot is priced as its model.
fn priced<const N: usize>(table: &[(&str, [f64; N])], model: &str) -> Option<[f64; N]> {
    table
        .iter()
        .filter(|(name, _)| model.starts_with(name))
        .max_by_key(|(name, _)| name.len())
        .map(|(_, prices)| *prices)
}

fn tokens(value: &Value) -> f64 {
    value.as_u64().unwrap_or(0) as f64
}

fn data_url(image: &Image) -> String {
    format!("data:{};base64,{}", image.mime, image.base64)
}

/// The image in a provider's base64, by what its bytes are rather than what the answer says.
fn decoded(base64: &str) -> Result<Image, String> {
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(base64.trim())
        .map_err(|_| "the answer's image is not valid base64".to_string())?;
    let mime = crate::tool::image::sniff(&bytes).ok_or("the answer's image is not PNG, JPEG, GIF or WebP")?;

    Ok(Image::from_bytes(mime, &bytes))
}

async fn post(request: reqwest::RequestBuilder, body: &Value) -> Result<Value, String> {
    let response = request
        .json(body)
        .timeout(TIMEOUT)
        .send()
        .await
        .map_err(|error| error.to_string())?;
    let status = response.status().as_u16();
    let text = response.text().await.unwrap_or_default();

    if (200..300).contains(&status) {
        return serde_json::from_str(&text).map_err(|error| error.to_string());
    }

    Err(format!("{status} {}", crate::llm::error_message(&text)))
}

/// One request to a provider: the client, the key that pays, the model, and where it goes in tests.
pub(super) struct Call<'a> {
    pub http: &'a reqwest::Client,
    pub key: &'a str,
    pub model: &'a str,
    pub base: Option<&'a str>,
}

pub(super) async fn generate(
    provider: &'static Provider,
    call: Call<'_>,
    request: &ImageRequest<'_>,
) -> Result<Generated, String> {
    let made = match provider.id {
        "openai" => openai(&call, request).await,
        "google" => google(&call, request).await,
        _ => xai(&call, request).await,
    }
    .map_err(|error| format!("{} could not make the image: {error}", provider.name))?;

    Ok(Generated {
        provider: provider.name,
        model: call.model.to_string(),
        image: made.image,
        cost: made.cost,
        revised_prompt: made.revised_prompt.filter(|prompt| !prompt.trim().is_empty()),
    })
}

/// What one wire answered.
struct Made {
    image: Image,
    cost: Option<f64>,
    revised_prompt: Option<String>,
}

fn made(image: Image, cost: Option<f64>, revised_prompt: Option<String>) -> Made {
    Made {
        image,
        cost,
        revised_prompt,
    }
}

/// `/images/generations`, or `/images/edits` with the references as data URLs in its JSON form.
async fn openai(call: &Call<'_>, request: &ImageRequest<'_>) -> Result<Made, String> {
    let Call { http, key, model, base } = *call;
    let root = base.unwrap_or("https://api.openai.com/v1");
    let mut body = json!({ "model": model, "prompt": request.prompt, "n": 1, "output_format": "png" });
    if let Some(size) = request.size.openai() {
        body["size"] = size.into();
    }
    let url = if request.references.is_empty() {
        format!("{root}/images/generations")
    } else {
        let images: Vec<Value> = request
            .references
            .iter()
            .map(|image| json!({ "image_url": data_url(image) }))
            .collect();
        body["images"] = images.into();
        format!("{root}/images/edits")
    };
    let answer = post(http.post(url).bearer_auth(key), &body).await?;
    let first = &answer["data"][0];
    let image = decoded(first["b64_json"].as_str().ok_or("the answer has no image")?)?;
    let revised = first["revised_prompt"].as_str().map(str::to_string);

    Ok(made(image, openai_cost(model, &answer["usage"]), revised))
}

fn openai_cost(model: &str, usage: &Value) -> Option<f64> {
    let [text_in, image_in, image_out, text_out] = priced(OPENAI_PRICES, model)?;
    if !usage.is_object() {
        return None;
    }
    let input = &usage["input_tokens_details"];
    let output = &usage["output_tokens_details"];
    let (texts_in, images_in) = match input.is_object() {
        true => (tokens(&input["text_tokens"]), tokens(&input["image_tokens"])),
        false => (tokens(&usage["input_tokens"]), 0.0),
    };
    let (images_out, texts_out) = match output.is_object() {
        true => (tokens(&output["image_tokens"]), tokens(&output["text_tokens"])),
        false => (tokens(&usage["output_tokens"]), 0.0),
    };

    Some((texts_in * text_in + images_in * image_in + images_out * image_out + texts_out * text_out) / 1e6)
}

/// `generateContent` on an image model, the references as inline parts before the prompt.
async fn google(call: &Call<'_>, request: &ImageRequest<'_>) -> Result<Made, String> {
    let Call { http, key, model, base } = *call;
    let root = base.unwrap_or("https://generativelanguage.googleapis.com/v1beta");
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
    let url = format!("{root}/models/{model}:generateContent");
    let answer = post(http.post(url).header("x-goog-api-key", key), &body).await?;

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
        ));
    };

    Ok(made(
        decoded(data)?,
        google_cost(model, &answer["usageMetadata"]),
        Some(said.join("\n")),
    ))
}

fn google_cost(model: &str, usage: &Value) -> Option<f64> {
    let [input, text_out, image_out] = priced(GOOGLE_PRICES, model)?;
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

/// `/images/generations`, or `/images/edits` with one `image` or several `images`, asking for base64.
async fn xai(call: &Call<'_>, request: &ImageRequest<'_>) -> Result<Made, String> {
    let Call { http, key, model, base } = *call;
    let root = base.unwrap_or("https://api.x.ai/v1");
    let mut body = json!({ "model": model, "prompt": request.prompt, "n": 1, "response_format": "b64_json" });
    if let Some(ratio) = request.size.ratio_among(&XAI_RATIOS) {
        body["aspect_ratio"] = ratio.into();
    }
    if let Some(tier) = request.size.tier(&XAI_TIERS) {
        body["resolution"] = tier.into();
    }
    let sources: Vec<Value> = request
        .references
        .iter()
        .map(|image| json!({ "url": data_url(image), "type": "image_url" }))
        .collect();
    let url = match sources.as_slice() {
        [] => format!("{root}/images/generations"),
        [one] => {
            body["image"] = one.clone();
            format!("{root}/images/edits")
        }
        _ => {
            body["images"] = sources.into();
            format!("{root}/images/edits")
        }
    };
    let answer = post(http.post(url).bearer_auth(key), &body).await?;
    let first = &answer["data"][0];
    let image = decoded(first["b64_json"].as_str().ok_or("the answer has no image")?)?;
    let cost = answer["usage"]["cost_in_usd_ticks"]
        .as_f64()
        .map(|ticks| ticks / XAI_TICKS_PER_DOLLAR)
        .or_else(|| {
            let [out, reference] = priced(XAI_PRICES, model)?;
            Some(out + reference * request.references.len() as f64)
        });

    Ok(made(image, cost, first["revised_prompt"].as_str().map(str::to_string)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prices_come_from_token_counts_and_a_snapshot_is_priced_as_its_model() {
        let usage = json!({
            "input_tokens": 50,
            "input_tokens_details": { "text_tokens": 10, "image_tokens": 40 },
            "output_tokens": 1000,
            "output_tokens_details": { "image_tokens": 1000, "text_tokens": 0 }
        });
        let cost = openai_cost("gpt-image-2-2026-04-21", &usage).unwrap();
        assert!((cost - (10.0 * 5.0 + 40.0 * 8.0 + 1000.0 * 30.0) / 1e6).abs() < 1e-12);
        assert_eq!(priced(OPENAI_PRICES, "gpt-image-1-mini"), Some([2.0, 2.5, 8.0, 0.0]));
        assert_eq!(
            openai_cost("dall-e-3", &usage),
            None,
            "an unknown model is not guessed at"
        );
        assert_eq!(openai_cost("gpt-image-2", &Value::Null), None);

        let usage = json!({
            "promptTokenCount": 100,
            "candidatesTokenCount": 1130,
            "candidatesTokensDetails": [{ "modality": "IMAGE", "tokenCount": 1120 }, { "modality": "TEXT", "tokenCount": 10 }]
        });
        let cost = google_cost("gemini-3.1-flash-image", &usage).unwrap();
        assert!((cost - (100.0 * 0.5 + 10.0 * 3.0 + 1120.0 * 60.0) / 1e6).abs() < 1e-12);
    }
}
