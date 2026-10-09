use super::{Decision, ReplyBody, Request, Rule};
use crate::tool::Ask;
use serde::{Deserialize, Serialize};
use std::path::Path;
use utoipa::ToSchema;

/// What the user approved with "always": kept for the workspace, across sessions and restarts.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(tag = "grant", rename_all = "snake_case")]
pub enum Grant {
    /// This path, command or target, taken literally: `[id].tsx` is a file name, not a glob.
    Exact { kind: String, target: String },
    /// A known subcommand with any arguments: `cargo test` covers `cargo test --release`, not `cargo publish`.
    Subcommand { prefix: String },
    /// Everything under a folder outside the workspace, as opencode's "always" for an external directory.
    Folder { kind: String, folder: String },
    /// A pattern the client supplied on purpose.
    Pattern(Rule),
}

impl Grant {
    pub(super) fn allows(&self, kind: &str, target: &str) -> bool {
        match self {
            Self::Exact {
                kind: granted,
                target: exact,
            } => granted == kind && exact == target,
            Self::Subcommand { prefix } => {
                kind == "bash"
                    && (target == prefix
                        || target
                            .strip_prefix(prefix.as_str())
                            .is_some_and(|rest| rest.starts_with(' ')))
            }
            Self::Folder { kind: granted, folder } => granted == kind && Path::new(target).starts_with(folder),
            Self::Pattern(rule) => rule.matches_target(kind, target),
        }
    }
}

/// What an "always" answer grants: the pattern the client named, or what `always_grants` reads from the ask.
pub(super) fn grants_for(request: &Request, reply: &ReplyBody) -> Vec<Grant> {
    match &reply.pattern {
        Some(pattern) => vec![Grant::Pattern(Rule {
            kind: request.ask.kind.clone(),
            pattern: pattern.clone(),
            decision: Decision::Allow,
        })],
        None => always_grants(&request.ask),
    }
}

/// What "always" covers, as opencode widens it: each command of a shell line on its own, widened only
/// to a known subcommand; a fetch's whole site; a path outside the workspace's whole folder (the
/// folder itself for a search of one). The exact target for everything else: a file that may hold
/// secrets, a guarded file inside the workspace, a shell line that hides what it runs or writes a
/// file through a redirection.
pub(super) fn always_grants(ask: &Ask) -> Vec<Grant> {
    let exact = |target: &str| Grant::Exact {
        kind: ask.kind.clone(),
        target: target.into(),
    };

    match (&ask.commands, ask.kind.as_str()) {
        (Some(commands), "bash") if ask.writes.is_empty() => commands
            .iter()
            .map(|command| match crate::tool::command::subcommand(command) {
                Some(prefix) => Grant::Subcommand { prefix },
                None => exact(command),
            })
            .collect(),
        (_, "webfetch") => vec![site(&ask.pattern).unwrap_or_else(|| exact(&ask.pattern))],
        (_, "read" | "edit") => {
            vec![outside_folder(ask, crate::config::home().as_deref()).unwrap_or_else(|| exact(&ask.pattern))]
        }
        _ => vec![exact(&ask.pattern)],
    }
}

/// Every page of the URL's site: its scheme, host and port.
fn site(url: &str) -> Option<Grant> {
    let origin = reqwest::Url::parse(url).ok()?.origin();

    origin.is_tuple().then(|| {
        Grant::Pattern(Rule {
            kind: "webfetch".into(),
            pattern: format!("{}/*", origin.ascii_serialization()),
            decision: Decision::Allow,
        })
    })
}

/// The folder of a path outside the workspace (`relative` is set only inside it), unless the path may
/// hold secrets or the folder would grant far too much: a drive root, the home folder or any folder
/// holding it (`/home` holds every user's), or where tools keep their sign-ins in home and below:
/// its hidden folders (`~/.ssh`, `~/.aws`), Windows' `AppData` and macOS's `Library`.
pub(super) fn outside_folder(ask: &Ask, home: Option<&Path>) -> Option<Grant> {
    let path = Path::new(&ask.pattern);
    if ask.relative.is_some() || !path.is_absolute() || crate::tool::sensitive::is_sensitive(path) {
        return None;
    }

    let folder = if path.is_dir() { path } else { path.parent()? };
    let home = home.map(crate::tool::canonical);
    let too_wide = folder.parent().is_none()
        || home.is_some_and(|home| home.starts_with(folder) || in_sign_in_folder(&home, folder));

    (!too_wide).then(|| Grant::Folder {
        kind: ask.kind.clone(),
        folder: folder.to_string_lossy().into_owned(),
    })
}

/// Whether `folder` is, or is inside, a folder of `home` where tools keep their sign-ins.
fn in_sign_in_folder(home: &Path, folder: &Path) -> bool {
    let first = folder
        .strip_prefix(home)
        .ok()
        .and_then(|relative| relative.components().next());

    first.is_some_and(|first| {
        let name = first.as_os_str().to_string_lossy();
        name.starts_with('.') || name.eq_ignore_ascii_case("AppData") || name.eq_ignore_ascii_case("Library")
    })
}
