//! Codex's sticky routing: the backend answers a turn's first request with `x-codex-turn-state`, and the turn's
//! later requests send it back so they stay on one backend; a new turn starts without one.

use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};

use reqwest::header::HeaderMap;

use crate::llm::Request;

pub(super) const HEADER: &str = "x-codex-turn-state";
/// Conversations remembered at once; past this the map starts over, which costs one request its pin.
const KEPT: usize = 256;

/// Per conversation: the turn the token was given for, and the token.
static TOKENS: LazyLock<Mutex<HashMap<String, (String, String)>>> = LazyLock::new(Mutex::default);

/// Where a request belongs: its conversation, and the prompt that started its turn.
#[derive(Clone, Debug)]
pub(super) struct Turn {
    connection: String,
    started: String,
}

impl Turn {
    pub(super) fn of(request: &Request) -> Option<Self> {
        Some(Self {
            connection: request.connection.clone()?,
            started: request.turn.clone()?,
        })
    }

    /// The token this turn was given, if it was given one.
    pub(super) fn token(&self) -> Option<String> {
        let tokens = TOKENS.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let (started, token) = tokens.get(&self.connection)?;

        (started == &self.started).then(|| token.clone())
    }

    /// Keeps the token a response named, unless this turn already has one: the first stays for the whole turn.
    pub(super) fn note(&self, headers: &HeaderMap) {
        let Some(token) = headers.get(HEADER).and_then(|value| value.to_str().ok()) else {
            return;
        };
        let mut tokens = TOKENS.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        if tokens
            .get(&self.connection)
            .is_some_and(|(started, _)| started == &self.started)
        {
            return;
        }
        if tokens.len() >= KEPT && !tokens.contains_key(&self.connection) {
            tokens.clear();
        }

        tokens.insert(self.connection.clone(), (self.started.clone(), token.to_string()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn turn(connection: &str, started: &str) -> Turn {
        Turn {
            connection: connection.into(),
            started: started.into(),
        }
    }

    fn named(token: &str) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(HEADER, token.parse().unwrap());
        headers
    }

    #[test]
    fn a_turn_keeps_its_first_token_and_the_next_turn_starts_without_one() {
        let first = turn("ses_turn_state", "msg_1");
        assert_eq!(first.token(), None);

        first.note(&named("a"));
        first.note(&named("b"));
        assert_eq!(first.token().as_deref(), Some("a"));

        let next = turn("ses_turn_state", "msg_2");
        assert_eq!(next.token(), None, "a token never crosses turns");
        next.note(&named("c"));
        assert_eq!(next.token().as_deref(), Some("c"));
        assert_eq!(first.token(), None);
        assert_eq!(turn("ses_other", "msg_2").token(), None);
    }
}
