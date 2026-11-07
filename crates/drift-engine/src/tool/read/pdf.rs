//! `read`'s `pages`: a PDF's page range copied into a smaller PDF, so a long document can be read in parts.

use crate::tool::ToolError;

/// Pages one read may show.
pub(super) const MAX_PAGES: u32 = 20;
/// A PDF longer than this must be read in parts.
pub(super) const WHOLE_PAGES: usize = 10;

/// The PDF to show, and which of how many pages it holds when it is a part.
#[derive(Debug)]
pub(super) struct Selected {
    pub(super) bytes: Vec<u8>,
    pub(super) part: Option<(u32, u32, usize)>,
}

/// `"3"` or `"1-5"`, one-based and inclusive.
fn range(text: &str) -> Result<(u32, u32), ToolError> {
    let bad = || ToolError(format!("`pages` is a page or a range such as \"1-5\", not `{text}`"));
    let (first, last) = text.split_once('-').unwrap_or((text, text));
    let first: u32 = first.trim().parse().map_err(|_| bad())?;
    let last: u32 = last.trim().parse().map_err(|_| bad())?;
    if first == 0 || last < first {
        return Err(bad());
    }
    if last - first >= MAX_PAGES {
        return Err(ToolError(format!("`pages` takes at most {MAX_PAGES} pages a read")));
    }

    Ok((first, last))
}

/// The whole PDF when it is short or cannot be parsed, or the asked pages of it; a long one needs `pages`.
pub(super) fn select(bytes: &[u8], pages: Option<&str>, name: &str) -> Result<Selected, ToolError> {
    let asked = pages.map(range).transpose()?;
    let document = match lopdf::Document::load_mem(bytes) {
        Ok(document) => document,
        Err(_) if asked.is_none() => {
            return Ok(Selected {
                bytes: bytes.to_vec(),
                part: None,
            });
        }
        Err(error) => return Err(ToolError(format!("{name}'s pages could not be read: {error}"))),
    };
    let count = document.get_pages().len();
    let Some((first, last)) = asked else {
        if count > WHOLE_PAGES {
            return Err(ToolError(format!(
                "{name} has {count} pages; read it in parts with `pages`, such as \"1-{WHOLE_PAGES}\" (at most {MAX_PAGES} a read)"
            )));
        }
        return Ok(Selected {
            bytes: bytes.to_vec(),
            part: None,
        });
    };
    if last as usize > count {
        return Err(ToolError(format!(
            "{name} has {count} pages, so `pages` {first}-{last} is past its end"
        )));
    }

    Ok(Selected {
        bytes: part_of(document, first, last, count)?,
        part: Some((first, last, count)),
    })
}

fn part_of(mut document: lopdf::Document, first: u32, last: u32, count: usize) -> Result<Vec<u8>, ToolError> {
    let others: Vec<u32> = (1..=count as u32)
        .filter(|page| *page < first || *page > last)
        .collect();
    document.delete_pages(&others);
    document.prune_objects();
    let mut bytes = Vec::new();
    document
        .save_to(&mut bytes)
        .map_err(|error| ToolError(format!("the pages could not be copied: {error}")))?;

    Ok(bytes)
}

/// A PDF of `count` pages, each saying its number.
#[cfg(test)]
pub(super) fn sample(count: u32) -> Vec<u8> {
    use lopdf::{Object, Stream, dictionary};
    let mut document = lopdf::Document::with_version("1.5");
    let pages_id = document.new_object_id();
    let kids: Vec<Object> = (1..=count)
        .map(|number| {
            let content = Stream::new(dictionary! {}, format!("BT (page {number}) Tj ET").into_bytes());
            let content_id = document.add_object(content);
            document
                .add_object(dictionary! {
                    "Type" => "Page",
                    "Parent" => pages_id,
                    "Contents" => content_id,
                    "MediaBox" => vec![0.into(), 0.into(), 100.into(), 100.into()],
                })
                .into()
        })
        .collect();
    document.objects.insert(
        pages_id,
        Object::Dictionary(dictionary! { "Type" => "Pages", "Kids" => kids, "Count" => count }),
    );
    let catalog = document.add_object(dictionary! { "Type" => "Catalog", "Pages" => pages_id });
    document.trailer.set("Root", catalog);
    let mut bytes = Vec::new();
    document.save_to(&mut bytes).unwrap();

    bytes
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_long_pdf_is_read_in_parts_of_at_most_twenty_pages() {
        let long = sample(30);
        assert!(select(&long, None, "long.pdf").unwrap_err().0.contains("has 30 pages"));

        let part = select(&long, Some("3-5"), "long.pdf").unwrap();
        assert_eq!(part.part, Some((3, 5, 30)));
        let pages = lopdf::Document::load_mem(&part.bytes).unwrap().get_pages().len();
        assert_eq!(pages, 3);

        assert!(
            select(&long, Some("1-21"), "long.pdf")
                .unwrap_err()
                .0
                .contains("at most 20")
        );
        assert!(
            select(&long, Some("29-31"), "long.pdf")
                .unwrap_err()
                .0
                .contains("past its end")
        );
        assert!(select(&long, Some("five"), "long.pdf").is_err());
        assert!(
            select(&sample(4), None, "short.pdf").unwrap().part.is_none(),
            "a short PDF goes whole"
        );
    }
}
