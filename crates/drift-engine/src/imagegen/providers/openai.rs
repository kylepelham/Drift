//! OpenAI's Images API, three ways: with an API key at OpenAI, with a ChatGPT sign-in at the Codex
//! backend (the route Codex itself uses, paid from the plan's allowance), and at any server that speaks
//! the same API. Edits take the references as data URLs in the JSON form.

use serde_json::{Value, json};

use super::{Auth, Call, Failure, Made, data_url, decoded, made, post, priced, tokens};
use crate::imagegen::ImageRequest;

pub(super) const MODELS: &[&str] = &[
    "gpt-image-2",
    "gpt-image-2.5-sunburst",
    "gpt-image-2.5-flare",
    "gpt-image-1.5",
    "gpt-image-1-mini",
    "gpt-image-1",
];

/// Per million tokens: text in, image in, image out, text out.
const PRICES: &[(&str, [f64; 4])] = &[
    ("gpt-image-2", [5.0, 8.0, 30.0, 0.0]),
    ("gpt-image-1.5", [5.0, 8.0, 32.0, 10.0]),
    ("chatgpt-image-latest", [5.0, 8.0, 32.0, 10.0]),
    ("gpt-image-1-mini", [2.0, 2.5, 8.0, 0.0]),
    ("gpt-image-1", [5.0, 10.0, 40.0, 0.0]),
];

pub(super) async fn generate(call: &Call<'_>, request: &ImageRequest<'_>) -> Result<Made, Failure> {
    let plan = matches!(call.auth, Auth::Plan(_));
    let root = match call.auth {
        Auth::Plan(_) => call.root(crate::llm::openai::CODEX_BASE_URL),
        _ => call.root("https://api.openai.com/v1"),
    };
    if call.model.trim().is_empty() {
        return Err("no model is chosen; type the server's image model in Settings".into());
    }

    let mut body = json!({ "model": call.model, "prompt": request.prompt, "n": 1 });
    // The Codex backend takes only the fields Codex sends; it chooses the format itself.
    if !plan {
        body["output_format"] = "png".into();
    }
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

    let http = call.http.post(url);
    let http = match call.auth {
        Auth::Plan(crate::llm::Credential::OAuth { access, account, .. }) => {
            crate::llm::openai::signed_by_subscription(http, access, account.as_deref())
        }
        Auth::Key(key) | Auth::Server(Some(key)) => http.bearer_auth(key),
        _ => http,
    };
    let answer = post(http, &body).await?;
    let first = &answer["data"][0];
    let image = decoded(first["b64_json"].as_str().ok_or("the answer has no image")?)?;
    let revised = first["revised_prompt"].as_str().map(str::to_string);
    // The plan pays for its images, and a server's own price is not Drift's to know.
    let cost = match call.auth {
        Auth::Key(_) => cost(call.model, &answer["usage"]),
        _ => None,
    };

    Ok(made(image, cost, revised))
}

pub(super) fn cost(model: &str, usage: &Value) -> Option<f64> {
    let [text_in, image_in, image_out, text_out] = priced(PRICES, model)?;
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
