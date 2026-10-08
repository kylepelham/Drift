//! Registry sources: where plugins, skills and MCP servers come from besides Drift's own
//! registries. A source is an https URL, a GitHub or Azure DevOps repository, or a folder; it may
//! carry a token, allow plain http, or trust an extra root certificate. Every fetch for a source,
//! the registry document and the downloads it names, goes through [`Fetcher`], so the webview never
//! holds a credential and a remote client reads the same registries.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::llm::credentials::Credentials;

const MAX_DOCUMENT_BYTES: usize = 8 * 1024 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum RegistryKind {
    Plugins,
    Mcp,
}

/// How a source is reached.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum SourceKind {
    /// A JSON document at a URL; downloads are whatever the document names.
    #[default]
    Url,
    /// A file in a GitHub repository, private ones with a token; downloads under the same repository use the API.
    Github,
    /// A file in an Azure DevOps Git repository, with a PAT; downloads under the same repository use the items API.
    AzureDevops,
    /// A folder on this machine or a share, holding the document and the files it names beside it.
    Folder,
}

/// A registry the user added. Secrets are not here: a token lives in the credential store under the source's id.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct RegistrySource {
    /// Stable, so the token outlives a rename; the engine makes one when a new source has none.
    #[serde(default)]
    #[schema(required)]
    pub id: String,
    pub name: String,
    pub kind: RegistryKind,
    #[serde(default)]
    pub source: SourceKind,
    /// For `url`: the document. For `github` and `azure_devops`: the repository's web URL. For `folder`: the folder.
    pub url: String,
    /// For a repository: the branch, tag or commit; the default branch when empty.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub r#ref: String,
    /// For a repository: the document's path inside it; `registry.json` when empty.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub path: String,
    /// Whether a token is stored for it; the token itself is never returned.
    #[serde(default)]
    pub has_token: bool,
    /// Plain http is refused unless the user says so for this source.
    #[serde(default)]
    pub allow_http: bool,
    /// An extra root certificate (PEM) trusted for this source's hosts, for an internal CA.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ca_pem: Option<String>,
}

/// The `/settings` body's view of a source: the same, plus a token to store or clear.
#[derive(Clone, Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct SourceInput {
    #[serde(flatten)]
    pub source: RegistrySource,
    /// A new token to keep; empty clears it; absent leaves it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token: Option<String>,
}

pub fn token_key(id: &str) -> String {
    format!("registry:{id}")
}

/// A parsed GitHub or Azure DevOps repository web URL.
#[derive(Debug, PartialEq)]
pub struct Repo {
    pub host: String,
    pub owner: String,
    pub project: Option<String>,
    pub name: String,
}

/// `https://github.com/owner/repo[.git][/...]`.
pub fn github_repo(url: &str) -> Option<Repo> {
    let rest = url.trim().strip_prefix("https://github.com/")?;
    let mut parts = rest.split('/').filter(|part| !part.is_empty());
    let owner = parts.next()?;
    let name = parts.next()?.trim_end_matches(".git");
    Some(Repo {
        host: "github.com".into(),
        owner: owner.into(),
        project: None,
        name: name.into(),
    })
}

/// `https://dev.azure.com/org/project/_git/repo` or `https://org.visualstudio.com/project/_git/repo`.
pub fn azure_repo(url: &str) -> Option<Repo> {
    let url = url.trim();
    let (host, rest) = url.strip_prefix("https://").and_then(|rest| rest.split_once('/'))?;
    let mut parts: Vec<&str> = rest.split('/').filter(|part| !part.is_empty()).collect();
    let git = parts.iter().position(|part| *part == "_git")?;
    let name = parts.get(git + 1)?.to_string();
    parts.truncate(git);
    let (owner, project) = if host == "dev.azure.com" {
        (parts.first()?.to_string(), parts.get(1)?.to_string())
    } else {
        (
            host.strip_suffix(".visualstudio.com")?.to_string(),
            parts.first()?.to_string(),
        )
    };
    Some(Repo {
        host: host.into(),
        owner,
        project: Some(project),
        name,
    })
}

/// What a source resolves to for its document, and how a path inside the same repository or folder becomes a fetchable location.
pub enum Location {
    Http {
        url: String,
        headers: Vec<(String, String)>,
    },
    File(PathBuf),
}

