//! Update check and install, delegated to the Tauri updater plugin.

/// Returns the version of an available update, or `None` if the app is current.
///
/// Debug builds never check: a local build's version usually trails the latest release, so it
/// would otherwise be offered an "update" that replaces the build under development. In release
/// builds the plugin compares semver against the manifest and only offers strictly newer versions.
#[tauri::command]
pub(crate) async fn check_update(app: tauri::AppHandle) -> Result<Option<String>, String> {
    if cfg!(debug_assertions) {
        return Ok(None);
    }
    let updater = tauri_plugin_updater::UpdaterExt::updater(&app).map_err(|e| e.to_string())?;
    let update = updater.check().await.map_err(|e| e.to_string())?;
    Ok(update.map(|u| u.version))
}

/// Downloads and installs the pending update, then restarts.
///
/// Never returns on success: `app.restart()` diverges, which is why there is no trailing `Ok(())`.
#[tauri::command]
pub(crate) async fn install_update(app: tauri::AppHandle) -> Result<(), String> {
    let updater = tauri_plugin_updater::UpdaterExt::updater(&app).map_err(|e| e.to_string())?;
    let update = updater
        .check()
        .await
        .map_err(|e| e.to_string())?
        .ok_or("no update available")?;
    update
        .download_and_install(|_, _| {}, || {})
        .await
        .map_err(|e| e.to_string())?;
    app.restart();
}
