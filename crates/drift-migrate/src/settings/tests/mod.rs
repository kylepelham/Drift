use drift_engine::Engine;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use super::*;

mod config;
mod credentials;
mod files;
mod report;
mod servers;

struct Dir(PathBuf);

impl Drop for Dir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

struct Fixture {
    dir: Dir,
    engine: Arc<Engine>,
    providers: Vec<String>,
}

impl Fixture {
    fn new() -> Self {
        let _ = rustls::crypto::ring::default_provider().install_default();
        let dir = Dir(std::env::temp_dir().join(format!("drift-settings-{}", drift_engine::id::new("t"))));
        let engine = Engine::open_with(
            &dir.0.join("data"),
            drift_engine::Options {
                file_credentials: true,
                ..Default::default()
            },
        )
        .unwrap();
        let providers = engine.catalog.read().unwrap().providers.keys().cloned().collect();

        Self { dir, engine, providers }
    }

    fn import(&self, home: &Path, settings: &Settings) -> SettingsReport {
        import_settings(
            &self.engine.store,
            &self.engine.credentials,
            &self.providers,
            home,
            settings,
        )
        .unwrap()
    }
}

fn settings(dir: &Path, auth: Value, config: Value, servers: Vec<OcServer>) -> Settings {
    Settings {
        auth: Some(auth),
        config: OcConfig::parse(&config.to_string()),
        config_dir: dir.to_path_buf(),
        servers,
    }
}

fn read_config(home: &Path) -> Value {
    let path = home.join(".config/drift/drift.json");
    let text = std::fs::read_to_string(path).unwrap();

    serde_json::from_str(&text).unwrap()
}
