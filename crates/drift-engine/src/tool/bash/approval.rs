use crate::tool::{Ask, Context, Reason, ToolError, canonical, command, sensitive};
use serde_json::Value;
use std::path::{Path, PathBuf};

const MOVES: [&str; 6] = ["cd", "chdir", "set-location", "sl", "pushd", "push-location"];
/// Readers of file contents under a whole directory: they would read `.env` and its kin too, which the
/// `grep` tool skips, so a line using one always asks.
const SEARCHERS: [&str; 3] = ["grep", "rg", "select-string"];

enum Move {
    Inside(PathBuf),
    Elsewhere,
    None,
}

/// Why a line cannot run unasked, or `None` when it stays inside the workspace and names no file
/// that may hold secrets: no move is left (one leaving the workspace, or one that cannot be read,
/// stays in the ask), no recursive content search, nothing undo cannot put back, no variable, and
/// every argument that names a path (a glob by the folder it starts from) resolves inside the
/// workspace. `--flag=value`, `rev:path` and redirections are judged by their path parts.
pub(super) fn why_it_asks(context: &Context, directory: &Path, ask: &Ask) -> Option<Reason> {
    let Some(commands) = &ask.commands else {
        return Some(Reason::Hidden);
    };
    let mut reasons = commands
        .iter()
        .enumerate()
        .filter_map(|(index, command)| command_reason(context, directory, command, ask.canonical.get(index)));

    reasons.next().or_else(|| {
        ask.writes
            .iter()
            .find_map(|target| target_reason(context, directory, target))
    })
}

/// The program and its subcommand are read as the command runs (`FOO=1 git push` is a push, `sls`
/// is `Select-String`); every written word but the program is judged as a path, assignments too.
fn command_reason(context: &Context, directory: &Path, command: &str, canonical: Option<&String>) -> Option<Reason> {
    let written: Vec<&str> = command.split(' ').collect();
    let runs: Vec<&str> = canonical
        .filter(|canonical| !canonical.is_empty())
        .map_or_else(|| written.clone(), |canonical| canonical.split(' ').collect());
    let program_at = written.len().saturating_sub(runs.len());
    let program = runs[0]
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(runs[0])
        .trim_end_matches(".exe")
        .to_ascii_lowercase();
    let git = (program == "git").then(|| git_subcommand(&runs[1..])).flatten();

    if MOVES.contains(&program.as_str()) {
        return Some(Reason::Moves);
    }
    if SEARCHERS.contains(&program.as_str()) || git == Some("grep") {
        return Some(Reason::Searches);
    }
    if git.is_some_and(|subcommand| beyond_undo(subcommand, &runs)) {
        return Some(Reason::BeyondUndo);
    }

    written
        .iter()
        .enumerate()
        .filter(|(index, _)| *index != program_at)
        .find_map(|(_, word)| word_reason(context, directory, word))
}

/// Git's subcommand, past its global options (`git -C sub push` is a push).
fn git_subcommand<'a>(arguments: &[&'a str]) -> Option<&'a str> {
    let mut arguments = arguments.iter();
    while let Some(argument) = arguments.next() {
        match *argument {
            "-C" | "-c" | "--git-dir" | "--work-tree" | "--namespace" => drop(arguments.next()),
            option if option.starts_with('-') => {}
            subcommand => return Some(subcommand),
        }
    }

    None
}

/// What undo cannot put back, so it asks however inside the workspace it stays: `git clean` deletes
/// ignored and untracked files undo never kept, `git reset --hard` moves refs, a push changes a remote.
fn beyond_undo(subcommand: &str, words: &[&str]) -> bool {
    matches!(subcommand, "clean" | "push") || (subcommand == "reset" && words.contains(&"--hard"))
}

/// A redirection word is judged by the file it names, so `>~/.bashrc` is judged as `~/.bashrc`.
fn word_reason(context: &Context, directory: &Path, word: &str) -> Option<Reason> {
    match command::redirect_target(word) {
        Some(target) => target_reason(context, directory, target),
        None => path_reason(context, directory, word),
    }
}

fn target_reason(context: &Context, directory: &Path, target: &str) -> Option<Reason> {
    (!target.is_empty() && !command::is_sink(target))
        .then(|| path_reason(context, directory, target))
        .flatten()
}

fn path_reason(context: &Context, directory: &Path, word: &str) -> Option<Reason> {
    let word = word.trim_matches(['\'', '"']);
    if word.starts_with('~') || word.contains(['$', '%', '`']) {
        return Some(Reason::Unresolved);
    }

    // Check a glob's literal prefix so ../* is still judged as a path outside the workspace.
    let word = word.split(['*', '?', '[']).next().unwrap_or_default();
    let value = word.split_once('=').map_or(word, |(_, value)| value);
    let parts = [word, value, value.rsplit_once(':').map_or(value, |(_, path)| path)];

    parts.iter().filter(|part| !part.is_empty()).find_map(|part| {
        let path = canonical(&directory.join(part));
        // Resolve links before deciding whether a bare name refers to a workspace file.
        let exists = std::fs::symlink_metadata(directory.join(part)).is_ok();
        let rooted = Path::new(part).components().next().is_some_and(|component| {
            matches!(
                component,
                std::path::Component::Prefix(_) | std::path::Component::RootDir
            )
        });
        let names_path = exists || rooted || part.contains(['/', '\\']) || part.starts_with('.');

        if sensitive::is_sensitive(&path) {
            Some(Reason::Secret)
        } else {
            (names_path && !context.inside_workspace(&path)).then_some(Reason::Outside)
        }
    })
}

/// Where a call runs: its `workdir`, which must be a directory inside the workspace, else the workspace.
pub(super) fn workdir(context: &Context, input: &Value) -> Result<PathBuf, ToolError> {
    let Some(asked) = input["workdir"].as_str().filter(|directory| !directory.is_empty()) else {
        return Ok(context.workspace.clone());
    };
    let directory = context.resolve(asked);
    if !context.inside_workspace(&directory) {
        return Err(ToolError(format!(
            "workdir {asked} is outside the workspace; use `cd` in the command instead, which asks"
        )));
    }
    if !directory.is_dir() {
        return Err(ToolError(format!("workdir {asked} is not a directory")));
    }

    Ok(directory)
}

/// Drops `cd` steps that stay inside the workspace: moving around it changes nothing, so it needs no
/// approval of its own and `cd crates && cargo test` asks only about `cargo test`. The directory is
/// followed along the chain from `start`; once a move leaves the workspace or cannot be read (`~`,
/// `-`, a variable, a glob), it and every later move still ask.
pub(super) fn drop_moves_within(context: &Context, start: &Path, ask: &mut Ask) {
    let mut here = Some(start.to_path_buf());
    ask.retain_commands(|command| {
        let Some(from) = &here else { return true };
        match move_within(context, from, command) {
            Move::Inside(next) => {
                here = Some(next);
                false
            }
            Move::Elsewhere => {
                here = None;
                true
            }
            Move::None => true,
        }
    });
}

fn move_within(context: &Context, from: &Path, command: &str) -> Move {
    let words: Vec<&str> = command.split(' ').collect();
    if !MOVES.contains(&words[0].to_ascii_lowercase().as_str()) {
        return Move::None;
    }
    let [_, target] = words[..] else { return Move::Elsewhere };
    if target.starts_with(['-', '~']) || target.contains(['$', '%', '*', '?', '[']) {
        return Move::Elsewhere;
    }

    let next = canonical(&from.join(target));
    if context.inside_workspace(&next) {
        Move::Inside(next)
    } else {
        Move::Elsewhere
    }
}
