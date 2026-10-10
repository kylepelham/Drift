//! A turn the provider refused because it could not read an image: the image goes, and the turn goes on.

use crate::llm::Block;

use super::*;

#[tokio::test]
async fn only_the_image_that_fails_a_strict_decode_is_dropped_and_the_turn_goes_on() {
    let h = harness().await;
    let (corrupt, kept) = two_images_read(&h).await;
    corrupt_blob(&h, &corrupt);

    h.provider.push_error(unreadable()).push(text("seen"));
    h.engine.submit(&h.session.id, prompt("carry on")).await.await_ok();
    until_idle(&h).await;

    assert_eq!(images_in_last_request(&h), 1, "the whole image is still sent");
    let calls = image_calls(&h);
    assert!(calls.iter().all(|(_, hashes)| !hashes.contains(&corrupt)));
    assert!(calls.iter().any(|(_, hashes)| hashes.contains(&kept)));

    let messages = transcript(&h);
    let said = format!("{messages:?}");
    assert!(
        messages.iter().all(|message| message.info.error.is_none()),
        "the refused reply is gone once the retry works"
    );
    assert!(said.contains("[an image (image/png) is not shown: the provider could not process it]"));
    assert!(said.contains("seen"));
}

#[tokio::test]
async fn with_no_corrupt_image_the_newest_all_go_and_a_second_refusal_ends_the_turn() {
    let h = harness().await;
    two_images_read(&h).await;

    h.provider.push_error(unreadable()).push_error(unreadable());
    h.engine.submit(&h.session.id, prompt("carry on")).await.await_ok();
    until_idle(&h).await;

    assert_eq!(
        images_in_last_request(&h),
        0,
        "no culprit found, so every newest image goes"
    );
    assert!(image_calls(&h).iter().all(|(_, hashes)| hashes.is_empty()));
    assert_eq!(h.provider.responses_left(), 0, "one recovery per turn, then it stops");

    let last = transcript(&h).pop().unwrap();
    let error = last.info.error.unwrap_or_default();
    assert!(error.contains("Could not process image"), "{error}");
}

fn unreadable() -> crate::llm::Error {
    crate::llm::Error::api(400, "invalid_request_error", "Could not process image")
}

/// Reads `a.png` and `b.png` in one turn and returns their stored hashes.
async fn two_images_read(h: &Harness) -> (String, String) {
    for (name, shade) in [("a.png", 40), ("b.png", 200)] {
        let mut bytes = Vec::new();
        image::RgbImage::from_pixel(16, 16, image::Rgb([shade, 10, 10]))
            .write_to(&mut std::io::Cursor::new(&mut bytes), image::ImageFormat::Png)
            .unwrap();
        std::fs::write(h._dir.join("ws").join(name), bytes).unwrap();
    }

    h.provider
        .push(tool_call("read", r#"{"path": "a.png"}"#))
        .push(tool_call("read", r#"{"path": "b.png"}"#))
        .push(text("looked"));
    h.engine.submit(&h.session.id, prompt("look")).await.await_ok();
    until_idle(h).await;

    let hash = |name: &str| {
        image_calls(h)
            .into_iter()
            .find(|(input, _)| input.contains(name))
            .and_then(|(_, hashes)| hashes.first().cloned())
            .unwrap()
    };
    (hash("a.png"), hash("b.png"))
}

/// Cuts a stored image in half, as images stored before the strict check could be.
fn corrupt_blob(h: &Harness, hash: &str) {
    let mut cut = h.engine.store.blob(hash).unwrap().unwrap();
    cut.truncate(cut.len() / 2);

    h.engine
        .store
        .lock()
        .execute(
            "UPDATE blob SET data = ?1 WHERE hash = ?2",
            rusqlite::params![cut, hash],
        )
        .unwrap();
}

/// Each call's input and the image hashes it still carries.
fn image_calls(h: &Harness) -> Vec<(String, Vec<String>)> {
    transcript(h)
        .into_iter()
        .flat_map(|message| message.parts)
        .filter_map(|row| match row.part {
            Part::ToolCall { input, metadata, .. } => {
                let hashes = crate::tool::image::stored(metadata.as_deref())
                    .into_iter()
                    .map(|image| image.hash)
                    .collect();
                Some((input.to_string(), hashes))
            }
            _ => None,
        })
        .collect()
}

fn images_in_last_request(h: &Harness) -> usize {
    let requests = h.provider.requests.lock().unwrap();
    let last = requests.last().unwrap();

    last.messages
        .iter()
        .flat_map(|message| &message.blocks)
        .filter(|block| matches!(block, Block::Image { .. }))
        .count()
}
