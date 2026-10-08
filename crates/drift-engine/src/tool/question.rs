use serde_json::{Value, json};

use super::ToolMetadata;
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
                    },
                    "async": { "type": "boolean", "description": "Default true: ask and carry on; the answer arrives later as its own message. False waits here for the answer, for a decision nothing else can go ahead without." }
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
            let items: Vec<Item> = serde_json::from_value(input["questions"].clone())
                .map_err(|e| ToolError(format!("invalid questions: {e}")))?;
            if items.is_empty() {
                return Err(ToolError("at least one question is required".into()));
            }
            let mut request = question::new_request(&ctx.session_id, &ctx.message_id, &ctx.call_id, items.clone());
            // A subagent's turn ends before a late answer could reach its parent, so it always waits.
            let subagent = ctx
                .engine
                .store
                .session(&ctx.session_id)?
                .is_some_and(|s| s.visibility == crate::session::types::Visibility::Hidden);
            if input["async"].as_bool().unwrap_or(true) && !subagent {
                request.is_async = true;
                request.generation = ctx.engine.worker_scope(&ctx.session_id).1;
                let id = request.id.clone();
                ctx.engine.questions.ask_async(&ctx.engine.hub, request);
                let output = format!(
                    "Asked the user ({id}). Their answer will arrive in this conversation as its own message. Carry on with work that does not depend on it; if nothing else can be done, finish your turn and wait."
                );
                return Ok(Output {
                    title: items[0].header.clone(),
                    output,
                    metadata: ToolMetadata {
                        request_id: Some(id),
                        asynchronous: Some(true),
                        ..Default::default()
                    },
                });
            }
            let answers = ctx.engine.questions.ask(&ctx.engine.hub, request, &ctx.abort).await;
            let Some(answers) = answers else {
                return Err(ToolError("The user declined to answer.".into()));
            };
            let lines: Vec<String> = items
                .iter()
                .zip(answers.iter())
                .map(|(item, chosen)| format!("{}: {}", item.header, chosen.join(", ")))
                .collect();
            Ok(Output {
                title: items[0].header.clone(),
                output: lines.join("\n"),
                metadata: ToolMetadata {
                    answers: Some(answers),
                    ..Default::default()
                },
            })
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
        let input = json!({ "async": false, "questions": [{ "question": "Which db?", "header": "Database", "options": [{ "label": "sqlite" }, { "label": "postgres" }] }] });
        let (out, ()) = tokio::join!(Question.run(&ctx, input), async {
            let asked = rx.recv().await.unwrap();
            let Event::QuestionAsked { request } = asked.event else {
                panic!()
            };
            assert_eq!(request.questions[0].header, "Database");
            ctx.engine
                .questions
                .reply(&ctx.engine.hub, &request.id, Some(vec![vec!["sqlite".into()]]))
                .unwrap();
        });
        let out = out.unwrap();
        assert_eq!(out.output, "Database: sqlite");
    }

    #[tokio::test]
    async fn declining_is_an_error_result() {
        let sandbox = Sandbox::new("question-decline");
        let ctx = sandbox.ctx_clone();
        let mut rx = ctx.engine.hub.attach(None).rx;
        let input = json!({ "async": false, "questions": [{ "question": "Go?", "header": "Go", "options": [] }] });
        let (out, ()) = tokio::join!(Question.run(&ctx, input), async {
            let Event::QuestionAsked { request } = rx.recv().await.unwrap().event else {
                panic!()
            };
            ctx.engine.questions.reply(&ctx.engine.hub, &request.id, None).unwrap();
        });
        assert!(out.unwrap_err().0.contains("declined"));
    }
}
