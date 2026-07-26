//! Cross-module tests: these cover behaviour that spans several of the shell's modules, so they
//! stay attached to the crate root rather than to any one of them.

use crate::clipboard::clipboard_utf16;
use crate::config::config_path;
use crate::editor::{editor_arguments, editor_kind, EditorKind};
use crate::engine::{basic_authorization, Engine};
use crate::watcher::{file_signatures, watched_mcp_paths};
use std::path::Path;

#[test]
fn config_paths_stay_under_the_config_directory() {
    let root = Path::new("config");
    assert_eq!(
        config_path(root, "plugins/example.mjs").unwrap(),
        root.join("plugins/example.mjs")
    );
    assert!(config_path(root, "../outside.mjs").is_err());
    assert!(config_path(root, "C:\\outside.mjs").is_err());
}

#[test]
fn editor_locations_use_one_direct_gui_invocation() {
    assert_eq!(editor_kind(Path::new("Code.exe")), EditorKind::GotoFlag);
    assert_eq!(
        editor_kind(Path::new("sublime_text.exe")),
        EditorKind::Location
    );
    assert_eq!(
        editor_kind(Path::new("notepad++.exe")),
        EditorKind::NotepadPlus
    );
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
fn engine_credentials_are_random_and_basic_auth_encoded() {
    let first = Engine::default().password;
    let second = Engine::default().password;
    assert_eq!(first.len(), 64);
    assert_ne!(first, second);
    assert_eq!(basic_authorization("user", "pass"), "dXNlcjpwYXNz");
}

#[test]
fn clipboard_text_is_utf16_and_null_terminated() {
    assert_eq!(
        clipboard_utf16("Drift \u{1fabc}"),
        "Drift \u{1fabc}\0".encode_utf16().collect::<Vec<_>>()
    );
}

#[test]
fn mcp_watch_paths_include_plugins_and_external_file_references() {
    let root =
        std::env::temp_dir().join(format!("drift-mcp-watch-test-{}", std::process::id()));
    std::fs::remove_dir_all(&root).ok();
    let config_root = root.join("workspace/.opencode");
    let plugin = config_root.join("plugin/nested/server.ts");
    let external = root.join("outside/secret.txt");
    std::fs::create_dir_all(plugin.parent().unwrap()).unwrap();
    std::fs::create_dir_all(external.parent().unwrap()).unwrap();
    std::fs::write(&plugin, "export default {}\n").unwrap();
    std::fs::write(&external, "one").unwrap();
    let config = config_root.join("opencode.json");
    let reference = external.to_string_lossy().replace('\\', "/");
    std::fs::write(
        &config,
        format!(r#"{{"mcp":{{"x":{{"token":"{{file:{reference}}}"}}}}}}"#),
    )
    .unwrap();

    let paths = watched_mcp_paths(
        vec![config.clone()],
        vec![config_root.join("plugin"), config_root.join("plugins")],
    );
    assert!(paths.contains(&config));
    assert!(paths.contains(&plugin));
    assert!(paths.contains(&external));
    assert!(paths.len() <= 4096);
    assert_eq!(
        paths,
        watched_mcp_paths(
            vec![config.clone()],
            vec![config_root.join("plugin"), config_root.join("plugins")],
        )
    );
    let before = file_signatures(paths.clone());
    std::fs::write(&external, "two").unwrap();
    let after = file_signatures(paths);
    assert_ne!(before, after);
    let before_plugin = after;
    std::fs::write(&plugin, "export default { changed: true }\n").unwrap();
    let after_plugin = file_signatures(watched_mcp_paths(
        vec![config],
        vec![config_root.join("plugin"), config_root.join("plugins")],
    ));
    assert_ne!(before_plugin, after_plugin);
    std::fs::remove_dir_all(root).ok();
}
