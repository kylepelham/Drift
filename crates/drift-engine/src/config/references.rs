//! `references` in drift.json: named folders and git repositories outside the workspace the model may consult.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use super::Config;

/// One `references` entry as drift.json writes it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(untagged)]
pub enum ReferenceEntry {
    /// A folder: relative to the file's directory, absolute, or starting `~/`.
    Path(String),
    Repository {
        repository: String,
        #[serde(default)]
        branch: Option<String>,
        #[serde(default)]
        description: String,
    },
    Folder {
        path: String,
        #[serde(default)]
        description: String,
    },
}

/// Where a reference's files are.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", tag = "kind")]
pub enum Place {
    Folder {
        #[schema(value_type = String)]
        path: PathBuf,
    },
    /// Cloned shallow under Drift's data directory (`Engine::reference_path`).
    Repository { url: String, branch: Option<String> },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Reference {
    pub name: String,
    pub description: String,
    pub place: Place,
    /// Named by the project's own drift.json, so it grants nothing until the user allows reading there.
    pub project: bool,
}

impl Config {
    /// Adds a file's references; a project's never replaces one of the user's by the same name.
    pub(super) fn apply_references(
        &mut self,
        entries: &std::collections::BTreeMap<String, ReferenceEntry>,
        resolve: impl Fn(&str) -> PathBuf,
        project: bool,
    ) {
        for (name, entry) in entries {
            let users = self
                .references
                .iter()
                .any(|known| known.name == *name && !known.project);
            if project && users {
                continue;
            }
            let (description, place) = match entry {
                ReferenceEntry::Path(path) => (String::new(), Place::Folder { path: resolve(path) }),
                ReferenceEntry::Folder { path, description } => {
                    (description.clone(), Place::Folder { path: resolve(path) })
                }
                ReferenceEntry::Repository {
                    repository,
                    branch,
                    description,
                } => (
                    description.clone(),
                    Place::Repository {
                        url: repository.clone(),
                        branch: branch.clone(),
                    },
                ),
            };
            self.references.retain(|known| known.name != *name);
            self.references.push(Reference {
                name: name.clone(),
                description,
                place,
                project,
            });
        }
    }
}

/// The folder a repository reference is cloned into: its name and a hash of where it comes from.
pub fn repository_folder(data_dir: &Path, name: &str, url: &str, branch: Option<&str>) -> PathBuf {
    use sha2::Digest;
    let digest = sha2::Sha256::digest(format!("{url}#{}", branch.unwrap_or_default()).as_bytes());
    let slug: String = name
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() {
                character
            } else {
                '-'
            }
        })
        .collect();

    data_dir
        .join("references")
        .join(format!("{slug}-{}", &crate::hex_bytes(&digest)[..8]))
}
