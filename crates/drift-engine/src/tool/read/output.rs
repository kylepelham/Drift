use super::{MAX_ENTRIES, REMINDER_BYTES};
use crate::tool::{Context, Output, ToolError, ToolMetadata, display, image};
use std::fmt::Write;
use std::path::Path;

/// A path that does not exist, with up to three names beside it that it may have meant.
pub(super) fn missing(context: &Context, path: &Path) -> ToolError {
    let shown = display(path, &context.workspace);
    let wanted = path
        .file_name()
        .map(|name| name.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    let mut close: Vec<String> = path
        .parent()
        .and_then(|directory| std::fs::read_dir(directory).ok())
        .into_iter()
        .flatten()
        .flatten()
        .filter(|entry| {
            let name = entry.file_name().to_string_lossy().to_lowercase();
            // Short names would produce irrelevant suggestions for almost every missing file.
            wanted.len() >= 3 && name.len() >= 3 && (name.contains(&wanted) || wanted.contains(&name))
        })
        .map(|entry| display(&entry.path(), &context.workspace))
        .collect();

    close.sort();
    close.truncate(3);

    if close.is_empty() {
        ToolError(format!("{shown} does not exist"))
    } else {
        ToolError(format!(
            "{shown} does not exist. Did you mean one of these?\n{}",
            close.join("\n")
        ))
    }
}

/// An image or PDF comes back for the model to look at, not as text.
pub(super) fn attached(context: &Context, path: &Path, mime: &str, bytes: &[u8]) -> Result<Output, ToolError> {
    let name = display(path, &context.workspace);
    let (kind, limit) = if mime == image::PDF {
        ("a PDF", image::MAX_PDF_BYTES)
    } else {
        ("an image", image::MAX_SOURCE_BYTES)
    };
    if bytes.len() > limit {
        return Err(ToolError(format!(
            "{name} is {kind} of {} bytes; too large to look at (the limit is {} MB)",
            bytes.len(),
            limit / 1024 / 1024
        )));
    }

    let file = image::Image::from_bytes(mime, bytes);
    Ok(Output {
        title: name.clone(),
        output: format!(
            "{name} is {kind} ({mime}, {} KB); it follows this result.",
            bytes.len().div_ceil(1024)
        ),
        metadata: ToolMetadata {
            images: Some(image::metadata(&[file])),
            ..Default::default()
        },
    })
}

/// Subdirectory instruction files not yet shown this session, within half a result; one that does
/// not fit is named so the model can read it.
pub(super) fn reminders(context: &Context, path: &Path) -> String {
    let mut output = String::new();
    for (file, text) in crate::config::nested_instructions(&context.workspace, path) {
        if file == path || !context.files.first_showing(&file) {
            continue;
        }

        let name = display(&file, &context.workspace);
        let reminder =
            format!("\n\n<system-reminder>\nInstructions from {name}, for files under it:\n{text}\n</system-reminder>");
        if output.len() + reminder.len() <= REMINDER_BYTES {
            output.push_str(&reminder);
        } else {
            write!(
                output,
                concat!(
                    "\n\n<system-reminder>\n{} holds instructions for files under it; ",
                    "read it before working there.\n</system-reminder>"
                ),
                name
            )
            .expect("writing to a String cannot fail");
        }
    }

    output
}

pub(super) async fn list_dir(context: &Context, path: &Path) -> Result<Output, ToolError> {
    let mut entries = tokio::fs::read_dir(path).await?;
    let mut names = Vec::new();
    while let Some(entry) = entries.next_entry().await? {
        let suffix = if entry.file_type().await.is_ok_and(|kind| kind.is_dir()) {
            "/"
        } else {
            ""
        };
        names.push(format!("{}{suffix}", entry.file_name().to_string_lossy()));
    }

    names.sort();
    let total = names.len();
    names.truncate(MAX_ENTRIES);
    let mut output = names.join("\n");
    if total > MAX_ENTRIES {
        write!(
            output,
            "\n\n({} more entries; use glob with a pattern to narrow it)",
            total - MAX_ENTRIES
        )
        .expect("writing to a String cannot fail");
    }

    Ok(Output::new(display(path, &context.workspace), output))
}
