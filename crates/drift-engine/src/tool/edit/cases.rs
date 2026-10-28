//! opencode v2's `test/tool-edit.test.ts` cases (at 501b15af2), in Drift's terms.

use serde_json::{Value, json};

use super::Edit;
use crate::tool::tests::Sandbox;
use crate::tool::{Output, Tool, ToolError};

async fn edit(sandbox: &Sandbox, input: Value) -> Result<Output, ToolError> {
    Edit.run(&sandbox.ctx, input).await
}

fn call(path: &str, old: &str, new: &str) -> Value {
    json!({ "path": path, "old_string": old, "new_string": new })
}

fn read(sandbox: &Sandbox, name: &str) -> String {
    std::fs::read_to_string(sandbox.ctx.workspace.join(name)).unwrap()
}

#[tokio::test]
async fn no_op_empty_missing_and_ambiguous_replacements_are_refused_and_write_nothing() {
    let sandbox = Sandbox::new("edit-refused");
    sandbox.file("matches.txt", "same same");

    assert!(edit(&sandbox, call("matches.txt", "same", "same")).await.is_err());
    assert!(edit(&sandbox, call("matches.txt", "", "after")).await.is_err());
    assert!(edit(&sandbox, call("matches.txt", "missing", "after")).await.is_err());
    let ambiguous = edit(&sandbox, call("matches.txt", "same", "after")).await.unwrap_err();
    assert!(ambiguous.0.contains("2 places"), "{}", ambiguous.0);
    assert_eq!(read(&sandbox, "matches.txt"), "same same");
}

#[tokio::test]
async fn replace_all_replaces_every_exact_occurrence() {
    let sandbox = Sandbox::new("edit-all");
    sandbox.file("all.txt", "same same same");
    let mut input = call("all.txt", "same", "after");
    input["replace_all"] = json!(true);

    let out = edit(&sandbox, input).await.unwrap();
    assert_eq!(out.metadata.replacements, Some(3));
    assert_eq!(read(&sandbox, "all.txt"), "after after after");
}

#[tokio::test]
async fn typography_is_folded_only_after_the_exact_match_finds_nothing() {
    let sandbox = Sandbox::new("edit-unicode");
    sandbox.file(
        "unicode.txt",
        "exact - match\ncurly \u{201c}quotes\u{201d}\nminus \u{2212} one\nspace\u{a0}here\nexact \u{2212} match\n",
    );

    edit(
        &sandbox,
        call("unicode.txt", "curly \"quotes\"\nminus - one\nspace here", "normalized"),
    )
    .await
    .unwrap();
    edit(&sandbox, call("unicode.txt", "exact - match", "selected"))
        .await
        .unwrap();
    assert_eq!(
        read(&sandbox, "unicode.txt"),
        "selected\nnormalized\nexact \u{2212} match\n"
    );
}

#[tokio::test]
async fn trailing_whitespace_is_ignored_and_untouched_lines_keep_theirs() {
    let sandbox = Sandbox::new("edit-trailing");
    sandbox.file("whitespace.txt", "before  \nmatch  \nnext\t\nafter  \n");

    edit(&sandbox, call("whitespace.txt", "match\nnext", "changed"))
        .await
        .unwrap();
    assert_eq!(read(&sandbox, "whitespace.txt"), "before  \nchanged\nafter  \n");
}

#[tokio::test]
async fn typography_and_trailing_whitespace_together_are_a_miss() {
    let sandbox = Sandbox::new("edit-combined");
    sandbox.file("both.txt", "a\u{2014}b  \nnext\n");

    assert!(edit(&sandbox, call("both.txt", "a-b\nnext", "changed")).await.is_err());
    assert_eq!(read(&sandbox, "both.txt"), "a\u{2014}b  \nnext\n");
}

#[tokio::test]
async fn trailing_whitespace_matches_never_overlap_and_crlf_is_kept() {
    let sandbox = Sandbox::new("edit-overlap");
    sandbox.file("overlap.txt", "a  \na  \na  \n");
    sandbox.file("windows.txt", "a  \r\nb\t\r\n");
    let mut all = call("overlap.txt", "a\na", "x");
    all["replace_all"] = json!(true);

    assert_eq!(edit(&sandbox, all).await.unwrap().metadata.replacements, Some(1));
    edit(&sandbox, call("windows.txt", "a\nb", "x")).await.unwrap();
    assert_eq!(read(&sandbox, "overlap.txt"), "x\na  \n");
    assert_eq!(read(&sandbox, "windows.txt"), "x\r\n");
}

