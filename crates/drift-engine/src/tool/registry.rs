use super::{Tool, apply_patch, bash, edit, glob, grep, question, read, skill, task, todo, webfetch, write};
use crate::llm::ToolSpec;
use crate::llm::catalog::ToolProfile;
use std::sync::Arc;

/// The built-in tools. MCP tools come from the connected servers themselves (`Engine::offered_tools`).
pub struct Registry {
    builtin: Vec<Arc<dyn Tool>>,
}

impl Registry {
    pub fn builtin() -> Self {
        Self {
            builtin: vec![
                Arc::new(read::Read),
                Arc::new(write::Write),
                Arc::new(edit::Edit),
                Arc::new(apply_patch::ApplyPatch),
                Arc::new(bash::Bash::detect()),
                Arc::new(glob::Glob),
                Arc::new(grep::Grep),
                Arc::new(webfetch::WebFetch),
                Arc::new(todo::TodoWrite),
                Arc::new(question::Question),
                Arc::new(skill::Skill),
                Arc::new(task::Task),
                Arc::new(task::TaskOutput),
                Arc::new(task::TaskStop),
                Arc::new(task::ReadThread),
            ],
        }
    }

    /// The model's profile decides how it edits: search/replace tools or the patch format it was trained on.
    pub fn specs(&self, profile: ToolProfile) -> Vec<ToolSpec> {
        self.offered(profile).iter().map(|tool| tool.spec()).collect()
    }

    pub fn offered(&self, profile: ToolProfile) -> Vec<Arc<dyn Tool>> {
        let hidden: &[&str] = match profile {
            ToolProfile::Edit => &["apply_patch"],
            ToolProfile::ApplyPatch => &["edit", "write"],
        };

        self.builtin
            .iter()
            .filter(|tool| !hidden.contains(&tool.spec().name.as_str()))
            .cloned()
            .collect()
    }

    pub fn get(&self, name: &str) -> Option<Arc<dyn Tool>> {
        self.builtin.iter().find(|tool| tool.spec().name == name).cloned()
    }
}

impl crate::Engine {
    /// Every tool a turn starting now in `workspace` could be offered: the built-ins for the profile, then every server's connected there.
    pub fn offered_tools(&self, profile: ToolProfile, workspace: Option<&std::path::Path>) -> Vec<Arc<dyn Tool>> {
        self.tools
            .offered(profile)
            .into_iter()
            .chain(self.mcp.tools(&self.store, workspace))
            .collect()
    }

    pub fn tool_specs(&self, profile: ToolProfile, workspace: Option<&std::path::Path>) -> Vec<ToolSpec> {
        self.offered_tools(profile, workspace)
            .iter()
            .map(|tool| tool.spec())
            .collect()
    }
}
