//! Skill packs, unpacked from a pinned tarball under the user's skills folder, and the per-skill switch.

use std::io::Read;
use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

const SKILLS_DIR: &str = "skills";
const MARKER: &str = ".drift-pack.json";
const MAX_ARCHIVE_BYTES: usize = 32 * 1024 * 1024;
const MAX_FILE_BYTES: u64 = 4 * 1024 * 1024;

/// What a registry entry of kind `skills` needs to be installed.
#[derive(Clone, Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct InstallPack {
    /// Becomes the folder name: letters, digits, `-` and `_` only.
    pub id: String,
    pub name: String,
    /// A `.tar.gz` over https, as GitHub's codeload serves one.
    pub archive: String,
    /// Folders inside the archive to keep (after its top-level folder); empty keeps everything.
    #[serde(default)]
    pub subdirs: Vec<String>,
    #[serde(default)]
    pub source: Option<String>,
    #[serde(default)]
    pub image: Option<String>,
    /// Skill folder names to keep; empty keeps every skill the archive holds.
    #[serde(default)]
    pub skills: Vec<String>,
    /// The registry source it was listed by; its token and trust apply to the archive. An empty
    /// `archive` with a repository source means the source's own repository.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub registry: Option<String>,
}

/// One skill the user has, from a pack or their own folders.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct UserSkill {
    pub name: String,
    pub description: String,
    /// Its folder, absolute.
    pub path: String,
    /// The pack it came from, by id; none for the user's own.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pack: Option<String>,
    /// In the workspace (or a parent up to its repository root) rather than the user's own folders.
    #[serde(default)]
    pub workspace: bool,
    pub enabled: bool,
}

/// An installed pack as the API reports it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Pack {
    pub id: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image: Option<String>,
    pub archive: String,
    /// The skills it brought, by folder name.
    pub skills: Vec<String>,
    pub installed_at: i64,
}

fn valid_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 64 && id.chars().all(|ch| ch.is_ascii_alphanumeric() || ch == '-' || ch == '_')
}

pub fn packs_dir() -> Result<PathBuf, String> {
    Ok(super::plugins::config_dir()?.join(SKILLS_DIR))
}

