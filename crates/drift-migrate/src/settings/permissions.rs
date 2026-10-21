//! opencode's permission rules as Drift's: v1's map per kind and v2's ordered `permissions`; in both the last match wins.

use super::SettingsReport;
use super::ordered::{Kinds, Setting};
use serde_json::{Value, json};

const DECISIONS: [&str; 3] = ["allow", "ask", "deny"];

/// opencode's permission kinds (v1) and actions (v2) as Drift's kinds.
/// v2 renamed `bash` to `shell` and `task` to `subagent`, and folded `write` and `patch` into `edit`.
pub(super) fn drift_kind(action: &str) -> Option<&'static str> {
    Some(match action {
        "*" => "*",
        "read" => "read",
        "edit" | "write" | "patch" => "edit",
        "bash" | "shell" => "bash",
        "task" | "subagent" => "task",
        "webfetch" => "webfetch",
        "glob" => "glob",
        "grep" => "grep",
        "skill" => "skill",
        _ => return None,
    })
}

/// v1's decisions and pattern maps as rules in written order; `at` names where they were found, for the report.
pub(super) fn from_kinds(kinds: &Kinds, at: &str, report: &mut SettingsReport) -> Vec<Value> {
    let mut rules = Vec::new();

    for (action, setting) in kinds {
        let Some(kind) = drift_kind(action) else {
            report.setting(&format!("{at}.{action}"), "Drift has no such permission");
            continue;
        };

        let patterns: Vec<(&str, Value)> = match setting {
            Setting::One(decision) => vec![("*", Value::String(decision.clone()))],
            Setting::Patterns(entries) => entries
                .0
                .iter()
                .map(|(pattern, decision)| (pattern.as_str(), decision.clone()))
                .collect(),
            Setting::Other(_) => Vec::new(),
        };
        for (pattern, decision) in patterns {
            rules.extend(rule(
                kind,
                pattern,
                &decision,
                &format!("{at}.{action} {pattern}"),
                report,
            ));
        }
    }

    rules
}

/// v2's `{ action, resource, effect }` list as rules in the same order.
pub(super) fn from_list(list: &Value, at: &str, report: &mut SettingsReport) -> Vec<Value> {
    let Some(list) = list.as_array() else {
        report.setting(at, "not a list of rules");
        return Vec::new();
    };

    let mut rules = Vec::new();
    for entry in list {
        let action = entry["action"].as_str().unwrap_or_default();
        let pattern = entry["resource"].as_str().unwrap_or("*");
        match drift_kind(action) {
            Some(kind) => rules.extend(rule(
                kind,
                pattern,
                &entry["effect"],
                &format!("{at}.{action} {pattern}"),
                report,
            )),
            None => report.setting(&format!("{at}.{action}"), "Drift has no such permission"),
        }
    }

    rules
}

fn rule(kind: &str, pattern: &str, decision: &Value, name: &str, report: &mut SettingsReport) -> Option<Value> {
    match decision.as_str().filter(|decision| DECISIONS.contains(decision)) {
        Some(decision) => Some(json!({ "kind": kind, "pattern": pattern, "decision": decision })),
        None => {
            report.setting(name, "not allow, ask or deny");
            None
        }
    }
}
