use super::*;

impl Engine {
    /// Scales the images a call returned within provider limits and moves them to the blob table,
    /// leaving `{mime, hash}` in its metadata; a scaled or dropped image is said in the result.
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
}
