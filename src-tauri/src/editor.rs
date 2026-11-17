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

/// Asks where to save a JSON file named `name` and writes `contents` there; `None` when the dialog was closed.
/// Desktop only: a remote device must never choose a path on this computer.
#[tauri::command]
pub(crate) async fn save_json_file(name: String, contents: String) -> Result<Option<String>, String> {
    let Some(path) = rfd::AsyncFileDialog::new()
        .set_file_name(&name)
        .add_filter("JSON", &["json"])
        .save_file()
        .await
    else {
        return Ok(None);
    };
    let path = path.path().to_path_buf();
    tokio::fs::write(&path, contents)
        .await
        .map_err(|error| error.to_string())?;

    Ok(Some(path.to_string_lossy().to_string()))
}

#[tauri::command(async)]
pub(crate) fn open_file(
    app: tauri::AppHandle,
    path: String,
    line: Option<u32>,
    column: Option<u32>,
) -> Result<OpenFileResult, String> {
    let file = local_file(&path)?;
    let launches = launches_code(&path, &std::env::var("PATHEXT").unwrap_or_default()) || executable(file);
    let position = (line.unwrap_or(1).max(1), column.unwrap_or(1).max(1));
    let positioned = (line.is_some() || launches) && open_positioned(&path, position.0, position.1);
    if positioned {
        return Ok(OpenFileResult { positioned });
    }

    let opener = app.opener();
    let opened = if launches {
        opener.reveal_item_in_dir(&path)
    } else {
        opener.open_path(&path, None::<&str>)
    };
    opened.map_err(|error| error.to_string())?;

    Ok(OpenFileResult { positioned })
}

/// `path` as a file on this computer that exists: never a link, a share or a name the shell would complete.
fn local_file(path: &str) -> Result<&Path, String> {
    // The shell stops at a NUL, so `evil.exe\0.txt` would run evil.exe.
    if path.contains('\0') {
        return Err("The file path contains a NUL character".into());
    }
    let file = Path::new(path);
    if !local_absolute(file) {
        return Err(format!("Only a file on this computer opens: {path}"));
    }
    if !file.is_file() {
        return Err(format!("No such file: {path}"));
    }

    Ok(file)
}

#[cfg(windows)]
fn local_absolute(path: &Path) -> bool {
    use std::path::{Component, Prefix};
    let shared = path.to_str().is_some_and(|text| {
        let mut start = text.chars();
        matches!((start.next(), start.next()), (Some('\\' | '/'), Some('\\' | '/')))
    });
    let disk = matches!(
        path.components().next(),
        Some(Component::Prefix(prefix)) if matches!(prefix.kind(), Prefix::Disk(_))
    );

    !shared && disk && path.is_absolute()
}

#[cfg(not(windows))]
fn local_absolute(path: &Path) -> bool {
    path.is_absolute()
}

/// A file any execute bit marks as a program, which the default handler may run.
#[cfg(unix)]
fn executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path).is_ok_and(|metadata| metadata.permissions().mode() & 0o111 != 0)
}

#[cfg(not(unix))]
fn executable(_path: &Path) -> bool {
    false
}

/// Files the OS default handler would run rather than show: programs, scripts, installers, shortcuts and disk images.
fn launches_code(path: &str, path_extensions: &str) -> bool {
    const LAUNCHERS: &[&str] = &[
        "app",
        "appimage",
        "appinstaller",
        "application",
        "appref-ms",
        "appx",
        "appxbundle",
        "bat",
        "chm",
        "cmd",
        "com",
        "command",
        "cpl",
        "desktop",
        "diagcab",
        "exe",
        "hta",
        "img",
        "inf",
        "iso",
        "jar",
        "jnlp",
        "js",
        "jse",
        "library-ms",
        "lnk",
        "msc",
        "msi",
        "msix",
        "msixbundle",
        "msp",
        "msu",
        "pif",
        "ps1",
        "ps1xml",
        "psc1",
        "psm1",
        "py",
        "pyw",
        "rdp",
        "reg",
        "scf",
        "scr",
        "search-ms",
        "searchconnector-ms",
        "settingcontent-ms",
        "sh",
        "url",
        "vb",
        "vbe",
        "vbs",
        "vhd",
        "vhdx",
        "website",
        "ws",
        "wsc",
        "wsf",
        "wsh",
        "xll",
    ];
    const MACOS_LAUNCHERS: &[&str] = &[
        "dmg", "fileloc", "inetloc", "mpkg", "pkg", "terminal", "webloc", "workflow",
    ];
    // Windows ignores trailing dots and spaces, so `run.bat.` still runs as a batch file.
    let name = path.trim_end_matches(['.', ' ']);
    let Some(extension) = Path::new(name).extension().and_then(|value| value.to_str()) else {
        return false;
    };
    let extension = extension.to_ascii_lowercase();
    LAUNCHERS.contains(&extension.as_str())
        || (cfg!(target_os = "macos") && MACOS_LAUNCHERS.contains(&extension.as_str()))
        || path_extensions
            .split(';')
            .any(|known| known.trim().trim_start_matches('.').eq_ignore_ascii_case(&extension))
}

