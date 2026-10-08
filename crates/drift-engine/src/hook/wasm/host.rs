use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde_json::Value;

use super::bindings::drift::plugin::host::{Host, Level};
use super::bindings::drift::plugin::{files, http, notify, process, store};
use super::{State, wit};

const RUN_LIMIT: Duration = Duration::from_secs(60);
const FETCH_LIMIT: Duration = Duration::from_secs(30);
const OUTPUT_BYTES: usize = 64 * 1024;
const BODY_BYTES: usize = 1024 * 1024;

#[derive(Debug, thiserror::Error)]
enum HostError {
    #[error("Drift is shutting down")]
    Shutdown,
    #[error("no such path")]
    NoPath,
    #[error("{path}: {source}")]
    Path { path: String, source: std::io::Error },
    #[error("workspace: {0}")]
    Workspace(std::io::Error),
    #[error("{0} is outside the workspace")]
    Outside(String),
}

impl State {
    fn engine(&self) -> Result<std::sync::Arc<crate::Engine>, HostError> {
        self.site.engine.upgrade().ok_or(HostError::Shutdown)
    }

    /// A workspace-relative path that stays inside the workspace, or why it may not be used.
    fn inside(&self, path: &str) -> Result<PathBuf, HostError> {
        let joined = self.workspace.join(path);
        let parent = joined.parent().ok_or(HostError::NoPath)?;
        let real_parent = std::fs::canonicalize(parent).map_err(|source| HostError::Path {
            path: path.to_owned(),
            source,
        })?;
        let workspace = std::fs::canonicalize(&self.workspace).map_err(HostError::Workspace)?;

        if !real_parent.starts_with(&workspace) {
            return Err(HostError::Outside(path.to_owned()));
        }

        let filename = joined.file_name().ok_or(HostError::NoPath)?;
        Ok(real_parent.join(filename))
    }

    /// Time a host call took is the plugin's to keep, not charged against its own budget.
    async fn clocked<T>(&mut self, work: impl std::future::Future<Output = T>) -> T {
        let started = Instant::now();
        let result = work.await;
        self.deadline += started.elapsed();

        result
    }

    fn store_key(&self) -> String {
        format!("plugin:{}", self.site.entry)
    }
}

impl wit::Host for State {}

impl Host for State {
    async fn log(&mut self, level: Level, message: String) {
        let level = match level {
            Level::Info => "info",
            Level::Warn => "warn",
            Level::Error => "error",
        };

        eprintln!("drift: plugin {} [{level}]: {message}", self.name);
    }

    async fn config(&mut self) -> String {
        self.site.config.to_string()
    }
}

impl store::Host for State {
    async fn get(&mut self, key: String) -> Option<String> {
        let engine = self.engine().ok()?;
        let values: serde_json::Map<String, Value> = engine
            .store
            .setting(&self.store_key())
            .ok()
            .flatten()
            .unwrap_or_default();

        values.get(&key).and_then(Value::as_str).map(str::to_owned)
    }

    async fn set(&mut self, key: String, value: String) {
        let Ok(engine) = self.engine() else { return };

        let mut values: serde_json::Map<String, Value> = engine
            .store
            .setting(&self.store_key())
            .ok()
            .flatten()
            .unwrap_or_default();
        values.insert(key, Value::String(value));

        let _ = engine.store.set_setting(&self.store_key(), &values);
    }
}

impl files::Host for State {
    async fn read(&mut self, path: String) -> Result<String, String> {
        let file = self.inside(&path).map_err(|error| error.to_string())?;

        self.clocked(async {
            tokio::fs::read_to_string(&file)
                .await
                .map_err(|error| format!("{path}: {error}"))
        })
        .await
    }

    async fn write(&mut self, path: String, content: String) -> Result<(), String> {
        // Reject traversal before creating a new folder for the file.
        if Path::new(&path).components().any(|part| {
            matches!(
                part,
                std::path::Component::ParentDir | std::path::Component::Prefix(_) | std::path::Component::RootDir
            )
        }) {
            return Err(format!("{path} is outside the workspace"));
        }

        if let Some(parent) = self.workspace.join(&path).parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let file = self.inside(&path).map_err(|error| error.to_string())?;

        self.clocked(async {
            tokio::fs::write(&file, content)
                .await
                .map_err(|error| format!("{path}: {error}"))
        })
        .await
    }
}

impl process::Host for State {
    async fn run(&mut self, program: String, args: Vec<String>, timeout_ms: u32) -> Result<process::Output, String> {
        let workspace = self.workspace.clone();
        let limit = Duration::from_millis(u64::from(timeout_ms)).min(RUN_LIMIT);

        self.clocked(async move {
            // Windows needs the resolved program path, not just its bare name.
            let resolved = crate::platform::process::which(&program).unwrap_or_else(|| PathBuf::from(&program));
            let mut command = tokio::process::Command::new(&resolved);
            command
                .args(&args)
                .current_dir(&workspace)
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped())
                .kill_on_drop(true);
            crate::platform::process::use_current_path(&mut command, &Default::default());
            crate::platform::process::prepare(&mut command);
            #[cfg(windows)]
            command.creation_flags(0x0800_0000);

            let child = command.spawn().map_err(|error| format!("{program}: {error}"))?;
            let output = tokio::time::timeout(limit, child.wait_with_output())
                .await
                .map_err(|_| format!("{program} ran past {} ms", limit.as_millis()))?
                .map_err(|error| error.to_string())?;

            Ok(process::Output {
                code: output.status.code().unwrap_or(-1),
                stdout: bounded(output.stdout, OUTPUT_BYTES),
                stderr: bounded(output.stderr, OUTPUT_BYTES),
            })
        })
        .await
    }
}

impl http::Host for State {
    async fn fetch(
        &mut self,
        method: String,
        url: String,
        headers: Vec<(String, String)>,
        body: Option<String>,
    ) -> Result<http::Response, String> {
        let client = self.engine().map_err(|error| error.to_string())?.http.clone();

        self.clocked(async move {
            let method =
                reqwest::Method::from_bytes(method.as_bytes()).map_err(|_| format!("unknown method {method}"))?;
            let mut request = client.request(method, &url).timeout(FETCH_LIMIT);
            for (name, value) in headers {
                request = request.header(name, value);
            }
            if let Some(body) = body {
                request = request.body(body);
            }

            let response = request.send().await.map_err(|error| error.to_string())?;
            let status = response.status().as_u16();
            let bytes = response.bytes().await.map_err(|error| error.to_string())?;

            Ok(http::Response {
                status,
                body: bounded(bytes.to_vec(), BODY_BYTES),
            })
        })
        .await
    }
}

impl notify::Host for State {
    async fn show(&mut self, title: String, body: String, tone: notify::Tone) {
        let Ok(engine) = self.engine() else { return };

        let tone = match tone {
            notify::Tone::Info => "info",
            notify::Tone::Success => "success",
            notify::Tone::Warning => "warning",
            notify::Tone::Error => "error",
        };

        engine.hub.publish(crate::event::Event::PluginNotice {
            plugin: self.name.clone(),
            title,
            body,
            tone: tone.into(),
        });
    }
}

fn bounded(mut bytes: Vec<u8>, limit: usize) -> String {
    bytes.truncate(limit);

    String::from_utf8_lossy(&bytes).into_owned()
}
