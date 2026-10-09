//! Opt-in checks (drift.json `checks`) run after a tool writes files; what they report goes back to the model.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;
use tokio::io::AsyncReadExt;
use tokio_util::sync::CancellationToken;

use crate::config::CheckConfig;
use crate::platform::process;

const TIMEOUT: Duration = Duration::from_secs(60);
/// How many check runs go at once.
const PARALLEL: usize = 4;
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
            CheckConfig::Custom { command, extensions } if !command.is_empty() => Some(Check {
                name: name.clone(),
                command: command.clone(),
                extensions: extensions.clone(),
            }),
            _ => None,
        })
        .collect()
}

/// Runs the checks that apply to a step's files (per file with `$FILE`, else once), a few at a time within `budget`, reported in config order.
pub async fn run(
    files: &[PathBuf],
    workspace: &Path,
    checks: &[Check],
    budget: Duration,
    stop: &CancellationToken,
) -> Vec<Report> {
    use futures_util::StreamExt;

    let deadline = tokio::time::Instant::now() + budget;
    let runs = planned(files, workspace, checks);

    futures_util::stream::iter(runs)
        .map(|(check, file, workspace)| async move {
            let verdict = run_one(&check, file.as_ref(), &workspace, deadline, stop).await;
            Report {
                name: check.name,
                file,
                verdict,
            }
        })
        .buffered(PARALLEL)
        .collect()
        .await
}

/// Every run a step's files call for, in config order; each owns what it needs, so the runs can go side by side in a
/// future the engine can move between threads.
fn planned(files: &[PathBuf], workspace: &Path, checks: &[Check]) -> Vec<(Check, Option<PathBuf>, PathBuf)> {
    checks
        .iter()
        .flat_map(|check| {
            let matching: Vec<&PathBuf> = files.iter().filter(|file| applies(check, file)).collect();
            let per_file = check.command.iter().any(|part| part.contains("$FILE"));
            match (matching.is_empty(), per_file) {
                (true, _) => Vec::new(),
                (false, true) => matching
                    .into_iter()
                    .map(|file| (check.clone(), Some(file.clone()), workspace.to_path_buf()))
                    .collect(),
                (false, false) => vec![(check.clone(), None, workspace.to_path_buf())],
            }
        })
        .collect()
}

fn applies(check: &Check, file: &Path) -> bool {
    let name = file
        .file_name()
        .map(|name| name.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    check.extensions.iter().any(|ext| name.ends_with(&ext.to_lowercase()))
}

/// Whether any of `checks` runs over `file`, so a change to it while they ran can be theirs.
pub fn covers(checks: &[Check], file: &Path) -> bool {
    checks.iter().any(|check| applies(check, file))
}

async fn run_one(
    check: &Check,
    file: Option<&PathBuf>,
    workspace: &Path,
    deadline: tokio::time::Instant,
    stop: &CancellationToken,
) -> Verdict {
    if stop.is_cancelled() || tokio::time::Instant::now() >= deadline {
        return Verdict::Unavailable("stopped or the step's time for checks ran out".into());
    }

    let mut parts = check.command.iter().map(|part| match file {
        Some(file) => part.replace("$FILE", &file.to_string_lossy()),
        None => part.clone(),
    });
    let named = parts.next().unwrap_or_default();
    let Some(program) = process::which(&named) else {
        return Verdict::Unavailable(format!("{named} is not on PATH"));
    };

    let mut command = tokio::process::Command::new(program);
    process::use_current_path(&mut command, &Default::default());
    command
        .args(parts)
        .current_dir(workspace)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    #[cfg(windows)]
    command.creation_flags(0x0800_0000);

    let (child, tree) = match process::spawn_owned(&mut command).await {
        Ok(owned) => owned,
        Err(error) => return Verdict::Unavailable(format!("could not start {named}: {error}")),
    };
    wait_check(child, tree, deadline.min(tokio::time::Instant::now() + TIMEOUT), stop).await
}

/// Stop and timeouts kill and reap the child before history capture can resume.
async fn wait_check(
    mut child: tokio::process::Child,
    tree: process::Tree,
    deadline: tokio::time::Instant,
    stop: &CancellationToken,
) -> Verdict {
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    let completed = tokio::select! {
        result = async { tokio::try_join!(child.wait(), read_pipe(stdout), read_pipe(stderr)) } => Some(result),
        () = stop.cancelled() => None,
        () = tokio::time::sleep_until(deadline) => None,
    };

    tree.kill();
    let _ = child.kill().await;
    drop(child);
    tree.stop().await;

    match completed {
        None => Verdict::Unavailable("stopped or the step's time for checks ran out".into()),
        Some(Err(error)) => Verdict::Unavailable(error.to_string()),
        Some(Ok((status, _, _))) if status.success() => Verdict::Passed,
        Some(Ok((_, stdout, stderr))) => Verdict::Problems(shown(&stdout, &stderr)),
    }
}

async fn read_pipe(pipe: Option<impl tokio::io::AsyncRead + Unpin>) -> std::io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    if let Some(mut pipe) = pipe {
        pipe.read_to_end(&mut bytes).await?;
    }
    Ok(bytes)
}

