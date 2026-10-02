//! What clients see of a server and what they send to change one. Env and header values are
//! secrets: they go in, never out.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use super::{OAuthClient, ServerConfig, ServerRow};

/// A server as clients see it: every field but the values of its env vars and headers.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ServerView {
    pub name: String,
    pub config: ServerConfigView,
    pub enabled: bool,
    /// Read-only agents may use the tools it marks read-only (`PUT /mcp/{name}/readOnlyTrusted`).
    pub read_only_trusted: bool,
    pub updated_at: i64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ServerConfigView {
    Stdio {
        command: String,
        args: Vec<String>,
        /// Names only.
        env: Vec<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        cwd: Option<String>,
        #[serde(rename = "timeoutSeconds", skip_serializing_if = "Option::is_none")]
        timeout_seconds: Option<u64>,
    },
    Http {
        url: String,
        /// Names only.
        headers: Vec<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        oauth: Option<OAuthView>,
        #[serde(rename = "timeoutSeconds", skip_serializing_if = "Option::is_none")]
        timeout_seconds: Option<u64>,
    },
    Sse {
        url: String,
        /// Names only.
        headers: Vec<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        oauth: Option<OAuthView>,
        #[serde(rename = "timeoutSeconds", skip_serializing_if = "Option::is_none")]
        timeout_seconds: Option<u64>,
    },
}

/// A pre-registered app as clients see it: whether it has a secret, never the secret.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct OAuthView {
    pub client_id: String,
    pub has_secret: bool,
    pub scopes: Vec<String>,
}

/// A pre-registered app as a client sends it: a `null` secret keeps the saved one for the same client id, an empty one clears it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct OAuthInput {
    pub client_id: String,
    #[serde(default)]
    pub client_secret: Option<String>,
    #[serde(default)]
    pub scopes: Vec<String>,
}

/// A config as a client sends it. A `null` value keeps the one saved under that name, so a secret can be kept without being read.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ServerConfigInput {
    Stdio {
        command: String,
        #[serde(default)]
        args: Vec<String>,
        #[serde(default)]
        env: BTreeMap<String, Option<String>>,
        #[serde(default)]
        cwd: Option<String>,
        #[serde(default, rename = "timeoutSeconds")]
        timeout_seconds: Option<u64>,
    },
    Http {
        url: String,
        #[serde(default)]
        headers: BTreeMap<String, Option<String>>,
        #[serde(default)]
        oauth: Option<OAuthInput>,
        #[serde(default, rename = "timeoutSeconds")]
        timeout_seconds: Option<u64>,
    },
    Sse {
        url: String,
        #[serde(default)]
        headers: BTreeMap<String, Option<String>>,
        #[serde(default)]
        oauth: Option<OAuthInput>,
        #[serde(default, rename = "timeoutSeconds")]
        timeout_seconds: Option<u64>,
    },
}

impl ServerView {
    pub fn of(row: &ServerRow) -> Self {
        let names = |values: &BTreeMap<String, String>| values.keys().cloned().collect();
        let config = match &row.config {
            ServerConfig::Stdio { command, args, env, cwd, timeout_seconds } => {
                ServerConfigView::Stdio { command: command.clone(), args: args.clone(), env: names(env), cwd: cwd.clone(), timeout_seconds: *timeout_seconds }
            }
            ServerConfig::Http { url, headers, oauth, timeout_seconds } => {
                ServerConfigView::Http { url: url.clone(), headers: names(headers), oauth: oauth.as_ref().map(OAuthView::of), timeout_seconds: *timeout_seconds }
            }
            ServerConfig::Sse { url, headers, oauth, timeout_seconds } => {
                ServerConfigView::Sse { url: url.clone(), headers: names(headers), oauth: oauth.as_ref().map(OAuthView::of), timeout_seconds: *timeout_seconds }
            }
        };
        Self { name: row.name.clone(), config, enabled: row.enabled, read_only_trusted: row.read_only_trusted, updated_at: row.updated_at }
    }
}

impl ServerConfigInput {
    /// The config to save: kept values come from `saved`, and one with nothing saved to keep is refused, naming it.
    pub fn resolve(self, saved: Option<&ServerConfig>) -> Result<ServerConfig, String> {
        let (saved_env, saved_headers) = match saved {
            Some(ServerConfig::Stdio { env, .. }) => (Some(env), None),
            Some(ServerConfig::Http { headers, .. } | ServerConfig::Sse { headers, .. }) => (None, Some(headers)),
            None => (None, None),
        };
        let cwd_of = |cwd: Option<String>| cwd.map(|c| c.trim().to_string()).filter(|c| !c.is_empty());
        let saved_app = saved.and_then(|config| config.remote()).and_then(|(_, app)| app);
        Ok(match self {
            Self::Stdio { command, args, env, cwd, timeout_seconds } => ServerConfig::Stdio { command, args, env: keep(env, saved_env)?, cwd: cwd_of(cwd), timeout_seconds },
            Self::Http { url, headers, oauth, timeout_seconds } => ServerConfig::Http { url, headers: keep(headers, saved_headers)?, oauth: app(oauth, saved_app), timeout_seconds },
            Self::Sse { url, headers, oauth, timeout_seconds } => ServerConfig::Sse { url, headers: keep(headers, saved_headers)?, oauth: app(oauth, saved_app), timeout_seconds },
        })
    }
}

