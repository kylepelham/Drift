//! `/review`: the engine's own skill. It renders review instructions for the workspace's changes or a named
//! target, at a level that sets how many finder subagents run, and the model reports through `report_findings`.

pub mod audit;
mod scope;
#[cfg(test)]
mod tests;

use std::path::Path;

use crate::config::{Builtin, Skill, split_arguments};
use scope::Scope;

pub const NAME: &str = "review";
/// Changed lines one finder subagent covers.
const LINES_PER_FINDER: usize = 150;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Level {
    Low,
    #[default]
    Medium,
    High,
    Max,
}

impl Level {
    /// `xhigh` is Claude Code's name for the level below its max; here it runs as max.
    fn parse(word: &str) -> Option<Self> {
        match word.to_ascii_lowercase().as_str() {
            "low" => Some(Self::Low),
            "medium" => Some(Self::Medium),
            "high" => Some(Self::High),
            "max" | "xhigh" => Some(Self::Max),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
            Self::Max => "max",
        }
    }

    /// The fewest and most finder subagents; low runs none.
    fn finders(self) -> Option<(usize, usize)> {
        match self {
            Self::Low => None,
            Self::Medium => Some((2, 4)),
            Self::High => Some((3, 6)),
            Self::Max => Some((4, 8)),
        }
    }

    /// The most findings one report carries.
    fn cap(self) -> usize {
        match self {
            Self::Low => 5,
            Self::Medium => 8,
            Self::High => 10,
            Self::Max => 15,
        }
    }

    /// Which verified candidates survive.
    fn keep(self) -> &'static str {
        if self == Self::Medium {
            "Keep confirmed findings, and plausible ones only where the failure would lose data, corrupt state or open \
             a security hole: at this level precision matters more than coverage."
        } else {
            "Keep confirmed and plausible findings: at this level coverage matters more than precision."
        }
    }
}

/// What follows `/review`.
#[derive(Debug, Default, PartialEq)]
pub struct Request {
    pub level: Level,
    pub target: Option<String>,
    pub fix: bool,
    pub comment: bool,
}

/// The first level named wins; other words, joined, are the target, and unknown flags are dropped.
pub fn parse(arguments: &str) -> Request {
    let mut request = Request::default();
    let mut level = None;
    let mut target = Vec::new();

    for word in split_arguments(arguments) {
        match word.as_str() {
            "--fix" => request.fix = true,
            "--comment" => request.comment = true,
            flag if flag.starts_with("--") => {}
            other => match Level::parse(other).filter(|_| level.is_none()) {
                Some(named) => level = Some(named),
                None => target.push(word),
            },
        }
    }

    request.level = level.unwrap_or_default();
    request.target = Some(target.join(" ")).filter(|target| !target.is_empty());
    request
}

/// `/review` as config lists it; its instructions are the usage the slash menu reads, not the prompt.
pub fn skill() -> Skill {
    Skill {
        name: NAME.into(),
        description: "Reviews the current changes for bugs, regressions and needless complexity, and reports \
                      findings the user can walk through in the diff. Use when asked for a code review."
            .into(),
        path: String::new(),
        instructions: include_str!("prompts/usage.md").trim().into(),
        argument_hint: Some("[low|medium|high|max] [target] [--fix] [--comment]".into()),
        builtin: Some(Builtin::Review),
    }
}

/// The review instructions for `arguments`, measuring the workspace's changes when no target is named.
pub async fn instructions(workspace: &Path, arguments: &str) -> String {
    let request = parse(arguments);
    let scope = match &request.target {
        Some(_) => None,
        None => Some(scope::resolve(workspace).await),
    };

    render(&request, scope.as_ref())
}

