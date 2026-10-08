use super::{Block, ChatMessage, MAX_IMAGE_DATA_SENT, MAX_IMAGES_SENT, catalog};

/// Loads stored files and keeps the newest images and PDFs the model can read.
/// Stops at [`MAX_IMAGES_SENT`] files or [`MAX_IMAGE_DATA_SENT`] base64 bytes.
/// Older files and unsupported kinds become explanatory text blocks.
pub fn prepare_files(
    mut messages: Vec<ChatMessage>,
    model: &catalog::Model,
    load: impl Fn(&str) -> Option<Vec<u8>>,
) -> Vec<ChatMessage> {
    let mut budget = FileBudget {
        reads_images: model.attachment,
        reads_pdfs: model.pdf,
        sent: 0,
        data: 0,
    };

    for block in messages
        .iter_mut()
        .rev()
        .flat_map(|message| message.blocks.iter_mut().rev())
    {
        if matches!(block, Block::Image { .. } | Block::Pdf { .. } | Block::Stored { .. }) {
            let file = std::mem::replace(block, Block::Text(String::new()));
            *block = budget.decide(file, &load);
        }
    }

    messages
}

/// Remaining capacity for request files, counted from the newest back to the oldest.
struct FileBudget {
    reads_images: bool,
    reads_pdfs: bool,
    sent: usize,
    data: usize,
}

impl FileBudget {
    fn decide(&mut self, file: Block, load: &impl Fn(&str) -> Option<Vec<u8>>) -> Block {
        let line = |text: &str| Block::Text(text.into());
        let pdf = matches!(&file, Block::Pdf { .. })
            || matches!(&file, Block::Stored { mime, .. } if mime == "application/pdf");
        if pdf && !self.reads_pdfs {
            return line("[A PDF was here, but this model cannot read PDFs.]");
        }
        if !pdf && !self.reads_images {
            return line("[An image was here, but this model cannot read images.]");
        }

        let earlier = "[An earlier image or PDF was here; only the newest ones are sent.]";
        if self.sent >= MAX_IMAGES_SENT {
            return line(earlier);
        }

        let file = match file {
            Block::Stored { mime, hash } => {
                let Some(bytes) = load(&hash) else {
                    return line("[An image or PDF was here but is no longer kept.]");
                };
                let base64 = base64::Engine::encode(&base64::engine::general_purpose::STANDARD, bytes);

                if pdf {
                    Block::Pdf { base64 }
                } else {
                    Block::Image { base64, mime }
                }
            }
            other => other,
        };
        let size = match &file {
            Block::Image { base64, .. } | Block::Pdf { base64 } => base64.len(),
            _ => 0,
        };
        if self.data + size > MAX_IMAGE_DATA_SENT {
            return line(earlier);
        }

        self.sent += 1;
        self.data += size;

        file
    }
}
