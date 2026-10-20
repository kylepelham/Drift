//! A project's browser setup, the `browser` section of its `drift.json`: which engine its pages need, where it
//! lives, how to sign in, and notes for the agent. Passwords are never in the file: a sign-in names a credential
//! the user stored for the workspace, kept in the keychain.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use super::{Kind, Storage, Window};
use crate::Engine;

/// The names, users and domains of a workspace's credentials; their passwords are keychain secrets.
const CREDENTIALS_KEY: &str = "browserCredentials";

/// `browser` in drift.json.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", default)]
pub struct BrowserProject {
    /// The engine this project's pages need, over Settings.
    pub engine: Option<Kind>,
    /// Where the app lives; `open` with a path, or with no address, starts here.
    pub base_url: Option<String>,
    /// Address patterns (`*` matches anything) that choose an engine when the browser opens on one.
    pub engines: BTreeMap<Kind, Vec<String>>,
    pub storage: Option<Storage>,
    pub window: Option<Window>,
    /// Hosts the browser may sign in to with the user's Windows account (patterns such as `*.corp.local`).
    pub integrated_auth: Vec<String>,
    /// How to sign in to the app with `login`.
    pub login: Option<Login>,
    /// What the agent should know about the app: its layout, menus, frames, quirks.
    pub notes: Option<String>,
}

/// How `login` signs in. Values may hold `{user}`, `{password}` and `{domain}` from the named credential.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", default)]
pub struct Login {
    /// The credential to sign in with, stored in Settings > Browser for this workspace.
    pub credential: String,
    /// An address that signs in by itself, relative to `baseUrl` or absolute.
    pub url: Option<String>,
    /// Where the sign-in form is, relative to `baseUrl` or absolute; the current page when left out.
    pub path: Option<String>,
    /// Form fields to fill: a selector (`#id`, `name`, or CSS) and its value.
    pub fields: BTreeMap<String, String>,
    /// The selector of the button that sends the form.
    pub submit: Option<String>,
}

/// A credential as Settings shows it: never its password.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Credential {
    pub name: String,
    pub user: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub domain: String,
    #[serde(default)]
    pub has_password: bool,
}

/// What a sign-in fills in, password included; it never leaves the engine.
pub(crate) struct Secret {
    pub(crate) user: String,
    pub(crate) password: String,
    pub(crate) domain: String,
}

impl BrowserProject {
    /// The engine the first pattern matching `url` names.
    pub(crate) fn engine_for(&self, url: &str) -> Option<Kind> {
        let url = self.resolve(url);

        self.engines
            .iter()
            .find(|(_, patterns)| patterns.iter().any(|pattern| matches(pattern, &url)))
            .map(|(kind, _)| *kind)
    }

    /// Host patterns for Windows sign-in: a host, or `*.` and a domain of two labels or more; any other is refused.
    pub(crate) fn integrated_auth_hosts(&self) -> (Vec<String>, Vec<String>) {
        self.integrated_auth
            .iter()
            .map(|pattern| pattern.trim().to_lowercase())
            .partition(|pattern| host_pattern(pattern))
    }

    /// An address as the agent wrote it: a path is taken from `baseUrl`, a bare host gets a scheme.
    pub(crate) fn resolve(&self, url: &str) -> String {
        match &self.base_url {
            Some(base) if looks_like_path(url) => join(base, url),
            _ => super::with_scheme(url),
        }
    }
}

fn host_pattern(pattern: &str) -> bool {
    let host = pattern.strip_prefix("*.").unwrap_or(pattern);
    let labels: Vec<&str> = host.split('.').collect();
    let plain = labels
        .iter()
        .all(|label| !label.is_empty() && label.chars().all(|c| c.is_ascii_alphanumeric() || c == '-'));

    plain && (host.len() == pattern.len() || labels.len() >= 2)
}

