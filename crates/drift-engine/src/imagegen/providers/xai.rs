//! xAI's `/images/generations`, or `/images/edits` with one `image` or several `images`, asking for base64.

use serde_json::{Value, json};

use super::{Call, Failure, Made, data_url, decoded, made, post, priced};
use crate::imagegen::ImageRequest;
use crate::imagegen::size::{XAI_RATIOS, XAI_TIERS};

pub(super) const MODELS: &[&str] = &[
    "grok-imagine-image-2.0",
    "grok-imagine-image",
    "grok-imagine-image-quality",
];

/// Per image out at the lowest resolution, per reference image in; only used when an answer names no cost.
const PRICES: &[(&str, [f64; 2])] = &[
    ("grok-imagine-image-2.0", [0.04, 0.01]),
    ("grok-imagine-image-quality", [0.05, 0.01]),
    ("grok-imagine-image", [0.02, 0.002]),
];
/// xAI's cost unit: a ten-billionth of a dollar.
const TICKS_PER_DOLLAR: f64 = 1e10;

pub(super) async fn generate(call: &Call<'_>, request: &ImageRequest<'_>) -> Result<Made, Failure> {
    let root = call.root("https://api.x.ai/v1");
    let mut body = json!({ "model": call.model, "prompt": request.prompt, "n": 1, "response_format": "b64_json" });
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
    let answer = post(call.http.post(url).bearer_auth(call.key()), &body).await?;
    let first = &answer["data"][0];
    let image = decoded(first["b64_json"].as_str().ok_or("the answer has no image")?)?;
    let cost = answer["usage"]["cost_in_usd_ticks"]
        .as_f64()
        .map(|ticks| ticks / TICKS_PER_DOLLAR)
        .or_else(|| {
            let [out, reference] = priced(PRICES, call.model)?;
            Some(out + reference * request.references.len() as f64)
        });

    Ok(made(image, cost, first["revised_prompt"].as_str().map(str::to_string)))
}
