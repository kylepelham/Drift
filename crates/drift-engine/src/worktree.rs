//! Git worktrees a thread works in: branch `drift/<name>` checked out under
//! `<data>/worktrees/<workspace id>/<name>`, never inside the repository. Drift never merges them back.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::Stdio;

use crate::Engine;
use crate::event::Event;
use crate::session::types::Session;
use crate::store::Workspace;
use crate::tool::canonical;

pub const BRANCH_PREFIX: &str = "drift/";
/// The tools offered only in a workspace inside a git repository.
pub const TOOLS: [&str; 4] = ["worktree_create", "worktree_list", "worktree_remove", "session_move"];
const NAME_LIMIT: usize = 64;

#[derive(Debug, PartialEq)]
pub enum Error {
    NotRepository,
    /// Why the name cannot be a worktree's.
    Name(String),
    Exists(String),
    Missing(String),
    Dirty {
        name: String,
        changes: usize,
    },
    /// A running conversation works in it; carries its title.
    InUse(String),
    Git(String),
    Store(String),
}

impl std::fmt::Display for Error {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotRepository => write!(formatter, "this workspace is not inside a git repository"),
            Self::Name(why) => write!(formatter, "{why}"),
            Self::Exists(name) => write!(formatter, "a worktree named `{name}` already exists; pick another name"),
            Self::Missing(name) => write!(
                formatter,
                "this workspace has no worktree named `{name}`; worktree_list shows the ones it has"
            ),
            Self::Dirty { name, changes } => write!(
                formatter,
                "worktree `{name}` has {changes} uncommitted change(s), so it was not removed; commit them there, \
                 or pass `force: true` to drop them (the user is asked first)"
            ),
            Self::InUse(title) => write!(
                formatter,
                "the conversation \"{title}\" is running in that worktree; it can be removed once that ends, \
                 and this session must move out of it first with session_move"
            ),
            Self::Git(message) => write!(formatter, "git: {message}"),
            Self::Store(message) => write!(formatter, "store: {message}"),
        }
    }
}

impl std::error::Error for Error {}

impl From<rusqlite::Error> for Error {
    fn from(error: rusqlite::Error) -> Self {
        Self::Store(error.to_string())
    }
}

/// A worktree just made.
pub struct Created {
    /// Where a session in it works: the worktree, at the workspace's place in the repository.
    pub directory: PathBuf,
    pub branch: String,
    pub commit: String,
    /// Uncommitted changes where it was made from, which it does not have.
    pub left_behind: usize,
}

/// One of the workspace's worktrees as git lists it.
pub struct Listed {
    pub name: String,
    pub path: PathBuf,
    pub branch: Option<String>,
    pub changes: usize,
    /// Its branch is already in the workspace's current commit.
    pub merged: bool,
}

pub struct Removed {
    pub branch: Option<String>,
    /// The branch went too, because it was merged; an unmerged one is always kept.
    pub branch_deleted: bool,
    /// Sessions that worked there and now work in the workspace folder.
    pub moved: Vec<String>,
}

/// A worktree name is one path segment and a valid branch name after `drift/`.
pub fn check_name(name: &str) -> Result<(), Error> {
    let allowed = |character: char| character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.');
    let problem = if name.is_empty() || name.len() > NAME_LIMIT {
        Some(format!("a worktree name has 1 to {NAME_LIMIT} characters"))
    } else if !name.chars().all(allowed) {
        Some("a worktree name uses only letters, digits, `-`, `_` and `.`".to_string())
    } else if name.starts_with(['.', '-']) || name.ends_with('.') || name.contains("..") || name.ends_with(".lock") {
        Some("a worktree name cannot start with `.` or `-`, end with `.` or `.lock`, or hold `..`".to_string())
    } else {
        None
    };

    problem.map_or(Ok(()), |why| Err(Error::Name(why)))
}

/// A name for a worktree a prompt asked for: its first words, then a few random characters.
pub fn name_for(text: &str) -> String {
    let words: Vec<String> = text
        .split(|character: char| !character.is_ascii_alphanumeric())
        .filter(|word| !word.is_empty())
        .take(4)
        .map(str::to_ascii_lowercase)
        .collect();
    let stem = if words.is_empty() {
        "thread".to_string()
    } else {
        words.join("-")
    };
    let stem: String = stem.chars().take(40).collect();

    format!("{}-{}", stem.trim_end_matches('-'), crate::random_hex(2))
}

