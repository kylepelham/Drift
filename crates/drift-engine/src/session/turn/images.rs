use std::collections::HashSet;

use super::*;

impl Engine {
    /// Scales a call's returned images within provider limits and moves them to the blob table.
    /// Metadata keeps `{mime, hash}`, and the result mentions any image that was scaled or dropped.
    pub(super) async fn keep_images(
        &self,
        message_id: &str,
        mut meta: ToolMetadata,
        mut text: String,
    ) -> (ToolMetadata, String) {
        let returned = crate::tool::image::returned(&meta);
        if returned.is_empty() {
            return (meta, text);
        }

        let mimes: Vec<String> = returned.iter().map(|image| image.mime.clone()).collect();
        let normalized = tokio::task::spawn_blocking(move || {
            returned
                .into_iter()
                .map(|image| crate::tool::image::normalize(image).map_err(|error| error.to_string()))
                .collect::<Vec<_>>()
        })
        .await
        .unwrap_or_else(|_| {
            mimes
                .iter()
                .map(|_| Err("it could not be prepared".to_string()))
                .collect()
        });

        let mut stored = Vec::new();
        for (mime, result) in mimes.into_iter().zip(normalized) {
            let result = result.and_then(|(image, note)| self.keep_image(message_id, image).map(|kept| (kept, note)));
            match result {
                Ok((kept, note)) => {
                    if let Some(note) = note {
                        let _ = write!(text, "\n\n[an image ({mime}) was {note} to fit the model's limits]");
                    }
                    stored.push(kept);
                }
                Err(reason) => {
                    let _ = write!(text, "\n\n[an image ({mime}) is not shown: {reason}]");
                }
            }
        }
        meta.images = Some(crate::tool::image::stored_metadata(&stored));

        (meta, text)
    }

    fn keep_image(
        &self,
        message_id: &str,
        image: crate::tool::image::Image,
    ) -> Result<crate::tool::image::Stored, String> {
        let bytes = image.bytes().ok_or("its data is not valid base64")?;
        let hash = self
            .store
            .put_blob(message_id, &bytes)
            .map_err(|_| "it could not be kept".to_string())?;

        Ok(crate::tool::image::Stored { mime: image.mime, hash })
    }

    /// After a provider could not read an image: among the newest images calls returned, removes those
    /// that fail a strict decode, else all of them, each leaving a note in its call's result, so the
    /// conversation can go on. Then the refused reply goes. False when no call image was there to remove.
    pub(super) fn drop_unreadable_images(&self, session_id: &str, reply: String) -> bool {
        let Some(window) = self.request_window(session_id) else {
            return false;
        };
        let newest = newest_images(&window);
        let corrupt: HashSet<String> = newest
            .iter()
            .filter(|hash| {
                let bytes = self.store.blob(hash).ok().flatten();
                bytes.is_some_and(|bytes| crate::tool::image::check(&bytes).is_err())
            })
            .cloned()
            .collect();
        let dropped = if corrupt.is_empty() { newest } else { corrupt };
        if dropped.is_empty() {
            return false;
        }

        for mut row in window.into_iter().flat_map(|message| message.parts) {
            if without_images(&mut row.part, &dropped) && self.store.save_part(&row).is_ok() {
                self.hub.publish(Event::PartUpdated { part: row });
            }
        }
        self.discard_refused_reply(session_id, reply);

        true
    }
}

/// The hashes of the newest images calls returned, as many as a request carries; PDFs are left alone.
fn newest_images(window: &[MessageWithParts]) -> HashSet<String> {
    window
        .iter()
        .rev()
        .flat_map(|message| message.parts.iter().rev())
        .flat_map(|row| match &row.part {
            Part::ToolCall { metadata, .. } => crate::tool::image::stored(metadata.as_deref()),
            _ => Vec::new(),
        })
        .filter(|image| image.mime.starts_with("image/"))
        .take(llm::MAX_IMAGES_SENT)
        .map(|image| image.hash)
        .collect()
}

/// Takes `dropped` images out of a call, noting each in its result; true when the call changed.
fn without_images(part: &mut Part, dropped: &HashSet<String>) -> bool {
    let Part::ToolCall {
        metadata: Some(metadata),
        output,
        ..
    } = part
    else {
        return false;
    };
    let (gone, kept): (Vec<_>, Vec<_>) = crate::tool::image::stored(Some(metadata))
        .into_iter()
        .partition(|image| dropped.contains(&image.hash));
    if gone.is_empty() {
        return false;
    }

    metadata.images = Some(crate::tool::image::stored_metadata(&kept));
    let text = output.get_or_insert_default();
    for image in gone {
        let _ = write!(
            text,
            "\n\n[an image ({}) is not shown: the provider could not process it]",
            image.mime
        );
    }

    true
}
