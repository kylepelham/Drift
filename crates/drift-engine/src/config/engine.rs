use std::path::{Path, PathBuf};

use super::{plugins, sources};
use crate::Engine;
use crate::hook::PluginInfo;

/// Plugins the user switched off in Settings, by their drift.json entry.
const DISABLED_PLUGINS_KEY: &str = "disabledPlugins";
/// Skill folders the user switched off in Settings; the files themselves are never touched.
const DISABLED_SKILLS_KEY: &str = "disabledSkills";
const REGISTRY_SOURCES_KEY: &str = "registrySources";

#[derive(Debug, thiserror::Error)]
pub enum RegistryError {
    #[error("a registry source needs a name and a location")]
    MissingLocation,
    #[error("a URL source needs https (or http allowed for it): {0}")]
    InsecureUrl(String),
    #[error("{0}")]
    Credentials(String),
    #[error(transparent)]
    Store(#[from] rusqlite::Error),
}

impl Engine {
    /// Reads drift.json again and loads every plugin that is not switched off.
    pub async fn reload_plugins(&self) -> Vec<PluginInfo> {
        let disabled: Vec<String> = self
            .store
            .setting(DISABLED_PLUGINS_KEY)
            .ok()
            .flatten()
            .unwrap_or_default();

        self.hooks
            .load(
                &self.data_dir.join("plugin-cache"),
                super::user_plugins(),
                &disabled,
                self.me.clone(),
            )
            .await
    }

    pub fn registry_sources(&self) -> Vec<sources::RegistrySource> {
        let mut sources: Vec<sources::RegistrySource> = self
            .store
            .setting(REGISTRY_SOURCES_KEY)
            .ok()
            .flatten()
            .unwrap_or_default();

        for source in &mut sources {
            source.has_token = self.credentials.secret(&sources::token_key(&source.id)).is_some();
        }

        sources
    }

    pub fn registry_source(&self, id: &str) -> Option<sources::RegistrySource> {
        self.registry_sources().into_iter().find(|source| source.id == id)
    }

    /// Stores the sources and each one's token; a source dropped from the list loses its token too.
    pub fn set_registry_sources(&self, inputs: Vec<sources::SourceInput>) -> Result<(), RegistryError> {
        let before = self.registry_sources();
        let mut sources = Vec::new();

        for input in inputs {
            let mut source = input.source;
            if source.name.trim().is_empty() || source.url.trim().is_empty() {
                return Err(RegistryError::MissingLocation);
            }
            if source.id.trim().is_empty() {
                source.id = crate::random_hex(8);
            }

            let is_url = matches!(source.source, sources::SourceKind::Url);
            if is_url
                && !source.url.starts_with("https://")
                && !(source.allow_http && source.url.starts_with("http://"))
            {
                return Err(RegistryError::InsecureUrl(source.url));
            }

            let token_key = sources::token_key(&source.id);
            match input.token.as_deref().map(str::trim) {
                Some("") => self
                    .credentials
                    .remove_secret(&token_key)
                    .map_err(|error| RegistryError::Credentials(error.to_string()))?,
                Some(token) => self
                    .credentials
                    .set_secret(&token_key, token)
                    .map_err(|error| RegistryError::Credentials(error.to_string()))?,
                None => {}
            }
            source.has_token = false;
            sources.push(source);
        }

        for gone in before.iter().filter(|old| !sources.iter().any(|new| new.id == old.id)) {
            let _ = self.credentials.remove_secret(&sources::token_key(&gone.id));
        }

        self.store.set_setting(REGISTRY_SOURCES_KEY, &sources)?;
        Ok(())
    }

    pub fn fetcher(&self) -> sources::Fetcher {
        sources::Fetcher::new(self.http.clone(), self.credentials.clone())
    }

    /// Fetches a registry plugin, checks its hash, lists it in drift.json with its config, and reloads.
    pub async fn install_plugin(&self, install: plugins::Install) -> Result<Vec<PluginInfo>, plugins::PluginError> {
        let source = install.registry.as_deref().and_then(|id| self.registry_source(id));
        let path = plugins::fetch_component(&self.fetcher(), source.as_ref(), &install).await?;
        let dir = plugins::config_dir()?;
        plugins::edit_plugins(&dir, |plugins| {
            plugins::set_entry(plugins, &path, install.config);
        })?;

        Ok(self.reload_plugins().await)
    }

    /// Removes a plugin's drift.json entry and its component, and reloads.
    pub async fn remove_plugin(&self, path: &str) -> Result<Vec<PluginInfo>, plugins::PluginError> {
        plugins::remove(&plugins::config_dir()?, path)?;

        Ok(self.reload_plugins().await)
    }

    /// Replaces a plugin's config in drift.json and reloads, so it reads the new values.
    pub async fn configure_plugin(
        &self,
        path: &str,
        config: serde_json::Value,
    ) -> Result<Vec<PluginInfo>, plugins::PluginError> {
        let dir = plugins::config_dir()?;
        plugins::edit_plugins(&dir, |plugins| plugins::set_entry(plugins, path, config))?;

        Ok(self.reload_plugins().await)
    }

    /// Switches a plugin on or off by its drift.json entry and reloads.
    pub async fn set_plugin_enabled(&self, path: &str, enabled: bool) -> rusqlite::Result<Vec<PluginInfo>> {
        let mut disabled: Vec<String> = self.store.setting(DISABLED_PLUGINS_KEY)?.unwrap_or_default();
        disabled.retain(|entry| entry != path);
        if !enabled {
            disabled.push(path.to_owned());
        }

        self.store.set_setting(DISABLED_PLUGINS_KEY, &disabled)?;
        Ok(self.reload_plugins().await)
    }

    /// The skill folders switched off, as the engine compares folders.
    pub fn disabled_skills(&self) -> Vec<PathBuf> {
        self.store
            .setting::<Vec<String>>(DISABLED_SKILLS_KEY)
            .ok()
            .flatten()
            .unwrap_or_default()
            .into_iter()
            .map(PathBuf::from)
            .collect()
    }

    /// Turns a skill on or off for every workspace and session from the next turn; its files stay as they are.
    pub fn set_skill_enabled(&self, folder: &Path, enabled: bool) -> rusqlite::Result<()> {
        let key = folder.to_string_lossy().into_owned();
        let mut disabled: Vec<String> = self.store.setting(DISABLED_SKILLS_KEY)?.unwrap_or_default();
        disabled.retain(|entry| *entry != key);
        if !enabled {
            disabled.push(key);
        }

        self.store.set_setting(DISABLED_SKILLS_KEY, &disabled)
    }
}
