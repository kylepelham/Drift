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
        let real_dir = entry.file_type().is_ok_and(|kind| kind.is_dir());
        if real_dir && installer_dir_name(&entry.file_name().to_string_lossy()) && holds_only_installers(&entry.path())
        {
            let _ = std::fs::remove_dir_all(entry.path());
        }
    }
}

/// The plugin's own `Drift-{version}-updater-{six random alphanumerics}`, and nothing looser.
fn installer_dir_name(name: &str) -> bool {
    let Some((version, random)) = name
        .strip_prefix("Drift-")
        .and_then(|rest| rest.rsplit_once("-updater-"))
    else {
        return false;
    };
    let numbered = version.split('.').count() >= 3 && version.starts_with(|c: char| c.is_ascii_digit());
    let version_chars = version
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '+'));
    numbered && version_chars && random.len() == 6 && random.chars().all(|c| c.is_ascii_alphanumeric())
}

/// A folder the updater filled holds installer files and nothing else.
fn holds_only_installers(dir: &Path) -> bool {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return false;
    };
    entries.into_iter().all(|entry| {
        entry.is_ok_and(|entry| {
            let file = entry.file_type().is_ok_and(|kind| kind.is_file());
            let extension = Path::new(&entry.file_name())
                .extension()
                .map(std::ffi::OsStr::to_ascii_lowercase);
            file && extension.is_some_and(|e| e == "exe" || e == "msi")
        })
    })
}

#[cfg(test)]
mod tests {
    #[test]
    fn only_the_updaters_own_installer_folders_are_cleared() {
        let temp = drift_engine::platform::temp::test_run().join(format!("updater-{}", std::process::id()));
        let kept = [
            "Drift",
            "Other-1.0.0-updater-abcdef",
            "Drift-x-updater-abcdef",
            "Drift-2.2.1-updater-mine",
        ];
        for name in ["Drift-2.2.1-updater-9DLfhx", "Drift-2.2.1-updater-Notes1"]
            .iter()
            .chain(&kept)
        {
            std::fs::create_dir_all(temp.join(name)).unwrap();
            std::fs::write(temp.join(name).join("Drift_2.2.1_x64-setup.exe"), "installer").unwrap();
        }
        std::fs::write(temp.join("Drift-2.2.1-updater-Notes1").join("notes.txt"), "someone's").unwrap();

        super::clear_installers_in(&temp);

        assert!(!temp.join("Drift-2.2.1-updater-9DLfhx").exists());
        assert!(temp.join("Drift-2.2.1-updater-Notes1").join("notes.txt").exists());
        for name in kept {
            assert!(temp.join(name).exists(), "{name} was removed");
        }
    }
}
