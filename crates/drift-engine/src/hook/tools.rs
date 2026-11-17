//! Tools plugins add: each call goes to its plugin, asked again while it answers `wait`, so waiting costs no turn.

use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::Value;

use super::{Hook, ToolAnswer, ToolRequest};
use crate::llm::ToolSpec;
use crate::tool::{Context, Output, RunFuture, Tool, ToolError, ToolMetadata};

const RETRY_MIN: Duration = Duration::from_millis(100);
const RETRY_MAX: Duration = Duration::from_secs(60);

/// Every plugin's tools, in plugin order; a name an earlier plugin took is logged and left out.
pub(super) fn of(hooks: &[Arc<dyn Hook>]) -> Vec<Arc<dyn Tool>> {
    let mut tools: Vec<Arc<dyn Tool>> = Vec::new();
    for hook in hooks {
        for spec in hook.tools() {
            if tools.iter().any(|tool| tool.spec().name == spec.name) {
                eprintln!(
                    "drift: plugin {}'s tool {} is left out: an earlier plugin has one of that name",
                    hook.name(),
                    spec.name
                );
                continue;
            }
            tools.push(Arc::new(PluginTool {
                hook: hook.clone(),
                spec,
            }));
        }
    }

    tools
}

struct PluginTool {
    hook: Arc<dyn Hook>,
    spec: ToolSpec,
}

impl Tool for PluginTool {
    fn spec(&self) -> ToolSpec {
        self.spec.clone()
    }

    fn ask(&self, _ctx: &Context, _input: &Value) -> Option<crate::tool::Ask> {
        None
    }

    /// A plugin may write files or run programs, so a read-only agent never calls one.
    fn stays_read_only(&self, _ctx: &Context, _input: &Value) -> bool {
        false
    }

    fn run<'a>(&'a self, ctx: &'a Context, input: Value) -> RunFuture<'a> {
        Box::pin(async move {
            let started = Instant::now();
            let cap = ctx.wait_cap();
            let mut request = ToolRequest {
                session_id: ctx.session_id.clone(),
                workspace: ctx.workspace.to_string_lossy().into_owned(),
                agent: ctx.agent.clone(),
                tool: self.spec.name.clone(),
                input,
                waited_ms: 0,
                worker: ctx.in_worker(),
            };

            loop {
                let (retry_ms, status) = match self.hook.call_tool(&request).await {
                    ToolAnswer::Done(text) => return Ok(Output::new(self.spec.name.clone(), text)),
                    ToolAnswer::Failed(why) => return Err(ToolError(why)),
                    ToolAnswer::Wait { retry_ms, status } => (retry_ms, status),
                };
                if started.elapsed() >= cap {
                    return Err(ToolError(format!(
                        "gave up after waiting {} minutes: {status}",
                        cap.as_secs() / 60
                    )));
                }
                ctx.progress.show(ToolMetadata {
                    output: Some(status),
                    ..Default::default()
                });
                let pause = Duration::from_millis(retry_ms.into()).clamp(RETRY_MIN, RETRY_MAX);
                tokio::time::sleep(pause.min(cap.saturating_sub(started.elapsed()))).await;
                request.waited_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// Answers `wait` until it has been asked `waits` times, then says how long it waited.
    struct Gate {
        name: &'static str,
        tools: &'static [&'static str],
        waits: usize,
        asked: Mutex<Vec<ToolRequest>>,
    }

    impl Gate {
        fn new(name: &'static str, tools: &'static [&'static str], waits: usize) -> Arc<Self> {
            Arc::new(Self {
                name,
                tools,
                waits,
                asked: Mutex::default(),
            })
        }
    }

    #[async_trait::async_trait]
    impl Hook for Gate {
        fn name(&self) -> &str {
            self.name
        }

        fn tools(&self) -> Vec<ToolSpec> {
            self.tools
                .iter()
                .map(|name| ToolSpec {
                    name: (*name).into(),
                    description: format!("{name} from {}", self.name),
                    input_schema: serde_json::json!({ "type": "object" }),
                })
                .collect()
        }

        async fn call_tool(&self, request: &ToolRequest) -> ToolAnswer {
            let mut asked = self.asked.lock().unwrap();
            asked.push(request.clone());
            if request.input["fail"].as_bool() == Some(true) {
                return ToolAnswer::Failed("refused".into());
            }
            if asked.len() <= self.waits {
                return ToolAnswer::Wait {
                    retry_ms: 1,
                    status: format!("{} waiting", self.name),
                };
            }

            ToolAnswer::Done(format!("{} done after {} asks", self.name, asked.len()))
        }
    }

    #[tokio::test]
    async fn a_waiting_tool_is_asked_again_showing_its_status_until_it_is_done() {
        let mut sandbox = crate::tool::tests::Sandbox::new("plugin-tool-wait");
        let shown = Arc::new(Mutex::new(Vec::new()));
        let seen = shown.clone();
        sandbox.ctx.progress = crate::tool::Progress::new(move |metadata| seen.lock().unwrap().push(metadata.output));
        let gate = Gate::new("locks", &["lock"], 2);
        let tool = &of(&[gate.clone() as Arc<dyn Hook>])[0];

        let output = tool
            .run(&sandbox.ctx, serde_json::json!({ "name": "gpu" }))
            .await
            .unwrap();
        assert_eq!(output.output, "locks done after 3 asks");

        let asked = gate.asked.lock().unwrap().clone();
        assert_eq!(asked[0].waited_ms, 0);
        assert!(
            asked[2].waited_ms >= 200,
            "each retry waits at least 100 ms: {}",
            asked[2].waited_ms
        );
        assert!(
            asked
                .iter()
                .all(|request| request.tool == "lock" && request.input["name"] == "gpu" && !request.worker)
        );
        assert_eq!(*shown.lock().unwrap(), vec![Some("locks waiting".to_owned()); 2]);

        let failed = tool.run(&sandbox.ctx, serde_json::json!({ "fail": true })).await;
        assert_eq!(failed.err(), Some(ToolError("refused".into())));
    }

    #[tokio::test]
    async fn a_plugin_tool_named_as_a_built_in_or_an_earlier_plugins_is_left_out() {
        let sandbox = crate::tool::tests::Sandbox::new("plugin-tool-names");
        let engine = &sandbox.ctx.engine;
        let first = Gate::new("first", &["read", "lock"], 0);
        let second = Gate::new("second", &["lock", "bench"], 0);
        engine.hooks.set(vec![first, second], vec![]);

        let names: Vec<String> = engine.plugin_tools().iter().map(|tool| tool.spec().name).collect();
        assert_eq!(names, ["lock", "bench"]);
        let lock = engine.plugin_tools().remove(0);
        assert_eq!(lock.spec().description, "lock from first");
        assert!(
            !lock.stays_read_only(&sandbox.ctx, &Value::Null),
            "a read-only agent never calls a plugin"
        );

        let offered = engine.offered_tools(crate::llm::catalog::ToolProfile::Edit, None);
        assert_eq!(offered.iter().filter(|tool| tool.spec().name == "read").count(), 1);
        assert!(offered.iter().any(|tool| tool.spec().name == "bench"));
    }
}
