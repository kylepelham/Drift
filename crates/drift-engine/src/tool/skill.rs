use serde_json::{json, Value};

use super::{required_str, Ask, Context, Output, RunFuture, Tool, ToolError};
use crate::llm::ToolSpec;

pub struct Skill;

impl Tool for Skill {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "skill".into(),
            description: "Loads a skill listed in the system prompt: returns its instructions and the directory holding any files it refers to. Load a skill before doing work its description covers.".into(),
            input_schema: json!({
                "type": "object",
                "properties": { "name": { "type": "string", "description": "The skill's name as listed." }, "arguments": { "type":"string", "description":"Arguments for a skill command's template, when supplied." } },
                "required": ["name"]
            }),
        }
    }

    fn ask(&self, _ctx: &Context, input: &Value) -> Option<Ask> {
        let name = input["name"].as_str()?;
        Some(Ask::new("skill", name, format!("Load skill {name}")).allow_by_default())
    }

    fn run<'a>(&'a self, ctx: &'a Context, input: Value) -> RunFuture<'a> {
        Box::pin(async move {
            let name = required_str(&input, "name")?;
            let skill = ctx.config.skill(name).ok_or_else(|| ToolError(format!("no skill named `{name}`; the available skills are listed in the system prompt")))?;
            let instructions = input["arguments"].as_str().map_or_else(|| skill.instructions.clone(), |arguments| crate::config::Command::new(name.into(), skill.description.clone(), skill.instructions.clone()).expand(arguments));
            // The instructions as the turn's config read them, never the file as it is now.
            let dir = std::path::PathBuf::from(&skill.path);
            let files = tokio::task::spawn_blocking(move || files_in(&dir)).await.unwrap_or_default();
            let listed = if files.is_empty() {
                String::new()
            } else {
                format!("\n\nFiles in the skill directory (up to {MAX_FILES}; paths in the instructions are relative to it):\n{}", files.join("\n"))
            };
            Ok(Output {
                title: skill.name.clone(),
                output: format!("Skill directory: {}\n\n{}{listed}", skill.path, instructions),
                metadata: json!({ "path": skill.path, "files": files }),
            })
        })
    }
}

/// The most of a skill's own files named when it loads, as opencode samples them.
const MAX_FILES: usize = 10;

/// The skill's files beside its SKILL.md, as absolute paths, walked as git would list them.
fn files_in(dir: &std::path::Path) -> Vec<String> {
    let mut files: Vec<String> = super::walk(dir)
        .flatten()
        .filter(|entry| entry.file_type().is_some_and(|kind| kind.is_file()) && entry.file_name() != "SKILL.md")
        .take(MAX_FILES)
        .map(|entry| entry.path().display().to_string())
        .collect();
    files.sort();
    files
}

#[cfg(test)]
mod tests {
    use super::super::tests::Sandbox;
    use super::*;

    #[tokio::test]
    async fn loads_a_project_skill_by_name() {
        let mut sandbox = Sandbox::new("skill");
        sandbox.file(".drift/skills/deploy/SKILL.md", "---\nname: deploy\ndescription: Ships\n---\nRun the deploy script.");
        assert!(Skill.run(&sandbox.ctx, json!({ "name": "deploy" })).await.is_err(), "added after the turn began: not its skill");
        sandbox.reload_config();
        // Rewritten after the turn's config was read: the turn still gets what it was offered.
        sandbox.file(".drift/skills/deploy/SKILL.md", "---\nname: deploy\ndescription: Ships\n---\nDelete everything.");
        let out = Skill.run(&sandbox.ctx, json!({ "name": "deploy" })).await.unwrap();
        assert!(out.output.starts_with("Skill directory: "));
        assert!(out.output.ends_with("Run the deploy script."), "{}", out.output);
        sandbox.file(".drift/skills/deploy/scripts/ship.sh", "echo ship");
        let listed = Skill.run(&sandbox.ctx, json!({ "name": "deploy" })).await.unwrap();
        assert!(listed.output.contains("Files in the skill directory") && listed.output.contains("ship.sh") && !listed.output.contains("SKILL.md\n"), "{}", listed.output);
        assert_eq!(listed.metadata["files"].as_array().unwrap().len(), 1);
        assert!(Skill.run(&sandbox.ctx, json!({ "name": "nope" })).await.unwrap_err().0.contains("no skill named"));
    }
}
