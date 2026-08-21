//! Cross-module tests: these cover behaviour that spans several of the shell's modules, so they
//! stay attached to the crate root rather than to any one of them.

use crate::clipboard::clipboard_utf16;
use crate::config::config_path;
use crate::editor::{editor_arguments, editor_kind, EditorKind};
use crate::engine::{basic_authorization, Engine};
use crate::updater::installed_alongside_uninstaller;
use crate::watcher::{
    file_signatures, resolve_skill_path, watched_mcp_paths, watched_skill_paths, SkillWatchRoots,
};
use std::collections::HashSet;
use std::path::{Path, PathBuf};

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

#[test]
fn skill_watch_paths_detect_additions_removals_and_content_changes() {
    let root = std::env::temp_dir().join(format!("drift-skill-watch-test-{}", std::process::id()));
    std::fs::remove_dir_all(&root).ok();
    let skills = root.join(".agents/skills");
    let first = skills.join("unslop/SKILL.md");
    let unrelated = skills.join("unslop/reference.md");
    std::fs::create_dir_all(first.parent().unwrap()).unwrap();
    std::fs::write(&first, "---\nname: unslop\ndescription: old\n---\nOld\n").unwrap();
    std::fs::write(&unrelated, "ignored").unwrap();

    let paths = watched_skill_paths(vec![skills.clone()]);
    assert_eq!(paths, vec![first.clone()]);
    let before = file_signatures(paths);

    std::fs::write(&first, "---\nname: unslop\ndescription: new\n---\nNew\n").unwrap();
    let changed = file_signatures(watched_skill_paths(vec![skills.clone()]));
    assert_ne!(before, changed);

    let second = skills.join("review/SKILL.md");
    std::fs::create_dir_all(second.parent().unwrap()).unwrap();
    std::fs::write(&second, "---\nname: review\n---\nReview\n").unwrap();
    let added = file_signatures(watched_skill_paths(vec![skills.clone()]));
    assert_ne!(changed, added);

    std::fs::remove_file(&first).unwrap();
    let removed = file_signatures(watched_skill_paths(vec![skills]));
    assert_ne!(added, removed);
    assert_eq!(removed.len(), 1);
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn configured_skill_paths_resolve_relative_to_the_workspace() {
    let workspace = if cfg!(windows) { r"S:\repo" } else { "/repo" };
    assert_eq!(
        resolve_skill_path(workspace, "shared/skills").unwrap(),
        Path::new(workspace).join("shared/skills")
    );
    assert!(resolve_skill_path(workspace, "").is_err());
}

#[test]
fn configured_skill_paths_survive_missing_workspaces_then_drop_removed_workspaces() {
    let roots = SkillWatchRoots::default();
    roots.replace(PathBuf::from("first"), vec![PathBuf::from("first-skills")]);
    roots.replace(
        PathBuf::from("second"),
        vec![PathBuf::from("second-skills")],
    );
    assert_eq!(
        roots.paths(None).into_iter().collect::<HashSet<_>>(),
        HashSet::from([
            PathBuf::from("first-skills"),
            PathBuf::from("second-skills"),
        ])
    );
    let active = HashSet::from([PathBuf::from("second")]);
    assert_eq!(
        roots.paths(Some(&active)),
        vec![PathBuf::from("second-skills")]
    );
    assert_eq!(roots.paths(None), vec![PathBuf::from("second-skills")]);
}
