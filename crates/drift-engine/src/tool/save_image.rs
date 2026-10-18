//! Writes an image the user pasted into the conversation to a file, by the number the model sees beside it.

use base64::Engine as _;
use serde_json::{Value, json};

use super::{Ask, Context, Output, RunFuture, Tool, ToolError, ToolMetadata, display, required_str};
use crate::llm::ToolSpec;
use crate::session::types::{MetadataFile, Part};

pub struct SaveImage;

impl Tool for SaveImage {
    fn permissions(&self) -> &'static [&'static str] {
        &["edit"]
    }

    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "save_image".into(),
            description: "Writes an image the user pasted into this conversation, marked [Image N] beside it, to a file. Use it only when a file is needed, such as a reference image for a script or the project; you can already see the image.".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "image": { "type": "integer", "description": "N from the [Image N] beside the image." },
                    "path": { "type": "string", "description": "File to write, its extension matching the image type. Parent directories are created." }
                },
                "required": ["image", "path"]
            }),
        }
    }

    fn ask(&self, ctx: &Context, input: &Value) -> Option<Ask> {
        ctx.ask_to_write(&ctx.resolve(input["path"].as_str()?), "Save image")
    }

    fn mutates(&self) -> bool {
        true
    }

    fn touches(&self, ctx: &Context, input: &Value) -> Option<Vec<std::path::PathBuf>> {
        Some(vec![ctx.resolve(input["path"].as_str()?)])
    }

    fn run<'a>(&'a self, ctx: &'a Context, input: Value) -> RunFuture<'a> {
        Box::pin(async move {
            let number = input["image"]
                .as_u64()
                .and_then(|number| u32::try_from(number).ok())
                .ok_or_else(|| ToolError("`image` is required: N from [Image N]".into()))?;
            let path = ctx.resolve(required_str(&input, "path")?);
            let (mime, bytes) = pasted(ctx, number)?;

            let name = display(&path, &ctx.workspace);
            super::fits_history(&name, bytes.len())?;
            super::stage::replace(&ctx.engine.store, &path, &bytes).await?;

            Ok(Output {
                title: name.clone(),
                output: format!(
                    "Saved [Image {number}] ({mime}, {} KB) to {name}.",
                    bytes.len().div_ceil(1024)
                ),
                metadata: ToolMetadata {
                    files: Some(vec![MetadataFile::Path(path.to_string_lossy().into_owned())]),
                    ..Default::default()
                },
            })
        })
    }
}

/// The image's type and bytes, as the user pasted it.
fn pasted(ctx: &Context, number: u32) -> Result<(String, Vec<u8>), ToolError> {
    let missing = || ToolError(format!("this conversation has no [Image {number}]"));
    let Some(Part::File { mime, url, .. }) = ctx.engine.store.image_numbered(&ctx.session_id, number)? else {
        return Err(missing());
    };
    let (_, data) = url.split_once(',').ok_or_else(missing)?;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(data.trim())
        .map_err(|error| ToolError(format!("[Image {number}] is not valid base64: {error}")))?;

    Ok((mime, bytes))
}