fn shown(stdout: &[u8], stderr: &[u8]) -> String {
    let said = [stdout, stderr]
        .iter()
        .map(|stream| String::from_utf8_lossy(stream).trim_end().to_string())
        .filter(|text| !text.is_empty())
        .collect::<Vec<_>>()
        .join("\n");

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
            Check {
                name: "lint".into(),
                command: shell("echo bad line in $FILE&& exit 1"),
                extensions: vec![".TS".into()],
            },
            Check {
                name: "whole".into(),
                command: shell("echo fine"),
                extensions: vec![".ts".into(), ".md".into()],
            },
            Check {
                name: "gone".into(),
                command: vec!["definitely-missing-checker".into()],
                extensions: vec![".md".into()],
            },
            Check {
                name: "unrelated".into(),
                command: shell("exit 1"),
                extensions: vec![".rs".into()],
            },
        ];
        let reports = run(
            &[a.clone(), b],
            &dir,
            &checks,
            Duration::from_secs(60),
            &CancellationToken::new(),
        )
        .await;
        assert_eq!(reports.len(), 3, "{reports:?}");
        assert_eq!(
            reports[0].file.as_ref(),
            Some(&a),
            "a $FILE check runs per matching file only"
        );
        let Verdict::Problems(said) = &reports[0].verdict else {
            panic!("{reports:?}")
        };
        assert!(said.contains("bad line in") && said.contains("a.ts"), "{said}");
        assert_eq!(
            (reports[1].file.clone(), reports[1].verdict.clone()),
            (None, Verdict::Passed),
            "one without $FILE runs once"
        );
        assert!(matches!(&reports[2].verdict, Verdict::Unavailable(why) if why.contains("not on PATH")));
        std::fs::remove_dir_all(dir).ok();
    }

    #[tokio::test]
    async fn per_file_runs_go_side_by_side_and_the_steps_budget_bounds_them_all() {
        let dir = std::env::temp_dir().join(format!("drift-check-par-{}", crate::random_hex(4)));
        std::fs::create_dir_all(&dir).unwrap();
        let files: Vec<PathBuf> = (0..4).map(|index| dir.join(format!("f{index}.ts"))).collect();
        let pause = if cfg!(windows) {
            "ping -n 2 127.0.0.1 > nul && echo $FILE"
        } else {
            "sleep 1; echo $FILE"
        };
        let slow = vec![Check {
            name: "slow".into(),
            command: shell(pause),
            extensions: vec![".ts".into()],
        }];
        let started = std::time::Instant::now();
        let reports = run(&files, &dir, &slow, Duration::from_secs(30), &CancellationToken::new()).await;
        assert!(
            started.elapsed() < Duration::from_millis(2500),
            "four one-second runs side by side: {:?}",
            started.elapsed()
        );
        assert_eq!(
            reports
                .iter()
                .map(|report| report.file.clone().unwrap())
                .collect::<Vec<_>>(),
            files,
            "in order"
        );
        let hang = if cfg!(windows) {
            "ping -n 30 127.0.0.1"
        } else {
            "sleep 30"
        };
        let stuck = vec![Check {
            name: "stuck".into(),
            command: shell(hang),
            extensions: vec![".ts".into()],
        }];
        let started = std::time::Instant::now();
        let reports = run(
            &files[..1],
            &dir,
            &stuck,
            Duration::from_millis(500),
            &CancellationToken::new(),
        )
        .await;
        assert!(started.elapsed() < Duration::from_secs(5));
        assert!(
            matches!(&reports[0].verdict, Verdict::Unavailable(why) if why.contains("ran out")),
            "{reports:?}"
        );
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
        config.insert(
            "lint".into(),
            CheckConfig::Custom {
                command: vec!["eslint".into(), "$FILE".into()],
                extensions: vec![".ts".into()],
            },
        );
        config.insert("types".into(), CheckConfig::Enabled(false));
        assert_eq!(
            resolve(&config)
                .iter()
                .map(|check| check.name.as_str())
                .collect::<Vec<_>>(),
            ["lint"]
        );
    }
}
