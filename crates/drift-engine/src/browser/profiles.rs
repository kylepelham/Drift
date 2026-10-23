//! Where browser profiles live and how they are kept small: isolated ones go with their browser, shared ones
//! lose their caches when it ends, and leftovers from a crash are swept away.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::Duration;

use super::Kind;
use crate::store::Workspace;

const ROOT: &str = "browsers";
const SESSIONS: &str = "sessions";
const SHARED: &str = "shared";

/// Folders inside a profile that only hold caches; everything a sign-in needs is elsewhere.
const CACHES: &[&str] = &[
    "Cache",
    "Code Cache",
    "GPUCache",
    "DawnCache",
    "DawnGraphiteCache",
    "DawnWebGPUCache",
    "GraphiteDawnCache",
    "GrShaderCache",
    "ShaderCache",
    "Service Worker/CacheStorage",
    "Service Worker/ScriptCache",
    "component_crx_cache",
    "optimization_guide_model_store",
    "Crashpad",
];

/// A conversation's own profile, removed when its browser ends.
pub(crate) fn isolated(data_dir: &Path, session_id: &str, kind: Kind) -> PathBuf {
    data_dir.join(ROOT).join(SESSIONS).join(session_id).join(kind.id())
}

/// A workspace's profile, kept between its conversations: named for the workspace so the folder says whose it is.
pub(crate) fn shared(data_dir: &Path, workspace: &Workspace, kind: Kind) -> PathBuf {
    data_dir
        .join(ROOT)
        .join(SHARED)
        .join(shared_name(workspace))
        .join(kind.id())
}

fn shared_name(workspace: &Workspace) -> String {
    let readable: String = workspace
        .name
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || character == '-' || character == '_' {
                character
            } else {
                '-'
            }
        })
        .collect();
    let readable = readable.trim_matches('-');
    // Workspace ids are ASCII, so their last six bytes are their last six characters.
    let start = workspace.id.len().saturating_sub(6);
    let short_id = workspace.id.get(start..).unwrap_or(&workspace.id).to_string();

    if readable.is_empty() {
        return short_id;
    }

    format!("{readable}-{short_id}")
}

/// Removes a profile whose browser has ended; its files can stay locked for a moment after the process exits.
pub(crate) async fn remove(profile: &Path) {
    let session_folder = profile.parent().map(Path::to_path_buf);

    for _ in 0..20 {
        match tokio::fs::remove_dir_all(profile).await {
            Ok(()) => break,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => break,
            Err(_) => tokio::time::sleep(Duration::from_millis(250)).await,
        }
    }

    // The conversation's folder goes too once it holds no other engine's profile.
    if let Some(folder) = session_folder {
        let _ = tokio::fs::remove_dir(folder).await;
    }
}

/// Deletes the caches of a shared profile once its browser has ended; sign-ins stay.
pub(crate) async fn trim(profile: &Path) {
    let mut roots = vec![profile.to_path_buf()];
    if let Ok(entries) = std::fs::read_dir(profile) {
        let profiles = entries.flatten().map(|entry| entry.path()).filter(|path| {
            let name = path.file_name().unwrap_or_default().to_string_lossy();
            path.is_dir() && (name == "Default" || name.starts_with("Profile "))
        });
        roots.extend(profiles);
    }

    for root in roots {
        for cache in CACHES {
            let _ = tokio::fs::remove_dir_all(root.join(cache)).await;
        }
    }
}

/// Deletes isolated profiles no running browser uses (a crash leaves them) and shared ones of workspaces Drift
/// no longer has.
pub(crate) fn sweep(data_dir: &Path, running: &HashSet<String>, workspaces: &[Workspace]) {
    let root = data_dir.join(ROOT);

    for entry in read_dirs(&root.join(SESSIONS)) {
        let session = entry.file_name().unwrap_or_default().to_string_lossy().to_string();
        if !running.contains(&session) {
            let _ = std::fs::remove_dir_all(&entry);
        }
    }

    let kept: HashSet<String> = workspaces.iter().map(shared_name).collect();
    for entry in read_dirs(&root.join(SHARED)) {
        let name = entry.file_name().unwrap_or_default().to_string_lossy().to_string();
        if !kept.contains(&name) {
            let _ = std::fs::remove_dir_all(&entry);
        }
    }
}

fn read_dirs(folder: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(folder) else {
        return Vec::new();
    };

    entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.is_dir())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn workspace(id: &str, name: &str) -> Workspace {
        Workspace {
            id: id.into(),
            path: format!("C:/{name}"),
            name: name.into(),
            icon: String::new(),
            last_used: 0,
        }
    }

    #[test]
    fn a_shared_profile_is_named_for_its_workspace() {
        let data = Path::new("data");

        assert_eq!(
            shared(data, &workspace("wrk_0123456789", "My App"), Kind::Edge),
            data.join("browsers/shared/My-App-456789/edge")
        );
        assert_eq!(shared_name(&workspace("wrk_abcdef", "???")), "abcdef");
        assert_eq!(
            isolated(data, "ses_1", Kind::Chrome),
            data.join("browsers/sessions/ses_1/chrome")
        );
    }

    #[tokio::test]
    async fn trimming_keeps_sign_ins_and_drops_caches() {
        let profile = crate::platform::temp::test_run().join(format!("drift-trim-{}", crate::random_hex(4)));
        for folder in ["Default/Cache/Cache_Data", "Default/Network", "GrShaderCache"] {
            std::fs::create_dir_all(profile.join(folder)).unwrap();
        }
        std::fs::write(profile.join("Default/Network/Cookies"), "kept").unwrap();

        trim(&profile).await;

        assert!(profile.join("Default/Network/Cookies").is_file());
        assert!(!profile.join("Default/Cache").exists());
        assert!(!profile.join("GrShaderCache").exists());
        std::fs::remove_dir_all(profile).unwrap();
    }

    #[test]
    fn the_sweep_keeps_what_is_still_in_use() {
        let data = crate::platform::temp::test_run().join(format!("drift-sweep-{}", crate::random_hex(4)));
        let kept = workspace("wrk_aaaaaa", "Kept");
        for folder in [
            "browsers/sessions/ses_live/chrome",
            "browsers/sessions/ses_dead/chrome",
            "browsers/shared/Kept-aaaaaa/chrome",
            "browsers/shared/Gone-bbbbbb/chrome",
        ] {
            std::fs::create_dir_all(data.join(folder)).unwrap();
        }

        sweep(&data, &HashSet::from(["ses_live".to_string()]), &[kept]);

        assert!(data.join("browsers/sessions/ses_live").exists());
        assert!(!data.join("browsers/sessions/ses_dead").exists());
        assert!(data.join("browsers/shared/Kept-aaaaaa").exists());
        assert!(!data.join("browsers/shared/Gone-bbbbbb").exists());
        std::fs::remove_dir_all(data).unwrap();
    }
}
