use super::command;
use serde::{Deserialize, Serialize};
use std::path::Path;
use utoipa::ToSchema;

/// The most of a proposed change an approval shows; the rest is said to be cut.
pub const MAX_ASK_DIFF: usize = 64 * 1024;

/// What a call wants to do, for the permission service to judge before it runs.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Ask {
    /// `read`, `edit`, `bash`, ...: the rule namespace.
    pub kind: String,
    /// The thing being touched: a path, a command. Rules match it with globs.
    pub pattern: String,
    pub title: String,
    /// For a shell command, the simple commands it runs, each judged on its own; `None` when the line
    /// hides what it runs, so only an exact approval of the whole line allows it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub commands: Option<Vec<String>>,
    /// Files the shell line's redirections write. Any at all and only an exact approval of the whole
    /// line allows it: approving `git status` never approves `git status > victim.txt`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub writes: Vec<String>,
    /// `commands` as deny rules also see them (assignments dropped, aliases spelt out), one for one.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub canonical: Vec<String>,
    /// A path inside the workspace, relative with `/`, so a committed rule such as `src/**` matches it too.
    #[serde(skip)]
    pub relative: Option<String>,
    /// Proposed diff for review, excluded from permission rule and approval matching.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub diff: Option<String>,
    /// Default decision when no explicit rule or approval matches; never skips policy evaluation.
    #[serde(skip)]
    pub default_allow: bool,
    /// Why a shell line would not run unasked, for the approval to say (auto-accept never answers it).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<Reason>,
}

/// Why a shell line asks when no rule says to.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub enum Reason {
    /// A path it names resolves outside the workspace.
    Outside,
    /// A variable or `~` whose value is not known until it runs.
    Unresolved,
    /// A file that may hold secrets.
    Secret,
    /// A recursive content search, which would read secret files too.
    Searches,
    /// `git clean`, `git reset --hard` or `git push`, which undo cannot put back.
    BeyondUndo,
    /// A change of directory out of the workspace, or one that cannot be read.
    Moves,
    /// Substitution, a subshell or a launcher, which hides what it runs.
    Hidden,
}

impl Ask {
    pub fn new(kind: &str, pattern: impl Into<String>, title: impl Into<String>) -> Self {
        Self {
            kind: kind.into(),
            pattern: pattern.into(),
            title: title.into(),
            commands: None,
            writes: Vec::new(),
            canonical: Vec::new(),
            relative: None,
            diff: None,
            default_allow: false,
            reason: None,
        }
    }

    pub fn allow_by_default(mut self) -> Self {
        self.default_allow = true;
        self
    }

    /// The ask with the change it would make, cut to [`MAX_ASK_DIFF`] bytes at a line.
    pub fn with_diff(mut self, diff: Option<String>) -> Self {
        self.diff = diff.filter(|diff| !diff.is_empty()).map(|diff| clip_diff(&diff));
        self
    }

    /// An ask about a file: the absolute path, and the workspace-relative one when it is inside.
    pub fn path(kind: &str, path: &Path, workspace: &Path, title: impl Into<String>) -> Self {
        let relative = path
            .strip_prefix(workspace)
            .ok()
            .filter(|relative| !relative.as_os_str().is_empty())
            .map(|relative| relative.to_string_lossy().replace('\\', "/"));

        Self {
            relative,
            ..Self::new(kind, path.to_string_lossy(), title)
        }
    }

    /// What rules and approvals are matched against: the pattern, then the relative path if there is one.
    pub fn targets(&self) -> Vec<&str> {
        std::iter::once(self.pattern.as_str())
            .chain(self.relative.as_deref())
            .collect()
    }

    /// A shell ask as the dialect reads `line`.
    pub fn shell(dialect: command::Dialect, line: &str, title: impl Into<String>) -> Self {
        let mut ask = Self::new("bash", line, title);
        if let Some(split) = command::split(dialect, line) {
            ask.commands = Some(split.commands);
            ask.canonical = split.canonical;
            ask.writes = split.writes;
        }

        ask
    }

    /// Keeps only the commands `keep` says, their canonical forms with them.
    pub fn retain_commands(&mut self, mut keep: impl FnMut(&str) -> bool) {
        let Some(commands) = self.commands.take() else { return };
        let canonical = std::mem::take(&mut self.canonical);
        let pairs: Vec<(String, String)> = commands
            .into_iter()
            .zip(canonical.into_iter().chain(std::iter::repeat(String::new())))
            .filter(|(command, _)| keep(command))
            .collect();

        self.canonical = pairs.iter().map(|(_, canonical)| canonical.clone()).collect();
        self.commands = Some(pairs.into_iter().map(|(command, _)| command).collect());
    }
}

fn clip_diff(diff: &str) -> String {
    if diff.len() <= MAX_ASK_DIFF {
        return diff.to_string();
    }

    let boundary = diff.floor_char_boundary(MAX_ASK_DIFF);
    let cut = diff[..boundary].rfind('\n').unwrap_or(0);
    format!("{}\n... (the rest of the change is not shown)", &diff[..cut])
}
