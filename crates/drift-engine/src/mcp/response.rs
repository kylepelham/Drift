use rmcp::model::{CallToolRequestParams, ContentBlock};
use rmcp::service::ServiceError;
use std::time::{Duration, Instant};

use super::{Answer, CallError, Era, Listing, Live, RELIST_BACKOFF, ToolInfo, Transport};
use crate::tool::image::Image;

impl Live {
    pub(super) fn is_open(&self) -> bool {
        !self.service.is_transport_closed() && !self.service.is_closed()
    }

    pub(super) fn tools(&self) -> Vec<rmcp::model::Tool> {
        self.listing.lock().unwrap().tools.clone()
    }

    pub(super) fn listing_due(&self) -> bool {
        self.listing
            .lock()
            .unwrap()
            .stale_at
            .is_some_and(|at| at <= Instant::now())
    }

    /// Replaces the cached tool list and reports whether its contents changed.
    pub(super) fn relisted(&self, tools: Vec<rmcp::model::Tool>, ttl: Option<Duration>) -> bool {
        let mut listing = self.listing.lock().unwrap();
        let changed = listing.tools != tools;
        *listing = Listing::new(tools, ttl);

        changed
    }

    pub(super) fn relist_failed(&self) {
        self.listing.lock().unwrap().stale_at = Some(Instant::now() + RELIST_BACKOFF);
    }

    /// Stateless HTTP uses one POST per request, so there is no persistent session to watch between calls.
    pub(super) fn holds_nothing_open(&self) -> bool {
        self.era == Era::Stateless && self.transport == Transport::StreamableHttp
    }

    /// Ends the connection and kills its process tree even while others still hold it.
    pub(super) fn close(&self) {
        self.service.cancellation_token().cancel();
        if let Some(tree) = &self.tree {
            tree.kill();
        }
    }

    pub(super) async fn call(&self, name: &str, arguments: serde_json::Value) -> Result<Answer, CallError> {
        let mut params = CallToolRequestParams::new(name.to_string());
        params.arguments = arguments.as_object().cloned();

        let result = self.service.call_tool(params).await.map_err(call_error)?;
        let mut answer = Answer {
            text: String::new(),
            is_error: result.is_error.unwrap_or(false),
            images: Vec::new(),
        };
        let mut lines = Vec::new();
        for block in &result.content {
            match block {
                ContentBlock::Text(text) => lines.push(text.text.clone()),
                ContentBlock::Image(image) => match sendable(&image.mime_type, &image.data) {
                    Ok(()) => answer.images.push(Image {
                        mime: image.mime_type.clone(),
                        base64: image.data.clone(),
                    }),
                    Err(why) => lines.push(format!("[an image ({}) not shown: {why}]", image.mime_type)),
                },
                ContentBlock::Resource(resource) => take_resource(&resource.resource, &mut answer.images, &mut lines),
                other => lines.push(serde_json::to_string(other).unwrap_or_default()),
            }
        }
        answer.text = lines.join("\n");

        Ok(answer)
    }
}

fn call_error(error: ServiceError) -> CallError {
    match error {
        ServiceError::TransportClosed | ServiceError::TransportSend(_) | ServiceError::Cancelled { .. } => CallError::Lost,
        ServiceError::InputRequiredRoundsExceeded { .. } => CallError::Failed(concat!(
            "the server kept asking for input Drift does not give (a person's answer, a model's reply or the roots), ",
            "so the call did not finish",
        ).into()),
        other => CallError::Failed(other.to_string()),
    }
}

/// Whether an MCP image can go to a model: a format every provider takes, within the size limit.
fn sendable(mime: &str, base64: &str) -> Result<(), &'static str> {
    if !crate::tool::image::SENDABLE.contains(&mime) {
        return Err("only PNG, JPEG, GIF and WebP reach the model");
    }
    if base64.len() > crate::tool::image::MAX_SOURCE_BYTES * 4 / 3 + 4 {
        return Err("larger than 32 MB");
    }

    Ok(())
}

/// Attaches supported images and PDFs from read or embedded resources.
/// Other resources become text, matching the content sent to the model.
pub(super) fn take_resource(
    resource: &rmcp::model::ResourceContents,
    images: &mut Vec<Image>,
    lines: &mut Vec<String>,
) {
    match resource {
        rmcp::model::ResourceContents::BlobResourceContents {
            mime_type: Some(mime),
            blob,
            ..
        } if sendable(mime, blob).is_ok() || mime == crate::tool::image::PDF => {
            images.push(Image {
                mime: mime.clone(),
                base64: blob.clone(),
            });
        }
        other => lines.push(resource_text(other)),
    }
}

/// An embedded resource as the model reads it: its text, or a line naming a binary one.
pub(super) fn resource_text(resource: &rmcp::model::ResourceContents) -> String {
    match resource {
        rmcp::model::ResourceContents::TextResourceContents { uri, text, .. } => {
            format!("<resource uri=\"{uri}\">\n{text}\n</resource>")
        }
        rmcp::model::ResourceContents::BlobResourceContents {
            uri, mime_type, blob, ..
        } => {
            let kind = mime_type.as_deref().unwrap_or("unknown type");
            let bytes = blob.len() * 3 / 4;

            format!("[binary resource {uri} ({kind}, {bytes} bytes), not shown]")
        }
        other => serde_json::to_string(other).unwrap_or_default(),
    }
}

pub(super) fn tool_info(tool: &rmcp::model::Tool) -> ToolInfo {
    let read_only = tool
        .annotations
        .as_ref()
        .and_then(|annotations| annotations.read_only_hint)
        .unwrap_or(false);

    ToolInfo {
        name: tool.name.to_string(),
        description: tool.description.clone().unwrap_or_default().to_string(),
        read_only,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn only_png_jpeg_gif_and_webp_within_the_limit_are_sent() {
        assert!(sendable("image/png", "AAAA").is_ok() && sendable("image/webp", "AAAA").is_ok());
        assert!(sendable("image/svg+xml", "AAAA").unwrap_err().contains("only PNG"));
        assert!(sendable("image/bmp", "AAAA").is_err());

        assert!(
            sendable("image/png", &"A".repeat(8 * 1024 * 1024)).is_ok(),
            "scaled down when kept"
        );
        assert!(
            sendable("image/png", &"A".repeat(44 * 1024 * 1024))
                .unwrap_err()
                .contains("32 MB")
        );
    }

    #[test]
    fn an_embedded_image_or_pdf_is_attached_and_anything_else_is_text() {
        let resource =
            |value: serde_json::Value| serde_json::from_value::<rmcp::model::ResourceContents>(value).unwrap();
        let (mut images, mut lines) = (Vec::new(), Vec::new());

        take_resource(
            &resource(json!({ "uri": "shot://1", "mimeType": "image/png", "blob": "AAAA" })),
            &mut images,
            &mut lines,
        );
        take_resource(
            &resource(json!({ "uri": "doc://1", "mimeType": "application/pdf", "blob": "JVBE" })),
            &mut images,
            &mut lines,
        );
        take_resource(
            &resource(json!({ "uri": "zip://1", "mimeType": "application/zip", "blob": "UEsD" })),
            &mut images,
            &mut lines,
        );
        take_resource(
            &resource(json!({ "uri": "note://1", "text": "hello" })),
            &mut images,
            &mut lines,
        );

        assert_eq!(
            images.iter().map(|image| image.mime.as_str()).collect::<Vec<_>>(),
            ["image/png", "application/pdf"]
        );
        assert!(
            lines[0].starts_with("[binary resource zip://1") && lines[1].contains("hello"),
            "{lines:?}"
        );
    }
}