/// The snapshot owner of a worktree's undo history: its workspace and its name.
pub fn owner(workspace_id: &str, name: &str) -> String {
    format!("{workspace_id}/{name}")
}

pub fn parse_owner(owner: &str) -> Option<(&str, &str)> {
    owner.split_once('/')
}

/// The repository top above `directory`, found as [`crate::config::in_repository`] finds one.
fn repository_top(directory: &Path) -> Option<&Path> {
    directory.ancestors().find(|dir| dir.join(".git").exists())
}

impl Engine {
    fn worktrees_of(&self, workspace_id: &str) -> PathBuf {
        self.data_dir.join("worktrees").join(workspace_id)
    }

    /// The checkout of worktree `name`, at the repository's top.
    pub fn worktree_path(&self, workspace_id: &str, name: &str) -> PathBuf {
        self.worktrees_of(workspace_id).join(name)
    }

    /// Where a session in worktree `name` works: the workspace's place in the repository, inside the
    /// worktree. `None` once the worktree is gone.
    pub fn worktree_directory(&self, workspace: &Workspace, name: &str) -> Option<PathBuf> {
        let root = canonical(Path::new(&workspace.path));
        let inside = root.strip_prefix(repository_top(&root)?).ok()?;
        let directory = self.worktree_path(&workspace.id, name).join(inside);

        directory.is_dir().then(|| canonical(&directory))
    }

    /// The folder a session works in and the owner of its undo history: its worktree's while that exists,
    /// else its workspace's.
    pub fn working_place(&self, session: &Session, workspace: &Workspace) -> (PathBuf, String) {
        let worktree = session.worktree.as_deref().and_then(|name| {
            self.worktree_directory(workspace, name)
                .map(|directory| (directory, owner(&workspace.id, name)))
        });

        worktree.unwrap_or_else(|| (canonical(Path::new(&workspace.path)), workspace.id.clone()))
    }

    /// Makes branch `drift/<name>` from the commit `from` is on, checked out in a new worktree.
    pub async fn create_worktree(&self, workspace: &Workspace, from: &Path, name: &str) -> Result<Created, Error> {
        check_name(name)?;
        repository_top(from).ok_or(Error::NotRepository)?;
        let path = self.worktree_path(&workspace.id, name);
        if path.exists() {
            return Err(Error::Exists(name.into()));
        }
        std::fs::create_dir_all(self.worktrees_of(&workspace.id)).map_err(|error| Error::Git(error.to_string()))?;

        let branch = format!("{BRANCH_PREFIX}{name}");
        let add = [
            OsStr::new("worktree"),
            OsStr::new("add"),
            OsStr::new("-b"),
            OsStr::new(&branch),
            path.as_os_str(),
            OsStr::new("HEAD"),
        ];
        git(from, add).await?;
        let commit = git(&path, ["rev-parse", "--short", "HEAD"]).await?;
        let directory = self
            .worktree_directory(workspace, name)
            .unwrap_or_else(|| canonical(&path));

        Ok(Created {
            directory,
            branch,
            commit,
            left_behind: changes(from).await,
        })
    }

    /// The workspace's worktrees that Drift made, as git lists them.
    pub async fn worktrees(&self, workspace: &Workspace) -> Result<Vec<Listed>, Error> {
        let root = canonical(Path::new(&workspace.path));
        repository_top(&root).ok_or(Error::NotRepository)?;
        let ours = canonical(&self.worktrees_of(&workspace.id));
        let listing = git(&root, ["worktree", "list", "--porcelain"]).await?;

        let mut found = Vec::new();
        for (path, branch) in parse_listing(&listing) {
            let path = canonical(Path::new(&path));
            let Some(name) = path.strip_prefix(&ours).ok().and_then(single_segment) else {
                continue;
            };
            let merged = match &branch {
                Some(branch) => git(&root, ["merge-base", "--is-ancestor", branch, "HEAD"])
                    .await
                    .is_ok(),
                None => false,
            };
            found.push(Listed {
                name,
                changes: changes(&path).await,
                path,
                branch,
                merged,
            });
        }

        Ok(found)
    }