impl RegistrySource {
    fn document_path(&self) -> &str {
        if self.path.trim().is_empty() {
            "registry.json"
        } else {
            self.path.trim().trim_start_matches('/')
        }
    }

    /// Where the registry document is.
    pub fn document(&self, token: Option<&str>) -> Result<Location, String> {
        match self.source {
            SourceKind::Url => Ok(Location::Http {
                url: self.url.trim().to_owned(),
                headers: bearer(token),
            }),
            SourceKind::Folder => Ok(Location::File(Path::new(self.url.trim()).join(self.document_path()))),
            SourceKind::Github | SourceKind::AzureDevops => self.repo_file(self.document_path(), token),
        }
    }

    /// A file inside the source's repository or folder; for a `url` source, an absolute URL as given.
    pub fn file(&self, path_or_url: &str, token: Option<&str>) -> Result<Location, String> {
        match self.source {
            SourceKind::Url => Ok(Location::Http {
                url: path_or_url.to_owned(),
                headers: if same_host(&self.url, path_or_url) {
                    bearer(token)
                } else {
                    Vec::new()
                },
            }),
            SourceKind::Folder => {
                let relative = Path::new(path_or_url);
                if relative
                    .components()
                    .any(|part| !matches!(part, std::path::Component::Normal(_)))
                {
                    return Err("a folder source names files relative to itself".into());
                }
                Ok(Location::File(Path::new(self.url.trim()).join(relative)))
            }
            SourceKind::Github | SourceKind::AzureDevops => {
                if path_or_url.starts_with("https://") || path_or_url.starts_with("http://") {
                    return Ok(Location::Http {
                        url: path_or_url.to_owned(),
                        headers: if same_host(&self.url, path_or_url) {
                            self.repo_headers(token)
                        } else {
                            Vec::new()
                        },
                    });
                }
                self.repo_file(path_or_url, token)
            }
        }
    }

    /// The archive of the source's repository at its ref, for a skill pack kept in the same repository.
    pub fn archive(&self, token: Option<&str>) -> Result<Location, String> {
        match self.source {
            SourceKind::Github => {
                let repo = github_repo(&self.url).ok_or("not a GitHub repository URL")?;
                let r#ref = if self.r#ref.is_empty() {
                    "HEAD".to_owned()
                } else {
                    self.r#ref.clone()
                };
                Ok(Location::Http {
                    url: format!(
                        "https://api.github.com/repos/{}/{}/tarball/{ref}",
                        repo.owner, repo.name
                    ),
                    headers: self.repo_headers(token),
                })
            }
            SourceKind::AzureDevops => {
                let repo = azure_repo(&self.url).ok_or("not an Azure DevOps repository URL")?;
                let version = self.azure_version();
                Ok(Location::Http {
                    url: format!(
                        "https://{}/{}/{}/_apis/git/repositories/{}/items?path=/&$format=zip&download=true{version}&api-version=7.1",
                        repo.host,
                        repo.owner,
                        repo.project.unwrap_or_default(),
                        repo.name
                    ),
                    headers: self.repo_headers(token),
                })
            }
            _ => Err("only a repository source has an archive".into()),
        }
    }

    fn repo_file(&self, path: &str, token: Option<&str>) -> Result<Location, String> {
        let path = path.trim_start_matches('/');
        match self.source {
            SourceKind::Github => {
                let repo = github_repo(&self.url).ok_or("not a GitHub repository URL")?;
                let r#ref = if self.r#ref.is_empty() {
                    "HEAD".to_owned()
                } else {
                    self.r#ref.clone()
                };
                // The contents API with the raw media type serves private files with a token and public ones without.
                let url = format!(
                    "https://api.github.com/repos/{}/{}/contents/{path}?ref={ref}",
                    repo.owner, repo.name
                );
                let mut headers = self.repo_headers(token);
                headers.push(("accept".into(), "application/vnd.github.raw+json".into()));
                Ok(Location::Http { url, headers })
            }
            SourceKind::AzureDevops => {
                let repo = azure_repo(&self.url).ok_or("not an Azure DevOps repository URL")?;
                let version = self.azure_version();
                let url = format!(
                    "https://{}/{}/{}/_apis/git/repositories/{}/items?path=/{path}&download=true{version}&api-version=7.1",
                    repo.host,
                    repo.owner,
                    repo.project.unwrap_or_default(),
                    repo.name
                );
                Ok(Location::Http {
                    url,
                    headers: self.repo_headers(token),
                })
            }
            _ => unreachable!("repo_file is only called for repository sources"),
        }
    }

    fn azure_version(&self) -> String {
        if self.r#ref.is_empty() {
            String::new()
        } else {
            format!("&versionDescriptor.version={}", self.r#ref)
        }
    }

    /// GitHub takes a bearer token; Azure DevOps takes a PAT as basic auth with an empty user.
    fn repo_headers(&self, token: Option<&str>) -> Vec<(String, String)> {
        match (self.source, token) {
            (SourceKind::AzureDevops, Some(token)) => vec![(
                "authorization".into(),
                format!("Basic {}", {
                    use base64::Engine;
                    base64::engine::general_purpose::STANDARD.encode(format!(":{token}"))
                }),
            )],
            (_, Some(token)) => vec![("authorization".into(), format!("Bearer {token}"))],
            (_, None) => Vec::new(),
        }
    }
}

