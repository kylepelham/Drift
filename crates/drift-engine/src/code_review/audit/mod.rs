//! `/audit`: the review of the whole codebase, or one folder or file of it, as it is now rather than a diff. The
//! engine lists and counts the files, groups them into areas and splits those among finders by lines.

mod survey;
#[cfg(test)]
mod tests;

use std::path::Path;

use super::{Level, Request, after, checks, edits, fill, finish, parse, report};
use crate::config::{Builtin, Skill};
use survey::{Survey, Surveyed};

pub const NAME: &str = "audit";
/// Lines of code one finder subagent covers.
const LINES_PER_FINDER: usize = 15_000;
/// How finders split the work when the engine could not list the files.
const UNSURVEYED_PARTS: &str = "Split the folders you listed into that many parts, balanced by lines, and never run \
                                more finders than there are folders.";

/// The fewest and most finder subagents; low runs none.
fn finders(level: Level) -> Option<(usize, usize)> {
    match level {
        Level::Low => None,
        Level::Medium => Some((3, 6)),
        Level::High => Some((5, 10)),
        Level::Max => Some((8, 16)),
    }
}

/// The most findings one report carries.
fn cap(level: Level) -> usize {
    match level {
        Level::Low => 8,
        Level::Medium => 15,
        Level::High => 25,
        Level::Max => 32,
    }
}

/// `/audit` as config lists it; its instructions are the usage the slash menu reads, not the prompt.
pub fn skill() -> Skill {
    Skill {
        name: NAME.into(),
        description: "Reviews the whole codebase, or one folder of it, for bugs, risky code, security holes, dead \
                      code and needless complexity, and reports findings the user can walk through. Use when asked \
                      to review or audit the codebase rather than a change."
            .into(),
        path: String::new(),
        instructions: include_str!("prompts/usage.md").trim().into(),
        argument_hint: Some("[low|medium|high|max] [path] [--fix]".into()),
        builtin: Some(Builtin::Audit),
    }
}

/// The audit instructions for `arguments`, surveying the workspace or the path they name.
pub async fn instructions(workspace: &Path, arguments: &str) -> String {
    let request = parse(arguments);
    let surveyed = survey::run(workspace, request.target.as_deref()).await;

    render(&request, &surveyed)
}

fn render(request: &Request, surveyed: &Surveyed) -> String {
    let level = request.level;
    let target = request.target.as_deref();
    let mut sections = vec![fill(
        include_str!("prompts/intro.md"),
        &[("level", level.name()), ("edits", edits(request.fix))],
    )];

    let survey = match surveyed {
        Surveyed::Empty => {
            let place = target.map_or_else(|| "the workspace".into(), |target| format!("`{target}`"));
            return finish(
                sections,
                &fill(include_str!("prompts/scope_empty.md"), &[("place", &place)]),
            );
        }
        Surveyed::NotRepo => {
            let under = target.map(|target| format!(" under `{target}`")).unwrap_or_default();
            sections.push(fill(include_str!("prompts/scope_none.md"), &[("under", &under)]));
            None
        }
        Surveyed::Files(survey) => {
            sections.push(scope(survey, target));
            Some(survey)
        }
    };

    sections.push(include_str!("../prompts/conventions.md").into());
    sections.push(method(level, survey));
    sections.push(report(level, ", `scope` `codebase`", cap(level), "the codebase"));
    sections.push(after(request.fix, "code"));

    sections.join("\n").trim_end().to_string()
}

fn scope(survey: &Survey, target: Option<&str>) -> String {
    let subject = target.map_or_else(|| "The whole workspace".into(), |target| format!("`{target}`"));
    let unmeasured = match survey.unmeasured {
        0 => String::new(),
        more => format!(" {more} more files were past the first 20000 and not measured."),
    };

    fill(
        include_str!("prompts/scope.md"),
        &[
            ("subject", &subject),
            ("lines", &survey.lines().to_string()),
            ("files", &survey.files.len().to_string()),
            ("unmeasured", &unmeasured),
            ("hot_spots", &hot_spots(survey)),
        ],
    )
}

/// The files changed most lately, then the largest.
fn hot_spots(survey: &Survey) -> String {
    let churn = if survey.churn.is_empty() {
        "Nothing here changed in the last 90 days.".to_string()
    } else {
        let listed: Vec<String> = survey
            .churn
            .iter()
            .map(|(path, commits)| format!("- `{path}` ({commits})"))
            .collect();
        format!("Changed most in the last 90 days, by commits:\n\n{}", listed.join("\n"))
    };
    let largest: Vec<String> = survey
        .largest()
        .iter()
        .map(|file| format!("- `{}` ({} lines)", file.path, file.lines))
        .collect();

    format!("{churn}\n\nLargest:\n\n{}", largest.join("\n"))
}

/// How to find and check candidates at `level`, with each finder's areas when the files were surveyed.
fn method(level: Level, survey: Option<&Survey>) -> String {
    let Some((fewest, most)) = finders(level) else {
        return include_str!("prompts/low.md").into();
    };

    let (finders, parts) = match survey {
        Some(survey) => assignments(survey, fewest, most),
        None => (
            format!("finder subagents, about one per {LINES_PER_FINDER} lines and {fewest} to {most} in all"),
            UNSURVEYED_PARTS.into(),
        ),
    };
    let mut angles = include_str!("prompts/angles.md").trim_end().to_string();
    if level == Level::Max {
        angles = format!("{angles}\n{}", include_str!("prompts/angles_max.md").trim_end());
    }

    let find = fill(
        include_str!("prompts/find.md"),
        &[("finders", &finders), ("parts", &parts), ("angles", &angles)],
    );
    format!("{find}\n{}", checks(level))
}

/// The finder count for the survey's lines, never more than its areas, and each finder's areas.
fn assignments(survey: &Survey, fewest: usize, most: usize) -> (String, String) {
    let areas = survey.areas();
    let count = survey
        .lines()
        .div_ceil(LINES_PER_FINDER)
        .clamp(fewest, most)
        .min(areas.len())
        .max(1);
    let listed: Vec<String> = survey::balance(&areas, count)
        .iter()
        .enumerate()
        .map(|(at, (lines, part))| {
            let named: Vec<String> = part
                .iter()
                .map(|area| format!("{} ({})", area.label(), area.lines))
                .collect();
            format!("- Finder {}, about {lines} lines: {}", at + 1, named.join(", "))
        })
        .collect();
    let noun = if count == 1 { "subagent" } else { "subagents" };

    (
        format!("{count} finder {noun}"),
        format!(
            "The engine grouped the files into {} areas and balanced them by lines, one part per finder:\n\n{}",
            areas.len(),
            listed.join("\n")
        ),
    )
}
