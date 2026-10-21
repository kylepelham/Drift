//! Ideogram 3.0: `generate`, or `remix` from one reference image, both as multipart forms; the image is a
//! short-lived URL. The model picked is the rendering speed. Its answers carry no price, so none is charged.

use super::{Call, Failure, Made, download, made, send};
use crate::imagegen::ImageRequest;
use crate::imagegen::size::IDEOGRAM_RATIOS;
use crate::llm::multipart::Form;

pub(super) const MODELS: &[&str] = &["TURBO", "DEFAULT", "QUALITY"];

/// How much a remix keeps of its reference, out of 100.
const IMAGE_WEIGHT: &str = "50";

pub(super) async fn generate(call: &Call<'_>, request: &ImageRequest<'_>) -> Result<Made, Failure> {
    let root = call.root("https://api.ideogram.ai");
    let mut form = Form::new()
        .text("prompt", request.prompt)
        .text("rendering_speed", call.model);
    if let Some(ratio) = request.size.ratio_among(&IDEOGRAM_RATIOS) {
        form = form.text("aspect_ratio", &ratio.replace(':', "x"));
    }
    let action = match request.references.first() {
        Some(reference) => {
            let bytes = reference.bytes().unwrap_or_default();
            form = form
                .text("image_weight", IMAGE_WEIGHT)
                .file("image", "reference", &reference.mime, &bytes);
            "remix"
        }
        None => "generate",
    };

    let http = call
        .http
        .post(format!("{root}/v1/ideogram-v3/{action}"))
        .header("Api-Key", call.key());
    let answer = send(form.attach(http)).await?;
    let first = &answer["data"][0];
    if first["is_image_safe"] == false {
        return Err("Ideogram withheld the image as unsafe".into());
    }
    let url = first["url"].as_str().ok_or("the answer has no image")?;
    let revised = first["prompt"].as_str().filter(|said| *said != request.prompt);

    Ok(made(download(call.http, url).await?, None, revised.map(str::to_string)))
}
