use crate::llm::{Block, ChatMessage, MAX_IMAGES_SENT, Role as LlmRole};

use super::*;

#[tokio::test]
async fn images_from_tools_reach_a_model_that_reads_them_and_a_line_reaches_one_that_does_not() {
    let h = harness().await;
    h.engine.store.save_mcp_server("echo", &echo_server()).unwrap();
    h.engine
        .connect_mcp_in("echo", Some(&crate::tool::canonical(&h._dir.join("ws"))))
        .await
        .unwrap();
    let mut wide = Vec::new();
    image::RgbImage::new(2600, 20)
        .write_to(&mut std::io::Cursor::new(&mut wide), image::ImageFormat::Png)
        .unwrap();
    std::fs::write(h._dir.join("ws/shot.png"), wide).unwrap();
    std::fs::write(h._dir.join("ws/broken.png"), b"\x89PNG\r\n\x1a\nrest").unwrap();
    std::fs::write(h._dir.join("ws/mangled.jpg"), crate::tool::image::corrupt_jpeg()).unwrap();
    h.provider
        .push(tool_call("echo_echo", r#"{"text": "picture"}"#))
        .push(tool_call("read", r#"{"path": "shot.png"}"#))
        .push(tool_call("read", r#"{"path": "broken.png"}"#))
        .push(tool_call("read", r#"{"path": "mangled.jpg"}"#))
        .push(text("seen"));
    h.engine.submit(&h.session.id, prompt("look")).await.await_ok();
    until_idle(&h).await;

    let requests = h.provider.requests.lock().unwrap().clone();
    let last = requests.last().unwrap();
    let images: Vec<_> = last
        .messages
        .iter()
        .flat_map(|message| &message.blocks)
        .filter_map(|block| match block {
            Block::Image { mime, .. } => Some(mime.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(
        images,
        ["image/png", "image/jpeg"],
        "the MCP screenshot and the scaled (opaque, so JPEG) read image reach the model; the broken and corrupt ones do not"
    );
    let said = format!("{:?}", last.messages);
    assert!(said.contains("was scaled from 2600x20 to 2000x15 to fit the model's limits"));
    assert!(said.contains("[an image (image/png) is not shown: it could not be decoded"));
    assert!(said.contains("[an image (image/jpeg) is not shown: its data is corrupt ("));
    let stored = serde_json::to_string(&transcript(&h)).unwrap();
    assert!(
        stored.contains("\"hash\"") && !stored.contains("iVBORw0KGgo"),
        "the part names its images; their bytes live in the blob table"
    );

    let mut blind = model_with(0, false);
    blind.attachment = false;

    let text_only = crate::llm::prepare_files(last.messages.clone(), &blind, |_| None);
    assert!(
        text_only
            .iter()
            .flat_map(|message| &message.blocks)
            .all(|block| !matches!(block, Block::Image { .. }))
    );
    assert!(format!("{text_only:?}").contains("this model cannot read images"));
}

#[test]
fn only_the_newest_images_are_sent_and_a_lost_one_becomes_a_line() {
    let mut seeing = model_with(0, false);
    (seeing.attachment, seeing.pdf) = (true, true);

    let blocks = (0..MAX_IMAGES_SENT + 5)
        .map(|index| Block::Stored {
            mime: "image/png".into(),
            hash: format!("h{index}"),
        })
        .collect();
    let prepared = crate::llm::prepare_files(
        vec![ChatMessage {
            role: LlmRole::User,
            blocks,
        }],
        &seeing,
        |hash| (hash != "h14").then(|| b"png".to_vec()),
    );
    let kinds: Vec<_> = prepared[0]
        .blocks
        .iter()
        .map(|block| match block {
            Block::Image { .. } => "image",
            Block::Text(text) if text.contains("earlier") => "older",
            _ => "lost",
        })
        .collect();
    assert_eq!(
        kinds,
        [vec!["older"; 4], vec!["image"; MAX_IMAGES_SENT], vec!["lost"]].concat(),
        "the newest are loaded; one no longer kept says so and takes no slot"
    );

    let blocks = (0..4)
        .map(|index| Block::Image {
            mime: "image/png".into(),
            base64: format!("{index}{}", "A".repeat(8 * 1024 * 1024)),
        })
        .collect();
    let prepared = crate::llm::prepare_files(
        vec![ChatMessage {
            role: LlmRole::User,
            blocks,
        }],
        &seeing,
        |_| None,
    );
    let sent = prepared[0]
        .blocks
        .iter()
        .filter(|block| matches!(block, Block::Image { .. }))
        .count();
    assert_eq!(sent, 2, "a few large images fill the data budget before the count");
    assert!(
        matches!(&prepared[0].blocks[3], Block::Image { base64, .. } if base64.starts_with('3')),
        "the newest are the ones kept"
    );

    let pdf = vec![ChatMessage {
        role: LlmRole::User,
        blocks: vec![Block::Stored {
            mime: "application/pdf".into(),
            hash: "p".into(),
        }],
    }];
    let loaded = crate::llm::prepare_files(pdf.clone(), &seeing, |_| Some(b"%PDF-1.7".to_vec()));
    assert!(
        matches!(&loaded[0].blocks[0], Block::Pdf { .. }),
        "a stored PDF loads as a PDF"
    );
    seeing.pdf = false;

    let refused = crate::llm::prepare_files(pdf, &seeing, |_| Some(b"%PDF-1.7".to_vec()));
    assert!(matches!(&refused[0].blocks[0], Block::Text(text) if text.contains("cannot read PDFs")));
}
