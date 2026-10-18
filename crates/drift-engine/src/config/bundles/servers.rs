//! A bundle's MCP servers as Drift saves them: `.mcp.json` entries with `${...}` filled in from the
//! bundle's own paths and the values the user gave on install.

use std::collections::BTreeMap;

use serde_json::Value;

use crate::mcp::ServerConfig;

/// What `${NAME}` (or `${NAME:-default}`) stands for in a bundle's server definitions.
pub(super) struct Vars<'a> {
    pub(super) root: String,
    pub(super) data: String,
    pub(super) values: &'a BTreeMap<String, String>,
}

impl Vars<'_> {
    fn get(&self, name: &str) -> Option<String> {
        match name {
            "CLAUDE_PLUGIN_ROOT" => Some(self.root.clone()),
            "CLAUDE_PLUGIN_DATA" => Some(self.data.clone()),
            _ => self.values.get(name).filter(|value| !value.trim().is_empty()).cloned(),
        }
    }
}

/// The text with every `${...}` filled in; none when one names a value nobody gave and has no default.
pub(super) fn expand(text: &str, vars: &Vars) -> Option<String> {
    let mut out = String::new();
    let mut rest = text;

    while let Some(start) = rest.find("${") {
        let Some(length) = rest[start..].find('}') else { break };
        let inner = &rest[start + 2..start + length];
        let (name, default) = match inner.split_once(":-") {
            Some((name, default)) => (name, Some(default)),
            None => (inner, None),
        };
        out.push_str(&rest[..start]);
        out.push_str(&vars.get(name).or_else(|| default.map(str::to_owned))?);
        rest = &rest[start + length + 1..];
    }
    out.push_str(rest);

    Some(out)
}

/// The server as Drift runs it; none for a transport Drift does not speak. A header or env entry whose
/// value names something nobody gave is left out, so a server with a sign-in of its own can still use that.
pub(super) fn config(server: &Value, vars: &Vars) -> Option<ServerConfig> {
    let text = |key: &str| {
        server[key]
            .as_str()
            .map(|value| expand(value, vars).unwrap_or_else(|| value.into()))
    };
    let kind = server["type"].as_str().unwrap_or_default();

    if let Some(command) = text("command") {
        let args = server["args"]
            .as_array()
            .map(|args| {
                args.iter()
                    .filter_map(Value::as_str)
                    .map(|arg| expand(arg, vars).unwrap_or_else(|| arg.into()))
                    .collect()
            })
            .unwrap_or_default();
        let mut env = entries(&server["env"], vars);
        env.insert("CLAUDE_PLUGIN_ROOT".into(), vars.root.clone());
        env.insert("CLAUDE_PLUGIN_DATA".into(), vars.data.clone());
        return Some(ServerConfig::Stdio {
            command,
            args,
            env,
            cwd: None,
            timeout_seconds: None,
        });
    }

    let url = text("url")?;
    let headers = entries(&server["headers"], vars);
    match kind {
        "sse" => Some(ServerConfig::Sse {
            url,
            headers,
            oauth: None,
            timeout_seconds: None,
        }),
        "" | "http" | "streamable-http" => Some(ServerConfig::Http {
            url,
            headers,
            oauth: None,
            timeout_seconds: None,
        }),
        _ => None,
    }
}

fn entries(value: &Value, vars: &Vars) -> BTreeMap<String, String> {
    value
        .as_object()
        .into_iter()
        .flatten()
        .filter_map(|(name, value)| Some((name.clone(), expand(value.as_str()?, vars)?)))
        .collect()
}

/// The name a bundle's server is saved under: its own, made a valid server name, unless something has it; then the
/// bundle's id goes in front, and a number after that when that is taken too.
pub(super) fn row_name(declared: &str, bundle: &str, taken: impl Fn(&str) -> bool) -> String {
    let clean: String = declared
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
                ch
            } else {
                '-'
            }
        })
        .collect();

    // Every candidate is checked, so a name another server or this install already has is never reused.
    std::iter::once(clean.clone())
        .chain(std::iter::once(format!("{bundle}-{clean}")))
        .chain((2..).map(|number| format!("{bundle}-{clean}-{number}")))
        .find(|name| !taken(name))
        .unwrap_or(clean)
}
