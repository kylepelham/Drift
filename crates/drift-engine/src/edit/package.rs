//! What a launcher in a project's `node_modules/.bin` runs: the package it starts and every package
//! that one depends on, hashed file by file, so an approval covers the code and not just a version.

use std::collections::{BTreeSet, HashMap};
use std::path::{Component, Path, PathBuf};
use std::sync::Mutex;
use std::time::SystemTime;

use sha2::{Digest, Sha256};

/// Past this many packages the rest of a dependency tree is not followed.
const MAX_PACKAGES: usize = 2000;

/// A launcher's package version (when its package is found) and a hash of the launcher, that
/// package and its dependencies.
pub fn fingerprint(launcher: &Path, name: &str) -> (Option<String>, String) {
    let mut digest = Sha256::new();
    digest.update(std::fs::read(launcher).unwrap_or_default());
    let root = package_root(launcher, name);
    let version = root.as_deref().and_then(|root| manifest(root)["version"].as_str().map(str::to_string));
    for package in root.map(|root| closure(&root)).unwrap_or_default() {
        digest.update(package.to_string_lossy().as_bytes());
        hash_package(&package, &mut digest);
    }
    (version, digest.finalize().iter().take(4).map(|b| format!("{b:02x}")).collect())
}

/// The package the launcher starts: where a symlink points, else the paths a shim names (npm's
/// `.cmd` and `.ps1`, sh scripts, bun's `.bunx`), else `node_modules/<name>`.
fn package_root(launcher: &Path, name: &str) -> Option<PathBuf> {
    let bin = launcher.parent()?;
    let linked = std::fs::canonicalize(launcher).ok().filter(|target| target.parent() != std::fs::canonicalize(bin).ok().as_deref());
    let shim = launcher.with_extension("bunx");
    let text = [launcher, &shim].iter().filter_map(|file| std::fs::read(file).ok()).map(|bytes| String::from_utf8_lossy(&bytes).into_owned()).collect::<Vec<_>>().join("\n");
    let named = shim_targets(&text).into_iter().map(|target| normalize(&bin.join(target)));
    linked.into_iter().chain(named).find(|target| target.is_file()).and_then(|target| enclosing_package(&target)).or_else(|| {
        let fallback = bin.parent()?.join(name);
        fallback.join("package.json").is_file().then(|| std::fs::canonicalize(&fallback).unwrap_or(fallback))
    })
}

/// Paths a shim gives relative to its own directory.
fn shim_targets(text: &str) -> Vec<String> {
    let mut found = Vec::new();
    for marker in ["%~dp0", "%dp0%", "$basedir", "$PSScriptRoot"] {
        for (at, _) in text.match_indices(marker) {
            let rest = &text[at + marker.len()..];
            let path: String = rest.chars().take_while(|c| !c.is_whitespace() && !matches!(c, '"' | '\'' | '%' | '*' | '`')).collect();
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
    file.ancestors().skip(1).find(|dir| dir.join("package.json").is_file()).map(Path::to_path_buf)
}

fn manifest(package: &Path) -> serde_json::Value {
    std::fs::read(package.join("package.json")).ok().and_then(|bytes| serde_json::from_slice(&bytes).ok()).unwrap_or_default()
}

/// The package and everything it depends on, each found as Node finds it, sorted.
fn closure(root: &Path) -> BTreeSet<PathBuf> {
    let mut seen = BTreeSet::from([root.to_path_buf()]);
    let mut queue = vec![root.to_path_buf()];
    while let Some(package) = queue.pop() {
        let manifest = manifest(&package);
        let names = ["dependencies", "optionalDependencies"].into_iter().filter_map(|key| manifest[key].as_object()).flat_map(|deps| deps.keys().cloned().collect::<Vec<_>>());
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
fn hash_package(package: &Path, digest: &mut Sha256) {
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
        digest.update(content_hash(&file));
    }
}

type Seen = (u64, Option<SystemTime>, [u8; 32]);

/// A file's hash, kept while its size and modified time hold, so a format after every edit does not read a package again.
fn content_hash(file: &Path) -> [u8; 32] {
    static KNOWN: Mutex<Option<HashMap<PathBuf, Seen>>> = Mutex::new(None);
    let meta = std::fs::metadata(file).ok();
    let stamp = (meta.as_ref().map_or(0, |m| m.len()), meta.and_then(|m| m.modified().ok()));
    if let Some((len, modified, hash)) = KNOWN.lock().unwrap().get_or_insert_with(HashMap::new).get(file) {
        if (*len, *modified) == stamp {
            return *hash;
        }
    }
    let hash: [u8; 32] = Sha256::digest(std::fs::read(file).unwrap_or_default()).into();
    KNOWN.lock().unwrap().get_or_insert_with(HashMap::new).insert(file.to_path_buf(), (stamp.0, stamp.1, hash));
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
        write(&modules.join(".bin/fmt.cmd"), "@ECHO off\r\n\"%_prog%\"  \"%dp0%\\..\\@acme\\fmt\\bin\\fmt.js\" %*\r\n");
        write(&modules.join("@acme/fmt/package.json"), r#"{ "version": "1.2.0", "dependencies": { "helper": "^1" } }"#);
        write(&modules.join("@acme/fmt/bin/fmt.js"), "run()");
        write(&modules.join("helper/package.json"), r#"{ "version": "1.0.0" }"#);
        write(&modules.join("helper/index.js"), "help()");
        let launcher = modules.join(".bin/fmt.cmd");
        let (version, first) = fingerprint(&launcher, "fmt");
        assert_eq!(version.as_deref(), Some("1.2.0"), "found through the shim, not by the command's name");
        write(&modules.join("@acme/fmt/bin/fmt.js"), "steal()");
        let (_, edited) = fingerprint(&launcher, "fmt");
        assert_ne!(first, edited, "the package's code, with its package.json untouched");
        write(&modules.join("helper/index.js"), "steal()");
        assert_ne!(edited, fingerprint(&launcher, "fmt").1, "a dependency's code");
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn shims_name_their_targets_relative_to_themselves() {
        assert_eq!(shim_targets(r#""%_prog%" "%dp0%\..\prettier\bin\prettier.cjs" %*"#), ["../prettier/bin/prettier.cjs"]);
        assert_eq!(shim_targets(r#"exec node "$basedir/../prettier/bin/prettier.cjs" "$@""#), ["../prettier/bin/prettier.cjs"]);
    }
}
