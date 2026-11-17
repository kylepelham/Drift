//! Text matching excludes the UTF-8 BOM; replacement preserves it and the file's line endings.

#[derive(Clone, Copy, Default)]
pub(super) struct TextFormat {
    crlf: bool,
    bom: bool,
}

impl TextFormat {
    pub(super) fn detect(text: &str) -> Self {
        Self {
            crlf: text.contains("\r\n") && dominant(text, "\r\n") == "\r\n",
            bom: text.starts_with('\u{feff}'),
        }
    }

    pub(super) fn normalise(self, text: &str) -> String {
        text.strip_prefix('\u{feff}').unwrap_or(text).replace("\r\n", "\n")
    }

    pub(super) fn apply(self, text: &str) -> String {
        let bom = self.bom || text.starts_with('\u{feff}');
        let text = text.strip_prefix('\u{feff}').unwrap_or(text);
        let body = if self.crlf {
            text.replace("\r\n", "\n").replace('\n', "\r\n")
        } else {
            text.to_string()
        };

        if bom { format!("\u{feff}{body}") } else { body }
    }
}

/// The line ending most of `text` uses, or `fallback` when neither has more.
pub(super) fn dominant(text: &str, fallback: &'static str) -> &'static str {
    let crlf = text.matches("\r\n").count();
    let lf = text.matches('\n').count() - crlf;

    match crlf.cmp(&lf) {
        std::cmp::Ordering::Greater => "\r\n",
        std::cmp::Ordering::Less => "\n",
        std::cmp::Ordering::Equal => fallback,
    }
}

/// A typographic dash, quote or space as its ASCII form, one character for one, so offsets map back.
pub(super) fn ascii(character: char) -> char {
    match character {
        '\u{2010}'..='\u{2015}' | '\u{2212}' => '-',
        '\u{2018}'..='\u{201B}' => '\'',
        '\u{201C}'..='\u{201F}' => '"',
        '\u{00A0}' | '\u{2002}'..='\u{200A}' | '\u{202F}' | '\u{205F}' | '\u{3000}' => ' ',
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use crate::tool::tests::Sandbox;
    use crate::tool::{Tool, apply_patch::ApplyPatch, edit::Edit, read::Read, write::Write};
    use serde_json::json;

    #[test]
    fn a_file_keeps_crlf_unless_most_of_its_lines_end_in_lf() {
        let crlf = |text: &str| super::TextFormat::detect(text).crlf;
        assert!(crlf("a\r\nb\n"), "a tie stays CRLF, as it always did");
        assert!(crlf("a\r\nb\r\nc\n"));
        assert!(!crlf("a\r\nb\nc\n"));
        assert!(!crlf("no newline"));
        assert!(!crlf(""));
    }

    #[tokio::test]
    async fn edits_writes_and_patches_preserve_bom_and_crlf() {
        let sandbox = Sandbox::new("bom");
        let path = sandbox.file("a.txt", "\u{feff}one\r\ntwo\r\n");
        let read = Read.run(&sandbox.ctx, json!({ "path":"a.txt" })).await.unwrap();
        assert!(read.output.starts_with("1: one"));
        Edit.run(
            &sandbox.ctx,
            json!({ "path":"a.txt", "old_string":"one\ntwo", "new_string":"ONE\nTWO" }),
        )
        .await
        .unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "\u{feff}ONE\r\nTWO\r\n");
        Write
            .run(&sandbox.ctx, json!({ "path":"a.txt", "content":"first\nsecond\n" }))
            .await
            .unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "\u{feff}first\r\nsecond\r\n");
        ApplyPatch
            .run(
                &sandbox.ctx,
                json!({ "patch":"*** Begin Patch\n*** Update File: a.txt\n@@\n-first\n+FIRST\n*** End Patch" }),
            )
            .await
            .unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "\u{feff}FIRST\r\nsecond\r\n");
        ApplyPatch
            .run(
                &sandbox.ctx,
                json!({ "patch":"*** Begin Patch\n*** Add File: a.txt\n+replacement\n*** End Patch" }),
            )
            .await
            .unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "\u{feff}replacement\r\n");
    }

    #[tokio::test]
    async fn utf16_files_are_read_edited_written_and_patched_as_utf16() {
        use crate::tool::encoding::Encoding;
        let cases = [
            (Encoding::Utf16Le, "\u{feff}"),
            (Encoding::Utf16Be, "\u{feff}"),
            (Encoding::Utf16Le, ""),
        ];
        for (encoding, bom) in cases {
            let sandbox = Sandbox::new("utf16");
            let path = sandbox.ctx.workspace.join("a.txt");
            let on_disk = || std::fs::read(&path).unwrap();
            std::fs::write(&path, encoding.encode(&format!("{bom}one\r\ntwo\r\n"))).unwrap();

            let read = Read.run(&sandbox.ctx, json!({ "path":"a.txt" })).await.unwrap();
            assert!(read.output.starts_with("1: one\n2: two"), "{}", read.output);
            assert!(read.output.contains("UTF-16"), "{}", read.output);
            Edit.run(
                &sandbox.ctx,
                json!({ "path":"a.txt", "old_string":"one\ntwo", "new_string":"ÖNE\nTWO" }),
            )
            .await
            .unwrap();
            assert_eq!(on_disk(), encoding.encode(&format!("{bom}ÖNE\r\nTWO\r\n")));
            Write
                .run(&sandbox.ctx, json!({ "path":"a.txt", "content":"first\nsecond\n" }))
                .await
                .unwrap();
            assert_eq!(on_disk(), encoding.encode(&format!("{bom}first\r\nsecond\r\n")));
            ApplyPatch
                .run(
                    &sandbox.ctx,
                    json!({ "patch":"*** Begin Patch\n*** Update File: a.txt\n@@\n-first\n+FIRST\n*** End Patch" }),
                )
                .await
                .unwrap();
            assert_eq!(on_disk(), encoding.encode(&format!("{bom}FIRST\r\nsecond\r\n")));
            ApplyPatch
                .run(
                    &sandbox.ctx,
                    json!({ "patch":"*** Begin Patch\n*** Add File: a.txt\n+replacement\n*** End Patch" }),
                )
                .await
                .unwrap();
            assert_eq!(on_disk(), encoding.encode(&format!("{bom}replacement\r\n")));
        }
    }

    #[tokio::test]
    async fn binary_is_still_refused_and_a_new_file_is_utf8() {
        let sandbox = Sandbox::new("utf16-binary");
        let binary = sandbox.ctx.workspace.join("blob.bin");
        std::fs::write(&binary, [0x50, 0x4B, 0x03, 0x04, 0x14, 0x00, 0x00, 0x00, 0x08, 0x00]).unwrap();
        let refused = Read.run(&sandbox.ctx, json!({ "path":"blob.bin" })).await.unwrap_err();
        assert!(refused.0.contains("is binary"), "{}", refused.0);

        Write
            .run(&sandbox.ctx, json!({ "path":"new.txt", "content":"héllo\n" }))
            .await
            .unwrap();
        assert_eq!(
            std::fs::read(sandbox.ctx.workspace.join("new.txt")).unwrap(),
            "héllo\n".as_bytes()
        );
    }

    #[tokio::test]
    async fn grep_finds_text_in_a_utf16_file_with_a_byte_order_mark() {
        use crate::tool::{Tool, encoding::Encoding, grep::Grep};
        let sandbox = Sandbox::new("utf16-grep");
        let encoded = Encoding::Utf16Le.encode("\u{feff}first line\r\nneedle here\r\n");
        std::fs::write(sandbox.ctx.workspace.join("wide.txt"), encoded).unwrap();
        let found = Grep.run(&sandbox.ctx, json!({ "pattern": "needle" })).await.unwrap();
        assert!(found.output.contains("wide.txt:2: needle here"), "{}", found.output);
    }
}
