//! Every language server Drift could use, as Settings lists them: where each comes from, whether its program is
//! installed and has answered, and how to install one that is missing.

use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::Serialize;
use utoipa::ToSchema;

use super::client::Client;
use super::{RETRY, Servers, setup, table};
use crate::config::LspConfig;
use crate::hook::{LanguageServer, ServerSetup};

/// How long a check waits for a server to initialize.
const CHECK: Duration = Duration::from_secs(30);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum LspSource {
    /// One of Drift's own.
    Builtin,
    /// A plugin declares it.
    Plugin,
    /// The user's drift.json adds it.
    Config,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum LspState {
    /// drift.json turns it off.
    Off,
    /// Its program is not on PATH (nor in the workspace's `node_modules/.bin`).
    Missing,
    /// Its program is installed; it starts when a file it handles is read.
    Found,
    /// It is running for a project now.
    Running,
    /// A check started it and it answered.
    Works,
    /// It is installed but did not start or answer.
    Failed,
}

#[derive(Clone, Debug, PartialEq, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct LspStatus {
    /// Its name in drift.json's `lsp`.
    pub name: String,
    /// The language it serves, as people name it.
    pub label: String,
    pub source: LspSource,
    /// The plugin that declares it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub plugin: Option<String>,
    pub extensions: Vec<String>,
    /// The command it starts with, the first of those tried when it has several.
    pub command: String,
    pub state: LspState,
    /// The program that runs, when one is installed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// Why it did not start or answer.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// The command Install runs, for a missing server whose package manager is here.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub install: Option<String>,
    /// The programs an install needs, when none of them is here.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub needs: Vec<String>,
    /// Where its install guide is.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub guide: Option<String>,
    /// Where it comes from, for a plugin's server with neither an install nor a guide.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// A server before its state is looked up.
struct Entry {
    name: String,
    label: String,
    source: LspSource,
    plugin: Option<String>,
    extensions: Vec<String>,
    commands: Vec<Vec<String>>,
    /// How it is installed; drift.json's own servers say nothing.
    setup: Option<ServerSetup>,
}

/// Every server in the order Settings shows them: the built-ins less those a plugin or drift.json replaces, the
/// plugins' less those drift.json replaces, then drift.json's own.
fn entries(config: &BTreeMap<String, LspConfig>, plugins: &[(String, LanguageServer)]) -> Vec<Entry> {
    let custom = |name: &str| matches!(config.get(name), Some(LspConfig::Custom { .. }));
    let from_plugin = |name: &str| plugins.iter().any(|(_, server)| server.id == name);
    let words = |command: &[&str]| command.iter().map(ToString::to_string).collect::<Vec<_>>();

    let builtin = table::BUILTIN
        .iter()
        .filter(|server| !custom(server.name) && !from_plugin(server.name))
        .map(|server| Entry {
            name: server.name.into(),
            label: setup::of(server.name).map_or(server.name, |setup| setup.label).into(),
            source: LspSource::Builtin,
            plugin: None,
            extensions: words(server.extensions),
            commands: server.commands.iter().map(|command| words(command)).collect(),
            setup: setup::of(server.name).map(setup::Setup::server_setup),
        });
    let plugin = plugins
        .iter()
        .filter(|(_, server)| !custom(&server.id))
        .map(|(plugin, server)| Entry {
            name: server.id.clone(),
            label: server.label.clone(),
            source: LspSource::Plugin,
            plugin: Some(plugin.clone()),
            extensions: server.extensions.clone(),
            commands: vec![server.command.clone()],
            setup: Some(server.setup.clone()),
        });
    let own = config.iter().filter_map(|(name, server)| match server {
        LspConfig::Custom {
            command,
            extensions,
            language,
        } if !command.is_empty() => Some(Entry {
            name: name.clone(),
            label: language.clone().unwrap_or_else(|| name.clone()),
            source: LspSource::Config,
            plugin: None,
            extensions: extensions.clone(),
            commands: vec![command.clone()],
            setup: None,
        }),
        _ => None,
    });

    builtin.chain(plugin).chain(own).collect()
}

/// The first of `commands` installed: on PATH, or in the workspace's `node_modules/.bin`.
fn locate(commands: &[Vec<String>], workspace: Option<&Path>) -> Option<(PathBuf, Vec<String>)> {
    match workspace {
        Some(workspace) => table::installed(commands, workspace, workspace),
        None => commands.iter().find_map(|command| {
            let (program, args) = command.split_first()?;
            crate::platform::process::which(program).map(|found| (found, args.to_vec()))
        }),
    }
}

impl Servers {
    /// Every server Drift could use with `config` (drift.json's `lsp`) and the plugins' servers, and how each stands.
    pub fn statuses(
        &self,
        config: &BTreeMap<String, LspConfig>,
        plugins: &[(String, LanguageServer)],
        workspace: Option<&Path>,
    ) -> Vec<LspStatus> {
        let running = self.running_names();
        let failed = self.failed_names();
        let checked = self.checked.lock().unwrap().clone();

        entries(config, plugins)
            .into_iter()
            .map(|entry| {
                let found = locate(&entry.commands, workspace);
                let setup = entry.setup.as_ref();
                let (state, error) = if config.get(&entry.name) == Some(&LspConfig::Enabled(false)) {
                    (LspState::Off, None)
                } else if found.is_none() {
                    (LspState::Missing, None)
                } else if running.contains(&entry.name) {
                    (LspState::Running, None)
                } else {
                    match checked.get(&entry.name) {
                        Some(Ok(())) => (LspState::Works, None),
                        Some(Err(why)) => (LspState::Failed, Some(why.clone())),
                        None if failed.contains(&entry.name) => (
                            LspState::Failed,
                            Some("it did not start the last time Drift tried".into()),
                        ),
                        None => (LspState::Found, None),
                    }
                };
                let install = setup
                    .filter(|_| found.is_none())
                    .and_then(|setup| setup::recipe(setup, crate::platform::process::which))
                    .map(|(program, args)| command_line(&program, &args));
                let needs = match (setup, &install) {
                    (Some(setup), None) if found.is_none() => setup::needs(setup),
                    _ => Vec::new(),
                };

                LspStatus {
                    command: entry
                        .commands
                        .first()
                        .map(|command| command.join(" "))
                        .unwrap_or_default(),
                    path: found.map(|(program, _)| program.display().to_string()),
                    guide: setup.and_then(guide_of),
                    note: match setup {
                        Some(ServerSetup::None(why)) => Some(why.clone()),
                        _ => None,
                    },
                    name: entry.name,
                    label: entry.label,
                    source: entry.source,
                    plugin: entry.plugin,
                    extensions: entry.extensions,
                    state,
                    error,
                    install,
                    needs,
                }
            })
            .collect()
    }

    /// Starts the server named `name` in `workspace` (else an empty folder) and waits for it to initialize, then
    /// stops it; what it found is kept for [`Self::statuses`].
    pub async fn check(
        &self,
        name: &str,
        config: &BTreeMap<String, LspConfig>,
        plugins: &[(String, LanguageServer)],
        workspace: Option<&Path>,
    ) -> Result<(), String> {
        let entry = entries(config, plugins)
            .into_iter()
            .find(|entry| entry.name == name)
            .ok_or_else(|| format!("no language server is named {name}"))?;
        let (program, args) = locate(&entry.commands, workspace).ok_or_else(|| format!("{name} is not installed"))?;
        let folder = match workspace {
            Some(workspace) => workspace.to_path_buf(),
            None => {
                let empty = crate::platform::temp::root().join("lsp-check");
                std::fs::create_dir_all(&empty).map_err(|error| error.to_string())?;
                empty
            }
        };

        let answered = match Client::start(&program, &args, &folder).await {
            Ok(client) => {
                let ready = client.ready_by(tokio::time::Instant::now() + CHECK).await;
                client.shutdown().await;
                if ready {
                    Ok(())
                } else {
                    Err(format!(
                        "it started but did not answer within {} seconds",
                        CHECK.as_secs()
                    ))
                }
            }
            Err(error) => Err(format!("it could not start: {error}")),
        };
        self.forget_failures(name);
        self.checked.lock().unwrap().insert(name.to_owned(), answered.clone());

        answered
    }

    /// Runs the install command Settings offered for the server `name`: Drift's own for a built-in, its plugin's
    /// for a plugin's.
    pub async fn install(
        &self,
        name: &str,
        config: &BTreeMap<String, LspConfig>,
        plugins: &[(String, LanguageServer)],
    ) -> Result<(), String> {
        let setup = entries(config, plugins)
            .into_iter()
            .find(|entry| entry.name == name)
            .and_then(|entry| entry.setup)
            .ok_or_else(|| format!("Drift has no way to install {name}"))?;
        let (program, args) = setup::recipe(&setup, crate::platform::process::which).ok_or_else(|| {
            let needs = setup::needs(&setup);
            if needs.is_empty() {
                format!("{name} has no install command; see its guide")
            } else {
                format!("installing {name} needs {}", needs.join(" or "))
            }
        })?;

        setup::run(&program, &args).await?;
        self.forget_failures(name);
        self.checked.lock().unwrap().remove(name);
        Ok(())
    }

    fn running_names(&self) -> HashSet<String> {
        self.running
            .lock()
            .unwrap()
            .iter()
            .filter(|(_, client)| client.alive())
            .map(|((_, name, _), _)| name.clone())
            .collect()
    }

    fn failed_names(&self) -> HashSet<String> {
        let mut failed = self.failed.lock().unwrap();
        failed.retain(|_, at| at.elapsed() < RETRY);
        failed.keys().map(|(_, name, _)| name.clone()).collect()
    }

    /// A server just installed or checked is tried again at once wherever it failed to start.
    fn forget_failures(&self, name: &str) {
        self.failed.lock().unwrap().retain(|(_, failed, _), _| failed != name);
    }
}

fn guide_of(setup: &ServerSetup) -> Option<String> {
    match setup {
        ServerSetup::Install { guide, .. } | ServerSetup::Guide(guide) => Some(guide.clone()),
        ServerSetup::None(_) => None,
    }
}

/// A command as the user would type it, the program by its name.
fn command_line(program: &Path, args: &[String]) -> String {
    let name = program.file_stem().map_or_else(
        || program.display().to_string(),
        |stem| stem.to_string_lossy().into_owned(),
    );

    std::iter::once(name)
        .chain(args.iter().cloned())
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plugin_server(id: &str, command: &str, setup: ServerSetup) -> (String, LanguageServer) {
        let server = LanguageServer {
            id: id.into(),
            command: vec![command.into()],
            extensions: vec![".roc".into()],
            language: Some("roc".into()),
            label: format!("{id} language"),
            setup,
        };
        ("roc".into(), server)
    }

    #[test]
    fn plugins_and_drift_json_replace_built_ins_by_name_and_off_is_off() {
        let config = BTreeMap::from([
            ("gopls".to_owned(), LspConfig::Enabled(false)),
            (
                "clangd".to_owned(),
                LspConfig::Custom {
                    command: vec!["my-clangd".into()],
                    extensions: vec![".cpp".into()],
                    language: None,
                },
            ),
        ]);
        let guide = || ServerSetup::Guide("https://roc-lang.org/install".into());
        let plugins = [
            plugin_server("roc", "roc_language_server", guide()),
            plugin_server(
                "zls",
                "my-zls",
                ServerSetup::None("ships with the team's toolchain".into()),
            ),
        ];
        let statuses = Servers::default().statuses(&config, &plugins, None);
        let named = |name: &str| statuses.iter().find(|status| status.name == name).unwrap();

        assert_eq!(statuses.iter().filter(|status| status.name == "clangd").count(), 1);
        assert_eq!(
            (named("clangd").source, named("clangd").command.as_str()),
            (LspSource::Config, "my-clangd")
        );
        assert_eq!(
            (named("zls").source, named("zls").plugin.as_deref()),
            (LspSource::Plugin, Some("roc"))
        );
        assert_eq!(named("zls").note.as_deref(), Some("ships with the team's toolchain"));
        assert_eq!(
            (named("roc").label.as_str(), named("roc").guide.as_deref()),
            ("roc language", Some("https://roc-lang.org/install"))
        );
        assert_eq!(named("gopls").state, LspState::Off);
        assert_eq!(named("rust-analyzer").label, "Rust");
        let slang = named("slang");
        assert_eq!((slang.command.as_str(), slang.guide.is_some()), ("slangd", true));
        assert!(
            slang.install.is_none() && slang.needs.is_empty(),
            "slangd comes from the Slang or Vulkan SDK"
        );
    }

    #[test]
    fn a_missing_server_offers_the_install_whose_package_manager_is_here_else_names_what_it_needs() {
        let typescript = setup::of("typescript").unwrap().server_setup();
        let only = |have: &'static str| move |program: &str| (program == have).then(|| PathBuf::from(have));

        let (program, args) = setup::recipe(&typescript, only("bun")).unwrap();
        assert_eq!(
            command_line(&program, &args),
            "bun add --global typescript-language-server typescript"
        );
        assert_eq!(setup::recipe(&typescript, only("npm")).unwrap().1[0], "install");
        assert!(setup::recipe(&typescript, only("go")).is_none());
        assert_eq!(setup::needs(&typescript), ["npm", "bun"]);
        assert_eq!(setup::needs(&setup::of("gopls").unwrap().server_setup()), ["go"]);
        assert!(
            table::BUILTIN.iter().all(|server| setup::of(server.name).is_some()),
            "every built-in has a label and a guide"
        );
    }

    #[tokio::test]
    async fn a_plugins_server_installs_with_the_plugins_own_command() {
        let program = if cfg!(windows) { "cmd" } else { "true" };
        let args: Vec<String> = if cfg!(windows) {
            vec!["/c".into(), "exit 0".into()]
        } else {
            Vec::new()
        };
        let install = ServerSetup::Install {
            commands: vec![std::iter::once(program.to_owned()).chain(args).collect()],
            guide: "https://example.com".into(),
        };
        let plugins = [plugin_server("roc", "roc_language_server_not_here", install)];
        let servers = Servers::default();

        let status = servers.statuses(&BTreeMap::new(), &plugins, None);
        let roc = status.iter().find(|status| status.name == "roc").unwrap();
        assert_eq!(roc.install.as_deref().map(|line| line.starts_with(program)), Some(true));
        servers.install("roc", &BTreeMap::new(), &plugins).await.unwrap();

        let none = [plugin_server("roc", "x", ServerSetup::None("private".into()))];
        assert!(servers.install("roc", &BTreeMap::new(), &none).await.is_err());
    }
}
