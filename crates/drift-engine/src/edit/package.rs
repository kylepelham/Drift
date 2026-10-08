//! What a launcher in a project's `node_modules/.bin` runs: the package it starts, its plugins and
//! every package those depend on, hashed file by file, so an approval covers the code and not just
//! a version.

use std::collections::{BTreeSet, HashMap};
use std::path::{Component, Path, PathBuf};
use std::sync::Mutex;
use std::time::SystemTime;

use sha2::{Digest, Sha256};

/// Past this many packages the rest of a dependency tree is not followed.
const MAX_PACKAGES: usize = 2000;

/// Bytes of the SHA-256 kept: the hash is shown on the approval card, so it must be too long to
/// match by padding a tampered package.
const HASH_BYTES: usize = 16;

/// The dependency fields followed; peers too, which a plugin names the program it extends by.
const DEPENDENCY_FIELDS: [&str; 3] = ["dependencies", "optionalDependencies", "peerDependencies"];

/// A launcher's package version (when its package is found) and a hash of the launcher, that
/// package, the project's plugins for it and everything those depend on.
pub(super) fn fingerprint(launcher: &Path, name: &str) -> (Option<String>, String) {
    let mut digest = Sha256::new();
    digest.update(std::fs::read(launcher).unwrap_or_default());
    let root = package_root(launcher, name);
    let version = root
        .as_deref()
        .and_then(|root| manifest(root)["version"].as_str().map(str::to_string));
    let roots: Vec<PathBuf> = root.into_iter().chain(plugins(launcher, name)).collect();
    let mut seen = HashMap::new();
    for package in closure(&roots) {
        digest.update(package.to_string_lossy().as_bytes());
        hash_package(&package, &mut digest, &mut seen);
    }
    remember(launcher, seen);

    let digest = digest.finalize();
    (version, crate::hex_bytes(&digest[..HASH_BYTES]))
}

/// The project's plugins and shared configs for the program, by the npm naming convention
/// (`prettier-plugin-*`, `@scope/prettier-plugin-*`, `eslint-config-*`), from the `package.json`
/// beside the launcher's `node_modules`: loaded at run time, so no dependency field names them.
fn plugins(launcher: &Path, name: &str) -> Vec<PathBuf> {
    let Some(project) = launcher.parent().and_then(Path::parent).and_then(Path::parent) else {
        return Vec::new();
    };
    let manifest = manifest(project);
    let fields = ["dependencies", "devDependencies", "optionalDependencies"]
        .into_iter()
        .filter_map(|key| manifest[key].as_object());
    let named = |dependency: &&String| {
        let bare = dependency.rsplit('/').next().unwrap_or_default();
        [format!("{name}-plugin"), format!("{name}-config")]
            .iter()
            .any(|prefix| bare.starts_with(prefix.as_str()))
    };
    fields
        .flat_map(|deps| deps.keys().filter(named).cloned().collect::<Vec<_>>())
        .filter_map(|dependency| resolve(project, &dependency))
        .collect()
}

/// The package the launcher starts: where a symlink points, else the paths a shim names (npm's
/// `.cmd` and `.ps1`, sh scripts, bun's `.bunx`), else `node_modules/<name>`.
fn package_root(launcher: &Path, name: &str) -> Option<PathBuf> {
    let bin = launcher.parent()?;
    let linked = std::fs::canonicalize(launcher)
        .ok()
        .filter(|target| target.parent() != std::fs::canonicalize(bin).ok().as_deref());
    let shim = launcher.with_extension("bunx");
    let text = [launcher, &shim]
        .iter()
        .filter_map(|file| std::fs::read(file).ok())
        .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
        .collect::<Vec<_>>()
        .join("\n");
    let named = shim_targets(&text)
        .into_iter()
        .map(|target| normalize(&bin.join(target)));
    linked
        .into_iter()
        .chain(named)
        .find(|target| target.is_file())
        .and_then(|target| enclosing_package(&target))
        .or_else(|| {
            let fallback = bin.parent()?.join(name);
            fallback
                .join("package.json")
                .is_file()
                .then(|| std::fs::canonicalize(&fallback).unwrap_or(fallback))
        })
}

