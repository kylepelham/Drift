//! Stability AI's Stable Image API: a multipart form to `core`, `ultra` or `sd3` (which names its model),
//! answered as JSON with the image in base64. Ultra and SD 3.5 also start from one reference image.

use super::{Call, Failure, Made, decoded, made, send};
use crate::imagegen::ImageRequest;
use crate::imagegen::size::STABILITY_RATIOS;
use crate::llm::multipart::Form;

pub(super) const MODELS: &[&str] = &[
    "core",
    "ultra",
    "sd3.5-large",
    "sd3.5-large-turbo",
    "sd3.5-medium",
    "sd3.5-flash",
];

/// Credits per image, a cent each; only successful generations are charged.
const CREDITS: &[(&str, [f64; 1])] = &[
    ("core", [3.0]),
    ("ultra", [8.0]),
    ("sd3.5-large", [6.5]),
    ("sd3.5-large-turbo", [4.0]),
    ("sd3.5-medium", [3.5]),
    ("sd3.5-flash", [2.5]),
];
const DOLLARS_PER_CREDIT: f64 = 0.01;
/// How far an image started from a reference may move from it.
const STRENGTH: &str = "0.6";

pub(super) async fn generate(call: &Call<'_>, request: &ImageRequest<'_>) -> Result<Made, Failure> {
    let root = call.root("https://api.stability.ai");
    let sd3 = call.model.starts_with("sd3");
    let endpoint = if sd3 { "sd3" } else { call.model };
    let mut form = Form::new().text("prompt", request.prompt).text("output_format", "png");
    if sd3 {
        form = form.text("model", call.model);
    }
    match request.references.first() {
        Some(_) if call.model == "core" => return Err("Stable Image Core makes images from text only".into()),
        Some(reference) => {
            if sd3 {
                form = form.text("mode", "image-to-image");
            }
            let bytes = reference.bytes().unwrap_or_default();
            form = form
                .text("strength", STRENGTH)
                .file("image", "reference", &reference.mime, &bytes);
        }
        None => {
            if let Some(ratio) = request.size.ratio_among(&STABILITY_RATIOS) {
                form = form.text("aspect_ratio", ratio);
            }
        }
    }

    let http = call
        .http
        .post(format!("{root}/v2beta/stable-image/generate/{endpoint}"))
        .bearer_auth(call.key())
        .header("accept", "application/json");
    let answer = send(form.attach(http)).await?;
    if answer["finish_reason"] == "CONTENT_FILTERED" {
        return Err("Stability's content filter withheld the image".into());
    }
    let image = decoded(answer["image"].as_str().ok_or("the answer has no image")?)?;
    let cost = CREDITS
        .iter()
        .find(|(model, _)| *model == call.model)
        .map(|(_, [credits])| credits * DOLLARS_PER_CREDIT);

    Ok(made(image, cost, None))
}