/// Fetches the archive and unpacks the wanted folders under `skills/<id>`, replacing what was there.
pub async fn install(fetcher: &super::sources::Fetcher, source: Option<&super::sources::RegistrySource>, pack: InstallPack) -> Result<Pack, String> {
    if !valid_id(&pack.id) {
        return Err("a pack id is letters, digits, dashes and underscores".into());
    }
    let bytes = match source {
        Some(source) => {
            let token = fetcher.token(source);
            let location = if pack.archive.trim().is_empty() { source.archive(token.as_deref())? } else { source.file(&pack.archive, token.as_deref())? };
            fetcher.read(source, location, MAX_ARCHIVE_BYTES).await?
        }
        None => {
            if !pack.archive.starts_with("https://") {
                return Err("a pack is fetched over https only".into());
            }
            let drift = super::sources::RegistrySource { id: String::new(), name: "Drift".into(), kind: super::sources::RegistryKind::Plugins, source: Default::default(), url: pack.archive.clone(), r#ref: String::new(), path: String::new(), has_token: false, allow_http: false, ca_pem: None };
            fetcher.read(&drift, super::sources::Location::Http { url: pack.archive.clone(), headers: Vec::new() }, MAX_ARCHIVE_BYTES).await?
        }
    };
    let dir = packs_dir()?.join(&pack.id);
    let into = dir.clone();
    let subdirs = pack.subdirs.clone();
    let wanted = pack.skills.clone();
    let skills = tokio::task::spawn_blocking(move || unpack_any(&bytes, &into, &subdirs, &wanted)).await.map_err(|error| error.to_string())??;
    let installed = Pack { id: pack.id, name: pack.name, source: pack.source, image: pack.image, archive: pack.archive, skills, installed_at: crate::id::now_ms() };
    let marker = serde_json::to_string_pretty(&installed).map_err(|error| error.to_string())?;
    std::fs::write(dir.join(MARKER), marker).map_err(|error| format!("could not write {}: {error}", dir.display()))?;
    Ok(installed)
}

/// GitHub serves tar.gz; Azure DevOps serves zip. Either is told apart by its first bytes.
fn unpack_any(bytes: &[u8], into: &Path, subdirs: &[String], wanted: &[String]) -> Result<Vec<String>, String> {
    if bytes.starts_with(b"PK") {
        return unpack_zip(bytes, into, subdirs, wanted);
    }
    unpack(bytes, into, subdirs, wanted)
}

/// A zip archive the same way: Azure's has no top-level folder, so paths are taken as they are.
fn unpack_zip(bytes: &[u8], into: &Path, subdirs: &[String], wanted: &[String]) -> Result<Vec<String>, String> {
    if into.exists() {
        std::fs::remove_dir_all(into).map_err(|error| format!("could not replace {}: {error}", into.display()))?;
    }
    std::fs::create_dir_all(into).map_err(|error| format!("could not create {}: {error}", into.display()))?;
    let mut archive = zip::ZipArchive::new(std::io::Cursor::new(bytes)).map_err(|error| format!("not a zip archive: {error}"))?;
    let mut skills = Vec::new();
    for index in 0..archive.len() {
        let mut entry = archive.by_index(index).map_err(|error| error.to_string())?;
        if entry.is_dir() || entry.size() > MAX_FILE_BYTES {
            continue;
        }
        let Some(name) = entry.enclosed_name() else { continue };
        let relative = name.to_path_buf();
        if !subdirs.is_empty() && !subdirs.iter().any(|sub| relative.starts_with(sub.trim_matches('/'))) {
            continue;
        }
        if !wanted.is_empty() && !relative.components().any(|part| wanted.iter().any(|skill| part.as_os_str() == skill.as_str())) {
            continue;
        }
        let target = into.join(&relative);
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent).map_err(|error| format!("could not create {}: {error}", parent.display()))?;
        }
        let mut content = Vec::with_capacity(entry.size() as usize);
        entry.read_to_end(&mut content).map_err(|error| error.to_string())?;
        std::fs::write(&target, content).map_err(|error| format!("could not write {}: {error}", target.display()))?;
        if relative.file_name().is_some_and(|name| name == "SKILL.md") {
            if let Some(skill) = relative.parent().and_then(Path::file_name) {
                skills.push(skill.to_string_lossy().into_owned());
            }
        }
    }
    if skills.is_empty() {
        let _ = std::fs::remove_dir_all(into);
        return Err("the archive holds no SKILL.md in the folders asked for".into());
    }
    skills.sort();
    Ok(skills)
}

/// Writes the archive's files under `into` without its top folder, keeping only `subdirs` and `wanted` skills when given.
fn unpack(bytes: &[u8], into: &Path, subdirs: &[String], wanted: &[String]) -> Result<Vec<String>, String> {
    if into.exists() {
        std::fs::remove_dir_all(into).map_err(|error| format!("could not replace {}: {error}", into.display()))?;
    }
    std::fs::create_dir_all(into).map_err(|error| format!("could not create {}: {error}", into.display()))?;
    let mut archive = tar::Archive::new(flate2::read::GzDecoder::new(bytes));
    let entries = archive.entries().map_err(|error| format!("not a tar.gz archive: {error}"))?;
    let mut skills = Vec::new();
    for entry in entries {
        let mut entry = entry.map_err(|error| format!("could not read the archive: {error}"))?;
        if !entry.header().entry_type().is_file() || entry.size() > MAX_FILE_BYTES {
            continue;
        }
        let path = entry.path().map_err(|error| error.to_string())?.into_owned();
        let Some(relative) = inner_path(&path, subdirs) else { continue };
        if !wanted.is_empty() && !relative.components().any(|part| wanted.iter().any(|skill| part.as_os_str() == skill.as_str())) {
            continue;
        }
        let target = into.join(&relative);
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent).map_err(|error| format!("could not create {}: {error}", parent.display()))?;
        }
        let mut content = Vec::with_capacity(entry.size() as usize);
        entry.read_to_end(&mut content).map_err(|error| error.to_string())?;
        std::fs::write(&target, content).map_err(|error| format!("could not write {}: {error}", target.display()))?;
        if relative.file_name().is_some_and(|name| name == "SKILL.md") {
            if let Some(skill) = relative.parent().and_then(Path::file_name) {
                skills.push(skill.to_string_lossy().into_owned());
            }
        }
    }
    if skills.is_empty() {
        let _ = std::fs::remove_dir_all(into);
        return Err("the archive holds no SKILL.md in the folders asked for".into());
    }
    skills.sort();
    Ok(skills)
}