/// Paths a shim gives relative to its own directory.
fn shim_targets(text: &str) -> Vec<String> {
    let mut found = Vec::new();
    for marker in ["%~dp0", "%dp0%", "$basedir", "$PSScriptRoot"] {
        for (at, _) in text.match_indices(marker) {
            let rest = &text[at + marker.len()..];
            let path: String = rest
                .chars()
                .take_while(|c| !c.is_whitespace() && !matches!(c, '"' | '\'' | '%' | '*' | '`'))
                .collect();
            let path = path.trim_start_matches(['\\', '/']);
            if !path.is_empty() {
                found.push(path.replace('\\', "/"));
            }
        }
    }
    found
}

fn normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir => {}
            other => out.push(other),
        }
    }
    out
}

/// The nearest directory above `file` holding a `package.json`, its symlinks resolved.
fn enclosing_package(file: &Path) -> Option<PathBuf> {
    let file = std::fs::canonicalize(file).unwrap_or_else(|_| file.to_path_buf());
    file.ancestors()
        .skip(1)
        .find(|dir| dir.join("package.json").is_file())
        .map(Path::to_path_buf)
}

fn manifest(package: &Path) -> serde_json::Value {
    std::fs::read(package.join("package.json"))
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

/// The packages and everything they depend on, each found as Node finds it, sorted.
fn closure(roots: &[PathBuf]) -> BTreeSet<PathBuf> {
    let mut seen: BTreeSet<PathBuf> = roots.iter().cloned().collect();
    let mut queue = roots.to_vec();
    while let Some(package) = queue.pop() {
        let manifest = manifest(&package);
        let names = DEPENDENCY_FIELDS
            .into_iter()
            .filter_map(|key| manifest[key].as_object())
            .flat_map(|deps| deps.keys().cloned().collect::<Vec<_>>());
        for dependency in names.filter_map(|name| resolve(&package, &name)) {
            if seen.len() < MAX_PACKAGES && seen.insert(dependency.clone()) {
                queue.push(dependency);
            }
        }
    }
    seen
}

/// Where Node would load `name` from for code in `package`: the nearest `node_modules/<name>` above it.
fn resolve(package: &Path, name: &str) -> Option<PathBuf> {
    package
        .ancestors()
        .filter(|dir| dir.file_name().is_none_or(|last| last != "node_modules"))
        .map(|dir| dir.join("node_modules").join(name))
        .find(|candidate| candidate.join("package.json").is_file())
        .map(|found| std::fs::canonicalize(&found).unwrap_or(found))
}

/// Every file of the package, by path, its own `node_modules` left to [`closure`].
fn hash_package(package: &Path, digest: &mut Sha256, seen: &mut HashMap<PathBuf, Seen>) {
    let mut files = Vec::new();
    let mut dirs = vec![package.to_path_buf()];
    while let Some(dir) = dirs.pop() {
        for entry in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
            let Ok(kind) = entry.file_type() else { continue };
            if kind.is_file() {
                files.push(entry.path());
            } else if kind.is_dir() && entry.file_name() != "node_modules" {
                dirs.push(entry.path());
            }
        }
    }
    files.sort();
    for file in files {
        digest.update(file.strip_prefix(package).unwrap_or(&file).to_string_lossy().as_bytes());
        digest.update(content_hash(&file, seen));
    }
}

type Seen = (u64, Option<SystemTime>, [u8; 32]);

/// File hashes from each launcher's last fingerprint, so a format after every edit reads only what
/// changed. Each fingerprint replaces its launcher's entry, so files no longer used drop out.
static KNOWN: Mutex<Option<HashMap<PathBuf, HashMap<PathBuf, Seen>>>> = Mutex::new(None);