/// `scope` is `None` when the request names a target.
fn render(request: &Request, scope: Option<&Scope>) -> String {
    let level = request.level;
    let mut sections = vec![fill(
        include_str!("prompts/intro.md"),
        &[("level", level.name()), ("edits", edits(request.fix))],
    )];

    let lines = match scope {
        Some(Scope::NotRepo) => return finish(sections, include_str!("prompts/scope_none.md")),
        Some(Scope::Unlisted(command)) => {
            let text = fill(include_str!("prompts/scope_failed.md"), &[("command", command)]);
            return finish(sections, &text);
        }
        Some(Scope::Clean { base_name }) => {
            let text = fill(include_str!("prompts/scope_clean.md"), &[("base_name", base_name)]);
            return finish(sections, &text);
        }
        Some(Scope::Changes(changes)) => {
            sections.push(fill(
                include_str!("prompts/scope_changes.md"),
                &[
                    ("base", &changes.base),
                    ("base_name", &changes.base_name),
                    ("untracked", &changes.untracked_sentence()),
                    ("lines", &changes.lines.to_string()),
                    ("files", &changes.files.to_string()),
                ],
            ));
            Some(changes.lines)
        }
        None => {
            let target = request.target.as_deref().unwrap_or_default();
            sections.push(fill(include_str!("prompts/scope_target.md"), &[("target", target)]));
            None
        }
    };

    sections.push(include_str!("prompts/conventions.md").into());
    sections.push(method(level, lines));
    sections.push(report(level, "", level.cap(), "the change"));
    if request.comment {
        sections.push(include_str!("prompts/comment.md").into());
    }
    sections.push(after(request.fix, "change"));

    sections.join("\n").trim_end().to_string()
}

/// What the main agent may change while it reviews.
fn edits(fix: bool) -> &'static str {
    if fix {
        "Edit nothing until the findings are reported; the Fix section below says what to do then."
    } else {
        "Do not edit, write or format any file, and do not commit: this is a review only."
    }
}

fn finish(mut sections: Vec<String>, scope: &str) -> String {
    sections.push(scope.into());
    sections.join("\n").trim_end().to_string()
}

/// How to find and check candidates at `level`, with the finder count for `lines` changed lines when known.
fn method(level: Level, lines: Option<usize>) -> String {
    let Some((fewest, most)) = level.finders() else {
        return include_str!("prompts/low.md").into();
    };

    let finders = match lines {
        Some(lines) => format!(
            "{} finder subagents",
            lines.div_ceil(LINES_PER_FINDER).clamp(fewest, most)
        ),
        None => {
            format!("finder subagents, about one per {LINES_PER_FINDER} changed lines and {fewest} to {most} in all")
        }
    };
    let mut angles = include_str!("prompts/angles.md").trim_end().to_string();
    if level == Level::Max {
        angles = format!("{angles}\n{}", include_str!("prompts/angles_max.md").trim_end());
    }

    let find = fill(
        include_str!("prompts/pipeline.md"),
        &[("finders", &finders), ("angles", &angles)],
    );
    format!("{find}\n{}", checks(level))
}

/// The verify step, then at max the sweep.
fn checks(level: Level) -> String {
    let verify = fill(include_str!("prompts/verify.md"), &[("keep", level.keep())]);
    if level == Level::Max {
        return format!("{verify}\n{}", include_str!("prompts/sweep.md"));
    }
    verify
}

/// The report section; `scope` follows the level in its call and `subject` is what its last sentence is on.
fn report(level: Level, scope: &str, cap: usize, subject: &str) -> String {
    let verdict = if level == Level::Low {
        ""
    } else {
        ", with `verdict` from the verify step"
    };
    fill(
        include_str!("prompts/report.md"),
        &[
            ("level", level.name()),
            ("scope", scope),
            ("cap", &cap.to_string()),
            ("verdict", verdict),
            ("subject", subject),
        ],
    )
}

/// Fixing the findings, or the reminder to report outcomes later; `subject` is what a fix must not undo.
fn after(fix: bool, subject: &str) -> String {
    if fix {
        fill(include_str!("prompts/fix.md"), &[("subject", subject)])
    } else {
        include_str!("prompts/afterwards.md").into()
    }
}

/// `{key}` replaced by its value for each pair; braces naming anything else stay, as `gh api` reads them.
fn fill(template: &str, values: &[(&str, &str)]) -> String {
    values.iter().fold(template.to_string(), |text, (key, value)| {
        text.replace(&format!("{{{key}}}"), value)
    })
}
