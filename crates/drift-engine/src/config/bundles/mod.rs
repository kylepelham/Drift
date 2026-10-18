//! Bundles: a Claude Code plugin folder (skills, commands, agents, MCP servers and programs) installed from a
//! pinned archive under `~/.config/drift/bundles/<id>`, with one switch for the whole and one for each part.

mod engine;
mod layout;
mod servers;
#[cfg(test)]
mod tests;

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use super::archive::{self, ArchiveError};
pub use layout::{Item, Layout};

const BUNDLES_DIR: &str = "bundles";
const DATA_DIR: &str = "bundle-data";
const MARKER: &str = ".drift-bundle.json";
const STATE_KEY: &str = "bundles";
const MAX_ARCHIVE_BYTES: usize = 256 * 1024 * 1024;

#[derive(Debug, thiserror::Error)]
pub enum BundleError {
    #[error("a bundle id is letters, digits, dashes and underscores")]
    InvalidId,
    #[error("no such bundle")]
    NoBundle,
    #[error("the archive holds nothing under {0}")]
    Empty(String),
    #[error("could not {operation} {}: {source}", path.display())]
    File {
        operation: &'static str,
        path: PathBuf,
        source: std::io::Error,
    },
    #[error(transparent)]
    Archive(#[from] ArchiveError),
    #[error(transparent)]
    Config(#[from] super::plugins::PluginError),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error(transparent)]
    Worker(#[from] tokio::task::JoinError),
    #[error(transparent)]
    Store(#[from] rusqlite::Error),
}

fn file_error(operation: &'static str, path: &Path, source: std::io::Error) -> BundleError {
    BundleError::File {
        operation,
        path: path.to_path_buf(),
        source,
    }
}

/// What a registry entry of kind `bundle` needs to be installed.
#[derive(Clone, Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct InstallBundle {
    /// Becomes the folder name: letters, digits, `-` and `_` only.
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub version: String,
    /// A `.tar.gz` (or Azure DevOps zip) of a pinned commit, over https or through the source.
    pub archive: String,
    /// The plugin's folder inside the archive; empty for its root.
    #[serde(default)]
    pub path: String,
    #[serde(default)]
    pub source: Option<String>,
    #[serde(default)]
    pub image: Option<String>,
    /// What the bundle's MCP servers name as `${NAME}`, as the user typed them; one left out drops the header or env entry using it.
    #[serde(default)]
    pub values: BTreeMap<String, String>,
    /// The registry source it was listed by; its token and trust apply to the archive.
    #[serde(default)]
    pub registry: Option<String>,
}

/// What install wrote, kept beside the files.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Installed {
    id: String,
    name: String,
    version: String,
    #[serde(default)]
    source: Option<String>,
    #[serde(default)]
    image: Option<String>,
    archive: String,
    path: String,
    installed_at: i64,
    /// Each MCP server the bundle declares, by its own name, and the name it was saved under.
    #[serde(default)]
    servers: BTreeMap<String, String>,
}

/// The kinds of part a bundle switches one by one.
#[derive(Clone, Copy, Debug, PartialEq, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum BundlePartKind {
    Skill,
    Command,
    Agent,
    Server,
    /// The programs in `bin/`, on the shell's PATH together.
    Programs,
}

fn part_key(kind: BundlePartKind, name: &str) -> String {
    match kind {
        BundlePartKind::Skill => format!("skill:{name}"),
        BundlePartKind::Command => format!("command:{name}"),
        BundlePartKind::Agent => format!("agent:{name}"),
        BundlePartKind::Server => format!("server:{name}"),
        BundlePartKind::Programs => "programs".into(),
    }
}

/// A bundle's switches: the whole, and the parts turned off inside it. Everything starts on.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct State {
    #[serde(default)]
    off: bool,
    #[serde(default)]
    parts_off: BTreeSet<String>,
}

impl State {
    fn on(&self, kind: BundlePartKind, name: &str) -> bool {
        !self.off && !self.parts_off.contains(&part_key(kind, name))
    }
}

