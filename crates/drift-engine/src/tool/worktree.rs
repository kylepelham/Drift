//! The model's worktree tools: make, list and remove the workspace's worktrees, and move the session between them.

use serde_json::{Value, json};
use std::fmt::Write as _;
use std::path::Path;

use super::bash::Bash;
use super::{Ask, Context, Output, RunFuture, Tool, ToolError, required_str};
use crate::event::Event;
use crate::llm::ToolSpec;
use crate::session::types::Session;
use crate::store::Workspace;

const CREATE: &str = "Creates a git worktree for this workspace: branch `drift/<name>` from the commit this session is on, \
checked out in its own folder outside the repository. Uncommitted changes are not copied. The project's \
`worktree.setup` commands run in it, each asking like bash. Then call session_move with the directory it returns to \
work there. Drift never merges a worktree back: commit in it, then merge or open a pull request as the user asks.";
const LIST: &str = "Lists this workspace's worktrees: name, directory, branch, uncommitted changes, whether the branch \
is merged into the workspace's current commit, and where this session works.";
const REMOVE: &str = "Removes one of this workspace's worktrees by name. Uncommitted changes refuse the removal unless \
`force` is true, which asks the user first. The branch is deleted only when it is merged; an unmerged branch is kept.";
const MOVE: &str = "Moves this session to work in another directory: one of this workspace's worktrees (its directory \
or its name) or the workspace folder to come back. It takes effect when this step's calls are done, so calls made \
alongside it still run where the session was.";

pub struct Create {
    bash: Bash,
}

pub struct List;

pub struct Remove;

pub struct Move;

impl Create {
    pub fn new(bash: Bash) -> Self {
        Self { bash }
    }
}

/// The calling session and its workspace.
fn place(context: &Context) -> Result<(Session, Workspace), ToolError> {
    let store = &context.engine.store;
    let session = store
        .session(&context.session_id)?
        .ok_or_else(|| ToolError("this session is gone".into()))?;
    let workspace = store
        .workspace(&session.workspace_id)?
        .ok_or_else(|| ToolError("this session's workspace is gone".into()))?;

    Ok((session, workspace))
}

fn spec(name: &str, description: &str, properties: Value, required: &[&str]) -> ToolSpec {
    ToolSpec {
        name: name.into(),
        description: description.into(),
        input_schema: json!({ "type": "object", "properties": properties, "required": required }),
    }
}

impl Tool for Create {
    fn spec(&self) -> ToolSpec {
        let properties = json!({
            "name": { "type": "string", "description": "Letters, digits, `-`, `_` and `.`; the branch is `drift/<name>`." }
        });

        spec("worktree_create", CREATE, properties, &["name"])
    }

    fn ask(&self, _context: &Context, input: &Value) -> Option<Ask> {
        let name = input["name"].as_str().unwrap_or_default();

        Some(Ask::new("worktree", format!("create {name}"), format!("Create worktree {name}")).allow_by_default())
    }

    /// Making it is allowed by default; each setup command asks as a `bash` line would.
    fn asks(&self, context: &Context, input: &Value) -> Vec<Ask> {
        let name = input["name"].as_str().unwrap_or_default();
        let setup = context
            .config
            .worktree_setup
            .iter()
            .map(|line| Ask::shell(self.bash.dialect(), line, format!("Set up worktree {name}: {line}")));

        self.ask(context, input).into_iter().chain(setup).collect()
    }

    fn permissions(&self) -> &'static [&'static str] {
        &["worktree"]
    }

    fn mutates(&self) -> bool {
        true
    }

    /// Nothing in the workspace changes, so there is nothing to record for undo.
    fn touches(&self, _context: &Context, _input: &Value) -> Option<Vec<std::path::PathBuf>> {
        Some(Vec::new())
    }

    fn run<'a>(&'a self, context: &'a Context, input: Value) -> RunFuture<'a> {
        Box::pin(async move {
            let name = required_str(&input, "name")?;
            let (_, workspace) = place(context)?;
            let created = context
                .engine
                .create_worktree(&workspace, &context.workspace, name)
                .await?;

            let mut text = format!(
                "Created worktree `{name}` on branch {} from {}, at {}.",
                created.branch,
                created.commit,
                created.directory.display()
            );
            if created.left_behind > 0 {
                let _ = write!(
                    text,
                    "\n{} uncommitted change(s) in {} were not copied into it.",
                    created.left_behind,
                    context.workspace.display()
                );
            }
            let setup = self.set_up(context, &workspace, &created.directory).await?;
            text.push_str(&setup);
            let _ = write!(
                text,
                "\nThis session still works in {}; call session_move with directory \"{}\" to work in the worktree.",
                context.workspace.display(),
                created.directory.display()
            );

            Ok(Output::new(format!("Worktree {name}"), text))
        })
    }
}

impl Create {
    /// Runs `worktree.setup` in the new worktree in order, stopping at the first that fails.
    async fn set_up(&self, context: &Context, workspace: &Workspace, directory: &Path) -> Result<String, ToolError> {
        let root = crate::tool::canonical(Path::new(&workspace.path));
        let env = [("DRIFT_WORKSPACE", root.as_path()), ("DRIFT_WORKTREE", directory)];
        let mut text = String::new();
        for line in &context.config.worktree_setup {
            let ran = self.bash.run_to_end(context, line, directory, &env).await?;
            let _ = write!(text, "\n\nSetup `{line}`:\n{}", ran.output);
            if ran.metadata.exit != Some(0) {
                text.push_str("\nSetup stopped here; the worktree is made but may be incomplete.");
                break;
            }
        }

        Ok(text)
    }
}

impl Tool for List {
    fn spec(&self) -> ToolSpec {
        spec("worktree_list", LIST, json!({}), &[])
    }

