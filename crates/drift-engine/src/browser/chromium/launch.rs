//! Starting Chrome or Edge: which program, which profile folder, which switches, and the debugging address it opens.

use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use crate::browser::{Kind, Window};
use crate::platform::process::{Tree, spawn_owned};

/// How long a browser may take to open its debugging port.
const START_TIMEOUT: Duration = Duration::from_secs(30);
/// The viewport a page gets unless the agent asks for another.
pub(crate) const WIDTH: u32 = 1280;
pub(crate) const HEIGHT: u32 = 800;
/// Chrome's HTTP cache for a profile; its default grows to a share of the disk.
const DISK_CACHE_BYTES: u64 = 50 * 1024 * 1024;

/// Background work no test needs: model and component downloads, translation, casting, sync, and window
/// occlusion, which would stop an off-screen window from painting.
const DISABLED_FEATURES: &str = "Translate,OptimizationHints,OptimizationHintsFetching,OptimizationTargetPrediction,\
OptimizationGuideModelDownloading,OptimizationGuideOnDeviceModel,MediaRouter,CalculateNativeWinOcclusion,\
AutofillServerCommunication,InterestFeedContentSuggestions,PrivacySandboxSettings4";

/// How a browser starts, beyond its engine and profile.
pub(crate) struct Options<'a> {
    pub(crate) window: Window,
    /// Hosts Windows may sign in to with the user's own account, as an intranet expects.
    pub(crate) integrated_auth: &'a [String],
}

/// A started browser: its process, the job holding everything it started, and where to connect.
pub(crate) struct Launched {
    pub(crate) child: tokio::process::Child,
    pub(crate) tree: Tree,
    pub(crate) port: u16,
    pub(crate) url: String,
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum LaunchError {
    #[error("{0} could not start: {1}")]
    Spawn(&'static str, std::io::Error),
    #[error("its profile is already open in another {0} window")]
    ProfileInUse(&'static str),
    #[error("{0} did not open its debugging port within 30 s")]
    NoPort(&'static str),
}

/// The switches every browser gets: a private debugging port, a quiet profile, and nothing that marks it automated.
fn arguments(profile: &Path, options: &Options) -> Vec<String> {
    let mut arguments = vec![
        "--remote-debugging-port=0".to_string(),
        format!("--user-data-dir={}", profile.display()),
        "--no-first-run".into(),
        "--no-default-browser-check".into(),
        "--disable-blink-features=AutomationControlled".into(),
        format!("--disable-features={DISABLED_FEATURES}"),
        "--disable-component-update".into(),
        "--disable-sync".into(),
        "--disable-background-networking".into(),
        format!("--disk-cache-size={DISK_CACHE_BYTES}"),
        "--disable-backgrounding-occluded-windows".into(),
        "--disable-renderer-backgrounding".into(),
        "--disable-background-timer-throttling".into(),
        format!("--window-size={WIDTH},{HEIGHT}"),
    ];

    if !options.integrated_auth.is_empty() {
        let hosts = options.integrated_auth.join(",");
        arguments.push(format!("--auth-server-allowlist={hosts}"));
        arguments.push(format!("--auth-negotiate-delegate-allowlist={hosts}"));
    }
    match options.window {
        Window::Hidden => {
            arguments.push("--headless=new".into());
            arguments.push("--mute-audio".into());
        }
        Window::Offscreen => arguments.push("--window-position=-32000,-32000".into()),
    }
    if cfg!(target_os = "macos") {
        arguments.push("--use-mock-keychain".into());
    }
    if cfg!(target_os = "linux") {
        arguments.push("--password-store=basic".into());
    }
    arguments.push("about:blank".into());

    arguments
}

/// Starts `kind` on `profile` inside a job and waits for the debugging address it writes there.
pub(crate) async fn launch(
    kind: Kind,
    program: &Path,
    profile: &Path,
    options: &Options<'_>,
) -> Result<Launched, LaunchError> {
    let name = kind.label();
    std::fs::create_dir_all(profile).map_err(|error| LaunchError::Spawn(name, error))?;

    // A file left by an earlier run would name a port nothing listens on.
    let active_port = profile.join("DevToolsActivePort");
    let _ = std::fs::remove_file(&active_port);

    let mut command = tokio::process::Command::new(program);
    command
        .args(arguments(profile, options))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let (mut child, tree) = spawn_owned(&mut command)
        .await
        .map_err(|error| LaunchError::Spawn(name, error))?;

    let deadline = tokio::time::Instant::now() + START_TIMEOUT;
    loop {
        if let Some((port, path)) = read_active_port(&active_port) {
            let url = format!("ws://127.0.0.1:{port}{path}");
            return Ok(Launched { child, tree, port, url });
        }
        // A profile already open elsewhere makes the new process hand over to that one and exit.
        if let Ok(Some(_)) = child.try_wait() {
            return Err(LaunchError::ProfileInUse(name));
        }
        if tokio::time::Instant::now() >= deadline {
            tree.kill();
            return Err(LaunchError::NoPort(name));
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

/// The port and browser path Chrome writes once it listens, one per line.
fn read_active_port(path: &Path) -> Option<(u16, String)> {
    let text = std::fs::read_to_string(path).ok()?;
    let mut lines = text.lines();
    let port = lines.next()?.trim().parse().ok()?;
    let browser_path = lines.next()?.trim().to_string();

    browser_path
        .starts_with("/devtools/browser/")
        .then_some((port, browser_path))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_active_port_file_is_read_only_once_complete() {
        let dir = std::env::temp_dir().join(format!("drift-port-{}", crate::random_hex(4)));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("DevToolsActivePort");

        std::fs::write(&file, "9222\n").unwrap();
        assert_eq!(read_active_port(&file), None, "half written");

        std::fs::write(&file, "9222\n/devtools/browser/abc\n").unwrap();
        assert_eq!(read_active_port(&file), Some((9222, "/devtools/browser/abc".into())));

        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn nothing_marks_the_browser_as_automated() {
        let options = |window| Options {
            window,
            integrated_auth: &[],
        };
        let hidden = arguments(Path::new("profile"), &options(Window::Hidden));

        assert!(!hidden.iter().any(|argument| argument.contains("enable-automation")));
        assert!(hidden.contains(&"--disable-blink-features=AutomationControlled".to_string()));
        assert!(hidden.contains(&"--headless=new".to_string()));
        assert!(
            !arguments(Path::new("p"), &options(Window::Offscreen))
                .iter()
                .any(|a| a.starts_with("--headless"))
        );
    }
}
