//! Edge for IE mode beside Chromium: started with no debugging port, which would turn IE mode off, and driven
//! through Drift's extension, which relays the DevTools Protocol over `chrome.debugger` to a loopback socket.

use std::path::Path;
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::mpsc;

use super::cdp::{Connection, Event};
use super::launch::{DISABLED_FEATURES, HEIGHT, WIDTH};
use crate::platform::process::{Tree, spawn_owned};

const MANIFEST: &str = include_str!("relay/manifest.json");
const BACKGROUND: &str = include_str!("relay/background.js");
/// Where the extension's address goes in its script.
const ADDRESS_MARK: &str = "__DRIFT_RELAY__";
/// How long Edge may take to start and load the extension.
const CONNECT_WAIT: Duration = Duration::from_secs(30);

/// A started Edge: its process, the job holding what it started, and the extension's connection.
pub(crate) struct Relayed {
    pub(crate) child: tokio::process::Child,
    pub(crate) tree: Tree,
    pub(crate) pid: u32,
    pub(crate) cdp: Arc<Connection>,
    pub(crate) events: mpsc::UnboundedReceiver<Event>,
}

/// The switches IE mode with a site list needs; `docs/browser.md` says why each matters.
fn arguments(profile: &Path, extension: &Path, integrated_auth: &[String]) -> Vec<String> {
    let mut arguments = vec![
        // The process started must be the browser itself, whose windows IEDriverServer looks for.
        "--edge-skip-compat-layer-relaunch".to_string(),
        format!("--user-data-dir={}", profile.display()),
        format!("--load-extension={}", extension.display()),
        "--silent-debugger-extension-api".into(),
        "--no-first-run".into(),
        "--no-default-browser-check".into(),
        "--no-service-autorun".into(),
        "--disable-sync".into(),
        format!("--disable-features={DISABLED_FEATURES},msImplicitSignin,DisableLoadExtensionCommandLineSwitch"),
        "--disable-popup-blocking".into(),
        "--disable-backgrounding-occluded-windows".into(),
        "--disable-renderer-backgrounding".into(),
        "--disable-background-timer-throttling".into(),
        "--window-position=-32000,-32000".into(),
        format!("--window-size={WIDTH},{HEIGHT}"),
    ];

    if !integrated_auth.is_empty() {
        let hosts = integrated_auth.join(",");
        arguments.push(format!("--auth-server-allowlist={hosts}"));
        arguments.push(format!("--auth-negotiate-delegate-allowlist={hosts}"));
    }
    arguments.push("about:blank".into());

    arguments
}

/// Writes the extension into the profile, starts Edge on it, and waits for the extension to connect.
pub(crate) async fn launch(program: &Path, profile: &Path, integrated_auth: &[String]) -> Result<Relayed, String> {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .map_err(|error| error.to_string())?;
    let port = listener.local_addr().map_err(|error| error.to_string())?.port();
    let path = format!("/{}", crate::random_hex(16));

    let extension = profile.join("drift-relay");
    let address = format!("ws://127.0.0.1:{port}{path}");
    let written = std::fs::create_dir_all(&extension)
        .and_then(|()| std::fs::write(extension.join("manifest.json"), MANIFEST))
        .and_then(|()| {
            std::fs::write(
                extension.join("background.js"),
                BACKGROUND.replace(ADDRESS_MARK, &address),
            )
        });
    written.map_err(|error| format!("could not write Drift's Edge extension: {error}"))?;

    let mut command = tokio::process::Command::new(program);
    command
        .args(arguments(profile, &extension, integrated_auth))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let (child, tree) = spawn_owned(&mut command)
        .await
        .map_err(|error| format!("Edge could not start: {error}"))?;
    let pid = child.id().unwrap_or_default();

    let accepted = tokio::time::timeout(CONNECT_WAIT, Connection::accept(listener, &path)).await;
    let Ok(Ok((cdp, events))) = accepted else {
        tree.kill();
        return Err("Edge did not load Drift's extension within 30 s".into());
    };

    Ok(Relayed {
        child,
        tree,
        pid,
        cdp,
        events,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn edge_starts_without_a_debugging_port_and_with_the_extension() {
        let arguments = arguments(Path::new("profile"), Path::new("profile/drift-relay"), &[]);

        assert!(
            !arguments
                .iter()
                .any(|argument| argument.starts_with("--remote-debugging"))
        );
        assert!(arguments.contains(&"--edge-skip-compat-layer-relaunch".to_string()));
        assert!(
            arguments
                .iter()
                .any(|argument| argument.starts_with("--load-extension="))
        );
        assert!(BACKGROUND.contains(ADDRESS_MARK));
    }
}