/// `/a`, `main.asp?x=1` and `Home/Index` are paths; `example.com/a`, `localhost:3000` and `https://x` are not.
fn looks_like_path(url: &str) -> bool {
    const PAGES: [&str; 8] = [".asp", ".aspx", ".htm", ".html", ".php", ".jsp", ".cfm", ".axd"];
    if url.contains("://") || url.starts_with("about:") || url.starts_with("data:") {
        return false;
    }
    if url.starts_with('/') {
        return true;
    }

    let first = url.split(['/', '?', '#']).next().unwrap_or_default().to_lowercase();
    let host_like = first.contains('.') || first.contains(':') || first == "localhost";

    !host_like || PAGES.iter().any(|page| first.ends_with(page))
}

fn join(base: &str, path: &str) -> String {
    let base = super::with_scheme(base);
    if path.is_empty() {
        return base;
    }

    format!("{}/{}", base.trim_end_matches('/'), path.trim_start_matches('/'))
}

/// A glob over a whole address where `*` matches any run of characters, compared without case.
fn matches(pattern: &str, text: &str) -> bool {
    let pattern = pattern.to_lowercase();
    let text = text.to_lowercase();
    let parts: Vec<&str> = pattern.split('*').collect();
    let Some((first, rest)) = parts.split_first() else {
        return true;
    };
    if !text.starts_with(first) {
        return false;
    }

    let mut at = first.len();
    for (index, part) in rest.iter().enumerate() {
        let last = index == rest.len() - 1;
        if last {
            return text.len() >= at + part.len() && text[at..].ends_with(part);
        }
        match text[at..].find(part) {
            Some(found) => at += found + part.len(),
            None => return false,
        }
    }

    at == text.len()
}

/// `{user}`, `{password}` and `{domain}` filled from the credential.
pub(crate) fn fill_in(template: &str, secret: &Secret) -> String {
    template
        .replace("{user}", &secret.user)
        .replace("{password}", &secret.password)
        .replace("{domain}", &secret.domain)
}

fn secret_key(workspace_id: &str, name: &str) -> String {
    format!("browser:{workspace_id}:{name}")
}

fn list_key(workspace_id: &str) -> String {
    format!("{CREDENTIALS_KEY}:{workspace_id}")
}

impl Engine {
    /// A workspace's stored sign-ins, without their passwords.
    pub fn browser_credentials(&self, workspace_id: &str) -> Vec<Credential> {
        let stored: Vec<Credential> = self
            .store
            .setting(&list_key(workspace_id))
            .ok()
            .flatten()
            .unwrap_or_default();

        stored
            .into_iter()
            .map(|credential| Credential {
                has_password: self
                    .credentials
                    .secret(&secret_key(workspace_id, &credential.name))
                    .is_some(),
                ..credential
            })
            .collect()
    }

    /// Adds or changes a sign-in; a password left out keeps the stored one.
    pub fn save_browser_credential(
        &self,
        workspace_id: &str,
        credential: &Credential,
        password: Option<&str>,
    ) -> Result<(), String> {
        if credential.name.trim().is_empty() {
            return Err("a credential needs a name".into());
        }
        if let Some(password) = password {
            self.credentials
                .set_secret(&secret_key(workspace_id, &credential.name), password)
                .map_err(|error| error.to_string())?;
        }

        let mut list: Vec<Credential> = self.browser_credentials(workspace_id);
        list.retain(|stored| stored.name != credential.name);
        list.push(Credential {
            has_password: false,
            ..credential.clone()
        });
        list.sort_by(|a, b| a.name.cmp(&b.name));

        self.store
            .set_setting(&list_key(workspace_id), &list)
            .map_err(|error| error.to_string())
    }

    pub fn remove_browser_credential(&self, workspace_id: &str, name: &str) -> Result<(), String> {
        let _ = self.credentials.remove_secret(&secret_key(workspace_id, name));
        let mut list = self.browser_credentials(workspace_id);
        list.retain(|stored| stored.name != name);

        self.store
            .set_setting(&list_key(workspace_id), &list)
            .map_err(|error| error.to_string())
    }