fn bearer(token: Option<&str>) -> Vec<(String, String)> {
    token
        .map(|token| vec![("authorization".into(), format!("Bearer {token}"))])
        .unwrap_or_default()
}

fn host_of(url: &str) -> Option<String> {
    url.split("://")
        .nth(1)?
        .split('/')
        .next()
        .map(|host| host.to_ascii_lowercase())
}

/// A token is sent only to the host the source names, never to a download that points elsewhere.
pub fn same_host(source_url: &str, target_url: &str) -> bool {
    matches!((host_of(source_url), host_of(target_url)), (Some(a), Some(b)) if a == b)
}

/// Reads documents and files for sources: with their token, their http allowance and their extra root certificate.
pub struct Fetcher {
    http: reqwest::Client,
    credentials: Arc<Credentials>,
}

impl Fetcher {
    pub fn new(http: reqwest::Client, credentials: Arc<Credentials>) -> Self {
        Self { http, credentials }
    }

    pub fn token(&self, source: &RegistrySource) -> Option<String> {
        self.credentials.secret(&token_key(&source.id))
    }

    /// The bytes at a location, within `limit`.
    pub async fn read(&self, source: &RegistrySource, location: Location, limit: usize) -> Result<Vec<u8>, String> {
        match location {
            Location::File(path) => {
                let bytes = tokio::fs::read(&path)
                    .await
                    .map_err(|error| format!("{}: {error}", path.display()))?;
                if bytes.len() > limit {
                    return Err(format!("{} is larger than {} MiB", path.display(), limit / 1024 / 1024));
                }
                Ok(bytes)
            }
            Location::Http { url, headers } => {
                if url.starts_with("http://") && !source.allow_http {
                    return Err(
                        "plain http is refused for this source; allow it in the source's settings if you must".into(),
                    );
                }
                if !url.starts_with("http://") && !url.starts_with("https://") {
                    return Err(format!("not a URL: {url}"));
                }
                let client = self.client_for(source)?;
                let mut request = client.get(&url);
                for (name, value) in headers {
                    request = request.header(name, value);
                }
                let response = request
                    .send()
                    .await
                    .map_err(|error| format!("could not fetch {url}: {error}"))?;
                let status = response.status();
                if !status.is_success() {
                    let hint = if status.as_u16() == 401 || status.as_u16() == 403 {
                        " (a token may be needed, or the one stored may be wrong)"
                    } else if status.as_u16() == 404 {
                        " (not found; for a private repository that can also mean the token lacks access)"
                    } else {
                        ""
                    };
                    return Err(format!("could not fetch {url}: {status}{hint}"));
                }
                let bytes = response
                    .bytes()
                    .await
                    .map_err(|error| format!("could not fetch {url}: {error}"))?;
                if bytes.len() > limit {
                    return Err(format!("{url} is larger than {} MiB", limit / 1024 / 1024));
                }
                Ok(bytes.to_vec())
            }
        }
    }

    /// The registry document of a source, as JSON.
    pub async fn document(&self, source: &RegistrySource) -> Result<serde_json::Value, String> {
        let token = self.token(source);
        let bytes = self
            .read(source, source.document(token.as_deref())?, MAX_DOCUMENT_BYTES)
            .await?;
        serde_json::from_slice(&bytes).map_err(|error| format!("the registry is not valid JSON: {error}"))
    }

