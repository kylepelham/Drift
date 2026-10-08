use super::SettingsReport;
use serde_json::{Map, Value};
use std::path::Path;

/// Folder names under opencode's config directory paired with their corresponding Drift folder names.
const FOLDERS: [(&str, &str); 5] = [
    ("agents", "agents"),
    ("agent", "agents"),
    ("commands", "commands"),
    ("command", "commands"),
    ("skills", "skills"),
];

/// Copies opencode's global AGENTS.md, agents, commands and skills to the folders Drift reads.
/// Existing files are never replaced; JavaScript plugins are reported by name, not copied.
pub(super) fn copy_home(from: &Path, to: &Path, report: &mut SettingsReport) {
    copy_file(&from.join("AGENTS.md"), &to.join("AGENTS.md"), "AGENTS.md", report);
    for (source, target) in FOLDERS {
        copy_tree(&from.join(source), &to.join(target), target, report);
    }

    for folder in ["plugins", "plugin"] {
        for entry in std::fs::read_dir(from.join(folder)).into_iter().flatten().flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            report.left(
                &name,
                format!("plugin {name}: opencode plugins are JavaScript and Drift runs none"),
                |left| &mut left.plugins,
            );
        }
    }
}

fn copy_tree(from: &Path, to: &Path, shown: &str, report: &mut SettingsReport) {
    for entry in std::fs::read_dir(from).into_iter().flatten().flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let path = entry.path();
        let shown = format!("{shown}/{name}");
        let destination = to.join(&name);

        if path.is_dir() {
            copy_tree(&path, &destination, &shown, report);
        } else {
            copy_file(&path, &destination, &shown, report);
        }
    }
}

fn copy_file(from: &Path, to: &Path, shown: &str, report: &mut SettingsReport) {
    if !from.is_file() {
        return;
    }

    if to.exists() {
        report
            .skipped
            .push(format!("{shown}: you already have one in ~/.config/drift, kept"));
        return;
    }

    let copied = to
        .parent()
        .map_or(Ok(()), std::fs::create_dir_all)
        .and_then(|()| std::fs::copy(from, to));

    match copied {
        Ok(_) => report.files.push(shown.to_string()),
        Err(error) => report.left(shown, format!("{shown}: could not be copied ({error})"), |left| {
            &mut left.failed
        }),
    }
}

/// Writes drift.json only when some settings map and the user has no file yet; returns the written path.
pub(super) fn write_config(home: &Path, file: Map<String, Value>, report: &mut SettingsReport) -> Option<String> {
    if file.is_empty() {
        return None;
    }

    let path = home.join(".config").join("drift").join(drift_engine::config::FILE);
    let keys = file.keys().cloned().collect::<Vec<_>>().join(", ");
    if path.exists() {
        report.skipped.push(format!(
            "config {keys}: you already have {}, so it was not changed",
            path.display()
        ));
        return None;
    }

    let written = std::fs::create_dir_all(path.parent()?).and_then(|()| {
        let text = serde_json::to_string_pretty(&Value::Object(file)).unwrap();
        std::fs::write(&path, text)
    });

    match written {
        Ok(()) => Some(path.to_string_lossy().into_owned()),
        Err(error) => {
            report.left(
                "drift.json",
                format!("config {keys}: {} could not be written ({error})", path.display()),
                |left| &mut left.failed,
            );
            None
        }
    }
}
