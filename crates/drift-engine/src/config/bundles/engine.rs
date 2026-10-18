use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;

use super::servers::{self, Vars};
use super::{Active, Bundle, BundleError, BundlePartKind, InstallBundle, Installed, Layout, STATE_KEY, State};
use crate::Engine;
use crate::event::Event;
use crate::mcp::ServerConfig;

impl Engine {
    fn bundle_states(&self) -> BTreeMap<String, State> {
        self.store.setting(STATE_KEY).ok().flatten().unwrap_or_default()
    }

    pub(super) fn change_bundle_state(&self, id: &str, change: impl FnOnce(&mut State)) -> Result<State, BundleError> {
        let mut states = self.bundle_states();
        let state = states.entry(id.to_owned()).or_default();
        change(state);
        let changed = state.clone();
        self.store.set_setting(STATE_KEY, &states)?;

        Ok(changed)
    }

    /// Every installed bundle with its parts and switches.
    pub fn bundles(&self) -> Vec<Bundle> {
        let states = self.bundle_states();

        super::installed()
            .iter()
            .filter_map(|installed| {
                super::view(installed, &states.get(&installed.id).cloned().unwrap_or_default()).ok()
            })
            .collect()
    }

    /// The bundles that are on, as a workspace's config reads them.
    pub fn active_bundles(&self) -> Vec<Active> {
        let states = self.bundle_states();

        super::installed()
            .into_iter()
            .filter_map(|installed| {
                let state = states.get(&installed.id).cloned().unwrap_or_default();
                let root = super::bundles_dir().ok()?.join(&installed.id);
                let active = Active {
                    layout: Layout::read(&root),
                    data: super::data_dir(&installed.id).ok()?,
                    root,
                    state,
                };
                (!active.state.off).then_some(active)
            })
            .collect()
    }

    /// Fetches the bundle, unpacks it, saves its MCP servers (keeping the switches of an earlier install) and starts them.
    pub async fn install_bundle(self: &Arc<Self>, install: InstallBundle) -> Result<Vec<Bundle>, BundleError> {
        let source = install.registry.as_deref().and_then(|id| self.registry_source(id));
        let mut installed = super::install(&self.fetcher(), source.as_ref(), &install).await?;
        // Listed at once, so a failure saving its servers still leaves a bundle that can be seen and removed.
        super::write_marker(&installed)?;
        let root = super::bundles_dir()?.join(&installed.id);
        let data = super::data_dir(&installed.id)?;
        self.save_bundle_servers(&mut installed, &install.values, &root, &data)
            .await?;
        super::write_marker(&installed)?;

        Ok(self.bundles())
    }

    /// Removes the bundle's servers, files, data and switches.
    pub async fn remove_bundle(&self, id: &str) -> Result<Vec<Bundle>, BundleError> {
        super::find(id)?;
        for row in self.store.mcp_servers()? {
            if row.bundle.as_deref() == Some(id) {
                self.drop_bundle_server(&row.name).await?;
            }
        }
        super::remove_files(id)?;
        let mut states = self.bundle_states();
        states.remove(id);
        self.store.set_setting(STATE_KEY, &states)?;

        Ok(self.bundles())
    }

    /// The bundle's one switch: off, none of its parts apply and its servers stop; on, each part follows its own.
    pub async fn set_bundle_enabled(self: &Arc<Self>, id: &str, enabled: bool) -> Result<Vec<Bundle>, BundleError> {
        let installed = super::find(id)?;
        let state = self.change_bundle_state(id, |state| state.off = !enabled)?;
        self.apply_bundle_servers(&installed, &state).await?;

        Ok(self.bundles())
    }

    /// One part's switch; skills, commands, agents and programs apply from the next turn, a server at once.
    pub async fn set_bundle_part(
        self: &Arc<Self>,
        id: &str,
        kind: BundlePartKind,
        name: &str,
        enabled: bool,
    ) -> Result<Vec<Bundle>, BundleError> {
        let installed = super::find(id)?;
        let key = super::part_key(kind, name);
        let state = self.change_bundle_state(id, |state| {
            if enabled {
                state.parts_off.remove(&key);
            } else {
                state.parts_off.insert(key);
            }
        })?;
        if kind == BundlePartKind::Server {
            self.apply_bundle_servers(&installed, &state).await?;
        }

        Ok(self.bundles())
    }

