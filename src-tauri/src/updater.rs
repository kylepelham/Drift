//! Update check and install, delegated to the Tauri updater plugin.

use std::path::Path;

/// True when the executable sits in an NSIS-installed location next to its uninstaller.
pub(crate) fn installed_alongside_uninstaller(exe: &Path) -> bool {
    exe.parent().is_some_and(|dir| dir.join("uninstall.exe").is_file())
}

/// Debug builds and local release builds cannot update themselves.
fn updatable() -> bool {
    if cfg!(debug_assertions) {
        return false;
    }

    std::env::current_exe().is_ok_and(|exe| installed_alongside_uninstaller(&exe))
}

/// Whether this build can update itself, surfaced in About so local builds are recognizable.
#[tauri::command]
pub(crate) fn update_support() -> bool {
    updatable()
}

/// Returns the version of an available update, or None when the app is current.
#[tauri::command]
pub(crate) async fn check_update(app: tauri::AppHandle) -> Result<Option<String>, String> {
    if !updatable() {
        return Ok(None);
    }

    let updater = tauri_plugin_updater::UpdaterExt::updater(&app).map_err(|error| error.to_string())?;
    let update = updater.check().await.map_err(|error| error.to_string())?;

    Ok(update.map(|update| update.version))
}

/// Downloads the update, releases the sidecar executable, installs, and restarts.
#[tauri::command]
pub(crate) async fn install_update(app: tauri::AppHandle) -> Result<(), String> {
    if !updatable() {
        return Err("updates can only be installed from an installed copy of Drift".into());
    }

    let updater = tauri_plugin_updater::UpdaterExt::updater(&app).map_err(|error| error.to_string())?;
    let update = updater
        .check()
        .await
        .map_err(|error| error.to_string())?
        .ok_or("no update available")?;

    let bytes = update
        .download(|_, _| {}, || {})
        .await
        .map_err(|error| error.to_string())?;
    if let Err(error) = update.install(bytes) {
        return Err(error.to_string());
    }

    app.restart();
}
