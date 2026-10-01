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
                "properties": { "name": { "type": "string", "description": "The skill's name as listed." } },
                "required": ["name"]
            }),
        }
    }

    fn ask(&self, _ctx: &Context, _input: &Value) -> Option<Ask> {
        None
    }

    fn run<'a>(&'a self, ctx: &'a Context, input: Value) -> RunFuture<'a> {
        Box::pin(async move {
            let name = required_str(&input, "name")?;
            let skill = ctx.config.skill(name).ok_or_else(|| ToolError(format!("no skill named `{name}`; the available skills are listed in the system prompt")))?;
            // The instructions as the turn's config read them, never the file as it is now.
            Ok(Output {
                title: skill.name.clone(),
                output: format!("Skill directory: {}\n\n{}", skill.path, skill.instructions),
                metadata: json!({ "path": skill.path }),
            })
        })
    }
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
        assert!(Skill.run(&sandbox.ctx, json!({ "name": "nope" })).await.unwrap_err().0.contains("no skill named"));
    }
}
