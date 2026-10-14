//! What only Android can do, in `DriftPlugin.kt`: scan a QR code, and name this phone.

use tauri::Manager;

/// The Kotlin plugin on Android; elsewhere (the tests' host) there is no camera and the phone has no name.
pub(crate) struct Native {
    #[cfg(target_os = "android")]
    handle: tauri::plugin::PluginHandle<tauri::Wry>,
}

#[cfg(target_os = "android")]
impl Native {
    /// Opens the camera on a QR code; `None` when the user backs out.
    pub(crate) fn scan(&self) -> Result<Option<String>, String> {
        #[derive(serde::Deserialize)]
        struct Scanned {
            text: Option<String>,
        }

        self.handle
            .run_mobile_plugin::<Scanned>("scan", ())
            .map(|scanned| scanned.text)
            .map_err(|error| error.to_string())
    }

    /// The name the user gave this phone in its settings, else its model; how the PC lists it.
    pub(crate) fn device_name(&self) -> String {
        #[derive(serde::Deserialize)]
        struct Named {
            name: String,
        }

        self.handle
            .run_mobile_plugin::<Named>("deviceName", ())
            .map_or_else(|_| "Android".into(), |named| named.name)
    }
}

#[cfg(not(target_os = "android"))]
impl Native {
    pub(crate) fn scan(&self) -> Result<Option<String>, String> {
        Err("scanning needs the camera of an Android phone".into())
    }

    pub(crate) fn device_name(&self) -> String {
        "Android".into()
    }
}

/// Registers the Kotlin plugin and keeps its handle as an `Arc<Native>`.
pub(crate) fn plugin() -> tauri::plugin::TauriPlugin<tauri::Wry> {
    tauri::plugin::Builder::new("drift")
        .setup(|app, api| {
            #[cfg(target_os = "android")]
            let native = Native {
                handle: api.register_android_plugin("dev.drift.remote", "DriftPlugin")?,
            };
            #[cfg(not(target_os = "android"))]
            let native = {
                let _ = api;
                Native {}
            };

            app.manage(std::sync::Arc::new(native));
            Ok(())
        })
        .build()
}