#[tauri::command(async)]
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

    let arguments = editor_arguments(editor.kind, path, line, column);
    match spawn_editor(&editor.executable, &arguments) {
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
        local.as_ref().map(|root| root.join("Programs/Cursor/Cursor.exe")),
        local.as_ref().map(|root| root.join("Programs/Windsurf/Windsurf.exe")),
        local.as_ref().map(|root| root.join("Programs/Zed/Zed.exe")),
        program.as_ref().map(|root| root.join("Microsoft VS Code/Code.exe")),
        program.as_ref().map(|root| root.join("Sublime Text/sublime_text.exe")),
        program.as_ref().map(|root| root.join("Notepad++/notepad++.exe")),
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
        for (line, column) in [(None, None), (None, Some(3)), (Some(24), Some(3)), (Some(0), Some(0))] {
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
    fn launchable_files_are_never_handed_to_the_default_opener() {
        let pathext = ".COM;.EXE;.BAT;.CMD;.VBS;.JS;.WSF;.MSC;.CUSTOM";
        for path in [
            r"C:\repo\run.bat",
            r"C:\repo\RUN.CMD",
            r"C:\repo\tool.exe",
            r"C:\repo\setup.ps1",
            r"C:\repo\a.vbs",
            r"C:\repo\a.js",
            r"C:\repo\link.lnk",
            r"C:\repo\pkg.msi",
            r"C:\repo\saver.scr",
            r"C:\repo\run.bat. .",
            r"C:\repo\x.custom",
            "/repo/install.sh",
            r"C:\repo\open.settingcontent-ms",
            r"C:\repo\lib.library-ms",
            r"C:\repo\find.search-ms",
            r"C:\repo\find.searchconnector-ms",
            r"C:\repo\help.chm",
            r"C:\repo\get.appinstaller",
            r"C:\repo\pkg.msix",
            r"C:\repo\pkg.MSIXBUNDLE",
            r"C:\repo\pkg.appx",
            r"C:\repo\pkg.appxbundle",
            r"C:\repo\fix.diagcab",
            r"C:\repo\disk.iso",
            r"C:\repo\disk.img",
            r"C:\repo\disk.vhd",
            r"C:\repo\disk.vhdx",
            r"C:\repo\host.rdp",
            r"C:\repo\site.website",
            r"C:\repo\addin.xll",
            r"C:\repo\update.msu",
            r"C:\repo\console.psc1",
            r"C:\repo\types.ps1xml",
            r"C:\repo\app.jnlp",
            r"C:\repo\tool.py",
            r"C:\repo\tool.pyw",
        ] {
            assert!(launches_code(path, pathext), "{path}");
        }
        for path in [
            "/repo/x.terminal",
            "/repo/x.workflow",
            "/repo/x.pkg",
            "/repo/x.mpkg",
            "/repo/x.dmg",
            "/repo/x.fileloc",
            "/repo/x.webloc",
            "/repo/x.inetloc",
        ] {
            assert_eq!(launches_code(path, ""), cfg!(target_os = "macos"), "{path}");
        }
        for path in [
            r"C:\repo\notes.md",
            r"C:\repo\main.rs",
            r"C:\repo\Makefile",
            r"C:\repo\bat",
            "/repo/a.json",
        ] {
            assert!(!launches_code(path, pathext), "{path}");
            assert!(!launches_code(path, ""), "{path}");
        }
    }

    #[test]
    fn only_an_existing_local_file_is_opened() {
        let file = drift_engine::platform::temp::test_run().join("editor-local-file.txt");
        std::fs::write(&file, b"").unwrap();
        let existing = file.to_str().unwrap();
        assert_eq!(local_file(existing), Ok(file.as_path()));

        let with_nul = format!("{existing}\0.exe");
        let missing = format!("{existing}.missing");
        let mut refused = vec![
            with_nul.as_str(),
            missing.as_str(),
            "notes.txt",
            "search-ms:query=x",
            "ms-settings:privacy",
            "https://example.com/a.txt",
            "file:///etc/hosts",
        ];
        if cfg!(windows) {
            refused.extend([
                r"\\host\share\a.txt",
                "//host/share/a.txt",
                r"\\?\UNC\host\share\a.txt",
                r"\\.\C:\a.txt",
                r"C:a.txt",
                r"\a.txt",
            ]);
        }
        for path in refused {
            assert!(local_file(path).is_err(), "{path:?}");
        }
        let _ = std::fs::remove_file(file);
    }

    #[cfg(unix)]
    #[test]
    fn a_file_with_an_execute_bit_counts_as_code() {
        use std::os::unix::fs::PermissionsExt;
        let file = drift_engine::platform::temp::test_run().join("editor-executable");
        std::fs::write(&file, b"#!/bin/sh\n").unwrap();
        assert!(!executable(&file));
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o644 | 0o010)).unwrap();
        assert!(executable(&file));
        let _ = std::fs::remove_file(file);
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