/// One part as the bundle's screen shows it.
#[derive(Clone, Debug, PartialEq, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct BundlePart {
    pub name: String,
    pub description: String,
    /// Its own switch; the bundle's switch still has to be on for it to apply.
    pub enabled: bool,
}

/// An MCP server a bundle brought: its own name and the one Drift saved it under, whose status the MCP list carries.
#[derive(Clone, Debug, PartialEq, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct BundleServer {
    pub name: String,
    pub server: String,
    pub enabled: bool,
}

/// An installed bundle with everything it holds.
#[derive(Clone, Debug, PartialEq, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Bundle {
    pub id: String,
    pub name: String,
    pub version: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub image: Option<String>,
    pub enabled: bool,
    /// Its folder, absolute.
    pub root: String,
    pub skills: Vec<BundlePart>,
    pub commands: Vec<BundlePart>,
    pub agents: Vec<BundlePart>,
    pub servers: Vec<BundleServer>,
    /// The programs in `bin/`, put on the shell's PATH while `programs_enabled`.
    pub programs: Vec<String>,
    pub programs_enabled: bool,
    /// The events its hooks run on in Claude Code; Drift never runs them.
    pub hooks: Vec<String>,
    pub installed_at: i64,
}

fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '-' || ch == '_')
}

fn bundles_dir() -> Result<PathBuf, BundleError> {
    Ok(super::plugins::config_dir()?.join(BUNDLES_DIR))
}

fn data_dir(id: &str) -> Result<PathBuf, BundleError> {
    Ok(super::plugins::config_dir()?.join(DATA_DIR).join(id))
}

/// A path as `${CLAUDE_PLUGIN_ROOT}` gives it: forward slashes, so a shell never reads a backslash as an escape.
fn slashed(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

/// Fetches the archive and unpacks the plugin's folder as `bundles/<id>`, replacing what was there.
async fn install(
    fetcher: &super::sources::Fetcher,
    source: Option<&super::sources::RegistrySource>,
    bundle: &InstallBundle,
) -> Result<Installed, BundleError> {
    if !valid_id(&bundle.id) {
        return Err(BundleError::InvalidId);
    }

    let bytes = archive::fetch(fetcher, source, &bundle.archive, MAX_ARCHIVE_BYTES).await?;
    let into = bundles_dir()?.join(&bundle.id);
    let folder = PathBuf::from(bundle.path.trim_matches('/'));
    let written = tokio::task::spawn_blocking(move || unpack(&bytes, &into, &folder)).await??;
    if written == 0 {
        return Err(BundleError::Empty(format!("{}/", bundle.path.trim_matches('/'))));
    }

    Ok(Installed {
        id: bundle.id.clone(),
        name: bundle.name.clone(),
        version: bundle.version.clone(),
        source: bundle.source.clone(),
        image: bundle.image.clone(),
        archive: bundle.archive.clone(),
        path: bundle.path.clone(),
        installed_at: crate::id::now_ms(),
        servers: BTreeMap::new(),
    })
}

/// Every file under `folder` (all of them when it is empty), moved up so `folder` becomes `into`.
fn unpack(bytes: &[u8], into: &Path, folder: &Path) -> Result<usize, ArchiveError> {
    let keep = |relative: &Path| {
        let inner = relative.strip_prefix(folder).ok()?;
        (!inner.as_os_str().is_empty()).then(|| inner.to_path_buf())
    };
    let plan = archive::Unpack {
        into,
        keep: &keep,
        max_file: None,
    };

    let staged = archive::unpack(bytes, &plan)?;
    let count = staged.paths.len();
    // An archive with nothing under the folder leaves what was installed before as it was.
    if count > 0 {
        staged.commit(into)?;
    }

    Ok(count)
}

fn write_marker(installed: &Installed) -> Result<(), BundleError> {
    let file = bundles_dir()?.join(&installed.id).join(MARKER);
    std::fs::write(&file, serde_json::to_string_pretty(installed)?).map_err(|error| file_error("write", &file, error))
}

/// Every bundle installed, by its marker, in name order.
fn installed() -> Vec<Installed> {
    let Ok(dir) = bundles_dir() else { return Vec::new() };
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };

    let mut found: Vec<Installed> = entries
        .flatten()
        .filter_map(|entry| std::fs::read_to_string(entry.path().join(MARKER)).ok())
        .filter_map(|text| serde_json::from_str(&text).ok())
        .collect();
    found.sort_by(|left, right| left.name.cmp(&right.name));

    found
}

