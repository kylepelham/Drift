//! Text matching excludes the UTF-8 BOM; replacement preserves it and the file's line endings.

#[derive(Clone, Copy, Default)]
pub struct TextFormat {
    crlf: bool,
    bom: bool,
}

impl TextFormat {
    pub fn detect(text: &str) -> Self {
        Self { crlf: text.contains("\r\n"), bom: text.starts_with('\u{feff}') }
    }

    pub fn normalise(self, text: &str) -> String {
        text.strip_prefix('\u{feff}').unwrap_or(text).replace("\r\n", "\n")
    }

    pub fn apply(self, text: &str) -> String {
        let bom = self.bom || text.starts_with('\u{feff}');
        let text = text.strip_prefix('\u{feff}').unwrap_or(text);
        let body = if self.crlf { text.replace("\r\n", "\n").replace('\n', "\r\n") } else { text.to_string() };
        if bom { format!("\u{feff}{body}") } else { body }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;
    use crate::tool::{apply_patch::ApplyPatch, edit::Edit, read::Read, write::Write, Tool};
    use crate::tool::tests::Sandbox;

    #[tokio::test]
    async fn edits_writes_and_patches_preserve_bom_and_crlf() {
        let sandbox = Sandbox::new("bom");
        let path = sandbox.file("a.txt", "\u{feff}one\r\ntwo\r\n");
        let read = Read.run(&sandbox.ctx, json!({ "path":"a.txt" })).await.unwrap();
        assert!(read.output.starts_with("1: one"));
        Edit.run(&sandbox.ctx, json!({ "path":"a.txt", "old_string":"one\ntwo", "new_string":"ONE\nTWO" })).await.unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "\u{feff}ONE\r\nTWO\r\n");
        Write.run(&sandbox.ctx, json!({ "path":"a.txt", "content":"first\nsecond\n" })).await.unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "\u{feff}first\r\nsecond\r\n");
        ApplyPatch.run(&sandbox.ctx, json!({ "patch":"*** Begin Patch\n*** Update File: a.txt\n@@\n-first\n+FIRST\n*** End Patch" })).await.unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "\u{feff}FIRST\r\nsecond\r\n");
        ApplyPatch.run(&sandbox.ctx, json!({ "patch":"*** Begin Patch\n*** Add File: a.txt\n+replacement\n*** End Patch" })).await.unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "\u{feff}replacement\r\n");
    }
}