fn remember(launcher: &Path, seen: HashMap<PathBuf, Seen>) {
    KNOWN
        .lock()
        .unwrap()
        .get_or_insert_with(HashMap::new)
        .insert(launcher.to_path_buf(), seen);
}

/// A file's hash, from any launcher's last fingerprint while its size and modified time hold.
fn content_hash(file: &Path, seen: &mut HashMap<PathBuf, Seen>) -> [u8; 32] {
    let meta = std::fs::metadata(file).ok();
    let stamp = (
        meta.as_ref().map_or(0, std::fs::Metadata::len),
        meta.and_then(|m| m.modified().ok()),
    );
    let known = KNOWN
        .lock()
        .unwrap()
        .iter()
        .flatten()
        .find_map(|(_, files)| files.get(file).copied())
        .filter(|(len, modified, _)| (*len, *modified) == stamp);
    let hash = known.map_or_else(
        || Sha256::digest(std::fs::read(file).unwrap_or_default()).into(),
        |(_, _, hash)| hash,
    );
    seen.insert(file.to_path_buf(), (stamp.0, stamp.1, hash));
    hash
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(path: &Path, text: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    #[test]
    fn a_change_anywhere_in_the_package_or_its_dependencies_changes_the_hash() {
        let root = std::env::temp_dir().join(format!("drift-pkg-{}", crate::random_hex(4)));
        let modules = root.join("node_modules");
        write(
            &modules.join(".bin/fmt.cmd"),
            "@ECHO off\r\n\"%_prog%\"  \"%dp0%\\..\\@acme\\fmt\\bin\\fmt.js\" %*\r\n",
        );
        write(
            &modules.join("@acme/fmt/package.json"),
            r#"{ "version": "1.2.0", "dependencies": { "helper": "^1" } }"#,
        );
        write(&modules.join("@acme/fmt/bin/fmt.js"), "run()");
        write(&modules.join("helper/package.json"), r#"{ "version": "1.0.0" }"#);
        write(&modules.join("helper/index.js"), "help()");
        let launcher = modules.join(".bin/fmt.cmd");
        let (version, first) = fingerprint(&launcher, "fmt");
        assert_eq!(
            version.as_deref(),
            Some("1.2.0"),
            "found through the shim, not by the command's name"
        );
        write(&modules.join("@acme/fmt/bin/fmt.js"), "steal()");
        let (_, edited) = fingerprint(&launcher, "fmt");
        assert_ne!(first, edited, "the package's code, with its package.json untouched");
        write(&modules.join("helper/index.js"), "steal()");
        let helper = fingerprint(&launcher, "fmt").1;
        assert_ne!(edited, helper, "a dependency's code");
        assert_eq!(helper.len(), 32, "16 bytes, too many to match by padding");
        write(
            &root.join("package.json"),
            r#"{ "devDependencies": { "@acme/fmt": "^1", "fmt-plugin-sort": "^1" } }"#,
        );
        write(
            &modules.join("fmt-plugin-sort/package.json"),
            r#"{ "peerDependencies": { "peer-only": "*" } }"#,
        );
        write(&modules.join("peer-only/package.json"), "{}");
        let with_plugin = fingerprint(&launcher, "fmt").1;
        assert_ne!(helper, with_plugin, "the project's plugin for it counts");
        write(&modules.join("peer-only/index.js"), "steal()");
        assert_ne!(
            with_plugin,
            fingerprint(&launcher, "fmt").1,
            "and what the plugin names as a peer"
        );
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn shims_name_their_targets_relative_to_themselves() {
        assert_eq!(
            shim_targets(r#""%_prog%" "%dp0%\..\prettier\bin\prettier.cjs" %*"#),
            ["../prettier/bin/prettier.cjs"]
        );
        assert_eq!(
            shim_targets(r#"exec node "$basedir/../prettier/bin/prettier.cjs" "$@""#),
            ["../prettier/bin/prettier.cjs"]
        );
    }
}
