use serde_json::{json, Value};

use super::{required_str, Ask, Context, Output, RunFuture, Tool, ToolError};
use crate::config::Config;
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
            let config = Config::load(&ctx.workspace);
            let skill = config.skill(name).ok_or_else(|| ToolError(format!("no skill named `{name}`; the available skills are listed in the system prompt")))?;
            let text = tokio::fs::read_to_string(std::path::Path::new(&skill.path).join("SKILL.md")).await?;
            let body = crate::config::body(&text);
            Ok(Output {
                title: skill.name.clone(),
                output: format!("Skill directory: {}\n\n{body}", skill.path),
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
        let sandbox = Sandbox::new("skill");
        sandbox.file(".drift/skills/deploy/SKILL.md", "---\nname: deploy\ndescription: Ships\n---\nRun the deploy script.");
        let out = Skill.run(&sandbox.ctx, json!({ "name": "deploy" })).await.unwrap();
        assert!(out.output.starts_with("Skill directory: "));
        assert!(out.output.ends_with("Run the deploy script."));
        assert!(Skill.run(&sandbox.ctx, json!({ "name": "nope" })).await.unwrap_err().0.contains("no skill named"));
    }
}
