//! Named locks shared by every conversation and subagent, such as a merge or benchmark lock: the `lock` tool
//! takes one, waiting without spending turns while another session holds it, gives it back, or lists them.
//! A lock frees itself when its time runs out, when its holder is deleted, and when a subagent holding it ends.

wit_bindgen::generate!({ world: "tooling", path: "../../crates/drift-engine/wit" });

use std::time::{SystemTime, UNIX_EPOCH};

use drift::plugin::store;
use drift::plugin::types::Pending;
use serde_json::{Map, Value, json};

struct Locks;

const KEY: &str = "locks";
const RETRY_MS: u32 = 1_000;
const DEFAULT_TTL_MINUTES: u64 = 60;
const DESCRIPTION: &str = "Takes, gives back or lists named locks shared by every conversation and subagent in Drift, \
such as a merge or benchmark lock. `acquire` waits until the lock is free without spending your turns; do not \
sleep or poll for one yourself. Give a lock back with `release` as soon as the work it guards is done. A lock \
frees itself when its ttl runs out, and a subagent's locks free when it finishes.";

impl Guest for Locks {
    fn name() -> String {
        "locks".into()
    }

    fn settings() -> Vec<Setting> {
        Vec::new()
    }

    fn tools() -> Vec<ToolSpec> {
        let parameters = json!({
            "type": "object",
            "properties": {
                "action": { "type": "string", "enum": ["acquire", "release", "list"] },
                "name": { "type": "string", "description": "The lock, such as merge or bench. Required for acquire and release." },
                "wait_seconds": { "type": "integer", "minimum": 0, "description": "For acquire: the longest to wait for it; 0 tries once. Default: as long as Drift lets a call wait." },
                "ttl_minutes": { "type": "integer", "minimum": 1, "description": "For acquire: when it frees itself if never released. Default 60; acquiring it again starts this over." },
                "reason": { "type": "string", "description": "For acquire: what it guards, shown to whoever waits for it." },
                "force": { "type": "boolean", "description": "For release: free a lock another session holds. Only when that holder is gone, such as after Drift restarted." }
            },
            "required": ["action"]
        });

        vec![ToolSpec {
            name: "lock".into(),
            description: DESCRIPTION.into(),
            parameters: parameters.to_string(),
        }]
    }

    fn call_tool(request: ToolRequest) -> ToolAnswer {
        let input: Value = serde_json::from_str(&request.input).unwrap_or_default();
        let now = now_ms();
        let mut locks = held(now);
        let before = locks.clone();
        let answer = match input["action"].as_str() {
            Some("acquire") => acquire(&mut locks, &request, &input, now),
            Some("release") => release(&mut locks, &request, &input),
            Some("list") => ToolAnswer::Done(list(&locks, now)),
            _ => ToolAnswer::Failed("`action` must be acquire, release or list".into()),
        };
        if locks != before {
            save(&locks);
        }

        answer
    }

    fn before_tool(_call: ToolCall) -> BeforeTool {
        BeforeTool::Allow
    }

    fn after_tool(_outcome: ToolResult) -> AfterTool {
        AfterTool::Keep
    }

    fn prompt_submit(_prompt: Prompt) -> PromptSubmit {
        PromptSubmit::Keep
    }

    fn turn_end(_reply: Reply) -> TurnEnd {
        TurnEnd::Accept
    }

    fn permission(_ask: PermissionAsk) -> Permission {
        Permission::Pass
    }

    fn compaction(_compaction: Compaction) -> Compacting {
        Compacting::Instruct("Keep the names of the locks this conversation holds and has not released.".into())
    }

    /// A deleted session's locks free, and so do a subagent's once it has finished.
    fn session(session: Session, kind: SessionKind) {
        let ends = |lock: &Value| {
            lock["holder"] == session.id.as_str()
                && match kind {
                    SessionKind::Deleted => true,
                    SessionKind::Idle => lock["worker"] == true,
                    _ => false,
                }
        };
        let mut locks = held(now_ms());
        let before = locks.len();
        locks.retain(|_, lock| !ends(lock));
        if locks.len() != before {
            save(&locks);
        }
    }
}