    /// Removes worktree `name`; uncommitted changes refuse unless `force`. Its branch is deleted only when merged.
    pub async fn remove_worktree(&self, workspace: &Workspace, name: &str, force: bool) -> Result<Removed, Error> {
        let listed = self
            .worktrees(workspace)
            .await?
            .into_iter()
            .find(|listed| listed.name == name)
            .ok_or_else(|| Error::Missing(name.into()))?;
        let sessions = self.store.sessions_in_worktree(&workspace.id, name)?;
        if let Some(running) = sessions.iter().find(|session| self.turns.is_running(&session.id)) {
            return Err(Error::InUse(running.title.clone()));
        }
        if listed.changes > 0 && !force {
            return Err(Error::Dirty {
                name: name.into(),
                changes: listed.changes,
            });
        }

        let root = canonical(Path::new(&workspace.path));
        let mut remove = vec![OsStr::new("worktree"), OsStr::new("remove")];
        if force {
            remove.push(OsStr::new("--force"));
        }
        remove.push(listed.path.as_os_str());
        git(&root, remove).await?;
        let branch_deleted = match &listed.branch {
            Some(branch) => git(&root, ["branch", "-d", branch]).await.is_ok(),
            None => false,
        };

        let moved = self.move_out(&sessions);
        self.snapshots.forget(&owner(&workspace.id, name));

        Ok(Removed {
            branch: listed.branch,
            branch_deleted,
            moved,
        })
    }

    /// Sessions of a removed worktree work in the workspace folder from now on.
    fn move_out(&self, sessions: &[Session]) -> Vec<String> {
        let mut moved = Vec::new();
        for session in sessions {
            if let Ok(Some(session)) = self.store.set_session_worktree(&session.id, None) {
                moved.push(session.id.clone());
                self.hub.publish(Event::SessionUpdated { session });
            }
        }

        moved
    }

    /// After a purge: the worktree goes when no conversation is left in it, nothing is uncommitted and its
    /// branch is merged. Anything else stays for the user, listed by `worktree_list`.
    pub async fn retire_worktree(&self, workspace_id: &str, name: &str) -> bool {
        let Ok(Some(workspace)) = self.store.workspace(workspace_id) else {
            return false;
        };
        if !self
            .store
            .sessions_in_worktree(workspace_id, name)
            .is_ok_and(|left| left.is_empty())
        {
            return false;
        }
        let Ok(listed) = self.worktrees(&workspace).await else {
            return false;
        };
        let done = listed
            .iter()
            .any(|listed| listed.name == name && listed.merged && listed.changes == 0);

        done && self.remove_worktree(&workspace, name, false).await.is_ok()
    }
}

/// Each worktree's path and branch from `git worktree list --porcelain`; a detached one has no branch.
fn parse_listing(listing: &str) -> Vec<(String, Option<String>)> {
    listing
        .split("\n\n")
        .filter_map(|entry| {
            let path = entry.lines().find_map(|line| line.strip_prefix("worktree "))?;
            let branch = entry
                .lines()
                .find_map(|line| line.strip_prefix("branch refs/heads/"))
                .map(String::from);
            Some((path.to_string(), branch))
        })
        .collect()
}

fn single_segment(relative: &Path) -> Option<String> {
    let mut parts = relative.components();
    match (parts.next(), parts.next()) {
        (Some(std::path::Component::Normal(name)), None) => Some(name.to_string_lossy().into_owned()),
        _ => None,
    }
}

/// Uncommitted changes in a checkout, untracked files included; none when git cannot say.
async fn changes(directory: &Path) -> usize {
    git(directory, ["status", "--porcelain"])
        .await
        .map_or(0, |status| status.lines().filter(|line| !line.is_empty()).count())
}

/// Git in a repository's own checkout, with what it printed to stderr when it fails.
async fn git<I, S>(directory: &Path, args: I) -> Result<String, Error>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let mut command = tokio::process::Command::new("git");
    command
        .current_dir(directory)
        .args(args)
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    #[cfg(windows)]
    command.creation_flags(0x0800_0000);

    let output = command
        .output()
        .await
        .map_err(|_| Error::Git("git is not installed".into()))?;
    if !output.status.success() {
        return Err(Error::Git(String::from_utf8_lossy(&output.stderr).trim().to_string()));
    }

    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

#[cfg(test)]
mod tests;