    fn ask(&self, _context: &Context, _input: &Value) -> Option<Ask> {
        None
    }

    fn run<'a>(&'a self, context: &'a Context, _input: Value) -> RunFuture<'a> {
        Box::pin(async move {
            let (session, workspace) = place(context)?;
            let listed = context.engine.worktrees(&workspace).await?;
            let mut text = format!("The workspace folder is {}.", workspace.path);
            if listed.is_empty() {
                text.push_str("\nIt has no worktrees yet; worktree_create makes one.");
            }
            for worktree in &listed {
                let directory = context
                    .engine
                    .worktree_directory(&workspace, &worktree.name)
                    .unwrap_or_else(|| worktree.path.clone());
                let branch = worktree.branch.as_deref().unwrap_or("no branch (detached)");
                let merged = if worktree.merged { "merged" } else { "not merged" };
                let _ = write!(
                    text,
                    "\n- {}: {} on {branch}, {} uncommitted change(s), {merged}",
                    worktree.name,
                    directory.display(),
                    worktree.changes
                );
                if session.worktree.as_deref() == Some(worktree.name.as_str()) {
                    text.push_str("; this session works here");
                }
            }
            if session.worktree.is_none() {
                text.push_str("\nThis session works in the workspace folder.");
            }

            Ok(Output::new(format!("{} worktree(s)", listed.len()), text))
        })
    }
}

impl Tool for Remove {
    fn spec(&self) -> ToolSpec {
        let properties = json!({
            "name": { "type": "string", "description": "The worktree's name, as worktree_list shows it." },
            "force": { "type": "boolean", "description": "Drop its uncommitted changes too; the user is asked first. Default false." }
        });

        spec("worktree_remove", REMOVE, properties, &["name"])
    }

    fn ask(&self, _context: &Context, input: &Value) -> Option<Ask> {
        let name = input["name"].as_str().unwrap_or_default();
        if input["force"].as_bool() == Some(true) {
            let title = format!("Remove worktree {name} and drop its uncommitted changes");
            return Some(Ask::new("worktree", format!("remove --force {name}"), title));
        }

        Some(Ask::new("worktree", format!("remove {name}"), format!("Remove worktree {name}")).allow_by_default())
    }

    fn permissions(&self) -> &'static [&'static str] {
        &["worktree"]
    }

    fn mutates(&self) -> bool {
        true
    }

    fn touches(&self, _context: &Context, _input: &Value) -> Option<Vec<std::path::PathBuf>> {
        Some(Vec::new())
    }

    fn run<'a>(&'a self, context: &'a Context, input: Value) -> RunFuture<'a> {
        Box::pin(async move {
            let name = required_str(&input, "name")?;
            let force = input["force"].as_bool() == Some(true);
            let (session, workspace) = place(context)?;
            if session.worktree.as_deref() == Some(name) {
                return Err(ToolError(format!(
                    "this session works in worktree `{name}`; move out with session_move first"
                )));
            }

            let removed = context.engine.remove_worktree(&workspace, name, force).await?;
            let mut text = format!("Removed worktree `{name}`.");
            match (&removed.branch, removed.branch_deleted) {
                (Some(branch), true) => {
                    let _ = write!(text, " Its branch {branch} was merged, so it was deleted too.");
                }
                (Some(branch), false) => {
                    let _ = write!(text, " Its branch {branch} is not merged, so it was kept.");
                }
                (None, _) => {}
            }
            if !removed.moved.is_empty() {
                let _ = write!(
                    text,
                    " {} other conversation(s) that worked there now work in the workspace folder.",
                    removed.moved.len()
                );
            }

            Ok(Output::new(format!("Removed {name}"), text))
        })
    }
}

impl Move {
    /// The worktree `asked` names, by directory or name, or `None` for the workspace folder.
    async fn target(context: &Context, workspace: &Workspace, asked: &str) -> Result<Option<String>, ToolError> {
        let resolved = context.resolve(asked);
        if resolved == crate::tool::canonical(Path::new(&workspace.path)) {
            return Ok(None);
        }

        let listed = context.engine.worktrees(workspace).await?;
        let found = listed.iter().find(|worktree| {
            worktree.name == asked
                || worktree.path == resolved
                || context.engine.worktree_directory(workspace, &worktree.name).as_ref() == Some(&resolved)
        });
        match found {
            Some(worktree) => Ok(Some(worktree.name.clone())),
            None => Err(ToolError(format!(
                "{asked} is neither the workspace folder ({}) nor one of its worktrees; worktree_list shows them",
                workspace.path
            ))),
        }
    }
}

impl Tool for Move {
    fn spec(&self) -> ToolSpec {
        let properties = json!({
            "directory": { "type": "string", "description": "A worktree's directory or name, or the workspace folder." }
        });

        spec("session_move", MOVE, properties, &["directory"])
    }

    fn ask(&self, _context: &Context, _input: &Value) -> Option<Ask> {
        None
    }

    fn run<'a>(&'a self, context: &'a Context, input: Value) -> RunFuture<'a> {
        Box::pin(async move {
            let asked = required_str(&input, "directory")?;
            let (_, workspace) = place(context)?;
            let target = Self::target(context, &workspace, asked).await?;
            let session = context
                .engine
                .store
                .set_session_worktree(&context.session_id, target.as_deref())?
                .ok_or_else(|| ToolError("this session is gone".into()))?;
            let (directory, _) = context.engine.working_place(&session, &workspace);
            context.engine.hub.publish(Event::SessionUpdated { session });

            Ok(Output::new(
                "Moved",
                format!(
                    "From the next step on this session works in {}. Calls in this step still run in {}.",
                    directory.display(),
                    context.workspace.display()
                ),
            ))
        })
    }
}
