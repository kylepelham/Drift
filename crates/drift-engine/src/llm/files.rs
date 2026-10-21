use super::{Block, ChatMessage, FILES_DROPPED_TOGETHER, MAX_IMAGE_DATA_SENT, MAX_IMAGES_SENT, catalog};

/// Loads stored files and keeps the newest images and PDFs the model can read, within [`MAX_IMAGES_SENT`] files and
/// [`MAX_IMAGE_DATA_SENT`] base64 bytes. Older files become lines, [`FILES_DROPPED_TOGETHER`] at a time from the
/// oldest, so the first file sent stays put while new ones arrive and the cached prefix before it holds.
pub fn prepare_files(
    mut messages: Vec<ChatMessage>,
    model: &catalog::Model,
    load: impl Fn(&str) -> Option<Vec<u8>>,
) -> Vec<ChatMessage> {
    let reads = Reads {
        images: model.attachment,
        pdfs: model.pdf,
    };
    let mut files: Vec<&mut Block> = messages
        .iter_mut()
        .flat_map(|message| message.blocks.iter_mut())
        .filter(|block| matches!(block, Block::Image { .. } | Block::Pdf { .. } | Block::Stored { .. }))
        .collect();

    // Kinds the model cannot read are named and take no place in the window.
    for file in &mut files {
        if let Some(line) = reads.refusal(file) {
            **file = Block::Text(line.into());
        }
    }
    let mut readable: Vec<&mut Block> = files
        .into_iter()
        .filter(|file| !matches!(file, Block::Text(_)))
        .collect();

    // Newest first, as many as fit the count and the data budget.
    let mut loaded: Vec<Option<Block>> = readable.iter().map(|_| None).collect();
    let mut data = 0;
    for (index, file) in readable.iter().enumerate().rev().take(MAX_IMAGES_SENT) {
        let block = load_file(file, &load);
        data += sent_bytes(&block);
        if data > MAX_IMAGE_DATA_SENT {
            break;
        }
        loaded[index] = Some(block);
    }

    let first_kept = first_kept(readable.len(), loaded.iter().filter(|block| block.is_some()).count());
    for (index, file) in readable.iter_mut().enumerate() {
        **file = match loaded[index].take().filter(|_| index >= first_kept) {
            Some(block) => block,
            None => Block::Text("[An earlier image or PDF was here; only the newest ones are sent.]".into()),
        };
    }

    messages
}

/// The first of `total` files to send when the newest `fitting` fit: on a grid of [`FILES_DROPPED_TOGETHER`] from the
/// oldest, so the boundary moves once per batch; exactly at the fit when no grid point leaves a file to send.
fn first_kept(total: usize, fitting: usize) -> usize {
    let exact = total - fitting;
    let on_grid = exact.div_ceil(FILES_DROPPED_TOGETHER) * FILES_DROPPED_TOGETHER;

    if on_grid < total { on_grid } else { exact }
}

/// What the model reads.
struct Reads {
    images: bool,
    pdfs: bool,
}

impl Reads {
    /// The line that stands for a file of a kind the model cannot read.
    fn refusal(&self, file: &Block) -> Option<&'static str> {
        let pdf = is_pdf(file);
        if pdf && !self.pdfs {
            return Some("[A PDF was here, but this model cannot read PDFs.]");
        }
        if !pdf && !self.images {
            return Some("[An image was here, but this model cannot read images.]");
        }

        None
    }
}

fn is_pdf(file: &Block) -> bool {
    matches!(file, Block::Pdf { .. }) || matches!(file, Block::Stored { mime, .. } if mime == "application/pdf")
}

/// The file as sent: a stored one read from its blob, or a line when the blob is gone.
fn load_file(file: &Block, load: &impl Fn(&str) -> Option<Vec<u8>>) -> Block {
    let Block::Stored { mime, hash } = file else {
        return file.clone();
    };
    let Some(bytes) = load(hash) else {
        return Block::Text("[An image or PDF was here but is no longer kept.]".into());
    };
    let base64 = base64::Engine::encode(&base64::engine::general_purpose::STANDARD, bytes);

    if is_pdf(file) {
        Block::Pdf { base64 }
    } else {
        Block::Image {
            base64,
            mime: mime.clone(),
        }
    }
}

fn sent_bytes(block: &Block) -> usize {
    match block {
        Block::Image { base64, .. } | Block::Pdf { base64 } => base64.len(),
        _ => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_boundary_moves_once_per_batch() {
        let boundaries: Vec<usize> = (20..=31)
            .map(|total| first_kept(total, total.min(MAX_IMAGES_SENT)))
            .collect();

        assert_eq!(boundaries, [0, 5, 5, 5, 5, 5, 10, 10, 10, 10, 10, 15]);
    }

    #[test]
    fn a_few_files_too_large_for_the_grid_are_cut_exactly() {
        assert_eq!(first_kept(4, 2), 2, "a grid point at 5 would send nothing");
        assert_eq!(first_kept(3, 3), 0);
    }
}
