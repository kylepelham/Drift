//! Cross-module tests: these cover behaviour that spans several of the shell's modules, so they
//! stay attached to the crate root rather than to any one of them.

use crate::clipboard::clipboard_utf16;
use crate::config::config_path;
use crate::editor::{EditorKind, editor_arguments, editor_kind};
use crate::updater::installed_alongside_uninstaller;
use std::path::Path;

#[test]
fn config_paths_stay_under_the_config_directory() {
    let root = Path::new("config");
    assert_eq!(
        config_path(root, "plugins/example.mjs").unwrap(),
        root.join("plugins/example.mjs")
    );
    assert!(config_path(root, "../outside.mjs").is_err());
    #[cfg(windows)]
    assert!(config_path(root, "C:\\outside.mjs").is_err());
}

#[test]
fn editor_locations_use_one_direct_gui_invocation() {
    assert_eq!(editor_kind(Path::new("Code.exe")), EditorKind::GotoFlag);
    assert_eq!(editor_kind(Path::new("sublime_text.exe")), EditorKind::Location);
    assert_eq!(editor_kind(Path::new("notepad++.exe")), EditorKind::NotepadPlus);
    assert_eq!(
        editor_arguments(EditorKind::GotoFlag, "S:\\repo\\app.ts", 24, 3),
        ["--goto", "S:\\repo\\app.ts:24:3"]
    );
    assert_eq!(
        editor_arguments(EditorKind::NotepadPlus, "S:\\repo\\app.ts", 24, 3),
        ["-n24", "-c3", "S:\\repo\\app.ts"]
    );
}

#[test]
fn updates_are_only_offered_next_to_the_uninstaller() {
    let root = std::env::temp_dir().join(format!("drift-updater-test-{}", std::process::id()));
    std::fs::remove_dir_all(&root).ok();
    let installed = root.join("installed");
    let portable = root.join("target/release");
    std::fs::create_dir_all(&installed).unwrap();
    std::fs::create_dir_all(&portable).unwrap();
    std::fs::write(installed.join("uninstall.exe"), "nsis").unwrap();
    assert!(installed_alongside_uninstaller(&installed.join("drift.exe")));
    assert!(!installed_alongside_uninstaller(&portable.join("drift.exe")));
    assert!(!installed_alongside_uninstaller(Path::new("drift.exe")));
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn clipboard_text_is_utf16_and_null_terminated() {
    assert_eq!(
        clipboard_utf16("Drift \u{1fabc}"),
        "Drift \u{1fabc}\0".encode_utf16().collect::<Vec<_>>()
    );
}