    /// The engine's client, or one that also trusts the source's own root certificate.
    fn client_for(&self, source: &RegistrySource) -> Result<reqwest::Client, String> {
        let Some(pem) = source.ca_pem.as_deref().filter(|pem| !pem.trim().is_empty()) else {
            return Ok(self.http.clone());
        };
        let cert = reqwest::Certificate::from_pem(pem.as_bytes())
            .map_err(|error| format!("the source's certificate is not PEM: {error}"))?;
        reqwest::Client::builder()
            .add_root_certificate(cert)
            .connect_timeout(std::time::Duration::from_secs(15))
            .build()
            .map_err(|error| error.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn source(kind: SourceKind, url: &str, r#ref: &str) -> RegistrySource {
        RegistrySource {
            id: "s".into(),
            name: "Acme".into(),
            kind: RegistryKind::Plugins,
            source: kind,
            url: url.into(),
            r#ref: r#ref.into(),
            path: String::new(),
            has_token: true,
            allow_http: false,
            ca_pem: None,
        }
    }

    fn http(location: Location) -> (String, Vec<(String, String)>) {
        match location {
            Location::Http { url, headers } => (url, headers),
            Location::File(path) => panic!("expected a URL, got {}", path.display()),
        }
    }

    #[test]
    fn repository_urls_parse_and_resolve_to_api_locations_with_the_right_auth() {
        assert_eq!(
            github_repo("https://github.com/acme/tools.git/"),
            Some(Repo {
                host: "github.com".into(),
                owner: "acme".into(),
                project: None,
                name: "tools".into()
            })
        );
        assert_eq!(
            azure_repo("https://dev.azure.com/koderly/PDT%20Projects/_git/drift-plugins"),
            Some(Repo {
                host: "dev.azure.com".into(),
                owner: "koderly".into(),
                project: Some("PDT%20Projects".into()),
                name: "drift-plugins".into()
            })
        );
        assert_eq!(
            azure_repo("https://koderly.visualstudio.com/PDT/_git/plugins")
                .unwrap()
                .owner,
            "koderly"
        );
        assert!(github_repo("https://gitlab.com/a/b").is_none());

        let gh = source(SourceKind::Github, "https://github.com/acme/tools", "v2");
        let (url, headers) = http(gh.document(Some("tok")).unwrap());
        assert_eq!(
            url,
            "https://api.github.com/repos/acme/tools/contents/registry.json?ref=v2"
        );
        assert!(headers.contains(&("authorization".to_owned(), "Bearer tok".to_owned())));
        assert!(
            headers
                .iter()
                .any(|(name, value)| name == "accept" && value.contains("raw"))
        );
        let (url, _) = http(gh.file("dist/guard.wasm", None).unwrap());
        assert_eq!(
            url,
            "https://api.github.com/repos/acme/tools/contents/dist/guard.wasm?ref=v2"
        );
        let (url, _) = http(gh.archive(Some("tok")).unwrap());
        assert_eq!(url, "https://api.github.com/repos/acme/tools/tarball/v2");

        let az = source(
            SourceKind::AzureDevops,
            "https://dev.azure.com/koderly/PDT/_git/plugins",
            "main",
        );
        let (url, headers) = http(az.document(Some("pat")).unwrap());
        assert!(url.starts_with("https://dev.azure.com/koderly/PDT/_apis/git/repositories/plugins/items?path=/registry.json&download=true&versionDescriptor.version=main"), "{url}");
        assert_eq!(headers[0].0, "authorization");
        assert!(headers[0].1.starts_with("Basic "));

        let plain = source(SourceKind::Url, "https://registry.acme.test/plugins.json", "");
        let (_, headers) = http(
            plain
                .file("https://registry.acme.test/dist/x.wasm", Some("tok"))
                .unwrap(),
        );
        assert_eq!(headers.len(), 1, "same host gets the token");
        let (_, headers) = http(plain.file("https://cdn.elsewhere.test/x.wasm", Some("tok")).unwrap());
        assert!(headers.is_empty(), "another host never sees it");
    }

    #[test]
    fn a_folder_source_reads_beside_its_document_and_never_above_it() {
        let folder = source(SourceKind::Folder, "S:/share/drift", "");
        match folder.document(None).unwrap() {
            Location::File(path) => assert_eq!(path, Path::new("S:/share/drift").join("registry.json")),
            Location::Http { .. } => panic!(),
        }
        assert!(folder.file("../secrets.json", None).is_err());
        assert!(matches!(folder.file("dist/guard.wasm", None), Ok(Location::File(_))));
    }
}
