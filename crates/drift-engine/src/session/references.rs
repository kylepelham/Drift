//! References at turn time: where each one's files are, cloning and refreshing the user's repositories in
//! the background, and the list the instruction baseline carries.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, SystemTime};

use crate::Engine;
use crate::config::references::{Place, Reference, repository_folder};

/// A repository reference is fetched again at most this often, and only when a turn uses its config.
const REFRESH: Duration = Duration::from_secs(24 * 60 * 60);
const GIT_LIMIT: Duration = Duration::from_secs(300);

/// The clones and fetches running now, by folder, so a busy session never starts a second.
#[derive(Default)]
pub(crate) struct Fetches(pub(crate) Mutex<HashSet<PathBuf>>);

/// One reference as the baseline lists it.
pub struct Listed {
    pub name: String,
    pub description: String,
    pub path: PathBuf,
    /// Why the model cannot simply read there, when it cannot.
    pub note: Option<&'static str>,
}

impl Engine {
    /// Where a reference's files are: its folder, or the clone of its repository.
    pub(crate) fn reference_path(&self, reference: &Reference) -> PathBuf {
        match &reference.place {
            Place::Folder { path } => crate::tool::canonical(path),
            Place::Repository { url, branch } => crate::tool::canonical(&repository_folder(
                &self.data_dir,
                &reference.name,
                url,
                branch.as_deref(),
            )),
        }
    }

    /// The references a turn lists, starting a clone or a daily fetch for the user's own repositories.
    pub(crate) fn references_for_turn(&self, references: &[Reference]) -> Vec<Listed> {
        references
            .iter()
            .map(|reference| {
                let path = self.reference_path(reference);
                let note = match (&reference.place, reference.project) {
                    (Place::Repository { .. }, true) => {
                        Some("a repository this project names; Drift fetches only the ones your own drift.json names")
                    }
                    (Place::Repository { url, branch }, false) => self.refresh(&path, url, branch.as_deref()),
                    (Place::Folder { .. }, true) => Some("named by this project, so reading there asks first"),
                    (Place::Folder { .. }, false) => (!path.is_dir()).then_some("missing"),
                };
                Listed {
                    name: reference.name.clone(),
                    description: reference.description.clone(),
                    path,
                    note,
                }
            })
            .collect()
    }

    /// Starts a clone when the folder is missing, a fetch when it is a day old; says so while cloning.
    fn refresh(&self, folder: &Path, url: &str, branch: Option<&str>) -> Option<&'static str> {
        let cloned = folder.join(".git").is_dir();
        let fresh = cloned && fetched_within(folder, REFRESH);
        if fresh || !self.reference_fetches.0.lock().unwrap().insert(folder.to_path_buf()) {
            return (!cloned).then_some("still being cloned; look again in a moment");
        }

        let me = self.me.clone();
        let (folder, url, branch) = (folder.to_path_buf(), url.to_string(), branch.map(str::to_string));
        tokio::spawn(async move {
            let outcome = tokio::time::timeout(GIT_LIMIT, update(&folder, &url, branch.as_deref(), cloned)).await;
            if let Ok(Err(error)) | Err(error) = outcome.map_err(|_| "timed out".to_string()) {
                eprintln!("drift: reference {url} could not be updated: {error}");
            }
            if let Some(engine) = me.upgrade() {
                engine.reference_fetches.0.lock().unwrap().remove(&folder);
            }
        });

        (!cloned).then_some("still being cloned; look again in a moment")
    }
}

/// Whether the clone was made or fetched within `within`, by its `FETCH_HEAD` or else its `HEAD`.
fn fetched_within(folder: &Path, within: Duration) -> bool {
    let git = folder.join(".git");
    let touched = ["FETCH_HEAD", "HEAD"]
        .iter()
        .find_map(|name| std::fs::metadata(git.join(name)).and_then(|meta| meta.modified()).ok());

    touched
        .and_then(|time| SystemTime::now().duration_since(time).ok())
        .is_some_and(|age| age < within)
}

/// A shallow clone, or a shallow fetch of the branch moved onto; the clone is Drift's own, never edited.
async fn update(folder: &Path, url: &str, branch: Option<&str>, cloned: bool) -> Result<(), String> {
    if !cloned {
        let parent = folder.parent().unwrap_or(folder);
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|error| error.to_string())?;
        let mut args = vec!["clone", "--depth", "1", "--quiet"];
        if let Some(branch) = branch {
            args.extend(["--branch", branch]);
        }
        let folder = folder.to_string_lossy();
        args.extend(["--", url, &folder]);
        return git(parent, &args).await;
    }

    git(
        folder,
        &["fetch", "--depth", "1", "--quiet", "origin", branch.unwrap_or("HEAD")],
    )
    .await?;
    git(folder, &["reset", "--hard", "--quiet", "FETCH_HEAD"]).await
}

async fn git(directory: &Path, args: &[&str]) -> Result<(), String> {
    let mut command = tokio::process::Command::new("git");
    command
        .current_dir(directory)
        .args(args)
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(std::process::Stdio::null())
        .kill_on_drop(true);
    #[cfg(windows)]
    command.creation_flags(0x0800_0000);
    let output = command.output().await.map_err(|error| error.to_string())?;

    if output.status.success() {
        Ok(())
    } else {
        Err(String::from_utf8_lossy(&output.stderr).trim().to_string())
    }
}
