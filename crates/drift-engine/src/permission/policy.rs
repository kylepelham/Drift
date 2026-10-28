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
    /// Rules consulted only after every other, the global policy's too: a project's allow rules.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub after_global: Vec<Rule>,
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

        self.matcher()
            .is_some_and(|matcher| matcher.is_match(system_form(target).as_ref()))
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

    fn matcher(&self) -> Option<globset::GlobMatcher> {
        GlobBuilder::new(&system_form(&self.pattern))
            .literal_separator(false)
            .case_insensitive(cfg!(windows))
            .build()
            .ok()
            .map(|glob| glob.compile_matcher())
    }
}

/// Windows paths ignore case and take either slash, so there rules and targets match as the system reads them.
fn system_form(text: &str) -> std::borrow::Cow<'_, str> {
    if cfg!(windows) {
        text.replace('\\', "/").into()
    } else {
        text.into()
    }
}

impl Policy {
    pub fn explicit(&self, ask: &Ask) -> Option<Decision> {
        self.rules
            .iter()
            .chain(&self.after_global)
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
                let matcher = rule.matcher();
                (rule, matcher)
            })
            .collect();

        Self(compiled)
    }

    pub fn explicit(&self, ask: &Ask) -> Option<Decision> {
        let targets: Vec<_> = ask.targets().into_iter().map(system_form).collect();

        self.0
            .iter()
            .find(|(rule, matcher)| {
                (rule.kind == ask.kind || rule.kind == "*")
                    && matcher
                        .as_ref()
                        .is_some_and(|matcher| targets.iter().any(|target| matcher.is_match(target.as_ref())))
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
