//! Stable Diffusion WebUI's API (AUTOMATIC1111, Forge, SD.Next): `txt2img`, or `img2img` from one reference,
//! answered with the image in base64. The WebUI serves it only when started with `--api`.

use serde_json::json;

use super::{Call, Failure, Made, decoded, made, post};
use crate::imagegen::ImageRequest;

const STEPS: u32 = 25;
const GUIDANCE: f64 = 7.0;
/// How far an image started from a reference may move from it.
const DENOISE_FROM_REFERENCE: f64 = 0.6;

pub(super) async fn generate(call: &Call<'_>, request: &ImageRequest<'_>) -> Result<Made, Failure> {
    let root = call.root("http://127.0.0.1:7860");
    let (width, height) = request.size.pixels(1024, 64).unwrap_or((1024, 1024));
    let mut body = json!({
        "prompt": request.prompt, "width": width, "height": height, "steps": STEPS, "cfg_scale": GUIDANCE, "seed": -1
    });
    if !call.model.trim().is_empty() {
        body["override_settings"] = json!({ "sd_model_checkpoint": call.model.trim() });
    }
    let action = match request.references.first() {
        Some(reference) => {
            body["init_images"] = json!([reference.base64]);
            body["denoising_strength"] = DENOISE_FROM_REFERENCE.into();
            "img2img"
        }
        None => "txt2img",
    };

    let answer = post(call.http.post(format!("{root}/sdapi/v1/{action}")), &body)
        .await
        .map_err(|failure| match failure.status {
            Some(404) => "the WebUI has no API here; start it with --api".into(),
            _ => failure,
        })?;
    let image = decoded(answer["images"][0].as_str().ok_or("the answer has no image")?)?;

    Ok(made(image, None, None))
}
