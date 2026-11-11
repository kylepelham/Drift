//! Driving the Drift on another computer: finding computers nearby, asking one to accept this device, the computers
//! this device is linked with, and a proxy on 127.0.0.1 that a WebView treats as that computer's gateway, pinning it
//! so no certificate warning can ever appear. The Android app and the desktop share it (`docs/android.md`,
//! `docs/remote.md`).

pub mod computers;
pub mod nearby;
pub mod pc;
pub mod proxy;
pub mod tls;

use std::borrow::Cow;
use std::net::Ipv4Addr;

/// What the app running the proxy provides: the UI it serves, how computers list this device, where to look for
/// computers nearby, and its own Drift if it has one to go back to.
pub trait Host: Send + Sync + 'static {
    /// A file of Drift's built UI, by its path inside `dist/`.
    fn asset(&self, path: &str) -> Option<Cow<'static, [u8]>>;

    /// How a computer lists this device once linked. May block, so it is called off the async threads.
    fn device_name(&self) -> String;

    /// The broadcast address of each network this device is on, besides the one the system broadcasts to.
    fn broadcasts(&self) -> Vec<Ipv4Addr> {
        Vec::new()
    }

    /// The name of this device's own Drift, listed first on the computers page, when it has one: a desktop does,
    /// a phone does not.
    fn this_computer(&self) -> Option<String> {
        None
    }

    /// The id of this device's own Drift, its certificate authority as hex, so it never asks itself to connect.
    fn this_computer_id(&self) -> Option<String> {
        None
    }

    /// Shows this device's own Drift in place of the computer the proxy serves.
    fn show_this_computer(&self) -> Result<(), String> {
        Err("this device has no Drift of its own".into())
    }
}

/// Where tests write: Drift's own folder in the system's temporary directory, never loose beside it.
#[cfg(test)]
fn test_dir() -> std::path::PathBuf {
    let dir = std::env::temp_dir().join("Drift").join("tests").join("remote-client");
    std::fs::create_dir_all(&dir).unwrap();
    dir
}