impl OAuthView {
    fn of(app: &OAuthClient) -> Self {
        Self { client_id: app.client_id.clone(), has_secret: app.client_secret.is_some(), scopes: app.scopes.clone() }
    }
}

/// The app to save; one with no client id is none, and a secret left out keeps the saved one only for the same app.
fn app(sent: Option<OAuthInput>, saved: Option<&OAuthClient>) -> Option<OAuthClient> {
    let sent = sent.filter(|sent| !sent.client_id.trim().is_empty())?;
    let client_id = sent.client_id.trim().to_string();
    let client_secret = match sent.client_secret {
        Some(secret) => Some(secret).filter(|secret| !secret.is_empty()),
        None => saved.filter(|saved| saved.client_id == client_id).and_then(|saved| saved.client_secret.clone()),
    };
    let scopes = sent.scopes.into_iter().map(|scope| scope.trim().to_string()).filter(|scope| !scope.is_empty()).collect();
    Some(OAuthClient { client_id, client_secret, scopes })
}

fn keep(values: BTreeMap<String, Option<String>>, saved: Option<&BTreeMap<String, String>>) -> Result<BTreeMap<String, String>, String> {
    values
        .into_iter()
        .map(|(name, value)| match value.or_else(|| saved.and_then(|saved| saved.get(&name)).cloned()) {
            Some(value) => Ok((name, value)),
            None => Err(format!("{name} has no saved value to keep")),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn row(config: ServerConfig) -> ServerRow {
        ServerRow { name: "docs".into(), config, enabled: true, hash: "0011223344556677".into(), updated_at: 1, era: None, read_only_trusted: false }
    }

    #[test]
    fn a_view_names_secrets_without_their_values() {
        let app = OAuthClient { client_id: "drift-app".into(), client_secret: Some("app-secret".into()), scopes: vec!["read".into()] };
        let headers = [("Authorization".to_string(), "Bearer secret-token".to_string())].into();
        let saved = row(ServerConfig::Http { url: "https://example.com/mcp".into(), headers, oauth: Some(app), timeout_seconds: None });
        let shown = serde_json::to_string(&ServerView::of(&saved)).unwrap();
        assert!(shown.contains("Authorization") && !shown.contains("secret-token"), "{shown}");
        assert!(shown.contains(r#""oauth":{"clientId":"drift-app","hasSecret":true,"scopes":["read"]}"#) && !shown.contains("app-secret"), "{shown}");
    }

    #[test]
    fn an_apps_secret_is_kept_only_for_the_same_app_and_an_empty_one_clears_it() {
        let saved = ServerConfig::Sse { url: "https://x.example/sse".into(), headers: Default::default(), oauth: Some(OAuthClient { client_id: "one".into(), client_secret: Some("s1".into()), scopes: vec![] }), timeout_seconds: None };
        let resolve = |oauth: serde_json::Value| {
            let sent: ServerConfigInput = serde_json::from_value(json!({ "type": "sse", "url": "https://x.example/sse", "oauth": oauth })).unwrap();
            sent.resolve(Some(&saved)).unwrap().remote().unwrap().1.cloned()
        };
        assert_eq!(resolve(json!({ "clientId": "one", "scopes": [" read ", ""] })), Some(OAuthClient { client_id: "one".into(), client_secret: Some("s1".into()), scopes: vec!["read".into()] }));
        assert_eq!(resolve(json!({ "clientId": "two" })).unwrap().client_secret, None, "another app does not get this one's secret");
        assert_eq!(resolve(json!({ "clientId": "one", "clientSecret": "" })).unwrap().client_secret, None);
        assert_eq!(resolve(json!({ "clientId": " " })), None, "no client id, no app");
    }

    #[test]
    fn a_null_value_keeps_the_saved_one_and_a_new_one_replaces_it() {
        let saved = ServerConfig::Stdio { command: "npx".into(), args: vec![], env: [("TOKEN".to_string(), "old".to_string())].into(), cwd: None, timeout_seconds: None };
        let sent: ServerConfigInput = serde_json::from_value(json!({ "type": "stdio", "command": "npx", "env": { "TOKEN": null, "MODE": "fast" }, "cwd": " C:/tools ", "timeoutSeconds": 60 })).unwrap();
        let ServerConfig::Stdio { env, cwd, timeout_seconds, .. } = sent.resolve(Some(&saved)).unwrap() else { panic!() };
        assert_eq!(env, BTreeMap::from([("TOKEN".to_string(), "old".to_string()), ("MODE".to_string(), "fast".to_string())]));
        assert_eq!((cwd.as_deref(), timeout_seconds), (Some("C:/tools"), Some(60)));
        let nothing_kept: ServerConfigInput = serde_json::from_value(json!({ "type": "stdio", "command": "npx", "env": { "OTHER": null } })).unwrap();
        assert_eq!(nothing_kept.resolve(Some(&saved)).unwrap_err(), "OTHER has no saved value to keep");
        let moved: ServerConfigInput = serde_json::from_value(json!({ "type": "sse", "url": "https://legacy.example/sse" })).unwrap();
        assert!(matches!(moved.resolve(Some(&saved)).unwrap(), ServerConfig::Sse { .. }));
    }
}