/// Where an archive entry lands: its path without the top folder, inside `subdirs` when given, never climbing out.
fn inner_path(path: &Path, subdirs: &[String]) -> Option<PathBuf> {
    let mut parts = path.components();
    parts.next()?;
    let relative: PathBuf = parts.as_path().to_path_buf();
    if relative.as_os_str().is_empty() || relative.components().any(|part| !matches!(part, Component::Normal(_))) {
        return None;
    }
    if !subdirs.is_empty() && !subdirs.iter().any(|sub| relative.starts_with(sub.trim_matches('/'))) {
        return None;
    }
    Some(relative)
}

/// Every pack installed this way, by its marker.
pub fn list() -> Vec<Pack> {
    let Ok(dir) = packs_dir() else { return Vec::new() };
    let Ok(entries) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut packs: Vec<Pack> = entries
        .flatten()
        .filter_map(|entry| std::fs::read_to_string(entry.path().join(MARKER)).ok())
        .filter_map(|text| serde_json::from_str(&text).ok())
        .collect();
    packs.sort_by(|a, b| a.name.cmp(&b.name));
    packs
}

/// The folders skills are read from, as the engine walks them: the workspace's up to its repository root, then the user's.
fn skill_dirs(workspace: Option<&Path>) -> Vec<(PathBuf, bool)> {
    let home = super::home();
    let user: Vec<PathBuf> = home.iter().flat_map(|home| super::HOME_SKILL_DIRS.map(|skills| home.join(skills))).collect();
    let mut dirs: Vec<(PathBuf, bool)> = workspace
        .map(|workspace| super::skill_folders(workspace, home.as_deref(), Vec::new()))
        .unwrap_or_default()
        .into_iter()
        .filter(|dir| !user.contains(dir))
        .map(|dir| (dir, true))
        .collect();
    dirs.extend(user.into_iter().map(|dir| (dir, false)));
    dirs
}

/// Every skill in the user's folders and the workspace's, each marked off when its folder is in `off`.
pub fn list_skills(workspace: Option<&Path>, off: &[PathBuf]) -> Vec<UserSkill> {
    let packs = packs_dir().unwrap_or_default();
    let mut skills = Vec::new();
    for (dir, in_workspace) in skill_dirs(workspace) {
        for file in super::skill_files(&dir) {
            let Ok(text) = std::fs::read_to_string(&file) else { continue };
            let folder = file.parent().unwrap_or(&dir);
            let doc = super::frontmatter::parse(&text);
            let name = doc.field("name").unwrap_or_else(|| folder.file_name().unwrap_or_default().to_string_lossy().into_owned());
            let pack = folder.strip_prefix(&packs).ok().and_then(|rest| rest.components().next()).map(|part| part.as_os_str().to_string_lossy().into_owned());
            skills.push(UserSkill { name, description: doc.field("description").unwrap_or_default(), path: folder.to_string_lossy().into_owned(), pack, workspace: in_workspace, enabled: !off.contains(&crate::tool::canonical(folder)) });
        }
    }
    skills.sort_by(|a, b| a.pack.cmp(&b.pack).then(a.name.cmp(&b.name)));
    skills
}

/// The skill folder the switch names, as the engine compares it; only a folder under a skill folder of the user's or the workspace's.
pub fn skill_folder(folder: &str, workspace: Option<&Path>) -> Result<PathBuf, String> {
    let folder = Path::new(folder);
    let allowed = skill_dirs(workspace).iter().any(|(dir, _)| folder.starts_with(dir));
    if !allowed || folder.components().any(|part| matches!(part, Component::ParentDir)) {
        return Err("not one of your skills".into());
    }
    Ok(crate::tool::canonical(folder))
}

