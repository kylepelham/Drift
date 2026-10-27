use std::path::{Path, PathBuf};

use serde::Deserialize;
use serde_json::{Value, json};

use super::{Ask, Context, Output, RunFuture, Tool, ToolError, ToolMetadata, canonical};
use crate::llm::ToolSpec;
use crate::session::types::{FindingOutcome, FindingVerdict, ReviewFinding};

/// The most findings one review reports, as Claude's review caps them.
pub const MAX_FINDINGS: usize = 32;
const MAX_SHORT_SUMMARY: usize = 60;
/// A finding's line is recorded only from a file this small, and only this much of it.
const MAX_READ_BYTES: u64 = 1024 * 1024;
const MAX_CODE_CHARS: usize = 400;

pub struct ReportFindings;

#[derive(Deserialize)]
struct Input {
    #[serde(default)]
    level: Option<String>,
    #[serde(default)]
    scope: Option<Scope>,
    #[serde(default)]
    path: Option<String>,
    findings: Vec<Reported>,
}

/// What a review looked at: a change, or the codebase as `/audit` reviews it.
#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Scope {
    Changes,
    Codebase,
}

impl Scope {
    fn name(self) -> &'static str {
        match self {
            Self::Changes => "changes",
            Self::Codebase => "codebase",
        }
    }
}

#[derive(Deserialize)]
struct Reported {
    file: String,
    #[serde(default)]
    line: Option<u32>,
    summary: String,
    #[serde(default)]
    short_summary: Option<String>,
    #[serde(default)]
    failure_scenario: String,
    #[serde(default)]
    category: Option<String>,
    #[serde(default)]
    verdict: Option<FindingVerdict>,
    #[serde(default)]
    outcome: Option<FindingOutcome>,
}

impl Tool for ReportFindings {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "report_findings".into(),
            description: include_str!("prompts/report_findings.txt").trim().into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "level": { "type": "string", "description": "The review level that ran: low, medium, high or max." },
                    "scope": { "type": "string", "enum": ["changes", "codebase"], "description": "What was reviewed: `codebase` for an `/audit` review, else `changes`." },
                    "path": { "type": "string", "description": "The path an `/audit` was limited to, when it named one." },
                    "findings": {
                        "type": "array",
                        "maxItems": MAX_FINDINGS,
                        "description": "Most severe first; empty when nothing survived.",
                        "items": {
                            "type": "object",
                            "properties": {
                                "file": { "type": "string" },
                                "line": { "type": "integer", "minimum": 1 },
                                "summary": { "type": "string", "description": "One sentence stating the defect." },
                                "short_summary": { "type": "string", "maxLength": MAX_SHORT_SUMMARY },
                                "failure_scenario": { "type": "string" },
                                "category": { "type": "string", "description": "A kebab-case slug: correctness, simplification, efficiency, reuse, altitude, conventions, security, test-coverage." },
                                "verdict": { "type": "string", "enum": ["confirmed", "plausible"], "description": "Set when a verify pass ran." },
                                "outcome": { "type": "string", "enum": ["fixed", "skipped", "no_change_needed"], "description": "Only when re-reporting after fixes." }
                            },
                            "required": ["file", "summary", "failure_scenario"]
                        }
                    }
                },
                "required": ["findings"]
            }),
        }
    }

    fn ask(&self, _ctx: &Context, _input: &Value) -> Option<Ask> {
        None
    }

    fn run<'a>(&'a self, ctx: &'a Context, input: Value) -> RunFuture<'a> {
        Box::pin(async move {
            let input: Input =
                serde_json::from_value(input).map_err(|error| ToolError(format!("invalid findings: {error}")))?;
            if input.findings.len() > MAX_FINDINGS {
                return Err(ToolError(format!(
                    "{} findings is more than one review reports; keep the {MAX_FINDINGS} most severe",
                    input.findings.len()
                )));
            }

            let mut findings = Vec::with_capacity(input.findings.len());
            for reported in input.findings {
                findings.push(recorded(&ctx.workspace, reported).await?);
            }

            let level = input
                .level
                .map(|level| level.trim().to_lowercase())
                .filter(|level| !level.is_empty());
            let (title, output) = summary(&findings);

            Ok(Output {
                title,
                output,
                metadata: ToolMetadata {
                    count: Some(findings.len()),
                    findings: Some(findings),
                    level,
                    scope: input.scope.map(|scope| scope.name().into()),
                    path: input
                        .path
                        .map(|path| path.trim().to_string())
                        .filter(|path| !path.is_empty()),
                    ..Default::default()
                },
            })
        })
    }
}

/// A finding as it is kept: its file relative to the workspace and its line's text read from disk.
async fn recorded(workspace: &Path, reported: Reported) -> Result<ReviewFinding, ToolError> {
    let summary = reported.summary.trim().to_string();
    if reported.file.trim().is_empty() || summary.is_empty() {
        return Err(ToolError("every finding needs a `file` and a `summary`".into()));
    }

    let (file, full) = in_workspace(workspace, reported.file.trim());
    let line = reported.line.filter(|line| *line > 0);
    let code = match (&full, line) {
        (Some(path), Some(line)) => line_text(path, line).await,
        _ => None,
    };
    let short_summary = reported
        .short_summary
        .map(|text| text.trim().chars().take(MAX_SHORT_SUMMARY).collect::<String>())
        .filter(|text| !text.is_empty());
    let category = reported
        .category
        .map(|category| category.trim().to_lowercase())
        .filter(|category| !category.is_empty());

    Ok(ReviewFinding {
        file,
        line,
        summary,
        short_summary,
        failure_scenario: reported.failure_scenario.trim().to_string(),
        category,
        verdict: reported.verdict,
        outcome: reported.outcome,
        code,
    })
}

