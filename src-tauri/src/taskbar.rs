//! Whether the Windows taskbar is light or dark, which the app's own theme does not say: the window icon is
//! painted in the accent shade that stands out on it.

use tauri::{AppHandle, Emitter};

/// True on a light taskbar, false on a dark one, `None` where there is no setting to read.
#[tauri::command]
pub(crate) fn taskbar_light() -> Option<bool> {
    platform::light()
}

/// Tells the webview each time the taskbar turns light or dark.
pub(crate) fn watch(app: &AppHandle) {
    let app = app.clone();
    std::thread::spawn(move || {
        platform::on_change(|light| {
            let _ = app.emit("taskbar-theme-changed", light);
        });
    });
}

#[cfg(windows)]
mod platform {
    use std::ptr::null_mut;

    use windows_sys::Win32::Foundation::ERROR_SUCCESS;
    use windows_sys::Win32::System::Registry::{
        HKEY, HKEY_CURRENT_USER, KEY_NOTIFY, REG_NOTIFY_CHANGE_LAST_SET, RRF_RT_REG_DWORD, RegCloseKey, RegGetValueW,
        RegNotifyChangeKeyValue, RegOpenKeyExW,
    };
    use windows_sys::w;

    const PERSONALIZE: *const u16 = w!("Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize");

    /// `SystemUsesLightTheme` colours the taskbar; `AppsUseLightTheme`, which the webview follows, can differ.
    pub(super) fn light() -> Option<bool> {
        let mut value = 0u32;
        let mut size = size_of::<u32>() as u32;
        // SAFETY: the names are null-terminated literals, and the buffer is the DWORD `size` describes.
        let status = unsafe {
            RegGetValueW(
                HKEY_CURRENT_USER,
                PERSONALIZE,
                w!("SystemUsesLightTheme"),
                RRF_RT_REG_DWORD,
                null_mut(),
                (&raw mut value).cast(),
                &mut size,
            )
        };

        (status == ERROR_SUCCESS).then_some(value != 0)
    }

    /// Blocks the calling thread, calling `changed` whenever the taskbar's theme differs from before.
    pub(super) fn on_change(mut changed: impl FnMut(Option<bool>)) {
        let mut key: HKEY = null_mut();
        // SAFETY: the subkey is a null-terminated literal and `key` receives the opened handle.
        if unsafe { RegOpenKeyExW(HKEY_CURRENT_USER, PERSONALIZE, 0, KEY_NOTIFY, &mut key) } != ERROR_SUCCESS {
            return;
        }

        let mut shown = light();
        // SAFETY: `key` stays open for the loop; a synchronous wait blocks only this thread until a value changes.
        while unsafe { RegNotifyChangeKeyValue(key, 0, REG_NOTIFY_CHANGE_LAST_SET, null_mut(), 0) } == ERROR_SUCCESS {
            let now = light();
            if now != shown {
                shown = now;
                changed(now);
            }
        }

        // SAFETY: `key` was opened above and is not used after this.
        unsafe { RegCloseKey(key) };
    }
}

/// Elsewhere the icon keeps the app theme's accent.
#[cfg(not(windows))]
mod platform {
    pub(super) fn light() -> Option<bool> {
        None
    }

    pub(super) fn on_change(_changed: impl FnMut(Option<bool>)) {}
}
