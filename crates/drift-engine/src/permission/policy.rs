use crate::tool::Ask;
use globset::GlobBuilder;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum Decision {
    Allow,
    Deny,
    Ask,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct Rule {
    pub kind: String,
    pub pattern: String,
    pub decision: Decision,
}

/// Ordered rules; the first match wins, otherwise the operation's default applies.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Policy {
    pub rules: Vec<Rule>,
}

#[derive(Clone, Copy)]
pub struct Policies<'a> {
    pub workspace: &'a Policy,
    pub agent: &'a Policy,
}

/// Rules with their globs compiled, for checking many targets against the same policy; the first match wins.
pub struct Compiled(Vec<(Rule, Option<globset::GlobMatcher>)>);

impl Rule {
    fn matches(&self, ask: &Ask) -> bool {
        ask.targets()
            .iter()
            .any(|target| self.matches_target(&ask.kind, target))
    }

    pub(super) fn matches_target(&self, kind: &str, target: &str) -> bool {
        if self.kind != kind && self.kind != "*" {
            return false;
        }

        GlobBuilder::new(&self.pattern)
            .literal_separator(false)
            .build()
            .is_ok_and(|glob| glob.compile_matcher().is_match(target))
    }

    pub(super) fn has_wildcards(&self) -> bool {
        self.pattern.contains(['*', '?', '[', '{'])
    }

    /// Why the rule could never match as written: a kind that names no operation, or a pattern that is no glob.
    pub fn problem(&self) -> Option<String> {
        let kind_ok = self.kind == "*"
            || (!self.kind.is_empty()
                && self
                    .kind
                    .chars()
                    .all(|character| character.is_ascii_lowercase() || character == '-' || character == '_'));
        if !kind_ok {
            return Some(format!("\"{}\" is not a permission kind", self.kind));
        }
        if self.pattern.trim().is_empty() {
            return Some(format!("a {} rule needs a pattern; use * for every target", self.kind));
        }

        GlobBuilder::new(&self.pattern)
            .build()
            .err()
            .map(|error| format!("\"{}\" is not a valid pattern: {error}", self.pattern))
    }
}

impl Policy {
    pub fn explicit(&self, ask: &Ask) -> Option<Decision> {
        self.rules
            .iter()
            .find(|rule| rule.matches(ask))
            .map(|rule| rule.decision)
    }

    pub fn decide(&self, ask: &Ask) -> Decision {
        self.explicit(ask).unwrap_or_else(|| fallback(ask.default_allow))
    }
}

impl Compiled {
    pub(crate) fn new(rules: impl IntoIterator<Item = Rule>) -> Self {
        let compiled = rules
            .into_iter()
            .map(|rule| {
                let matcher = GlobBuilder::new(&rule.pattern)
                    .literal_separator(false)
                    .build()
                    .ok()
                    .map(|glob| glob.compile_matcher());
                (rule, matcher)
            })
            .collect();

        Self(compiled)
    }

    pub fn explicit(&self, ask: &Ask) -> Option<Decision> {
        let targets = ask.targets();

        self.0
            .iter()
            .find(|(rule, matcher)| {
                (rule.kind == ask.kind || rule.kind == "*")
                    && matcher
                        .as_ref()
                        .is_some_and(|matcher| targets.iter().any(|target| matcher.is_match(target)))
            })
            .map(|(rule, _)| rule.decision)
    }

    /// Whether every ask of `kind` is denied: the first rule for it that covers everything (`*`)
    /// denies, and no rule before it allows or asks about anything narrower.
    pub fn denies_all(&self, kind: &str) -> bool {
        for (rule, _) in self.0.iter().filter(|(rule, _)| rule.kind == kind || rule.kind == "*") {
            if rule.pattern == "*" {
                return rule.decision == Decision::Deny;
            }
            if rule.decision != Decision::Deny {
                return false;
            }
        }

        false
    }
}

pub(super) fn fallback(allow: bool) -> Decision {
    if allow { Decision::Allow } else { Decision::Ask }
}
