//! Makes an image with the provider chosen in Settings and saves it inside the workspace or the scratch
//! directory. The saved file goes back to the model as an image, and the call's price onto its reply.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use super::image::{Image, sniff};
use super::{Ask, Context, Output, RunFuture, Tool, ToolError, ToolMetadata, display, required_str, scratch_dir};
use crate::imagegen::{Generated, ImageRequest, MAX_REFERENCES, Paid, Size};
use crate::llm::ToolSpec;
use crate::session::types::MetadataFile;

pub struct GenerateImage;

/// What a call reads and writes, worked out the same way for its asks and its run.
struct Targets {
    prompt: String,
    references: Vec<PathBuf>,
    path: PathBuf,
    format: Format,
}

/// The format a path's extension names.
#[derive(Clone, Copy)]
struct Format {
    mime: &'static str,
    encoding: image::ImageFormat,
}

fn format_of(path: &Path) -> Result<Format, ToolError> {
    let extension = path
        .extension()
        .map(|extension| extension.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    let (mime, encoding) = match extension.as_str() {
        "png" => ("image/png", image::ImageFormat::Png),
        "jpg" | "jpeg" => ("image/jpeg", image::ImageFormat::Jpeg),
        "webp" => ("image/webp", image::ImageFormat::WebP),
        _ => {
            return Err(ToolError(format!(
                "{} should end in .png, .jpg, .jpeg or .webp; nothing was generated",
                path.display()
            )));
        }
    };

    Ok(Format { mime, encoding })
}

/// A new PNG in the scratch directory, named for the call so its asks and its run agree.
fn default_path(ctx: &Context) -> PathBuf {
    let name: String = ctx
        .call_id
        .chars()
        .filter(|char| char.is_ascii_alphanumeric() || matches!(char, '_' | '-'))
        .collect();

    scratch_dir().join("images").join(format!("{name}.png"))
}

fn targets(ctx: &Context, input: &Value) -> Result<Targets, ToolError> {
    let prompt = required_str(input, "prompt")?.trim().to_string();
    let references = input["references"]
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or_default()
        .iter()
        .map(|reference| {
            reference
                .as_str()
                .map(|path| ctx.resolve(path))
                .ok_or_else(|| ToolError("`references` must be a list of paths".into()))
        })
        .collect::<Result<Vec<_>, _>>()?;
    if references.len() > MAX_REFERENCES {
        return Err(ToolError(format!(
            "at most {MAX_REFERENCES} reference images; nothing was generated"
        )));
    }
    let path = match input["path"].as_str().filter(|path| !path.trim().is_empty()) {
        Some(path) => ctx.resolve(path),
        None => default_path(ctx),
    };
    if let Some(outside) = references
        .iter()
        .chain([&path])
        .find(|path| !ctx.workspace_or_scratch(path))
    {
        return Err(ToolError(format!(
            "{} is outside the workspace and the scratch directory; nothing was generated",
            outside.display()
        )));
    }
    let format = format_of(&path)?;

    Ok(Targets {
        prompt,
        references,
        path,
        format,
    })
}

impl Tool for GenerateImage {
    fn permissions(&self) -> &'static [&'static str] {
        &["generate_image"]
    }

    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "generate_image".into(),
            description: include_str!("prompts/generate_image.txt").trim().into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "prompt": { "type": "string", "description": "What the image shows, in full." },
                    "size": { "type": "string", "description": "auto, an aspect ratio such as 1:1, 3:2, 2:3, 16:9 or 9:16, or WIDTHxHEIGHT such as 1536x1024. Default: the size chosen in Settings." },
                    "references": { "type": "array", "items": { "type": "string" }, "maxItems": MAX_REFERENCES, "description": "PNG, JPEG or WebP files in the workspace to work from." },
                    "path": { "type": "string", "description": "Where to save it, inside the workspace or the scratch directory, ending in .png, .jpg, .jpeg or .webp. Default: a new PNG in the scratch directory." }
                },
                "required": ["prompt"]
            }),
        }
    }

    fn ask(&self, ctx: &Context, input: &Value) -> Option<Ask> {
        let prompt = input["prompt"].as_str()?;
        let provider = ctx
            .engine
            .image_ask_label()
            .unwrap_or_else(|| "the image provider".into());

        // A call is billed or spends a plan's allowance, so it asks unless a rule or an earlier "always" allows it.
        Some(Ask::new(
            "generate_image",
            prompt,
            format!("Generate an image with {provider}: {prompt}"),
        ))
    }

    fn asks(&self, ctx: &Context, input: &Value) -> Vec<Ask> {
        let Ok(targets) = targets(ctx, input) else {
            // The run refuses this call before anything is sent, so there is nothing to approve.
            return vec![Ask::new("generate_image", "*", "Generate an image").allow_by_default()];
        };
        let mut asks: Vec<Ask> = targets
            .references
            .iter()
            .filter_map(|path| ctx.ask_to_read(path, "Use as a reference image"))
            .collect();
        asks.extend(ctx.ask_to_write(&targets.path, "Save a generated image to"));
        asks.extend(self.ask(ctx, input));

        asks
    }

    fn mutates(&self) -> bool {
        true
    }

    fn touches(&self, ctx: &Context, input: &Value) -> Option<Vec<PathBuf>> {
        Some(
            targets(ctx, input)
                .map(|targets| vec![targets.path])
                .unwrap_or_default(),
        )
    }

    fn run<'a>(&'a self, ctx: &'a Context, input: Value) -> RunFuture<'a> {
        Box::pin(async move {
            let targets = targets(ctx, &input)?;
            let chosen = ctx.engine.image_settings().size;
            let size = Size::parse(input["size"].as_str().unwrap_or(&chosen)).map_err(ToolError)?;
            let references = references(ctx, &targets.references).await?;
            let request = ImageRequest {
                prompt: &targets.prompt,
                size,
                references,
            };
            let generated = ctx.engine.generate_image(request).await.map_err(ToolError)?;
            // Charged before saving: the provider bills the image whether or not it can be written.
            if let Some(cost) = generated.cost {
                ctx.engine.charge_reply(&ctx.message_id, cost);
            }

            save(ctx, &targets, generated).await
        })
    }
}

