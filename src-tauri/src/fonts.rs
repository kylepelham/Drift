//! The fonts installed on this machine, for Settings' font pickers.

use serde::Serialize;

/// One installed family, and whether its regular face is monospaced (a code font).
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub(crate) struct SystemFont {
    pub family: String,
    pub monospace: bool,
}

/// Every installed font family by its English name, sorted, each once.
#[tauri::command]
pub(crate) async fn system_fonts() -> Result<Vec<SystemFont>, String> {
    tauri::async_runtime::spawn_blocking(installed)
        .await
        .map_err(|error| error.to_string())?
}

fn installed() -> Result<Vec<SystemFont>, String> {
    let mut fonts = platform::families()?;
    fonts.sort();
    fonts.dedup_by(|a, b| a.family.eq_ignore_ascii_case(&b.family));

    Ok(fonts)
}

#[cfg(windows)]
mod platform {
    use windows::Win32::Graphics::DirectWrite::{
        DWRITE_FACTORY_TYPE_SHARED, DWriteCreateFactory, IDWriteFactory, IDWriteFont1, IDWriteFontCollection,
        IDWriteFontFamily, IDWriteLocalizedStrings,
    };
    use windows::core::{BOOL, Interface, w};

    use super::SystemFont;

    /// The system font collection through DirectWrite, as the webview itself finds fonts.
    pub(super) fn families() -> Result<Vec<SystemFont>, String> {
        let error = |error: windows::core::Error| error.message();
        // SAFETY: DirectWrite calls on interfaces it returned, each checked before use.
        unsafe {
            let factory: IDWriteFactory = DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED).map_err(error)?;
            let mut collection: Option<IDWriteFontCollection> = None;
            factory.GetSystemFontCollection(&mut collection, false).map_err(error)?;
            let Some(collection) = collection else {
                return Ok(Vec::new());
            };

            let fonts = (0..collection.GetFontFamilyCount())
                .filter_map(|index| collection.GetFontFamily(index).ok())
                .filter_map(|family| font(&family))
                .collect();
            Ok(fonts)
        }
    }

    /// SAFETY: `family` is a live DirectWrite family.
    unsafe fn font(family: &IDWriteFontFamily) -> Option<SystemFont> {
        // SAFETY: the caller's family is live, and each face it returns is checked before use.
        unsafe {
            let names = family.GetFamilyNames().ok()?;
            let name = english_name(&names)?;
            // A family whose first face cannot say is taken for a proportional one.
            let monospace = family
                .GetFont(0)
                .ok()
                .and_then(|face| face.cast::<IDWriteFont1>().ok())
                .is_some_and(|face| face.IsMonospacedFont().as_bool());

            Some(SystemFont {
                family: name,
                monospace,
            })
        }
    }

    /// SAFETY: `names` is a live DirectWrite string list.
    unsafe fn english_name(names: &IDWriteLocalizedStrings) -> Option<String> {
        // SAFETY: the buffer holds the length DirectWrite reported plus its terminator.
        unsafe {
            let mut index = 0;
            let mut exists = BOOL(0);
            names.FindLocaleName(w!("en-us"), &mut index, &mut exists).ok()?;
            if !exists.as_bool() {
                index = 0;
            }
            let length = names.GetStringLength(index).ok()?;
            let mut buffer = vec![0u16; length as usize + 1];
            names.GetString(index, &mut buffer).ok()?;

            let name = String::from_utf16_lossy(&buffer[..length as usize]);
            (!name.trim().is_empty() && !name.starts_with('@')).then_some(name)
        }
    }
}

/// Elsewhere the pickers fall back to typing a font's name.
#[cfg(not(windows))]
mod platform {
    pub(super) fn families() -> Result<Vec<super::SystemFont>, String> {
        Ok(Vec::new())
    }
}

#[cfg(all(test, windows))]
mod tests {
    #[test]
    fn the_installed_fonts_include_the_windows_ui_and_code_fonts() {
        let fonts = super::installed().unwrap();
        let find = |family: &str| fonts.iter().find(|font| font.family == family);

        assert!(find("Segoe UI").is_some_and(|font| !font.monospace), "{fonts:?}");
        assert!(find("Consolas").is_some_and(|font| font.monospace));
        let mut sorted = fonts.clone();
        sorted.sort();
        assert_eq!(fonts, sorted);
    }
}
