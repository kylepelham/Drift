//! Formatters run after a tool writes a file. Built-ins apply when their binary is on PATH; drift.json can add or disable.

use std::collections::BTreeMap;
use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use crate::config::FormatterConfig;

const TIMEOUT: Duration = Duration::from_secs(20);

struct Builtin {
    name: &'static str,
    /// `$FILE` becomes the path.
    command: &'static [&'static str],
    extensions: &'static [&'static str],
}

const BUILTINS: &[Builtin] = &[
    Builtin { name: "prettier", command: &["prettier", "--write", "$FILE"], extensions: &[".ts", ".tsx", ".js", ".jsx", ".json", ".css", ".md", ".html", ".yaml", ".yml"] },
    Builtin { name: "rustfmt", command: &["rustfmt", "--edition", "2021", "$FILE"], extensions: &[".rs"] },
    Builtin { name: "gofmt", command: &["gofmt", "-w", "$FILE"], extensions: &[".go"] },
    Builtin { name: "ruff", command: &["ruff", "format", "$FILE"], extensions: &[".py"] },
    Builtin { name: "black", command: &["black", "-q", "$FILE"], extensions: &[".py"] },
];

#[derive(Clone, Debug, PartialEq)]
pub struct Formatter {
    pub name: String,
    pub command: Vec<String>,
    pub extensions: Vec<String>,
}

/// The formatters that apply in a workspace: built-ins that are installed and not disabled, plus custom ones.
pub fn resolve(overrides: &BTreeMap<String, FormatterConfig>) -> Vec<Formatter> {
    let mut out = Vec::new();
    for builtin in BUILTINS {
        match overrides.get(builtin.name) {
            Some(FormatterConfig::Enabled(false)) => continue,
            Some(FormatterConfig::Custom { command, extensions }) => {
                out.push(Formatter { name: builtin.name.into(), command: command.clone(), extensions: extensions.clone() });
                continue;
            }
            _ => {}
        }
        if on_path(builtin.command[0]) {
            out.push(Formatter {
                name: builtin.name.into(),
                command: builtin.command.iter().map(|s| s.to_string()).collect(),
                extensions: builtin.extensions.iter().map(|s| s.to_string()).collect(),
            });
        }
    }
    for (name, config) in overrides {
        if let FormatterConfig::Custom { command, extensions } = config {
            if !out.iter().any(|f| &f.name == name) {
                out.push(Formatter { name: name.clone(), command: command.clone(), extensions: extensions.clone() });
            }
        }
    }
    out
}

/// Runs the first matching formatter. Failures are the formatter's problem, not the edit's: logged, never surfaced.
pub async fn format(path: &Path, workspace: &Path, formatters: &[Formatter]) -> Option<String> {
    let name = path.file_name()?.to_string_lossy().to_lowercase();
    let formatter = formatters.iter().find(|f| f.extensions.iter().any(|ext| name.ends_with(ext.as_str())))?;
    let mut parts = formatter.command.iter().map(|part| part.replace("$FILE", &path.to_string_lossy()));
    let program = parts.next()?;
    let mut command = tokio::process::Command::new(&program);
    command.args(parts).current_dir(workspace).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).kill_on_drop(true);
    #[cfg(windows)]
    command.creation_flags(0x0800_0000);
    let ran = tokio::time::timeout(TIMEOUT, command.status()).await;
    match ran {
        Ok(Ok(status)) if status.success() => Some(formatter.name.clone()),
        _ => None,
    }
}

fn on_path(program: &str) -> bool {
    let candidates: Vec<String> = if cfg!(windows) { vec![format!("{program}.exe"), format!("{program}.cmd"), format!("{program}.bat")] } else { vec![program.to_string()] };
    std::env::var_os("PATH")
        .map(|path| {
            std::env::split_paths(&path).any(|dir| candidates.iter().any(|c| dir.join(c).is_file()))
        })
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn overrides_disable_replace_and_add() {
        let mut overrides = BTreeMap::new();
        overrides.insert("prettier".into(), FormatterConfig::Enabled(false));
        overrides.insert("rustfmt".into(), FormatterConfig::Custom { command: vec!["cargo".into(), "fmt".into(), "--".into(), "$FILE".into()], extensions: vec![".rs".into()] });
        overrides.insert("zig".into(), FormatterConfig::Custom { command: vec!["zig".into(), "fmt".into(), "$FILE".into()], extensions: vec![".zig".into()] });
        let resolved = resolve(&overrides);
        assert!(!resolved.iter().any(|f| f.name == "prettier"));
        assert_eq!(resolved.iter().find(|f| f.name == "rustfmt").unwrap().command[0], "cargo");
        assert!(resolved.iter().any(|f| f.name == "zig"));
    }

    #[tokio::test]
    async fn runs_the_matching_formatter_and_ignores_failures() {
        let dir = std::env::temp_dir().join(format!("drift-fmt-{}", crate::random_hex(4)));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("a.txt");
        std::fs::write(&file, "x").unwrap();
        let (program, args): (&str, Vec<&str>) = if cfg!(windows) { ("cmd", vec!["/c", "echo formatted> $FILE"]) } else { ("sh", vec!["-c", "echo formatted > $FILE"]) };
        let mut command = vec![program.to_string()];
        command.extend(args.iter().map(|s| s.to_string()));
        let ok = vec![Formatter { name: "echo".into(), command, extensions: vec![".txt".into()] }];
        assert_eq!(format(&file, &dir, &ok).await.as_deref(), Some("echo"));
        assert!(std::fs::read_to_string(&file).unwrap().starts_with("formatted"));
        let broken = vec![Formatter { name: "nope".into(), command: vec!["definitely-missing-binary".into(), "$FILE".into()], extensions: vec![".txt".into()] }];
        assert_eq!(format(&file, &dir, &broken).await, None);
        assert_eq!(format(&dir.join("b.xyz"), &dir, &ok).await, None, "no formatter for the extension");
        std::fs::remove_dir_all(dir).ok();
    }
}