    /// Every stored password of a workspace long enough to recognise, to keep out of what the model reads.
    pub(crate) fn browser_passwords(&self, workspace_id: &str) -> Vec<String> {
        self.browser_credentials(workspace_id)
            .iter()
            .filter_map(|credential| self.credentials.secret(&secret_key(workspace_id, &credential.name)))
            .filter(|password| password.chars().count() >= 3)
            .collect()
    }

    /// A credential with its password, for a sign-in the engine performs.
    pub(crate) fn browser_secret(&self, workspace_id: &str, name: &str) -> Result<Secret, String> {
        let credential = self
            .browser_credentials(workspace_id)
            .into_iter()
            .find(|credential| credential.name == name)
            .ok_or_else(|| {
                format!("no credential named `{name}` is stored for this workspace; ask the user to add it in Settings > Browser")
            })?;
        let password = self
            .credentials
            .secret(&secret_key(workspace_id, name))
            .unwrap_or_default();

        Ok(Secret {
            user: credential.user,
            password,
            domain: credential.domain,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn address_patterns_match_whole_addresses() {
        assert!(matches(
            "*/Traveller/*",
            "https://nt.traveller.local/Traveller/main.asp"
        ));
        assert!(matches("https://*.corp.local/*", "https://intranet.corp.local/a"));
        assert!(!matches("*/Traveller/*", "https://nt.traveller.local/Mvc/Home"));
        assert!(matches("*", "anything"));
        assert!(!matches("*.asp", "https://x/a.aspx"));
    }

    #[test]
    fn engines_follow_the_address_and_paths_follow_the_base() {
        let project = BrowserProject {
            base_url: Some("https://app.local".into()),
            engines: BTreeMap::from([(Kind::Ie, vec!["*.asp*".into()])]),
            ..BrowserProject::default()
        };

        assert_eq!(project.resolve("/main.asp"), "https://app.local/main.asp");
        assert_eq!(project.resolve("main.asp"), "https://app.local/main.asp");
        assert_eq!(project.resolve("example.com"), "https://example.com");
        assert_eq!(project.engine_for("/main.asp?Site=go"), Some(Kind::Ie));
        assert_eq!(project.engine_for("/Home/Index"), None);
    }

    #[test]
    fn windows_sign_in_takes_only_host_patterns() {
        let project = BrowserProject {
            integrated_auth: [
                "*.corp.local",
                "Intranet",
                "*",
                "*.local",
                "a.corp.local,*",
                "x\");pref(\"y",
            ]
            .map(String::from)
            .to_vec(),
            ..BrowserProject::default()
        };
        let (allowed, refused) = project.integrated_auth_hosts();

        assert_eq!(allowed, ["*.corp.local", "intranet"]);
        assert_eq!(refused.len(), 4);
    }

    #[test]
    fn a_sign_in_template_takes_the_credential() {
        let secret = Secret {
            user: "kyle".into(),
            password: "orange kites".into(),
            domain: "go".into(),
        };

        assert_eq!(
            fill_in("main.asp?Login={user}&Pass={password}&Site={domain}", &secret),
            "main.asp?Login=kyle&Pass=orange kites&Site=go"
        );
    }

    #[test]
    fn the_section_reads_from_drift_json() {
        let text = r##"{ "engine": "ie", "baseUrl": "https://app.local", "engines": { "edge": ["*/Mvc/*"] },
            "login": { "credential": "admin", "fields": { "#User": "{user}" }, "submit": "#Go" } }"##;
        let project: BrowserProject = serde_json::from_str(text).unwrap();

        assert_eq!(project.engine, Some(Kind::Ie));
        assert_eq!(project.engines[&Kind::Edge], ["*/Mvc/*"]);
        assert_eq!(project.login.unwrap().submit.as_deref(), Some("#Go"));
    }
}
