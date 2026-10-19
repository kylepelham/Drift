//! fal.ai, a gateway to many image models: the model is the endpoint's path (`fal-ai/flux/dev`), called
//! synchronously. References go as data URLs in `image_url` and `image_urls`, which an editing endpoint
//! reads; pick one of those for edits. fal's answers carry no price, so none is charged.

use serde_json::{Value, json};

use super::{Call, Failure, Made, data_url, download, made, post};
use crate::imagegen::ImageRequest;

pub(super) const MODELS: &[&str] = &["fal-ai/flux/dev"];

pub(super) async fn generate(call: &Call<'_>, request: &ImageRequest<'_>) -> Result<Made, Failure> {
    let root = call.root("https://fal.run");
    let endpoint = call.model.trim().trim_matches('/');
    let mut body = json!({ "prompt": request.prompt, "num_images": 1, "output_format": "png", "sync_mode": true });
    if let Some((width, height)) = request.size.pixels(1024, 16) {
        body["image_size"] = json!({ "width": width, "height": height });
    }
    if let Some(first) = request.references.first() {
        let all: Vec<Value> = request.references.iter().map(|image| data_url(image).into()).collect();
        body["image_url"] = data_url(first).into();
        body["image_urls"] = all.into();
    }

    let http = call
        .http
        .post(format!("{root}/{endpoint}"))
        .header("authorization", format!("Key {}", call.key()));
    let answer = post(http, &body).await?;
    let image = &answer["images"][0];
    let url = image["url"]
        .as_str()
        .or_else(|| answer["image"]["url"].as_str())
        .ok_or("the answer has no image")?;
    if answer["has_nsfw_concepts"][0] == true {
        return Err("fal's safety checker withheld the image".into());
    }

    Ok(made(download(call.http, url).await?, None, None))
}