fn acquire(locks: &mut Map<String, Value>, request: &ToolRequest, input: &Value, now: u64) -> ToolAnswer {
    let Some(name) = lock_name(input) else {
        return ToolAnswer::Failed("`name` is required to acquire a lock".into());
    };
    let ttl = input["ttl_minutes"].as_u64().unwrap_or(DEFAULT_TTL_MINUTES).max(1);
    let expires = now + ttl * 60_000;
    let reason = input["reason"].as_str().unwrap_or_default();

    match locks.get(name) {
        Some(lock) if lock["holder"] != request.session_id.as_str() => {
            let status = format!("Waiting for {name}: {}", holding(lock, now));
            let limit = input["wait_seconds"].as_u64().map(|seconds| seconds * 1_000);
            if limit.is_some_and(|limit| request.waited_ms >= limit) {
                return ToolAnswer::Failed(format!("{name} is still held: {}", holding(lock, now)));
            }
            ToolAnswer::Wait(Pending {
                retry_ms: RETRY_MS,
                status,
            })
        }
        again => {
            let (verb, since) = match again.and_then(|lock| lock["since"].as_u64()) {
                Some(since) => ("Still hold", since),
                None => ("Acquired", now),
            };
            locks.insert(
                name.into(),
                json!({
                    "holder": request.session_id,
                    "agent": request.agent,
                    "worker": request.worker,
                    "reason": reason,
                    "since": since,
                    "expires": expires,
                }),
            );
            ToolAnswer::Done(format!(
                "{verb} {name}. Release it when done; it frees itself in {ttl} minutes otherwise."
            ))
        }
    }
}

fn release(locks: &mut Map<String, Value>, request: &ToolRequest, input: &Value) -> ToolAnswer {
    let Some(name) = lock_name(input) else {
        return ToolAnswer::Failed("`name` is required to release a lock".into());
    };
    let Some(lock) = locks.get(name) else {
        return ToolAnswer::Done(format!("{name} was not held."));
    };
    if lock["holder"] != request.session_id.as_str() && input["force"] != true {
        return ToolAnswer::Failed(format!(
            "{name} is held by another session, not this one; pass force only if that holder is gone"
        ));
    }

    locks.remove(name);
    ToolAnswer::Done(format!("Released {name}."))
}

fn list(locks: &Map<String, Value>, now: u64) -> String {
    if locks.is_empty() {
        return "No locks are held.".into();
    }

    locks
        .iter()
        .map(|(name, lock)| format!("{name}: {}", holding(lock, now)))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Who holds a lock, for how long so far and until when, and why.
fn holding(lock: &Value, now: u64) -> String {
    let minutes = |ms: u64| ms.div_ceil(60_000);
    let since = now.saturating_sub(lock["since"].as_u64().unwrap_or(now));
    let left = lock["expires"].as_u64().unwrap_or(now).saturating_sub(now);
    let reason = match lock["reason"].as_str() {
        Some(reason) if !reason.is_empty() => format!(", for {reason}"),
        _ => String::new(),
    };

    format!(
        "held by {} (session {}) for {} min{reason}; it frees itself in {} min",
        lock["agent"].as_str().unwrap_or("an agent"),
        lock["holder"].as_str().unwrap_or_default(),
        minutes(since),
        minutes(left)
    )
}

fn lock_name(input: &Value) -> Option<&str> {
    input["name"].as_str().map(str::trim).filter(|name| !name.is_empty())
}

/// The locks whose time has not run out.
fn held(now: u64) -> Map<String, Value> {
    let mut locks = store::get(KEY)
        .and_then(|saved| serde_json::from_str::<Map<String, Value>>(&saved).ok())
        .unwrap_or_default();
    locks.retain(|_, lock| lock["expires"].as_u64().is_some_and(|expires| expires > now));

    locks
}

fn save(locks: &Map<String, Value>) {
    store::set(KEY, &Value::Object(locks.clone()).to_string());
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or_default()
}

export!(Locks);