/// The reference files, each scaled within what providers take.
async fn references(ctx: &Context, paths: &[PathBuf]) -> Result<Vec<Image>, ToolError> {
    let mut images = Vec::new();
    for path in paths {
        let name = display(path, &ctx.workspace);
        let bytes = tokio::fs::read(path)
            .await
            .map_err(|error| ToolError(format!("{name} could not be read: {error}")))?;
        let mime = sniff(&bytes)
            .filter(|mime| *mime != "image/gif")
            .ok_or_else(|| ToolError(format!("{name} is not a PNG, JPEG or WebP image")))?;
        let image = Image::from_bytes(mime, &bytes);
        let (image, _) = tokio::task::spawn_blocking(move || super::image::normalize(image))
            .await?
            .map_err(|error| ToolError(format!("{name} cannot be sent: {error}")))?;
        images.push(image);
    }

    Ok(images)
}

/// The image in the path's format, with its width and height.
fn encoded(image: &Image, format: Format) -> Result<(Vec<u8>, u32, u32), ToolError> {
    let bytes = image
        .bytes()
        .ok_or_else(|| ToolError("the provider's image is not valid base64".into()))?;
    let picture = image::load_from_memory(&bytes)
        .map_err(|error| ToolError(format!("the provider's image could not be read: {error}")))?;
    let (width, height) = (picture.width(), picture.height());
    if image.mime == format.mime {
        return Ok((bytes, width, height));
    }

    let picture = match format.encoding {
        image::ImageFormat::Jpeg => image::DynamicImage::ImageRgb8(picture.to_rgb8()),
        _ => image::DynamicImage::ImageRgba8(picture.to_rgba8()),
    };
    let mut converted = Vec::new();
    picture
        .write_to(&mut std::io::Cursor::new(&mut converted), format.encoding)
        .map_err(|error| ToolError(format!("the image could not be saved as {}: {error}", format.mime)))?;

    Ok((converted, width, height))
}

async fn save(ctx: &Context, targets: &Targets, generated: Generated) -> Result<Output, ToolError> {
    let (image, format) = (generated.image.clone(), targets.format);
    let (bytes, width, height) = tokio::task::spawn_blocking(move || encoded(&image, format)).await??;
    let name = display(&targets.path, &ctx.workspace);
    super::fits_history(&name, bytes.len())?;
    super::stage::replace(&ctx.engine.store, &targets.path, &bytes).await?;

    Ok(Output {
        title: name.clone(),
        output: described(&generated, &name, width, height),
        metadata: ToolMetadata {
            images: Some(super::image::metadata(&[Image::from_bytes(format.mime, &bytes)])),
            files: Some(vec![MetadataFile::Path(targets.path.to_string_lossy().into_owned())]),
            provider: Some(generated.provider.into()),
            cost: generated.cost,
            ..Default::default()
        },
    })
}

fn described(generated: &Generated, name: &str, width: u32, height: u32) -> String {
    let mut text = format!(
        "Generated with {} {} and saved to {name} ({width}x{height}).",
        generated.provider, generated.model
    );
    let _ = match (generated.paid, generated.cost) {
        (Paid::Plan, _) => write!(
            text,
            " It used your ChatGPT plan's image allowance, so nothing was added to this conversation's cost."
        ),
        (Paid::Local, _) => write!(
            text,
            " It ran on your own server, so nothing was added to this conversation's cost."
        ),
        (Paid::Billed, Some(cost)) => write!(text, " It cost ${cost:.3}, added to this conversation's cost."),
        (Paid::Billed, None) => write!(
            text,
            " Drift does not know this model's price, so nothing was added to this conversation's cost."
        ),
    };
    if let Some(note) = &generated.handed_over {
        let _ = write!(text, " ({note}.)");
    }
    if let Some(note) = &generated.revised_prompt {
        let _ = write!(text, "\n\nThe provider's note on the prompt: {note}");
    }
    text.push_str("\n\nThe image follows; check it against what was asked.");

    text
}
