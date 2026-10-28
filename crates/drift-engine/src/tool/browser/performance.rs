//! Performance: recording a trace of a page and reading it back as numbers the agent can act on.

use std::fmt::Write as _;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use serde_json::Value;

use super::super::Context;
use super::Step;
use crate::browser::driver::{Driver, ONLY_CHROMIUM, Tab};
use crate::browser::lighthouse::{Audit, Failure, Report};
use crate::browser::performance;

/// Elements listed per failing audit; the saved report has them all.
const NODES_SHOWN: usize = 3;
const SNIPPET: usize = 200;

/// How long a reload's recording runs on after the load, for late paints and shifts.
const AFTER_LOAD: Duration = Duration::from_secs(3);

pub(super) async fn trace(context: &Context, tab: &Arc<dyn Tab>, input: &Value) -> Result<Step, String> {
    let throttle = input["throttle"].as_bool().unwrap_or(false);
    match input["record"].as_str().unwrap_or("reload") {
        "start" => {
            tab.start_trace(throttle).await?;
            Ok(Step::text(
                "Recording. Use the page as a user would (click, type, scroll), then call trace with `record: stop`.",
            ))
        }
        "stop" => finish(context, tab, "Recording").await,
        "reload" => reload(context, tab, throttle).await,
        other => Err(format!("`{other}` is not a way to record; use reload, start or stop")),
    }
}

/// Records the page loading again, and a little after for late paints and shifts.
async fn reload(context: &Context, tab: &Arc<dyn Tab>, throttle: bool) -> Result<Step, String> {
    tab.start_trace(throttle).await?;
    let reloaded = tokio::select! {
        reloaded = tab.go("reload") => reloaded,
        () = context.abort.cancelled() => Err("the trace was stopped".into()),
    };
    if reloaded.is_ok() {
        tokio::select! {
            () = tokio::time::sleep(AFTER_LOAD) => {}
            () = context.abort.cancelled() => {}
        }
    }

    let what = if throttle {
        "Reload with the CPU 4x slower and a slow 4G network"
    } else {
        "Reload"
    };
    let finished = finish(context, tab, what).await;
    reloaded?;

    finished
}

/// A Lighthouse audit of the page: accessibility, best practices and SEO, as scores and the audits that failed.
pub(super) async fn audit(
    context: &Context,
    driver: &Arc<dyn Driver>,
    tab: &Arc<dyn Tab>,
    input: &Value,
) -> Result<Step, String> {
    let port = driver.devtools_port().ok_or(ONLY_CHROMIUM)?;
    let snapshot = match input["mode"].as_str().unwrap_or("navigation") {
        "navigation" => false,
        "snapshot" => true,
        other => return Err(format!("`{other}` is not an audit mode; use navigation or snapshot")),
    };
    let mobile = match input["device"].as_str().unwrap_or("desktop") {
        "desktop" => false,
        "mobile" => true,
        other => return Err(format!("`{other}` is not a device; use desktop or mobile")),
    };
    if !snapshot && !tab.url().starts_with("http") {
        return Err(
            "a navigation audit loads the page again, which needs an http or https address; use `mode: snapshot`"
                .into(),
        );
    }

    let out = output_folder(context)?.join(format!(
        "{}-{}",
        crate::tool::call_file_name(&context.call_id),
        crate::random_hex(3)
    ));
    std::fs::create_dir_all(&out).map_err(|error| error.to_string())?;
    let user_agent = driver.desktop_agent();
    let audit = Audit {
        port,
        target: &tab.id(),
        snapshot,
        mobile,
        user_agent: user_agent.as_deref(),
        out: &out,
    };
    let report = context.engine.lighthouse(&audit, &context.abort).await?;
    driver.refresh().await;

    Ok(Step::text(report_text(&report, snapshot, mobile)))
}

fn report_text(report: &Report, snapshot: bool, mobile: bool) -> String {
    let mode = if snapshot { "snapshot" } else { "navigation" };
    let device = if mobile { "a phone" } else { "desktop" };
    let mut text = format!(
        "Lighthouse {} ({mode}, {device}) of {}
",
        report.version, report.url
    );
    if let Some(error) = &report.runtime_error {
        let _ = writeln!(text, "Lighthouse could not audit the page: {error}");
    }
    for warning in &report.warnings {
        let _ = writeln!(text, "Warning: {warning}");
    }
    let scores: Vec<String> = report
        .scores
        .iter()
        .map(|score| match score.score {
            Some(value) => format!("{} {:.0}", score.title, value * 100.0),
            None => format!("{} (not scored)", score.title),
        })
        .collect();
    let _ = writeln!(text, "Scores: {}", scores.join(", "));
    let _ = writeln!(text, "{} audits passed, {} failed.", report.passed, report.failed.len());

    for failure in &report.failed {
        write_failure(&mut text, failure);
    }
    let _ = write!(
        text,
        "
Full reports: {}",
        report.reports.join(", ")
    );

    text
}

fn write_failure(text: &mut String, failure: &Failure) {
    let value = failure
        .value
        .as_deref()
        .map(|value| format!(", {value}"))
        .unwrap_or_default();
    let _ = writeln!(
        text,
        "
- {} ({}; score {:.0}{value})",
        failure.title,
        failure.categories.join(", "),
        failure.score * 100.0
    );
    for node in failure.nodes.iter().take(NODES_SHOWN) {
        let name = node
            .selector
            .as_deref()
            .or(node.label.as_deref())
            .unwrap_or("an element");
        let _ = writeln!(text, "  - {name}");
        if let Some(snippet) = &node.snippet {
            let _ = writeln!(text, "    {}", clip(snippet));
        }
        if let Some(explanation) = node
            .explanation
            .as_deref()
            .filter(|explanation| !explanation.is_empty())
        {
            let explanation: Vec<&str> = explanation.split_whitespace().collect();
            let _ = writeln!(text, "    {}", clip(&explanation.join(" ")));
        }
    }
    if failure.total_nodes > NODES_SHOWN {
        let _ = writeln!(text, "  - and {} more", failure.total_nodes - NODES_SHOWN);
    }
}

fn clip(text: &str) -> String {
    if text.chars().count() <= SNIPPET {
        return text.to_string();
    }

    let kept: String = text.chars().take(SNIPPET).collect();
    format!("{kept}...")
}

/// Ends the recording, keeps the trace for the user, and summarises it.
async fn finish(context: &Context, tab: &Arc<dyn Tab>, what: &str) -> Result<Step, String> {
    let gzipped = tab.stop_trace().await?;
    let path = saved(context, &gzipped)?;
    let summary = tokio::task::spawn_blocking(move || performance::summarize(&gzipped))
        .await
        .map_err(|error| error.to_string())??;

    Ok(Step::text(format!(
        "{what} of {} traced. The trace is {} (open it in DevTools' Performance panel).\n\n{summary}",
        tab.url(),
        path.display()
    )))
}

/// The session's tool output folder, which is cleared after a week; reading back from it asks nothing.
fn output_folder(context: &Context) -> Result<PathBuf, String> {
    let folder = context.engine.data_dir.join("tool-output").join(&context.session_id);
    std::fs::create_dir_all(&folder).map_err(|error| error.to_string())?;

    Ok(folder)
}

fn saved(context: &Context, gzipped: &[u8]) -> Result<PathBuf, String> {
    let path = output_folder(context)?.join(format!(
        "{}-{}.trace.json.gz",
        crate::tool::call_file_name(&context.call_id),
        crate::random_hex(3)
    ));
    std::fs::write(&path, gzipped).map_err(|error| format!("the trace could not be saved: {error}"))?;

    Ok(path)
}
