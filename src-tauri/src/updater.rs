//! Update check and install, delegated to the Tauri updater plugin.

use std::path::Path;
use tauri::Manager;

/// True when the executable sits in an NSIS-installed location (next to its uninstaller).
///
/// Local release builds run from a cargo target directory. The installer can never replace
/// them, so offering updates there produces an endless "update available" loop against the
/// separately installed copy.
pub(crate) fn installed_alongside_uninstaller(exe: &Path) -> bool {
    exe.parent()
        .is_some_and(|dir| dir.join("uninstall.exe").is_file())
}

/// Debug builds never update: a local build's version usually trails the latest release, so it
/// would otherwise be offered an "update" that replaces the build under development.
fn updatable() -> bool {
    if cfg!(debug_assertions) {
        return false;
    }
    std::env::current_exe()
        .map(|exe| installed_alongside_uninstaller(&exe))
        .unwrap_or(false)
}

/// Whether this build can update itself, surfaced in About so a local build is recognizable.
#[tauri::command]
pub(crate) fn update_support() -> bool {
    updatable()
}

/// Returns the version of an available update, or `None` if the app is current.
#[tauri::command]
pub(crate) async fn check_update(app: tauri::AppHandle) -> Result<Option<String>, String> {
    if !updatable() {
        return Ok(None);
    }
    let updater = tauri_plugin_updater::UpdaterExt::updater(&app).map_err(|e| e.to_string())?;
    let update = updater.check().await.map_err(|e| e.to_string())?;
    Ok(update.map(|u| u.version))
}

/// Downloads and installs the pending update, then restarts.
///
/// The engine sidecar is killed between download and install: the NSIS run replaces files in
/// the install directory, and on Windows the plugin exits this process without firing
/// `RunEvent::Exit`, so the sidecar would otherwise survive and hold `drift-engine.exe` locked.
///
/// Never returns on success: the installer exits the process on Windows and `app.restart()`
/// diverges elsewhere, which is why there is no trailing `Ok(())`.
#[tauri::command]
pub(crate) async fn install_update(app: tauri::AppHandle) -> Result<(), String> {
    if !updatable() {
        return Err("updates can only be installed from an installed copy of Drift".into());
    }
    let updater = tauri_plugin_updater::UpdaterExt::updater(&app).map_err(|e| e.to_string())?;
    let update = updater
        .check()
        .await
        .map_err(|e| e.to_string())?
        .ok_or("no update available")?;
    let bytes = update
        .download(|_, _| {}, || {})
        .await
        .map_err(|e| e.to_string())?;
    let child = app.state::<crate::engine::Engine>().child.lock().unwrap().take();
    if let Some(mut child) = child {
        let _ = child.kill();
    }
    update.install(bytes).map_err(|e| e.to_string())?;
    app.restart();
}
