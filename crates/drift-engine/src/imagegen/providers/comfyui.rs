//! ComfyUI on the user's machine: a plain checkpoint workflow (text to image, or from one uploaded image)
//! queued at `/prompt`, its history polled, the saved image fetched from `/view`. It suits SD 1.5 and SDXL
//! checkpoints; models that need their own loaders are for ComfyUI's own interface.

use percent_encoding::{NON_ALPHANUMERIC, utf8_percent_encode};
use serde_json::{Value, json};

use super::{Call, Failure, Made, download, made, poll, post, seed, send};
use crate::imagegen::ImageRequest;
use crate::llm::multipart::Form;
use crate::tool::image::Image;

const STEPS: u32 = 25;
const GUIDANCE: f64 = 7.0;
/// How far an image started from a reference may move from it.
const DENOISE_FROM_REFERENCE: f64 = 0.6;

pub(super) async fn generate(call: &Call<'_>, request: &ImageRequest<'_>) -> Result<Made, Failure> {
    let root = call.root("http://127.0.0.1:8188");
    let checkpoint = match call.model.trim() {
        "" => first_checkpoint(call, root).await?,
        chosen => chosen.to_string(),
    };
    let uploaded = match request.references.first() {
        Some(reference) => Some(upload(call, root, reference).await?),
        None => None,
    };
    let graph = workflow(&checkpoint, request, uploaded.as_deref());

    let queued = post(
        call.http.post(format!("{root}/prompt")),
        &json!({ "prompt": graph, "client_id": "drift" }),
    )
    .await?;
    let id = queued["prompt_id"]
        .as_str()
        .ok_or("ComfyUI did not queue the workflow")?;
    let saved = poll(|| finished(call, root, id)).await?;
    let image = download(call.http, &format!("{root}/view?{saved}")).await?;

    Ok(made(image, None, None))
}

/// The first checkpoint ComfyUI lists, for a call that names none.
async fn first_checkpoint(call: &Call<'_>, root: &str) -> Result<String, Failure> {
    let info = send(call.http.get(format!("{root}/object_info/CheckpointLoaderSimple"))).await?;
    let names = &info["CheckpointLoaderSimple"]["input"]["required"]["ckpt_name"];
    // Older ComfyUI lists the choices first; newer writes `["COMBO", { "options": [...] }]`.
    let listed = names[0].as_array().or_else(|| names[1]["options"].as_array());

    listed
        .and_then(|names| names.first())
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| "ComfyUI lists no checkpoints; put one in its models/checkpoints folder".into())
}

async fn upload(call: &Call<'_>, root: &str, reference: &Image) -> Result<String, Failure> {
    let extension = reference.mime.trim_start_matches("image/");
    let form = Form::new().text("overwrite", "true").file(
        "image",
        &format!("drift-reference.{extension}"),
        &reference.mime,
        &reference.bytes().unwrap_or_default(),
    );
    let answer = send(form.attach(call.http.post(format!("{root}/upload/image")))).await?;

    answer["name"]
        .as_str()
        .map(str::to_string)
        .ok_or_else(|| "ComfyUI did not take the reference image".into())
}

fn workflow(checkpoint: &str, request: &ImageRequest<'_>, reference: Option<&str>) -> Value {
    let (width, height) = request.size.pixels(1024, 64).unwrap_or((1024, 1024));
    let (latent, denoise) = match reference {
        Some(_) => (
            json!({ "class_type": "VAEEncode", "inputs": { "pixels": ["5", 0], "vae": ["1", 2] } }),
            DENOISE_FROM_REFERENCE,
        ),
        None => (
            json!({ "class_type": "EmptyLatentImage", "inputs": { "width": width, "height": height, "batch_size": 1 } }),
            1.0,
        ),
    };
    let mut graph = json!({
        "1": { "class_type": "CheckpointLoaderSimple", "inputs": { "ckpt_name": checkpoint } },
        "2": { "class_type": "CLIPTextEncode", "inputs": { "text": request.prompt, "clip": ["1", 1] } },
        "3": { "class_type": "CLIPTextEncode", "inputs": { "text": "", "clip": ["1", 1] } },
        "4": latent,
        "6": { "class_type": "KSampler", "inputs": {
            "seed": seed(), "steps": STEPS, "cfg": GUIDANCE, "sampler_name": "euler", "scheduler": "normal",
            "denoise": denoise, "model": ["1", 0], "positive": ["2", 0], "negative": ["3", 0], "latent_image": ["4", 0]
        } },
        "7": { "class_type": "VAEDecode", "inputs": { "samples": ["6", 0], "vae": ["1", 2] } },
        "8": { "class_type": "SaveImage", "inputs": { "filename_prefix": "drift", "images": ["7", 0] } }
    });
    if let Some(name) = reference {
        graph["5"] = json!({ "class_type": "LoadImage", "inputs": { "image": name } });
    }

    graph
}

/// The saved image's `/view` query once the workflow is done; an error once ComfyUI says it failed.
async fn finished(call: &Call<'_>, root: &str, id: &str) -> Result<Option<String>, Failure> {
    let history = send(call.http.get(format!("{root}/history/{id}"))).await?;
    let entry = &history[id];
    if entry.is_null() {
        return Ok(None);
    }
    if entry["status"]["status_str"] == "error" {
        return Err(format!("the workflow failed: {}", failure(&entry["status"]["messages"])).into());
    }
    let saved = entry["outputs"]
        .as_object()
        .into_iter()
        .flat_map(|outputs| outputs.values())
        .find_map(|output| output["images"][0].as_object());

    Ok(saved.map(|image| {
        let field = |name: &str| {
            let value = image.get(name).and_then(Value::as_str).unwrap_or_default();
            utf8_percent_encode(value, NON_ALPHANUMERIC).to_string()
        };
        format!(
            "filename={}&subfolder={}&type={}",
            field("filename"),
            field("subfolder"),
            field("type")
        )
    }))
}

/// The exception ComfyUI recorded for a failed workflow.
fn failure(messages: &Value) -> String {
    messages
        .as_array()
        .into_iter()
        .flatten()
        .find(|message| message[0] == "execution_error")
        .and_then(|message| message[1]["exception_message"].as_str())
        .unwrap_or("no reason given")
        .trim()
        .to_string()
}
