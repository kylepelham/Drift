//! Lighthouse audits of a page in the conversation's Chrome or Edge. Lighthouse is a Node package, so Drift installs it
//! with npm into its engines folder the first time an audit runs, and runs `lighthouse.mjs` against the browser's port.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use serde::Deserialize;
use serde_json::{Value, json};
use tokio::io::AsyncReadExt as _;
use tokio_util::sync::CancellationToken;

use crate::Engine;
use crate::platform::process::{spawn_owned, which};

const RUNNER: &str = include_str!("lighthouse.mjs");
const INSTALL_WAIT: Duration = Duration::from_secs(300);
const AUDIT_WAIT: Duration = Duration::from_secs(180);
/// Lighthouse 13 needs Node 22.19; older Node gets 12, which audits the same categories but agentic browsing.
const NEWEST_NODE: (u32, u32) = (22, 19);

/// One audit, as the runner script takes it.
pub(crate) struct Audit<'a> {
    pub(crate) port: u16,
    pub(crate) target: &'a str,
    pub(crate) snapshot: bool,
    pub(crate) mobile: bool,
    /// The browser's own desktop user agent, so a hidden browser does not report itself as headless.
    pub(crate) user_agent: Option<&'a str>,
    pub(crate) out: &'a Path,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Report {
    pub(crate) url: String,
    pub(crate) version: String,
    pub(crate) runtime_error: Option<String>,
    #[serde(default)]
    pub(crate) warnings: Vec<String>,
    pub(crate) scores: Vec<Score>,
    pub(crate) passed: usize,
    pub(crate) failed: Vec<Failure>,
    pub(crate) reports: Vec<String>,
}

#[derive(Deserialize)]
pub(crate) struct Score {
    pub(crate) title: String,
    pub(crate) score: Option<f64>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Failure {
    pub(crate) title: String,
    pub(crate) score: f64,
    pub(crate) value: Option<String>,
    #[serde(default)]
    pub(crate) categories: Vec<String>,
    #[serde(default)]
    pub(crate) nodes: Vec<Node>,
    pub(crate) total_nodes: usize,
}

#[derive(Deserialize)]
pub(crate) struct Node {
    pub(crate) selector: Option<String>,
    pub(crate) snippet: Option<String>,
    pub(crate) label: Option<String>,
    pub(crate) explanation: Option<String>,
}

/// Where Lighthouse lives once installed, and which major version it is.
struct Install {
    folder: PathBuf,
    node: PathBuf,
    major: u32,
}

static INSTALLING: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

impl Engine {
    /// Runs one audit; `stop` cancels it. Installs Lighthouse first if this is the first audit.
    pub(crate) async fn lighthouse(&self, audit: &Audit<'_>, stop: &CancellationToken) -> Result<Report, String> {
        let install = self.lighthouse_install(stop).await?;
        let mut categories = vec!["accessibility", "best-practices", "seo"];
        if install.major >= 13 {
            categories.push("agentic-browsing");
        }
        let options = json!({
            "port": audit.port,
            "target": audit.target,
            "mode": if audit.snapshot { "snapshot" } else { "navigation" },
            "device": if audit.mobile { "mobile" } else { "desktop" },
            "userAgent": audit.user_agent,
            "categories": categories,
            "out": audit.out,
        });

        let mut command = tokio::process::Command::new(&install.node);
        command
            .arg(install.folder.join("lighthouse.mjs"))
            .arg(options.to_string())
            .current_dir(&install.folder);
        let output = run(&mut command, AUDIT_WAIT, stop).await?;
        let line = output
            .lines()
            .rev()
            .find(|line| line.starts_with('{'))
            .ok_or_else(|| format!("Lighthouse failed: {}", last_lines(&output)))?;

        serde_json::from_str(line).map_err(|error| format!("Lighthouse's result could not be read: {error}"))
    }

    async fn lighthouse_install(&self, stop: &CancellationToken) -> Result<Install, String> {
        let _installing = INSTALLING.lock().await;
        let node = which("node").ok_or("Lighthouse audits need Node.js on PATH; install it from nodejs.org")?;
        let major = if node_version(&node).await.is_some_and(|version| version >= NEWEST_NODE) {
            13
        } else {
            12
        };
        let folder = self.data_dir.join("browsers/engines/lighthouse");
        std::fs::create_dir_all(&folder).map_err(|error| error.to_string())?;
        write_if_changed(&folder.join("lighthouse.mjs"), RUNNER)?;

        if installed_major(&folder) != Some(major) {
            write_if_changed(&folder.join("package.json"), r#"{ "private": true, "type": "module" }"#)?;
            let npm = which("npm").ok_or("Lighthouse audits need npm, which comes with Node.js")?;
            let mut command = tokio::process::Command::new(npm);
            command
                .args(["install", &format!("lighthouse@{major}"), "--no-audit", "--no-fund"])
                .args(["--omit=optional", "--loglevel=error"])
                .current_dir(&folder);
            run(&mut command, INSTALL_WAIT, stop)
                .await
                .map_err(|error| format!("Lighthouse could not be installed with npm: {error}"))?;
        }

        Ok(Install { folder, node, major })
    }
}

/// Runs a command in its own job until it exits, `limit` passes or `stop` fires; its output on success.
async fn run(
    command: &mut tokio::process::Command,
    limit: Duration,
    stop: &CancellationToken,
) -> Result<String, String> {
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    crate::platform::process::use_current_path(command, &Default::default());
    let (mut child, tree) = spawn_owned(command).await.map_err(|error| error.to_string())?;
    let mut stdout = child.stdout.take().ok_or("no output")?;
    let mut stderr = child.stderr.take().ok_or("no output")?;

    let finished = async {
        let mut out = String::new();
        let mut err = String::new();
        let (_, _, status) = tokio::join!(
            stdout.read_to_string(&mut out),
            stderr.read_to_string(&mut err),
            child.wait()
        );
        (status, out, err)
    };
    let (status, out, err) = tokio::select! {
        finished = tokio::time::timeout(limit, finished) => finished.map_err(|_| {
            tree.kill();
            format!("it did not finish within {} s", limit.as_secs())
        })?,
        () = stop.cancelled() => {
            tree.kill();
            return Err("it was stopped".into());
        }
    };

    match status {
        Ok(status) if status.success() => Ok(out),
        _ => Err(last_lines(&format!("{out}\n{err}"))),
    }
}

async fn node_version(node: &Path) -> Option<(u32, u32)> {
    let output = tokio::process::Command::new(node)
        .arg("--version")
        .output()
        .await
        .ok()?;
    let text = String::from_utf8_lossy(&output.stdout);
    let mut parts = text.trim().trim_start_matches('v').split('.');
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next()?.parse().ok()?;

    Some((major, minor))
}

fn installed_major(folder: &Path) -> Option<u32> {
    let package = std::fs::read_to_string(folder.join("node_modules/lighthouse/package.json")).ok()?;
    let version: Value = serde_json::from_str(&package).ok()?;

    version["version"].as_str()?.split('.').next()?.parse().ok()
}

fn write_if_changed(path: &Path, content: &str) -> Result<(), String> {
    if std::fs::read_to_string(path).is_ok_and(|current| current == content) {
        return Ok(());
    }

    std::fs::write(path, content).map_err(|error| error.to_string())
}

/// The end of a failed command's output, where its error is.
fn last_lines(output: &str) -> String {
    let lines: Vec<&str> = output.lines().filter(|line| !line.trim().is_empty()).collect();
    let start = lines.len().saturating_sub(6);

    lines[start..].join("\n")
}
