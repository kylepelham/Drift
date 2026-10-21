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

/// Removes the folders earlier updates downloaded their installers into, which the plugin leaves in the
/// temporary directory; a running Drift means their install has finished.
pub(crate) fn clear_finished_installers() {
    clear_installers_in(&std::env::temp_dir());
}

fn clear_installers_in(temp: &Path) {
    for entry in std::fs::read_dir(temp).into_iter().flatten().flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.starts_with("Drift-") && name.contains("-updater-") {
            let _ = std::fs::remove_dir_all(entry.path());
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn only_the_updaters_own_installer_folders_are_cleared() {
        let temp = drift_engine::platform::temp::test_run().join(format!("updater-{}", std::process::id()));
        for name in ["Drift-2.2.1-updater-9DLfhx", "Drift", "Other-1.0-updater-x"] {
            std::fs::create_dir_all(temp.join(name)).unwrap();
        }

        super::clear_installers_in(&temp);

        assert!(!temp.join("Drift-2.2.1-updater-9DLfhx").exists());
        assert!(temp.join("Drift").exists() && temp.join("Other-1.0-updater-x").exists());
    }
}
