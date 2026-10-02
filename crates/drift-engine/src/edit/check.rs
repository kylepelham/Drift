//! Opt-in checks (drift.json `checks`) run after a tool writes files; what they report goes back to the model.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use crate::config::CheckConfig;
use crate::platform::process;

const TIMEOUT: Duration = Duration::from_secs(60);
/// The most of one check's output the model is shown.
const SHOWN: usize = 4 * 1024;

#[derive(Clone, Debug, PartialEq)]
pub struct Check {
    pub name: String,
    pub command: Vec<String>,
    pub extensions: Vec<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Verdict {
    Passed,
    /// It exited non-zero; what it printed, bounded.
    Problems(String),
    /// It could not start or did not finish; never shown to the model.
    Unavailable(String),
}

#[derive(Clone, Debug, PartialEq)]
pub struct Report {
    pub name: String,
    /// The file it checked; `None` for a check over the whole workspace.
    pub file: Option<PathBuf>,
    pub verdict: Verdict,
}

/// The checks the config defines; one set to `false` is off.
pub fn resolve(config: &BTreeMap<String, CheckConfig>) -> Vec<Check> {
    config
        .iter()
        .filter_map(|(name, check)| match check {
            CheckConfig::Custom { command, extensions } if !command.is_empty() => Some(Check { name: name.clone(), command: command.clone(), extensions: extensions.clone() }),
            _ => None,
        })
        .collect()
}

/// Runs each check that applies to the written files, in config order: per file when its command names `$FILE`, else once.
pub async fn run(files: &[PathBuf], workspace: &Path, checks: &[Check]) -> Vec<Report> {
    let mut reports = Vec::new();
    for check in checks {
        let matching: Vec<&PathBuf> = files.iter().filter(|file| applies(check, file)).collect();
        if matching.is_empty() {
            continue;
        }
        if !check.command.iter().any(|part| part.contains("$FILE")) {
            reports.push(Report { name: check.name.clone(), file: None, verdict: run_one(check, None, workspace).await });
            continue;
        }
        for file in matching {
            reports.push(Report { name: check.name.clone(), file: Some(file.clone()), verdict: run_one(check, Some(file), workspace).await });
        }
    }
    reports
}

fn applies(check: &Check, file: &Path) -> bool {
    let name = file.file_name().map(|name| name.to_string_lossy().to_lowercase()).unwrap_or_default();
    check.extensions.iter().any(|ext| name.ends_with(&ext.to_lowercase()))
}

async fn run_one(check: &Check, file: Option<&PathBuf>, workspace: &Path) -> Verdict {
    let mut parts = check.command.iter().map(|part| match file {
        Some(file) => part.replace("$FILE", &file.to_string_lossy()),
        None => part.clone(),
    });
    let named = parts.next().unwrap_or_default();
    let Some(program) = process::which(&named) else { return Verdict::Unavailable(format!("{named} is not on PATH")) };
    let mut command = tokio::process::Command::new(program);
    process::use_current_path(&mut command, &Default::default());
    command.args(parts).current_dir(workspace).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).kill_on_drop(true);
    #[cfg(windows)]
    command.creation_flags(0x0800_0000);
    process::prepare(&mut command);
    let child = match command.spawn() {
        Ok(child) => child,
        Err(error) => return Verdict::Unavailable(format!("could not start {named}: {error}")),
    };
    // Whatever stops the wait (time, or Stop dropping this future) takes the check's whole process tree with it.
    let _tree = child.id().and_then(|pid| process::Tree::adopt(pid).ok());
    match tokio::time::timeout(TIMEOUT, child.wait_with_output()).await {
        Err(_) => Verdict::Unavailable(format!("took longer than {} s", TIMEOUT.as_secs())),
        Ok(Err(error)) => Verdict::Unavailable(error.to_string()),
        Ok(Ok(output)) if output.status.success() => Verdict::Passed,
        Ok(Ok(output)) => Verdict::Problems(shown(&output.stdout, &output.stderr)),
    }
}

fn shown(stdout: &[u8], stderr: &[u8]) -> String {
    let said = [stdout, stderr].iter().map(|stream| String::from_utf8_lossy(stream).trim_end().to_string()).filter(|text| !text.is_empty()).collect::<Vec<_>>().join("\n");
    if said.is_empty() {
        return "(it printed nothing, but exited with an error)".into();
    }
    if said.len() <= SHOWN {
        return said;
    }
    let cut = (0..=SHOWN).rev().find(|&at| said.is_char_boundary(at)).unwrap_or(0);
    format!("{}\n[{} more bytes not shown]", &said[..cut], said.len() - cut)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shell(script: &str) -> Vec<String> {
        if cfg!(windows) {
            vec!["cmd".into(), "/c".into(), script.into()]
        } else {
            vec!["sh".into(), "-c".into(), script.into()]
        }
    }

    #[tokio::test]
    async fn a_failing_check_reports_what_it_printed_and_a_passing_one_is_quiet() {
        let dir = std::env::temp_dir().join(format!("drift-check-{}", crate::random_hex(4)));
        std::fs::create_dir_all(&dir).unwrap();
        let (a, b) = (dir.join("a.ts"), dir.join("b.md"));
        let checks = vec![
            Check { name: "lint".into(), command: shell("echo bad line in $FILE&& exit 1"), extensions: vec![".TS".into()] },
            Check { name: "whole".into(), command: shell("echo fine"), extensions: vec![".ts".into(), ".md".into()] },
            Check { name: "gone".into(), command: vec!["definitely-missing-checker".into()], extensions: vec![".md".into()] },
            Check { name: "unrelated".into(), command: shell("exit 1"), extensions: vec![".rs".into()] },
        ];
        let reports = run(&[a.clone(), b], &dir, &checks).await;
        assert_eq!(reports.len(), 3, "{reports:?}");
        assert_eq!(reports[0].file.as_ref(), Some(&a), "a $FILE check runs per matching file only");
        let Verdict::Problems(said) = &reports[0].verdict else { panic!("{reports:?}") };
        assert!(said.contains("bad line in") && said.contains("a.ts"), "{said}");
        assert_eq!((reports[1].file.clone(), reports[1].verdict.clone()), (None, Verdict::Passed), "one without $FILE runs once");
        assert!(matches!(&reports[2].verdict, Verdict::Unavailable(why) if why.contains("not on PATH")));
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn long_output_is_cut_on_a_character_boundary() {
        let long = "é".repeat(SHOWN);
        let said = shown(long.as_bytes(), b"");
        assert!(said.ends_with("more bytes not shown]") && said.len() < long.len());
        assert_eq!(shown(b"", b""), "(it printed nothing, but exited with an error)");
    }

    #[test]
    fn false_turns_a_check_off() {
        let mut config = BTreeMap::new();
        config.insert("lint".into(), CheckConfig::Custom { command: vec!["eslint".into(), "$FILE".into()], extensions: vec![".ts".into()] });
        config.insert("types".into(), CheckConfig::Enabled(false));
        assert_eq!(resolve(&config).iter().map(|check| check.name.as_str()).collect::<Vec<_>>(), ["lint"]);
    }
}