fn find(id: &str) -> Result<Installed, BundleError> {
    installed()
        .into_iter()
        .find(|bundle| bundle.id == id)
        .ok_or(BundleError::NoBundle)
}

/// The bundle's files and its data folder.
fn remove_files(id: &str) -> Result<(), BundleError> {
    if !valid_id(id) {
        return Err(BundleError::NoBundle);
    }
    let dir = bundles_dir()?.join(id);
    std::fs::remove_dir_all(&dir).map_err(|error| file_error("remove", &dir, error))?;
    let _ = std::fs::remove_dir_all(data_dir(id)?);

    Ok(())
}

fn view(installed: &Installed, state: &State) -> Result<Bundle, BundleError> {
    let root = bundles_dir()?.join(&installed.id);
    let layout = Layout::read(&root);
    let parts = |items: &[Item], kind| {
        items
            .iter()
            .map(|item| BundlePart {
                name: item.name.clone(),
                description: item.description.clone(),
                enabled: !state.parts_off.contains(&part_key(kind, &item.name)),
            })
            .collect()
    };
    let servers = installed
        .servers
        .iter()
        .map(|(name, server)| BundleServer {
            name: name.clone(),
            server: server.clone(),
            enabled: !state.parts_off.contains(&part_key(BundlePartKind::Server, name)),
        })
        .collect();

    Ok(Bundle {
        id: installed.id.clone(),
        name: installed.name.clone(),
        version: installed.version.clone(),
        source: installed.source.clone(),
        image: installed.image.clone(),
        enabled: !state.off,
        root: root.to_string_lossy().into_owned(),
        skills: parts(&layout.skills, BundlePartKind::Skill),
        commands: parts(&layout.commands, BundlePartKind::Command),
        agents: parts(&layout.agents, BundlePartKind::Agent),
        servers,
        programs: layout.programs,
        programs_enabled: !state.parts_off.contains(&part_key(BundlePartKind::Programs, "")),
        hooks: layout.hooks,
        installed_at: installed.installed_at,
    })
}

/// A bundle that is on, as a workspace's config reads it: its parts that are on, with `${CLAUDE_PLUGIN_ROOT}`
/// and `${CLAUDE_PLUGIN_DATA}` in their text pointing at its folders.
pub struct Active {
    root: PathBuf,
    data: PathBuf,
    layout: Layout,
    state: State,
}

impl Active {
    fn on<'a>(&'a self, items: &'a [Item], kind: BundlePartKind) -> impl Iterator<Item = &'a Item> {
        items.iter().filter(move |item| self.state.on(kind, &item.name))
    }

    pub fn skills(&self) -> impl Iterator<Item = &Item> {
        self.on(&self.layout.skills, BundlePartKind::Skill)
    }

    pub fn commands(&self) -> impl Iterator<Item = &Item> {
        self.on(&self.layout.commands, BundlePartKind::Command)
    }

    pub fn agents(&self) -> impl Iterator<Item = &Item> {
        self.on(&self.layout.agents, BundlePartKind::Agent)
    }

    /// The folder to put on the shell's PATH, while the bundle has programs and they are on.
    pub fn programs(&self) -> Option<PathBuf> {
        let on = !self.layout.programs.is_empty() && self.state.on(BundlePartKind::Programs, "");
        on.then(|| self.root.join("bin"))
    }

    /// A file of the bundle's with its own paths filled in.
    pub fn text(&self, file: &Path) -> Option<String> {
        let text = std::fs::read_to_string(file).ok()?;

        Some(
            text.replace("${CLAUDE_PLUGIN_ROOT}", &slashed(&self.root))
                .replace("${CLAUDE_PLUGIN_DATA}", &slashed(&self.data)),
        )
    }
}
