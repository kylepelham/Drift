//! Where each engine's program is, and getting the ones Drift can fetch itself: IEDriverServer, small, as soon as
//! IE mode needs it; Camoufox, large, when the user asks in Settings.

use std::collections::BTreeMap;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use super::Kind;
use crate::Engine;
use crate::platform::process::which;

/// Settings: a program path per engine the user set by hand; for IE mode, IEDriverServer's.
pub const PATHS_KEY: &str = "browserPaths";

const CAMOUFOX_RELEASES: &str = "https://api.github.com/repos/daijro/camoufox/releases?per_page=20";
const SELENIUM_RELEASES: &str = "https://api.github.com/repos/SeleniumHQ/selenium/releases?per_page=100";

/// Whether an engine can run here, and what it would take if not.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct EngineStatus {
    pub engine: Kind,
    pub available: bool,
    /// The program it runs, or for IE mode, IEDriverServer.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// Set by the user in Settings rather than found.
    pub custom: bool,
    /// What is missing, when it is not available.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub missing: Option<String>,
    /// Drift can download it.
    pub downloadable: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub download: Option<Download>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Download {
    pub done: u64,
    pub total: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Downloads in flight or failed, by engine.
#[derive(Default)]
pub struct Downloads(Mutex<BTreeMap<Kind, Download>>);

/// The program for a Chromium or Firefox engine where an installer puts it.
pub(crate) fn executable(kind: Kind) -> Option<PathBuf> {
    let env = |name: &str| std::env::var_os(name).map(PathBuf::from);
    let (relative, app, names): (&str, &str, &[&str]) = match kind {
        Kind::Chrome => (
            r"Google\Chrome\Application\chrome.exe",
            "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
            &["chrome", "google-chrome", "google-chrome-stable"],
        ),
        Kind::Edge | Kind::Ie => (
            r"Microsoft\Edge\Application\msedge.exe",
            "/Applications/Microsoft Edge.app/Contents/MacOS/Microsoft Edge",
            &["msedge", "microsoft-edge", "microsoft-edge-stable"],
        ),
        Kind::Firefox => (
            r"Mozilla Firefox\firefox.exe",
            "/Applications/Firefox.app/Contents/MacOS/firefox",
            &["firefox"],
        ),
        Kind::Camoufox => return None,
    };

    let mut candidates = Vec::new();
    if cfg!(windows) {
        let roots = [env("ProgramFiles"), env("ProgramFiles(x86)"), env("LOCALAPPDATA")];
        candidates.extend(roots.into_iter().flatten().map(|root| root.join(relative)));
    }
    if cfg!(target_os = "macos") {
        candidates.push(PathBuf::from(app));
    }

    candidates
        .into_iter()
        .find(|path| path.is_file())
        .or_else(|| names.iter().find_map(|name| which(name)))
}

fn engines_dir(data_dir: &Path) -> PathBuf {
    data_dir.join("browsers").join("engines")
}

fn camoufox_program(data_dir: &Path) -> PathBuf {
    let name = if cfg!(windows) { "camoufox.exe" } else { "camoufox" };
    engines_dir(data_dir).join("camoufox").join(name)
}

fn ie_driver_downloaded(data_dir: &Path) -> PathBuf {
    engines_dir(data_dir).join("IEDriverServer.exe")
}

impl Engine {
    pub fn browser_paths(&self) -> BTreeMap<Kind, String> {
        self.store.setting(PATHS_KEY).ok().flatten().unwrap_or_default()
    }

    pub fn set_browser_path(&self, kind: Kind, path: Option<&str>) -> rusqlite::Result<()> {
        let mut paths = self.browser_paths();
        match path.map(str::trim).filter(|path| !path.is_empty()) {
            Some(path) => paths.insert(kind, path.to_string()),
            None => paths.remove(&kind),
        };

        self.store.set_setting(PATHS_KEY, &paths)
    }

    /// The program an engine runs: the user's path, else where it is installed or downloaded.
    pub(crate) fn browser_program(&self, kind: Kind) -> Option<PathBuf> {
        if kind != Kind::Ie
            && let Some(custom) = self.browser_paths().get(&kind)
        {
            return Some(PathBuf::from(custom)).filter(|path| path.is_file());
        }
        if kind == Kind::Camoufox {
            return Some(camoufox_program(&self.data_dir)).filter(|path| path.is_file());
        }

        executable(kind)
    }

    /// IEDriverServer: the user's path, Drift's download, Selenium's usual folder, or PATH.
    pub(crate) fn ie_driver(&self) -> Option<PathBuf> {
        if let Some(custom) = self.browser_paths().get(&Kind::Ie) {
            return Some(PathBuf::from(custom)).filter(|path| path.is_file());
        }

        [
            ie_driver_downloaded(&self.data_dir),
            PathBuf::from(r"C:\selenium\IEDriverServer.exe"),
        ]
        .into_iter()
        .find(|path| path.is_file())
        .or_else(|| which("IEDriverServer"))
    }

    pub fn browser_engines(&self) -> Vec<EngineStatus> {
        Kind::ALL.into_iter().map(|kind| self.engine_status(kind)).collect()
    }

    fn engine_status(&self, kind: Kind) -> EngineStatus {
        let custom = self.browser_paths().contains_key(&kind);
        let path = match kind {
            Kind::Ie => self.ie_driver(),
            _ => self.browser_program(kind),
        };
        let missing = missing(self, kind, path.is_some());
        let download = self.browser_downloads.0.lock().unwrap().get(&kind).cloned();

        EngineStatus {
            engine: kind,
            available: missing.is_none(),
            path: path.map(|path| path.display().to_string()),
            custom,
            missing,
            downloadable: matches!(kind, Kind::Camoufox | Kind::Ie) && cfg!(windows),
            download,
        }
    }

    /// Fetches an engine Drift can download, in the background; its status shows the progress.
    pub fn download_browser_engine(self: &std::sync::Arc<Self>, kind: Kind) -> Result<(), String> {
        if !matches!(kind, Kind::Camoufox | Kind::Ie) {
            return Err(format!(
                "{} is installed by its own installer, not by Drift",
                kind.label()
            ));
        }
        {
            let mut downloads = self.browser_downloads.0.lock().unwrap();
            if downloads.get(&kind).is_some_and(|download| download.error.is_none()) {
                return Ok(());
            }
            downloads.insert(kind, Download::default());
        }

        let engine = self.clone();
        tokio::spawn(async move {
            let result = match kind {
                Kind::Camoufox => engine.fetch_camoufox().await,
                _ => engine.fetch_ie_driver().await,
            };
            let mut downloads = engine.browser_downloads.0.lock().unwrap();
            match result {
                Ok(()) => {
                    downloads.remove(&kind);
                }
                Err(error) => {
                    downloads.entry(kind).or_default().error = Some(error);
                }
            }
        });

        Ok(())
    }

    fn progress(&self, kind: Kind, done: u64, total: u64) {
        let mut downloads = self.browser_downloads.0.lock().unwrap();
        let download = downloads.entry(kind).or_default();
        download.done = done;
        download.total = total;
    }

    async fn fetch_camoufox(&self) -> Result<(), String> {
        let releases = self.github_json(CAMOUFOX_RELEASES).await?;
        let suffix = "-win.x86_64.zip";
        let asset = releases
            .as_array()
            .into_iter()
            .flatten()
            .filter(|release| release["prerelease"] == false)
            .flat_map(|release| release["assets"].as_array().cloned().unwrap_or_default())
            .find(|asset| asset["name"].as_str().is_some_and(|name| name.ends_with(suffix)))
            .ok_or("no Camoufox release for Windows was found")?;
        let url = asset["browser_download_url"].as_str().unwrap_or_default().to_string();

        let target = engines_dir(&self.data_dir).join("camoufox");
        let archive = engines_dir(&self.data_dir).join("camoufox.zip");
        self.fetch_to(Kind::Camoufox, &url, &archive).await?;
        unzip(archive.clone(), target, None).await?;
        let _ = tokio::fs::remove_file(&archive).await;

        Ok(())
    }

    /// The 32-bit driver: the 64-bit one types very slowly into IE.
    async fn fetch_ie_driver(&self) -> Result<(), String> {
        let releases = self.github_json(SELENIUM_RELEASES).await?;
        let asset = releases
            .as_array()
            .into_iter()
            .flatten()
            .flat_map(|release| release["assets"].as_array().cloned().unwrap_or_default())
            .find(|asset| {
                asset["name"]
                    .as_str()
                    .is_some_and(|name| name.starts_with("IEDriverServer_Win32_"))
            })
            .ok_or("no IEDriverServer release was found")?;
        let url = asset["browser_download_url"].as_str().unwrap_or_default().to_string();

        let archive = engines_dir(&self.data_dir).join("IEDriverServer.zip");
        self.fetch_to(Kind::Ie, &url, &archive).await?;
        unzip(archive.clone(), engines_dir(&self.data_dir), Some("IEDriverServer.exe")).await?;
        let _ = tokio::fs::remove_file(&archive).await;

        Ok(())
    }

    async fn github_json(&self, url: &str) -> Result<serde_json::Value, String> {
        let response = self
            .http
            .get(url)
            .header("accept", "application/vnd.github+json")
            .send()
            .await
            .map_err(|error| format!("GitHub could not be reached: {error}"))?;

        response.json().await.map_err(|error| error.to_string())
    }

    async fn fetch_to(&self, kind: Kind, url: &str, path: &Path) -> Result<(), String> {
        let response = self
            .http
            .get(url)
            .send()
            .await
            .and_then(reqwest::Response::error_for_status)
            .map_err(|error| format!("the download failed: {error}"))?;
        let total = response.content_length().unwrap_or(0);
        if let Some(folder) = path.parent() {
            std::fs::create_dir_all(folder).map_err(|error| error.to_string())?;
        }
        let mut file = std::fs::File::create(path).map_err(|error| error.to_string())?;

        let mut done = 0;
        let mut body = response.bytes_stream();
        while let Some(chunk) = body.next().await {
            let chunk = chunk.map_err(|error| format!("the download was cut off: {error}"))?;
            file.write_all(&chunk).map_err(|error| error.to_string())?;
            done += chunk.len() as u64;
            self.progress(kind, done, total);
        }

        Ok(())
    }
}

/// What keeps an engine from running here, if anything.
fn missing(engine: &Engine, kind: Kind, found: bool) -> Option<String> {
    let reason = match kind {
        Kind::Chrome | Kind::Edge | Kind::Firefox if !found => {
            format!("{} is not installed; install it, or set where it is", kind.label())
        }
        Kind::Camoufox if !found => "Camoufox is not downloaded yet (about 1.3 GB)".into(),
        Kind::Ie if !cfg!(windows) => "IE mode runs only on Windows".into(),
        Kind::Ie if executable(Kind::Edge).is_none() => "IE mode needs Microsoft Edge".into(),
        Kind::Ie if !found && engine.ie_driver().is_none() => "IEDriverServer is not downloaded yet".into(),
        _ => return None,
    };

    Some(reason)
}

/// Makes sure an engine can start, fetching IEDriverServer on the way; the error says what is missing.
pub(crate) async fn ensure(engine: &Engine, kind: Kind) -> Result<(), String> {
    if kind == Kind::Ie && cfg!(windows) && engine.ie_driver().is_none() && executable(Kind::Edge).is_some() {
        engine.fetch_ie_driver().await?;
    }

    let found = match kind {
        Kind::Ie => engine.ie_driver().is_some(),
        _ => engine.browser_program(kind).is_some(),
    };
    match missing(engine, kind, found) {
        Some(reason) => Err(format!(
            "{reason}. Settings > Browser shows each engine and can fetch the ones Drift can."
        )),
        None => Ok(()),
    }
}

/// Unpacks a zip into `target`, flattening a single top folder; `only` keeps just that file.
async fn unzip(archive: PathBuf, target: PathBuf, only: Option<&'static str>) -> Result<(), String> {
    tokio::task::spawn_blocking(move || {
        let file = std::fs::File::open(&archive).map_err(|error| error.to_string())?;
        let mut zip = zip::ZipArchive::new(std::io::BufReader::new(file)).map_err(|error| error.to_string())?;
        std::fs::create_dir_all(&target).map_err(|error| error.to_string())?;

        for index in 0..zip.len() {
            let mut entry = zip.by_index(index).map_err(|error| error.to_string())?;
            let Some(name) = entry.enclosed_name() else {
                continue;
            };
            if only.is_some_and(|only| name.file_name().is_none_or(|file| file != only)) {
                continue;
            }
            let path = match only {
                Some(only) => target.join(only),
                None => target.join(&name),
            };
            if entry.is_dir() {
                std::fs::create_dir_all(&path).map_err(|error| error.to_string())?;
                continue;
            }
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
            }
            let mut out = std::fs::File::create(&path).map_err(|error| error.to_string())?;
            std::io::copy(&mut entry, &mut out).map_err(|error| error.to_string())?;
        }

        Ok(())
    })
    .await
    .map_err(|error| error.to_string())?
}