/// The file as the review panel names it, and where it is on disk when it is inside the workspace.
fn in_workspace(workspace: &Path, file: &str) -> (String, Option<PathBuf>) {
    let path = Path::new(file);
    let full = canonical(&if path.is_absolute() {
        path.to_path_buf()
    } else {
        workspace.join(path)
    });

    match full.strip_prefix(workspace) {
        Ok(relative) => (relative.to_string_lossy().replace('\\', "/"), Some(full)),
        Err(_) => (file.replace('\\', "/"), None),
    }
}

async fn line_text(path: &Path, line: u32) -> Option<String> {
    let size = tokio::fs::metadata(path).await.ok()?.len();
    if size > MAX_READ_BYTES {
        return None;
    }

    let bytes = tokio::fs::read(path).await.ok()?;
    let text = String::from_utf8_lossy(&bytes);
    let found = text.lines().nth(line as usize - 1)?;

    Some(found.trim_end_matches('\r').chars().take(MAX_CODE_CHARS).collect())
}

/// The call's title and its one line for the model; the findings themselves are in the call it just made.
fn summary(findings: &[ReviewFinding]) -> (String, String) {
    if findings.is_empty() {
        return ("No findings".into(), "No findings reported.".into());
    }

    let outcomes: Vec<FindingOutcome> = findings.iter().filter_map(|finding| finding.outcome).collect();
    if outcomes.len() < findings.len() {
        let count = findings.len();
        let noun = if count == 1 { "finding" } else { "findings" };
        return (format!("{count} {noun}"), format!("{count} {noun} reported."));
    }

    let tally = |outcome: FindingOutcome| outcomes.iter().filter(|recorded| **recorded == outcome).count();
    let parts: Vec<String> = [
        (FindingOutcome::Fixed, "fixed"),
        (FindingOutcome::Skipped, "skipped"),
        (FindingOutcome::NoChangeNeeded, "needed no change"),
    ]
    .into_iter()
    .filter(|(outcome, _)| tally(*outcome) > 0)
    .map(|(outcome, label)| format!("{} {label}", tally(outcome)))
    .collect();
    let joined = parts.join(", ");

    (joined.clone(), format!("Outcomes recorded: {joined}."))
}

#[cfg(test)]
mod tests {
    use super::super::tests::Sandbox;
    use super::*;

    #[tokio::test]
    async fn findings_keep_their_line_and_a_relative_path() {
        let sandbox = Sandbox::new("findings");
        let absolute = sandbox.file("src/lib.rs", "fn a() {}\r\nfn b() { a(); }\n");
        let input = json!({
            "level": "High",
            "scope": "codebase",
            "path": " src ",
            "findings": [
                {
                    "file": absolute.to_string_lossy(),
                    "line": 2,
                    "summary": "b calls a twice ",
                    "short_summary": "x".repeat(80),
                    "failure_scenario": "any call",
                    "category": "Correctness",
                    "verdict": "confirmed"
                },
                { "file": "../outside.rs", "line": 1, "summary": "far away", "failure_scenario": "" }
            ]
        });

        let out = ReportFindings.run(&sandbox.ctx, input).await.unwrap();
        let findings = out.metadata.findings.unwrap();

        assert_eq!(out.output, "2 findings reported.");
        assert_eq!(out.metadata.level.as_deref(), Some("high"));
        assert_eq!(out.metadata.scope.as_deref(), Some("codebase"));
        assert_eq!(out.metadata.path.as_deref(), Some("src"));
        assert_eq!(findings[0].file, "src/lib.rs");
        assert_eq!(
            findings[0].code.as_deref(),
            Some("fn b() { a(); }"),
            "the line, without its CR"
        );
        assert_eq!(findings[0].summary, "b calls a twice");
        assert_eq!(
            findings[0].short_summary.as_ref().unwrap().chars().count(),
            MAX_SHORT_SUMMARY
        );
        assert_eq!(findings[0].category.as_deref(), Some("correctness"));
        assert_eq!(findings[0].verdict, Some(FindingVerdict::Confirmed));
        assert!(findings[1].code.is_none(), "nothing outside the workspace is read");
    }

    #[tokio::test]
    async fn outcomes_empty_reports_and_bad_input() {
        let sandbox = Sandbox::new("findings-outcomes");
        let finding =
            |outcome: &str| json!({ "file": "a.rs", "summary": "s", "failure_scenario": "f", "outcome": outcome });

        let fixed = ReportFindings
            .run(
                &sandbox.ctx,
                json!({ "findings": [finding("fixed"), finding("fixed"), finding("no_change_needed")] }),
            )
            .await
            .unwrap();
        assert_eq!(fixed.output, "Outcomes recorded: 2 fixed, 1 needed no change.");

        let none = ReportFindings
            .run(&sandbox.ctx, json!({ "findings": [] }))
            .await
            .unwrap();
        assert_eq!((none.title.as_str(), none.metadata.count), ("No findings", Some(0)));
        assert!(none.metadata.scope.is_none());

        let many: Vec<Value> = (0..=MAX_FINDINGS).map(|_| finding("fixed")).collect();
        assert!(
            ReportFindings
                .run(&sandbox.ctx, json!({ "findings": many }))
                .await
                .is_err()
        );
        let blank = json!({ "findings": [{ "file": " ", "summary": "s", "failure_scenario": "f" }] });
        assert!(ReportFindings.run(&sandbox.ctx, blank).await.is_err());
        let verdict =
            json!({ "findings": [{ "file": "a", "summary": "s", "failure_scenario": "f", "verdict": "REFUTED" }] });
        assert!(ReportFindings.run(&sandbox.ctx, verdict).await.is_err());
    }
}