    /// Saves each server the bundle in `root` declares, with `${...}` filled in, and drops the ones it no longer declares.
    pub(super) async fn save_bundle_servers(
        self: &Arc<Self>,
        installed: &mut Installed,
        values: &BTreeMap<String, String>,
        root: &Path,
        data: &Path,
    ) -> Result<(), BundleError> {
        let id = installed.id.clone();
        let state = self.bundle_states().remove(&id).unwrap_or_default();
        let vars = Vars {
            root: super::slashed(root),
            data: super::slashed(data),
            values,
        };
        let rows = self.store.mcp_servers()?;
        let unreadable = self.store.unreadable_mcp_servers()?;
        let owned = |name: &str| {
            rows.iter()
                .find(|row| row.name == name && row.bundle.as_deref() == Some(&id))
        };
        let taken = |name: &str| {
            owned(name).is_none()
                && (rows.iter().any(|row| row.name == name) || unreadable.iter().any(|row| row == name))
        };

        installed.servers.clear();
        let mut assigned = std::collections::HashSet::new();
        for (declared, value) in &Layout::read(root).servers {
            let name = servers::row_name(declared, &id, |name| taken(name) || assigned.contains(name));
            assigned.insert(name.clone());
            let kept = owned(&name).map(|row| &row.config);
            let Some(config) = servers::config(value, &vars).map(|config| keep_secrets(config, kept, value)) else {
                continue;
            };
            let on = state.on(BundlePartKind::Server, declared);
            let row = self
                .mcp
                .change(&name, &self.store, &self.hub, |store| {
                    store.save_bundle_mcp_server(&name, &config, &id, on)
                })
                .await?;
            self.hub.publish(Event::McpUpdated {
                server: self.mcp.status_of(row.clone()),
            });
            self.begin_remote_mcp(&row);
            installed.servers.insert(declared.clone(), name);
        }

        for row in rows.iter().filter(|row| row.bundle.as_deref() == Some(&id)) {
            if !installed.servers.values().any(|name| *name == row.name) {
                self.drop_bundle_server(&row.name).await?;
            }
        }

        Ok(())
    }

    /// Each of the bundle's servers on or off as its state says; one that changes connects or closes now.
    pub(super) async fn apply_bundle_servers(
        self: &Arc<Self>,
        installed: &Installed,
        state: &State,
    ) -> Result<(), BundleError> {
        for (declared, name) in &installed.servers {
            let Some(row) = self.store.mcp_server(name)? else {
                continue;
            };
            let on = state.on(BundlePartKind::Server, declared);
            if row.enabled == on {
                continue;
            }

            let changed = if on {
                self.store.set_mcp_enabled(name, true)?;
                self.store.mcp_server(name)?
            } else {
                self.mcp
                    .close(name, &self.store, &self.hub, |store| {
                        store.set_mcp_enabled(name, false)?;
                        store.mcp_server(name)
                    })
                    .await?
            };
            if let Some(row) = changed {
                self.hub.publish(Event::McpUpdated {
                    server: self.mcp.status_of(row.clone()),
                });
                self.begin_remote_mcp(&row);
            }
        }

        Ok(())
    }

    pub(super) async fn drop_bundle_server(&self, name: &str) -> Result<(), BundleError> {
        self.mcp
            .close(name, &self.store, &self.hub, |store| store.remove_mcp_server(name))
            .await?;
        let _ = crate::mcp::forget_sign_in(&self.credentials, name);
        self.hub.publish(Event::McpRemoved { name: name.to_owned() });

        Ok(())
    }
}

/// A reinstall without a value the user gave before keeps the saved one, so an update never drops a key; only
/// for a header or env entry the bundle still declares.
fn keep_secrets(mut config: ServerConfig, kept: Option<&ServerConfig>, declared: &serde_json::Value) -> ServerConfig {
    let (fresh, saved, names) = match (&mut config, kept) {
        (ServerConfig::Stdio { env, .. }, Some(ServerConfig::Stdio { env: saved, .. })) => {
            (env, saved, &declared["env"])
        }
        (ServerConfig::Http { headers, .. }, Some(ServerConfig::Http { headers: saved, .. }))
        | (ServerConfig::Sse { headers, .. }, Some(ServerConfig::Sse { headers: saved, .. })) => {
            (headers, saved, &declared["headers"])
        }
        _ => return config,
    };
    for (name, value) in saved.iter().filter(|(name, _)| names.get(name.as_str()).is_some()) {
        fresh.entry(name.clone()).or_insert_with(|| value.clone());
    }

    config
}
