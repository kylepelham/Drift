//! A prompt's ACP content blocks as the parts of a Drift prompt.

use drift_engine::session::types::Part;
use serde_json::Value;

use super::rpc::RpcError;

pub(super) fn parts(blocks: &[Value]) -> Result<Vec<Part>, RpcError> {
    let mut parts = blocks.iter().map(part).collect::<Result<Vec<_>, _>>()?;
    parts.retain(|part| !matches!(part, Part::Text { text } if text.trim().is_empty()));
    if parts.is_empty() {
        return Err(RpcError::invalid_params("the prompt is empty"));
    }

    Ok(parts)
}

/// Images become data URLs, linked files mentions the engine reads, embedded resources the text the editor sent.
fn part(block: &Value) -> Result<Part, RpcError> {
    let text = |field: &str| block[field].as_str().unwrap_or_default().to_string();
    match block["type"].as_str() {
        Some("text") => Ok(Part::Text { text: text("text") }),
        Some("image") => image(&text("mimeType"), &text("data"), block["uri"].as_str()),
        Some("resource_link") => Ok(link(&text("uri"), &text("name"), block["mimeType"].as_str())),
        Some("resource") => resource(&block["resource"]),
        Some(other) => Err(RpcError::invalid_params(format!("{other} content is not supported"))),
        None => Err(RpcError::invalid_params("a content block needs a type")),
    }
}

fn image(mime: &str, data: &str, uri: Option<&str>) -> Result<Part, RpcError> {
    if data.is_empty() || !mime.starts_with("image/") {
        return Err(RpcError::invalid_params(
            "an image needs its data and an image MIME type",
        ));
    }

    Ok(file(
        mime,
        &name_of(uri.unwrap_or("image")),
        format!("data:{mime};base64,{data}"),
    ))
}

fn link(uri: &str, name: &str, mime: Option<&str>) -> Part {
    if !uri.starts_with("file://") {
        let text = if name.is_empty() {
            uri.to_string()
        } else {
            format!("[{name}]({uri})")
        };
        return Part::Text { text };
    }

    let name = if name.is_empty() {
        name_of(uri)
    } else {
        name.to_string()
    };
    file(mime.unwrap_or("text/plain"), &name, uri.to_string())
}

fn resource(resource: &Value) -> Result<Part, RpcError> {
    let uri = resource["uri"].as_str().unwrap_or_default();
    if let Some(text) = resource["text"].as_str() {
        return Ok(Part::Text {
            text: format!("[{uri}]\n{text}"),
        });
    }

    let mime = resource["mimeType"].as_str().unwrap_or_default();
    match resource["blob"].as_str() {
        Some(blob) => image(mime, blob, Some(uri)),
        None => Err(RpcError::invalid_params("a resource needs text or a blob")),
    }
}

fn file(mime: &str, name: &str, url: String) -> Part {
    Part::File {
        mime: mime.into(),
        name: name.into(),
        url,
        path: None,
        image: None,
    }
}

/// The last segment of a URI or path, without any query or fragment.
fn name_of(uri: &str) -> String {
    let path = uri.split(['?', '#']).next().unwrap_or(uri);
    path.rsplit(['/', '\\'])
        .find(|segment| !segment.is_empty())
        .unwrap_or(path)
        .to_string()
}
