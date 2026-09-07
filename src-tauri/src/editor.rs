//! Locating an external editor and opening files at a position.

#[cfg(windows)]
use crate::CREATE_NO_WINDOW;
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::OnceLock;
use tauri_plugin_opener::OpenerExt;

#[derive(Serialize)]
pub(crate) struct OpenFileResult {
    positioned: bool,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum EditorKind {
    GotoFlag,
    Location,
    NotepadPlus,
}

struct Editor {
    executable: PathBuf,
    kind: EditorKind,
}

#[tauri::command]
pub(crate) async fn pick_folder() -> Option<String> {
    rfd::FileDialog::new()
        .pick_folder()
        .map(|path| path.to_string_lossy().to_string())
}

#[tauri::command]
pub(crate) fn open_file(
    app: tauri::AppHandle,
    path: String,
    line: Option<u32>,
    column: Option<u32>,
) -> Result<OpenFileResult, String> {
    let positioned =
        line.is_some_and(|line| open_positioned(&path, line.max(1), column.unwrap_or(1).max(1)));
    if positioned {
        return Ok(OpenFileResult { positioned });
    }
    app.opener()
        .open_path(&path, None::<&str>)
        .map_err(|error| error.to_string())?;
    Ok(OpenFileResult { positioned })
}

#[tauri::command]
pub(crate) fn open_file_in_editor(
    path: String,
    line: Option<u32>,
    column: Option<u32>,
) -> Result<OpenFileResult, String> {
    // Keep this separate from open_file: older hosts must reject it, not use an OS association.
    if !open_positioned(&path, line.unwrap_or(1).max(1), column.unwrap_or(1).max(1)) {
        return Err("No editor is available or the editor could not be started".into());
    }
    Ok(OpenFileResult { positioned: true })
}

fn open_positioned(path: &str, line: u32, column: u32) -> bool {
    static EDITOR: OnceLock<Option<Editor>> = OnceLock::new();
    let Some(editor) = EDITOR.get_or_init(detect_editor) else {
        return false;
    };
    match spawn_editor(
        &editor.executable,
        &editor_arguments(editor.kind, path, line, column),
    ) {
        Ok(mut child) => {
            std::thread::spawn(move || {
                let _ = child.wait();
            });
            true
        }
        Err(_) => false,
    }
}

pub(crate) fn editor_arguments(kind: EditorKind, path: &str, line: u32, column: u32) -> Vec<String> {
    let location = format!("{path}:{line}:{column}");
    match kind {
        EditorKind::GotoFlag => vec!["--goto".to_string(), location],
        EditorKind::Location => vec![location],
        EditorKind::NotepadPlus => {
            vec![format!("-n{line}"), format!("-c{column}"), path.to_string()]
        }
    }
}

#[cfg(windows)]
fn detect_editor() -> Option<Editor> {
    for name in ["DRIFT_EDITOR", "VISUAL", "EDITOR"] {
        let Some(value) = std::env::var(name).ok() else {
            continue;
        };
        let path = PathBuf::from(value.trim_matches('"'));
        if path.is_file() {
            return Some(Editor {
                kind: editor_kind(&path),
                executable: path,
            });
        }
    }
    let local = std::env::var_os("LOCALAPPDATA").map(PathBuf::from);
    let program = std::env::var_os("ProgramFiles").map(PathBuf::from);
    let candidates = [
        local
            .as_ref()
            .map(|root| root.join("Programs/Microsoft VS Code/Code.exe")),
        local
            .as_ref()
            .map(|root| root.join("Programs/Microsoft VS Code Insiders/Code - Insiders.exe")),
        local
            .as_ref()
            .map(|root| root.join("Programs/Cursor/Cursor.exe")),
        local
            .as_ref()
            .map(|root| root.join("Programs/Windsurf/Windsurf.exe")),
        local.as_ref().map(|root| root.join("Programs/Zed/Zed.exe")),
        program
            .as_ref()
            .map(|root| root.join("Microsoft VS Code/Code.exe")),
        program
            .as_ref()
            .map(|root| root.join("Sublime Text/sublime_text.exe")),
        program
            .as_ref()
            .map(|root| root.join("Notepad++/notepad++.exe")),
    ];
    candidates
        .into_iter()
        .flatten()
        .find(|path| path.is_file())
        .map(|path| Editor {
            kind: editor_kind(&path),
            executable: path,
        })
}

#[cfg(not(windows))]
fn detect_editor() -> Option<Editor> {
    let executable = ["DRIFT_EDITOR", "VISUAL", "EDITOR"]
        .into_iter()
        .find_map(|name| std::env::var(name).ok())
        .unwrap_or_else(|| "code".to_string());
    let path = PathBuf::from(executable);
    Some(Editor {
        kind: editor_kind(&path),
        executable: path,
    })
}

pub(crate) fn editor_kind(path: &Path) -> EditorKind {
    let name = path
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    if name.contains("notepad++") {
        return EditorKind::NotepadPlus;
    }
    if name.starts_with("zed") || name.starts_with("sublime") || name.starts_with("subl") {
        return EditorKind::Location;
    }
    EditorKind::GotoFlag
}

fn spawn_editor(executable: &Path, args: &[String]) -> std::io::Result<Child> {
    let mut command = Command::new(executable);
    command
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    command.spawn()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn editor_only_open_fails_closed_with_or_without_a_position() {
        // NUL is rejected before process creation, even if this machine has an editor.
        let path = "unlaunchable\0.cmd";
        for (line, column) in [
            (None, None),
            (None, Some(3)),
            (Some(24), Some(3)),
            (Some(0), Some(0)),
        ] {
            let result = open_file_in_editor(path.into(), line, column);
            assert_eq!(
                result.err().as_deref(),
                Some("No editor is available or the editor could not be started")
            );
        }
    }

    #[test]
    fn editor_spawn_rejects_invalid_file_arguments_and_missing_executables() {
        // A file cannot be the parent directory of an editor executable.
        let executable = std::env::current_exe().unwrap();
        assert!(spawn_editor(&executable.join("missing-editor.exe"), &[]).is_err());
        let args = editor_arguments(EditorKind::GotoFlag, "unlaunchable\0.cmd", 1, 1);
        let error = spawn_editor(&executable, &args).unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput);
    }

    #[test]
    fn editor_file_paths_remain_literal_arguments() {
        let path = r#"C:\repo\a & whoami; $(whoami) 'quoted'.cmd"#;
        assert_eq!(
            editor_arguments(EditorKind::GotoFlag, path, 1, 1),
            ["--goto".to_string(), format!("{path}:1:1")]
        );
        assert_eq!(
            editor_arguments(EditorKind::Location, path, 1, 1),
            [format!("{path}:1:1")]
        );
        assert_eq!(
            editor_arguments(EditorKind::NotepadPlus, path, 1, 1),
            ["-n1", "-c1", path]
        );
    }
}
