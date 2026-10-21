//! Recraft's OpenAI-shaped `/images/generations`, and `/images/imageToImage` (a V3 model) for a reference.
//! Every answer names the units it used, a thousand to the dollar; vector models are left out, since an SVG
//! is not an image Drift saves.

use serde_json::{Value, json};

use super::{Call, Failure, Made, decoded, download, made, post, priced, send};
use crate::imagegen::ImageRequest;
use crate::imagegen::size::RECRAFT_SIZES;
use crate::llm::multipart::Form;

pub(super) const MODELS: &[&str] = &[
    "recraftv4_1",
    "recraftv4_1_pro",
    "recraftv4",
    "recraftv4_pro",
    "recraftv3",
];

/// Per image, for an answer that names no units.
const PRICES: &[(&str, [f64; 1])] = &[
    ("recraftv4_1", [0.035]),
    ("recraftv4_1_pro", [0.21]),
    ("recraftv4", [0.04]),
    ("recraftv4_pro", [0.25]),
    ("recraftv3", [0.04]),
];
const UNITS_PER_DOLLAR: f64 = 1000.0;
/// How far image-to-image may move from the reference.
const STRENGTH: &str = "0.5";

pub(super) async fn generate(call: &Call<'_>, request: &ImageRequest<'_>) -> Result<Made, Failure> {
    let root = call.root("https://external.api.recraft.ai/v1");
    let (answer, model) = match request.references.first() {
        Some(reference) => {
            let form = Form::new()
                .text("prompt", request.prompt)
                .text("strength", STRENGTH)
                .text("model", "recraftv3")
                .text("response_format", "b64_json")
                .file(
                    "image",
                    "reference",
                    &reference.mime,
                    &reference.bytes().unwrap_or_default(),
                );
            let http = call
                .http
                .post(format!("{root}/images/imageToImage"))
                .bearer_auth(call.key());
            (send(form.attach(http)).await?, "recraftv3")
        }
        None => {
            let mut body = json!({ "prompt": request.prompt, "model": call.model, "response_format": "b64_json" });
            if let Some(size) = size(call.model, request) {
                body["size"] = size.into();
            }
            let http = call
                .http
                .post(format!("{root}/images/generations"))
                .bearer_auth(call.key());
            (post(http, &body).await?, call.model)
        }
    };

    let first = &answer["data"][0];
    let image = match (first["b64_json"].as_str(), first["url"].as_str()) {
        (Some(data), _) => decoded(data)?,
        (None, Some(url)) => download(call.http, url).await?,
        _ => return Err("the answer has no image".into()),
    };

    Ok(made(image, cost(model, &answer), None))
}

/// Pixels as asked; a ratio as the nearest standard size, which the high-resolution models do not take.
fn size(model: &str, request: &ImageRequest<'_>) -> Option<String> {
    match request.size {
        crate::imagegen::Size::Pixels(width, height) => Some(format!("{width}x{height}")),
        size if !model.ends_with("_pro") => size.ratio_among(&RECRAFT_SIZES).map(str::to_string),
        _ => None,
    }
}

fn cost(model: &str, answer: &Value) -> Option<f64> {
    answer["credits"]
        .as_f64()
        .map(|units| units / UNITS_PER_DOLLAR)
        .or_else(|| priced(PRICES, model).map(|[price]| price))
}
