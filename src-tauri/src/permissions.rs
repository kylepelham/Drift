use crate::store::Store;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use tauri::{Manager, State};

#[derive(Clone, Default)]
pub(crate) struct DictationConsent(Arc<AtomicBool>);

impl DictationConsent {
    pub(crate) fn enabled(&self) -> bool {
        self.0.load(Ordering::Acquire)
    }

    pub(crate) fn set(&self, enabled: bool) {
        self.0.store(enabled, Ordering::Release);
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum WebPermissionKind {
    Microphone,
    Other,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum PermissionDecision {
    Allow,
    Default,
}

pub(crate) fn permission_decision(
    uri: &str,
    kind: WebPermissionKind,
    consent: bool,
    app_origins: &[&str],
) -> PermissionDecision {
    let Ok(request) = url::Url::parse(uri) else {
        return PermissionDecision::Default;
    };
    let own_origin = app_origins.iter().any(|origin| {
        url::Url::parse(origin)
            .map(|app| request.origin() == app.origin())
            .unwrap_or(false)
    });
    if own_origin && kind == WebPermissionKind::Microphone && consent {
        PermissionDecision::Allow
    } else {
        PermissionDecision::Default
    }
}

#[tauri::command]
pub(crate) fn voice_dictation_set_enabled(
    consent: State<DictationConsent>,
    store: State<Store>,
    enabled: bool,
) -> Result<(), String> {
    if enabled {
        store
            .save_dictation_enabled(true)
            .map_err(|error| error.to_string())?;
        consent.set(true);
    } else {
        consent.set(false);
        store
            .save_dictation_enabled(false)
            .map_err(|error| error.to_string())?;
    }
    Ok(())
}

#[cfg(windows)]
pub(crate) fn install(app: &tauri::App) -> tauri::Result<()> {
    let consent = app.state::<DictationConsent>().inner().clone();
    for window in app.webview_windows().values() {
        let consent = consent.clone();
        window.with_webview(move |platform| {
            let result = attach_webview2_handler(platform, consent);
            if let Err(error) = result {
                eprintln!("failed to install WebView2 microphone permission handler: {error}");
            }
        })?;
    }
    Ok(())
}

#[cfg(windows)]
fn attach_webview2_handler(
    platform: tauri::webview::PlatformWebview,
    consent: DictationConsent,
) -> windows::core::Result<()> {
    use webview2_com::{
        take_pwstr,
        Microsoft::Web::WebView2::Win32::{
            COREWEBVIEW2_PERMISSION_KIND, COREWEBVIEW2_PERMISSION_KIND_MICROPHONE,
            COREWEBVIEW2_PERMISSION_STATE_ALLOW,
        },
        PermissionRequestedEventHandler,
    };
    use windows::core::PWSTR;

    unsafe {
        let webview = platform.controller().CoreWebView2()?;
        let handler = PermissionRequestedEventHandler::create(Box::new(move |_, args| {
            let Some(args) = args else { return Ok(()) };
            let mut kind = COREWEBVIEW2_PERMISSION_KIND::default();
            args.PermissionKind(&mut kind)?;
            let mut uri = PWSTR::null();
            args.Uri(&mut uri)?;
            let uri = take_pwstr(uri);
            let kind = if kind == COREWEBVIEW2_PERMISSION_KIND_MICROPHONE {
                WebPermissionKind::Microphone
            } else {
                WebPermissionKind::Other
            };
            if permission_decision(&uri, kind, consent.enabled(), app_origins())
                == PermissionDecision::Allow
            {
                args.SetState(COREWEBVIEW2_PERMISSION_STATE_ALLOW)?;
            }
            Ok(())
        }));
        let mut token = 0;
        webview.add_PermissionRequested(&handler, &mut token)
    }
}

#[cfg(windows)]
fn app_origins() -> &'static [&'static str] {
    #[cfg(debug_assertions)]
    {
        &["http://tauri.localhost", "http://localhost:5180"]
    }
    #[cfg(not(debug_assertions))]
    {
        &["http://tauri.localhost"]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn permission_matrix_only_allows_own_origin_microphone_with_consent() {
        let own = ["http://tauri.localhost"];
        assert_eq!(
            permission_decision(
                "http://tauri.localhost/settings",
                WebPermissionKind::Microphone,
                true,
                &own
            ),
            PermissionDecision::Allow
        );
        for (uri, kind, consent) in [
            (
                "http://tauri.localhost",
                WebPermissionKind::Microphone,
                false,
            ),
            ("https://example.com", WebPermissionKind::Microphone, true),
            (
                "http://tauri.localhost.evil.test",
                WebPermissionKind::Microphone,
                true,
            ),
            (
                "http://tauri.localhost:5180",
                WebPermissionKind::Microphone,
                true,
            ),
            ("http://tauri.localhost", WebPermissionKind::Other, true),
            ("not a url", WebPermissionKind::Microphone, true),
        ] {
            assert_eq!(
                permission_decision(uri, kind, consent, &own),
                PermissionDecision::Default
            );
        }
    }
}
