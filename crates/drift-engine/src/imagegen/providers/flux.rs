//! Black Forest Labs' FLUX models: a request answers at once with a polling URL and its cost in credits (a
//! cent each); the finished image is a signed URL to fetch within ten minutes.

use serde_json::{Value, json};

use super::{Call, Failure, Made, download, made, poll, post, send};
use crate::imagegen::ImageRequest;

pub(super) const MODELS: &[&str] = &[
    "flux-2-pro",
    "flux-2-max",
    "flux-2-flex",
    "flux-2-klein-9b",
    "flux-2-klein-4b",
];

const DOLLARS_PER_CREDIT: f64 = 0.01;

pub(super) async fn generate(call: &Call<'_>, request: &ImageRequest<'_>) -> Result<Made, Failure> {
    let root = call.root("https://api.bfl.ai/v1");
    let mut body = json!({ "prompt": request.prompt, "output_format": "png" });
    if let Some((width, height)) = request.size.pixels(1024, 16) {
        body["width"] = width.into();
        body["height"] = height.into();
    }
    for (index, image) in request.references.iter().enumerate() {
        let field = match index {
            0 => "input_image".to_string(),
            index => format!("input_image_{}", index + 1),
        };
        body[field] = image.base64.clone().into();
    }

    let submitted = post(
        call.http
            .post(format!("{root}/{}", call.model))
            .header("x-key", call.key()),
        &body,
    )
    .await?;
    let polling = submitted["polling_url"]
        .as_str()
        .ok_or("the answer has no polling URL")?;
    let result = poll(|| status(call, polling)).await?;
    let image = download(call.http, &result).await?;
    let cost = submitted["cost"].as_f64().map(|credits| credits * DOLLARS_PER_CREDIT);

    Ok(made(image, cost, None))
}

/// The image's URL once it is ready; an error once FLUX says it failed or was moderated.
async fn status(call: &Call<'_>, polling: &str) -> Result<Option<String>, Failure> {
    let answer: Value = send(call.http.get(polling).header("x-key", call.key())).await?;
    match answer["status"].as_str().unwrap_or_default() {
        "Ready" => answer["result"]["sample"]
            .as_str()
            .map(|url| Some(url.to_string()))
            .ok_or_else(|| "the finished answer has no image".into()),
        "Pending" | "Queued" | "Processing" | "Task not found" | "" => Ok(None),
        other => Err(format!("FLUX reported {other}").into()),
    }
}
