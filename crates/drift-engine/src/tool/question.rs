use serde_json::{json, Value};

use super::{Ask, Context, Output, RunFuture, Tool, ToolError};
use crate::llm::ToolSpec;
use crate::question::{self, Question as Item};

pub struct Question;

impl Tool for Question {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "question".into(),
            description: include_str!("prompts/question.txt").trim().into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "questions": {
                        "type": "array",
                        "items": {
                            "type": "object",
                            "properties": {
                                "question": { "type": "string", "description": "The full question." },
                                "header": { "type": "string", "description": "A few words naming the decision." },
                                "options": {
                                    "type": "array",
                                    "items": {
                                        "type": "object",
                                        "properties": { "label": { "type": "string" }, "description": { "type": "string" } },
                                        "required": ["label"]
                                    }
                                },
                                "multiple": { "type": "boolean", "description": "Allow choosing more than one option." },
                                "custom": { "type": "boolean", "description": "Allow a typed answer. Default true." }
                            },
                            "required": ["question", "header", "options"]
                        }
                    }
                },
                "required": ["questions"]
            }),
        }
    }

    fn ask(&self, _ctx: &Context, _input: &Value) -> Option<Ask> {
        None
    }

    fn run<'a>(&'a self, ctx: &'a Context, input: Value) -> RunFuture<'a> {
        Box::pin(async move {
            let items: Vec<Item> = serde_json::from_value(input["questions"].clone()).map_err(|e| ToolError(format!("invalid questions: {e}")))?;
            if items.is_empty() {
                return Err(ToolError("at least one question is required".into()));
            }
            let request = question::new_request(&ctx.session_id, &ctx.message_id, &ctx.call_id, items.clone());
            let answers = ctx.engine.questions.ask(&ctx.engine.hub, request, &ctx.abort).await;
            let Some(answers) = answers else {
                return Err(ToolError("The user declined to answer.".into()));
            };
            let lines: Vec<String> = items
                .iter()
                .zip(answers.iter())
                .map(|(item, chosen)| format!("{}: {}", item.header, chosen.join(", ")))
                .collect();
            Ok(Output { title: items[0].header.clone(), output: lines.join("\n"), metadata: json!({ "answers": answers }) })
        })
    }
}

#[cfg(test)]
mod tests {
    use super::super::tests::Sandbox;
    use super::*;
    use crate::event::Event;

    #[tokio::test]
    async fn waits_for_the_user_and_formats_answers() {
        let sandbox = Sandbox::new("question");
        let ctx = sandbox.ctx_clone();
        let mut rx = ctx.engine.hub.attach(None).rx;
        let input = json!({ "questions": [{ "question": "Which db?", "header": "Database", "options": [{ "label": "sqlite" }, { "label": "postgres" }] }] });
        let (out, ()) = tokio::join!(Question.run(&ctx, input), async {
            let asked = rx.recv().await.unwrap();
            let Event::QuestionAsked { request } = asked.event else { panic!() };
            assert_eq!(request.questions[0].header, "Database");
            ctx.engine.questions.reply(&ctx.engine.hub, &request.id, Some(vec![vec!["sqlite".into()]])).unwrap();
        });
        let out = out.unwrap();
        assert_eq!(out.output, "Database: sqlite");
    }

    #[tokio::test]
    async fn declining_is_an_error_result() {
        let sandbox = Sandbox::new("question-decline");
        let ctx = sandbox.ctx_clone();
        let mut rx = ctx.engine.hub.attach(None).rx;
        let input = json!({ "questions": [{ "question": "Go?", "header": "Go", "options": [] }] });
        let (out, ()) = tokio::join!(Question.run(&ctx, input), async {
            let Event::QuestionAsked { request } = rx.recv().await.unwrap().event else { panic!() };
            ctx.engine.questions.reply(&ctx.engine.hub, &request.id, None).unwrap();
        });
        assert!(out.unwrap_err().0.contains("declined"));
    }
}
