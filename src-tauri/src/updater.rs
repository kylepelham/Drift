//! Update check and install, delegated to the Tauri updater plugin.

// Update checks compare semver against the release manifest; the plugin only offers
// strictly newer versions, so dev builds ahead of the latest release stay put.
#[tauri::command]
pub(crate) async fn check_update(app: tauri::AppHandle) -> Result<Option<String>, String> {
    if cfg!(debug_assertions) {
        return Ok(None);
    }
    let updater = tauri_plugin_updater::UpdaterExt::updater(&app).map_err(|e| e.to_string())?;
    let update = updater.check().await.map_err(|e| e.to_string())?;
    Ok(update.map(|u| u.version))
}

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