pub fn remove(id: &str) -> Result<(), String> {
    if !valid_id(id) {
        return Err("no such pack".into());
    }
    let dir = packs_dir()?.join(id);
    if !dir.join(MARKER).is_file() {
        return Err("no such pack".into());
    }
    std::fs::remove_dir_all(&dir).map_err(|error| format!("could not remove {}: {error}", dir.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn archive(files: &[(&str, &str)]) -> Vec<u8> {
        let mut builder = tar::Builder::new(flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default()));
        for (path, text) in files {
            let mut header = tar::Header::new_gnu();
            header.set_size(text.len() as u64);
            header.set_mode(0o644);
            header.set_cksum();
            builder.append_data(&mut header, path, text.as_bytes()).unwrap();
        }
        builder.into_inner().unwrap().finish().unwrap()
    }

    #[test]
    fn a_workspaces_own_skills_list_beside_the_users_and_a_switch_never_touches_their_files() {
        let root = std::env::temp_dir().join(format!("drift-skill-list-{}", crate::random_hex(4)));
        let ws = root.join("ws");
        std::fs::create_dir_all(ws.join(".drift/skills/review")).unwrap();
        std::fs::write(ws.join(".drift/skills/review/SKILL.md"), "---
name: review
description: Reviews a diff.
---
body").unwrap();
        std::fs::create_dir_all(ws.join(".claude/skills/plain")).unwrap();
        std::fs::write(ws.join(".claude/skills/plain/SKILL.md"), "no front matter").unwrap();
        let listed = list_skills(Some(&ws), &[]);
        let names: Vec<(&str, bool, bool)> = listed.iter().map(|skill| (skill.name.as_str(), skill.workspace, skill.enabled)).collect();
        assert_eq!(names, vec![("plain", true, true), ("review", true, true)], "both layouts, the folder name standing in for a missing one");
        assert_eq!(listed[1].description, "Reviews a diff.");
        let off = skill_folder(&listed[1].path, Some(&ws)).unwrap();
        assert_eq!(list_skills(Some(&ws), std::slice::from_ref(&off)).iter().map(|skill| skill.enabled).collect::<Vec<_>>(), vec![true, false]);
        assert!(ws.join(".drift/skills/review/SKILL.md").is_file(), "the skill's own file is left as it is");
        let config = crate::config::Config::load_skipping(&ws, std::slice::from_ref(&off));
        assert!(config.skill("review").is_none() && config.commands.iter().all(|command| command.name != "review"), "off, the model and the slash menu never see it");
        assert!(config.skill("plain").is_some());
        assert!(skill_folder(&root.join("elsewhere").to_string_lossy(), Some(&ws)).is_err(), "only a skill folder may be switched");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn only_the_asked_folders_land_the_top_folder_is_stripped_and_escapes_are_dropped() {
        let into = std::env::temp_dir().join(format!("drift-pack-{}", crate::random_hex(4)));
        let bytes = archive(&[
            ("repo-main/README.md", "# hi"),
            ("repo-main/skills/grill-me/SKILL.md", "---\nname: grill-me\n---\nask"),
            ("repo-main/skills/grill-me/notes.md", "n"),
            ("repo-main/docs/other/SKILL.md", "not asked for"),
        ]);
        let skills = unpack(&bytes, &into, &["skills".to_owned()], &[]).unwrap();
        assert_eq!(skills, vec!["grill-me".to_owned()]);
        assert!(into.join("skills/grill-me/SKILL.md").is_file());
        assert!(into.join("skills/grill-me/notes.md").is_file());
        assert!(!into.join("README.md").exists() && !into.join("docs").exists());
        assert_eq!(inner_path(Path::new("repo-main/skills/../../escape.md"), &[]), None, "a climbing path is dropped");
        assert_eq!(inner_path(Path::new("repo-main"), &[]), None, "the top folder itself is nothing");
        let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        gz.write_all(b"junk").unwrap();
        assert!(unpack(&gz.finish().unwrap(), &into, &[], &[]).is_err());
        let bytes = archive(&[("r/skills/a/SKILL.md", "a"), ("r/skills/b/SKILL.md", "b"), ("r/skills/b/extra.md", "e")]);
        assert_eq!(unpack(&bytes, &into, &[], &["b".to_owned()]).unwrap(), vec!["b".to_owned()], "only the wanted skill lands");
        assert!(!into.join("skills/a").exists() && into.join("skills/b/extra.md").is_file());
        let _ = std::fs::remove_dir_all(&into);
    }
}