#[tokio::test]
async fn a_bom_and_crlf_endings_are_kept() {
    let sandbox = Sandbox::new("edit-bom");
    sandbox.file("windows.txt", "\u{feff}before\r\nrest\r\n");

    edit(&sandbox, call("windows.txt", "before\nrest", "after\nrest"))
        .await
        .unwrap();
    assert_eq!(read(&sandbox, "windows.txt"), "\u{feff}after\r\nrest\r\n");
}

#[tokio::test]
async fn a_mixed_file_keeps_each_regions_endings() {
    let sandbox = Sandbox::new("edit-mixed");
    sandbox.file("mostly-lf.txt", "head\r\nAAA OLD BBB\ntail\n");
    sandbox.file("mostly-crlf.txt", "one\r\ntwo\r\nthree\nfour\r\n");

    edit(&sandbox, call("mostly-lf.txt", "AAA OLD BBB", "AAA NEW\nLINE2"))
        .await
        .unwrap();
    edit(&sandbox, call("mostly-lf.txt", "LINE2\ntail", "LINE2\nTAIL"))
        .await
        .unwrap();
    edit(&sandbox, call("mostly-crlf.txt", "o\nthree\nfo", "O\nTHREE\nFO"))
        .await
        .unwrap();
    edit(&sandbox, call("mostly-crlf.txt", "one", "ONE\nUNO"))
        .await
        .unwrap();
    assert_eq!(read(&sandbox, "mostly-lf.txt"), "head\r\nAAA NEW\nLINE2\nTAIL\n");
    assert_eq!(
        read(&sandbox, "mostly-crlf.txt"),
        "ONE\r\nUNO\r\ntwO\r\nTHREE\nFOur\r\n"
    );
}

#[tokio::test]
async fn unchanged_and_in_place_lines_inside_a_match_keep_their_endings() {
    let sandbox = Sandbox::new("edit-pasted");
    sandbox.file(
        "store.cs",
        "All(item.Sku);\r\n\r\nFind(sku);\r\n\r\nvoid Add()\n{\n  _items[item.Sku] = item;\n}\r\n",
    );

    edit(
        &sandbox,
        call(
            "store.cs",
            "All(item.Sku);\n\nFind(sku);\n\nvoid Add()\n{\n  _items[item.Sku] = item;\n}",
            "All(item.Code);\n\nFind(code);\n\nvoid Add()\n{\n  _items[item.Code] = item;\n}\n\nbool Remove(string code)\n{\n}",
        ),
    )
    .await
    .unwrap();
    assert_eq!(
        read(&sandbox, "store.cs"),
        "All(item.Code);\r\n\r\nFind(code);\r\n\r\nvoid Add()\n{\n  _items[item.Code] = item;\n}\r\n\r\nbool Remove(string code)\r\n{\r\n}\r\n"
    );
}

#[tokio::test]
async fn an_edit_that_differs_only_in_carriage_returns_applies_to_the_exact_bytes() {
    let sandbox = Sandbox::new("edit-cr");
    sandbox.file("strip.sh", "a\r\nb\r\n");
    sandbox.file("whole.sh", "x\r\ny\r\n");
    sandbox.file("add.bat", "p\nq\n");
    let mut strip = call("strip.sh", "\r", "");
    strip["replace_all"] = json!(true);

    assert_eq!(edit(&sandbox, strip).await.unwrap().metadata.replacements, Some(2));
    edit(&sandbox, call("whole.sh", "x\r\ny\r\n", "x\ny\n")).await.unwrap();
    edit(&sandbox, call("add.bat", "p\nq", "p\r\nq")).await.unwrap();
    assert!(
        edit(&sandbox, call("add.bat", "p\nq", "p\r\nq")).await.is_err(),
        "the second time there is no LF pair left to change"
    );
    assert_eq!(read(&sandbox, "strip.sh"), "a\nb\n");
    assert_eq!(read(&sandbox, "whole.sh"), "x\ny\n");
    assert_eq!(read(&sandbox, "add.bat"), "p\r\nq\n");
}

#[tokio::test]
async fn an_edit_that_leaves_the_file_unchanged_fails_instead_of_reporting_success() {
    let sandbox = Sandbox::new("edit-unchanged");
    sandbox.file("quote.txt", "it\u{2019}s here\n");

    let error = edit(&sandbox, call("quote.txt", "it's here", "it\u{2019}s here"))
        .await
        .unwrap_err();
    assert!(error.0.contains("unchanged"), "{}", error.0);
    assert_eq!(read(&sandbox, "quote.txt"), "it\u{2019}s here\n");
}
