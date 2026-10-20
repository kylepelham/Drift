//! Why a request's cache read fell short. Each request is hashed segment by segment as it was sent:
//! the tool definitions, each section of the system prompt and each message. When a reply reads far
//! less from the cache than the previous request of its session held in all, the first segment that
//! changed is logged in one line and kept on the reply, where the context meter shows it.

use std::collections::HashMap;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::sync::Mutex;

use crate::llm::Request;
use crate::session::types::Usage;

/// A previous request this small is not worth reporting on: a cache never starts below it.
const MIN_PROMPT_TOKENS: u64 = 1024;

/// A provider keeps a cache at least this long; a miss after a longer pause is most likely its expiry.
const SHORTEST_CACHE_MS: i64 = 5 * 60 * 1000;

/// What a request was made of, as hashes by segment, how many prompt tokens it came to, and when it was answered.
struct Sent {
    segments: Vec<(String, u64)>,
    prompt_tokens: u64,
    at: i64,
}

#[derive(Default)]
pub(in crate::session) struct CacheWatch(Mutex<HashMap<String, Sent>>);

impl CacheWatch {
    /// Records the request `usage` answered, by its `segments`, and says why when its cache read fell short.
    pub(super) fn observe(&self, session_id: &str, segments: Vec<(String, u64)>, usage: Usage) -> Option<String> {
        let sent = Sent {
            segments,
            prompt_tokens: usage.input + usage.cache_read + usage.cache_write,
            at: crate::id::now_ms(),
        };
        let previous = self.0.lock().unwrap().insert(session_id.to_string(), sent)?;
        if previous.prompt_tokens < MIN_PROMPT_TOKENS || usage.cache_read * 2 >= previous.prompt_tokens {
            return None;
        }

        let now = &self.0.lock().unwrap()[session_id];
        let idle_minutes = (now.at - previous.at) / 60_000;
        let changed = first_change(&previous.segments, &now.segments).unwrap_or_else(|| {
            if now.at - previous.at >= SHORTEST_CACHE_MS {
                format!("nothing; the cache expired over {idle_minutes} idle minutes")
            } else {
                "nothing (the provider dropped the cache)".into()
            }
        });
        Some(format!(
            "read {} cached tokens of about {} expected; the first change was {changed}",
            usage.cache_read, previous.prompt_tokens
        ))
    }

    pub(in crate::session) fn forget(&self, session_id: &str) {
        self.0.lock().unwrap().remove(session_id);
    }
}

fn hash(value: impl Hash) -> u64 {
    let mut hasher = DefaultHasher::new();
    value.hash(&mut hasher);
    hasher.finish()
}

/// The request in the order a provider caches it: tools, the system prompt by section, then messages.
pub(super) fn segments(request: &Request) -> Vec<(String, u64)> {
    let tools = serde_json::to_string(&request.tools).unwrap_or_default();
    let mut found = vec![("the tool definitions".to_string(), hash(tools))];
    found.extend(system_sections(&request.system));
    found.extend(request.messages.iter().enumerate().map(|(index, message)| {
        // Debug output is stable within a run, which is as long as these hashes live.
        (format!("message {}", index + 1), hash(format!("{message:?}")))
    }));

    found
}

/// The system prompt cut at its `# ` headings, each section named by its heading.
fn system_sections(system: &str) -> Vec<(String, u64)> {
    let mut sections = Vec::new();
    let mut name = "the base prompt".to_string();
    let mut text = String::new();
    for line in system.lines() {
        if let Some(heading) = line.strip_prefix("# ") {
            sections.push((std::mem::take(&mut name), hash(&text)));
            name = format!("the system prompt's \"{heading}\" section");
            text.clear();
        }
        text.push_str(line);
        text.push('\n');
    }
    sections.push((name, hash(&text)));

    sections
}

/// The first segment that differs, in order; one present before and gone now counts as changed.
pub(super) fn first_change(before: &[(String, u64)], now: &[(String, u64)]) -> Option<String> {
    before
        .iter()
        .zip(now)
        .find(|(old, new)| old != new)
        .map(|(_, new)| new.0.clone())
        .or_else(|| before.get(now.len()).map(|gone| format!("{} (removed)", gone.0)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::{Block, ChatMessage, Role};

    fn request(system: &str, messages: &[&str]) -> Request {
        Request {
            model: "m".into(),
            system: system.into(),
            messages: messages
                .iter()
                .map(|text| ChatMessage {
                    role: Role::User,
                    blocks: vec![Block::Text((*text).into())],
                })
                .collect(),
            tools: Vec::new(),
            max_tokens: 100,
            reasoning: None,
            temperature: None,
            cache_key: None,
            connection: None,
            effort_marks: Vec::new(),
            claude: Default::default(),
            cache_hour: false,
            cache_conversation: true,
            deferred_tools: Vec::new(),
            tool_references: Default::default(),
            no_tool_calls: false,
            verbosity: None,
            show_thinking: false,
            top_p: None,
            top_k: None,
            mode: None,
            context_window: None,
        }
    }

    fn usage(input: u64, cache_read: u64) -> Usage {
        Usage {
            input,
            output: 10,
            cache_read,
            cache_write: 0,
        }
    }

    fn seen(watch: &CacheWatch, session: &str, system: &str, messages: &[&str], usage: Usage) -> Option<String> {
        watch.observe(session, segments(&request(system, messages)), usage)
    }

    #[test]
    fn a_short_cache_read_names_the_first_segment_that_changed() {
        let watch = CacheWatch::default();
        let base = "You help.\n\n# Environment\n\nDate: 1\n\n# Instructions from AGENTS.md\n\nBe brief.";
        assert_eq!(
            seen(&watch, "s", base, &["one"], usage(5000, 0)),
            None,
            "nothing before it"
        );

        let edited = base.replace("Be brief.", "Be thorough.");
        let report = seen(&watch, "s", &edited, &["one", "two"], usage(5200, 100)).unwrap();
        assert!(
            report.contains("\"Instructions from AGENTS.md\" section")
                && report.contains("read 100 cached tokens of about 5000"),
            "{report}"
        );

        assert_eq!(
            seen(&watch, "s", &edited, &["one", "two", "three"], usage(300, 5000)),
            None,
            "a cache read close to the previous prompt is fine"
        );
        let dropped = seen(&watch, "s", &edited, &["one", "two", "three", "four"], usage(5500, 0)).unwrap();
        assert!(dropped.contains("the provider dropped the cache"), "{dropped}");
    }

    #[test]
    fn a_small_previous_request_and_another_session_say_nothing() {
        let watch = CacheWatch::default();
        seen(&watch, "s", "a", &["one"], usage(500, 0));
        assert_eq!(seen(&watch, "s", "b", &["one"], usage(600, 0)), None);
        assert_eq!(seen(&watch, "other", "b", &["one"], usage(6000, 0)), None);
        watch.forget("other");
        assert_eq!(seen(&watch, "other", "c", &["one"], usage(6000, 0)), None);
    }
}
