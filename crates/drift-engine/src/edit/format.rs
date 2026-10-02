//! Formatters run after a tool writes a file. A built-in applies only where the project uses it (its
//! config or dependency is found) and its binary is on PATH; drift.json can add, force on (`true`) or disable.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use tokio::io::AsyncWriteExt;

use crate::config::FormatterConfig;

const TIMEOUT: Duration = Duration::from_secs(20);

/// What tells a built-in the project uses it; a formatter the project does not use would rewrite whole files.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Uses {
    /// Nothing to find: the language has one format (gofmt).
    Always,
    Prettier,
    Ruff,
    Black,
    Rustfmt,
}

struct Builtin {
    name: &'static str,
    /// `$FILE` becomes the path.
    command: &'static [&'static str],
    extensions: &'static [&'static str],
    uses: Uses,
    stdin: bool,
}

const BUILTINS: &[Builtin] = &[
    Builtin { name: "prettier", command: &["prettier", "--write", "$FILE"], extensions: &[".ts", ".tsx", ".js", ".jsx", ".json", ".css", ".md", ".html", ".yaml", ".yml"], uses: Uses::Prettier, stdin: false },
    // Through stdin, so only the edited file is formatted, never the child modules rustfmt would follow.
    Builtin { name: "rustfmt", command: &["rustfmt", "--emit", "stdout", "--edition", "$EDITION"], extensions: &[".rs"], uses: Uses::Rustfmt, stdin: true },
    Builtin { name: "gofmt", command: &["gofmt", "-w", "$FILE"], extensions: &[".go"], uses: Uses::Always, stdin: false },
    Builtin { name: "ruff", command: &["ruff", "format", "$FILE"], extensions: &[".py"], uses: Uses::Ruff, stdin: false },
    Builtin { name: "black", command: &["black", "-q", "$FILE"], extensions: &[".py"], uses: Uses::Black, stdin: false },
];

#[derive(Clone, Debug, PartialEq)]
pub struct Formatter {
    pub name: String,
    pub command: Vec<String>,
    pub extensions: Vec<String>,
    /// For a built-in left to detect: what must be found near the file. `Always` for anything configured.
    pub uses: Uses,
    /// Reads the file on stdin and prints the result, which is written back.
    pub stdin: bool,
}

impl Formatter {
    fn custom(name: &str, command: &[String], extensions: &[String]) -> Self {
        Self { name: name.into(), command: command.to_vec(), extensions: extensions.to_vec(), uses: Uses::Always, stdin: false }
    }
}

/// The formatters that may apply in a workspace: built-ins not disabled (each checked per file
/// against the project, and for an install in it or on PATH), plus custom ones.
pub fn resolve(overrides: &BTreeMap<String, FormatterConfig>) -> Vec<Formatter> {
    let mut out = Vec::new();
    for builtin in BUILTINS {
        let uses = match overrides.get(builtin.name) {
            Some(FormatterConfig::Enabled(false)) => continue,
            Some(FormatterConfig::Custom { command, extensions }) => {
                out.push(Formatter::custom(builtin.name, command, extensions));
                continue;
            }
            Some(FormatterConfig::Enabled(true)) => Uses::Always,
            None => builtin.uses,
        };
        // Whether it is installed is asked per file, since the project's own copy counts.
        out.push(Formatter {
            name: builtin.name.into(),
            command: builtin.command.iter().map(|s| s.to_string()).collect(),
            extensions: builtin.extensions.iter().map(|s| s.to_string()).collect(),
            uses,
            stdin: builtin.stdin,
        });
    }
    for (name, config) in overrides {
        if let FormatterConfig::Custom { command, extensions } = config {
            if !out.iter().any(|f| &f.name == name) {
                out.push(Formatter::custom(name, command, extensions));
            }
        }
    }
    out
}

/// Runs the first formatter that matches the file and that the project uses. Failures are the
/// formatter's problem, not the edit's: logged, never surfaced.
/// `allowed` holds the lines (see [`project_programs`]) of programs installed inside the project that
/// the user allowed; any other such program is the project's own code and never runs.
pub async fn format(path: &Path, workspace: &Path, formatters: &[Formatter], store: &crate::store::Store, allowed: &[String]) -> Option<String> {
    let (formatter, program) = chosen(path, workspace, formatters, allowed)?;
    let edition = edition(path, workspace);
    let parts = formatter.command.iter().skip(1).map(|part| part.replace("$FILE", &path.to_string_lossy()).replace("$EDITION", &edition));
    let mut command = tokio::process::Command::new(program);
    crate::platform::process::use_current_path(&mut command, &Default::default());
    // A stdin formatter finds its config from where it runs, so it runs beside the file.
    let cwd = if formatter.stdin { path.parent().unwrap_or(workspace) } else { workspace };
    command.args(parts).current_dir(cwd).stdout(if formatter.stdin { Stdio::piped() } else { Stdio::null() }).stderr(Stdio::null()).kill_on_drop(true);
    command.stdin(if formatter.stdin { Stdio::piped() } else { Stdio::null() });
    #[cfg(windows)]
    command.creation_flags(0x0800_0000);
    let ran = tokio::time::timeout(TIMEOUT, run(command, formatter.stdin.then_some(path), store)).await;
    matches!(ran, Ok(Some(()))).then(|| formatter.name.clone())
}

/// Runs the formatter; one that reads stdin gets the file, and its output replaces it through the
/// staged writer, so a failed write never cuts the file short.
async fn run(mut command: tokio::process::Command, through_stdin: Option<&Path>, store: &crate::store::Store) -> Option<()> {
    let Some(path) = through_stdin else {
        return command.status().await.ok().filter(|status| status.success()).map(|_| ());
    };
    let before = tokio::fs::read(path).await.ok()?;
    let mut child = command.spawn().ok()?;
    let mut input = child.stdin.take()?;
    let fed_bytes = before.clone();
    let feeding = async move {
        let fed = input.write_all(&fed_bytes).await;
        drop(input);
        fed
    };
    let (fed, output) = tokio::join!(feeding, child.wait_with_output());
    let output = output.ok().filter(|output| output.status.success() && !output.stdout.is_empty() && fed.is_ok())?;
    if output.stdout == before {
        return Some(());
    }
    crate::tool::stage::replace(store, path, &output.stdout).await.ok()
}

/// Where a formatter's program was found.
enum Found {
    /// Software the user installed: on PATH, or a path the user's own config gave.
    User(PathBuf),
    /// A copy inside the project (`node_modules/.bin`): the project's own code.
    Project(PathBuf),
}

/// The formatter that would run on `path`, and its program. A project copy the user has not allowed
/// is skipped, never swapped for one on PATH: another version formats differently.
fn chosen<'a>(path: &'a Path, workspace: &'a Path, formatters: &'a [Formatter], allowed: &[String]) -> Option<(&'a Formatter, PathBuf)> {
    candidates(path, workspace, formatters).find_map(|(formatter, found)| match found {
        Found::User(program) => Some((formatter, program)),
        Found::Project(program) => allowed.contains(&program_line(formatter, &program)).then_some((formatter, program)),
    })
}

/// The formatters that match `path` and that the project uses, with where each one's program is.
fn candidates<'a>(path: &'a Path, workspace: &'a Path, formatters: &'a [Formatter]) -> impl Iterator<Item = (&'a Formatter, Found)> + 'a {
    let name = path.file_name().map(|name| name.to_string_lossy().to_lowercase()).unwrap_or_default();
    formatters
        .iter()
        .filter(move |f| f.extensions.iter().any(|ext| name.ends_with(ext.as_str())) && project_uses(f.uses, path, workspace))
        .filter_map(move |f| Some((f, locate(f.command.first()?, path, workspace)?)))
}

/// The programs inside the project that formatting `files` could run, one line each, for the
/// approval the project's own commands need. A line names the file and a hash of its content
/// (`formatter prettier: C:\repo\node_modules\.bin\prettier.cmd (3f2a9c10)`), so a program replaced
/// at the same path is asked about again.
pub fn project_programs(files: &[PathBuf], workspace: &Path, formatters: &[Formatter]) -> Vec<String> {
    let mut lines: Vec<String> = files
        .iter()
        .flat_map(|file| candidates(file, workspace, formatters).filter_map(|(formatter, found)| match found {
            Found::Project(program) => Some(program_line(formatter, &program)),
            Found::User(_) => None,
        }).collect::<Vec<_>>())
        .collect();
    lines.sort();
    lines.dedup();
    lines
}

fn program_line(formatter: &Formatter, program: &Path) -> String {
    use sha2::Digest;
    let bytes = std::fs::read(program).unwrap_or_default();
    let hash: String = sha2::Sha256::digest(&bytes).iter().take(4).map(|b| format!("{b:02x}")).collect();
    format!("formatter {}: {} ({hash})", formatter.name, program.display())
}

/// A formatter's program: the project's own install first (`node_modules/.bin` from the file's
/// directory up to the repository root, where prettier usually lives), else PATH.
fn locate(name: &str, file: &Path, workspace: &Path) -> Option<Found> {
    if Path::new(name).components().count() > 1 {
        return crate::platform::process::which(name).map(Found::User);
    }
    let names: Vec<String> = if cfg!(windows) { vec![format!("{name}.cmd"), format!("{name}.exe"), name.to_string()] } else { vec![name.to_string()] };
    let local = dirs_up(file, workspace).into_iter().map(|dir| dir.join("node_modules").join(".bin")).find_map(|bin| names.iter().map(|n| bin.join(n)).find(|candidate| candidate.is_file()));
    match local {
        Some(program) => Some(Found::Project(program)),
        None => crate::platform::process::which(name).map(Found::User),
    }
}

/// Whether the project around `file` uses this formatter: its config or dependency in the file's
/// directory or any above it, up to the repository root (or the workspace when there is none).
fn project_uses(uses: Uses, file: &Path, workspace: &Path) -> bool {
    let dirs = dirs_up(file, workspace);
    let has = |names: &[&str]| dirs.iter().any(|dir| names.iter().any(|name| dir.join(name).is_file()));
    let mentions = |name: &str, needle: &str| dirs.iter().any(|dir| std::fs::read_to_string(dir.join(name)).is_ok_and(|text| text.contains(needle)));
    match uses {
        Uses::Always => true,
        Uses::Prettier => {
            has(&[".prettierrc", ".prettierrc.json", ".prettierrc.yaml", ".prettierrc.yml", ".prettierrc.json5", ".prettierrc.js", ".prettierrc.cjs", ".prettierrc.mjs", ".prettierrc.toml", "prettier.config.js", "prettier.config.cjs", "prettier.config.mjs", "prettier.config.ts"])
                || mentions("package.json", "\"prettier\"")
        }
        Uses::Ruff => has(&["ruff.toml", ".ruff.toml"]) || mentions("pyproject.toml", "[tool.ruff"),
        Uses::Black => mentions("pyproject.toml", "[tool.black"),
        Uses::Rustfmt => has(&["rustfmt.toml", ".rustfmt.toml"]),
    }
}

/// The Rust edition of the crate holding `file`, from its nearest Cargo.toml; 2021 when none says.
fn edition(file: &Path, workspace: &Path) -> String {
    let named = |dir: &PathBuf| {
        let text = std::fs::read_to_string(dir.join("Cargo.toml")).ok()?;
        let line = text.lines().find(|line| line.trim_start().starts_with("edition"))?;
        Some(line.split('"').nth(1)?.to_string())
    };
    dirs_up(file, workspace).iter().find_map(named).unwrap_or_else(|| "2021".into())
}

/// The file's directory and those above it, stopping at the repository root, else at the workspace.
fn dirs_up(file: &Path, workspace: &Path) -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    for dir in file.ancestors().skip(1) {
        dirs.push(dir.to_path_buf());
        if dir.join(".git").exists() || (dir == workspace && !crate::config::in_repository(workspace)) {
            break;
        }
    }
    dirs
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn format(path: &Path, workspace: &Path, formatters: &[Formatter]) -> Option<String> {
        super::format(path, workspace, formatters, &crate::store::tests::store(), &[]).await
    }

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
        let ok = vec![Formatter::custom("echo", &command, &[".txt".into()])];
        assert_eq!(format(&file, &dir, &ok).await.as_deref(), Some("echo"));
        assert!(std::fs::read_to_string(&file).unwrap().starts_with("formatted"));
        let broken = vec![Formatter::custom("nope", &["definitely-missing-binary".into(), "$FILE".into()], &[".txt".into()])];
        assert_eq!(format(&file, &dir, &broken).await, None);
        assert_eq!(format(&dir.join("b.xyz"), &dir, &ok).await, None, "no formatter for the extension");
        std::fs::remove_dir_all(dir).ok();
    }

    #[tokio::test]
    async fn a_formatter_installed_as_a_script_shim_runs() {
        let dir = std::env::temp_dir().join(format!("drift-fmt-shim-{}", crate::random_hex(4)));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("a.txt");
        std::fs::write(&file, "x").unwrap();
        let (shim, body) = if cfg!(windows) { ("tidy.cmd", "@echo shimmed> %1\r\n") } else { ("tidy", "#!/bin/sh\necho shimmed > \"$1\"\n") };
        std::fs::write(dir.join(shim), body).unwrap();
        #[cfg(unix)]
        std::fs::set_permissions(dir.join(shim), std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
        let tidy = vec![Formatter::custom("tidy", &[dir.join(shim).to_string_lossy().into(), "$FILE".into()], &[".txt".into()])];
        assert_eq!(format(&file, &dir, &tidy).await.as_deref(), Some("tidy"));
        assert!(std::fs::read_to_string(&file).unwrap().starts_with("shimmed"));
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn a_built_in_applies_only_where_the_project_uses_it() {
        let root = std::env::temp_dir().join(format!("drift-fmt-uses-{}", crate::random_hex(4)));
        let (repo, web, api) = (root.join("repo"), root.join("repo/web"), root.join("repo/api"));
        for dir in [&web, &api] {
            std::fs::create_dir_all(dir).unwrap();
        }
        std::fs::create_dir_all(repo.join(".git")).unwrap();
        std::fs::write(web.join("package.json"), r#"{ "devDependencies": { "prettier": "^3" } }"#).unwrap();
        std::fs::write(api.join("package.json"), r#"{ "dependencies": { "express": "^5" } }"#).unwrap();
        assert!(project_uses(Uses::Prettier, &web.join("a.ts"), &repo), "declared beside the file");
        assert!(!project_uses(Uses::Prettier, &api.join("a.ts"), &repo), "a global prettier never touches a project that does not use it");
        assert!(!project_uses(Uses::Rustfmt, &api.join("lib.rs"), &repo), "no rustfmt.toml, no rustfmt");
        std::fs::write(repo.join("rustfmt.toml"), "max_width = 160\n").unwrap();
        assert!(project_uses(Uses::Rustfmt, &api.join("lib.rs"), &repo), "found up to the repository root");
        std::fs::write(root.join("pyproject.toml"), "[tool.ruff]\n").unwrap();
        assert!(!project_uses(Uses::Ruff, &api.join("a.py"), &repo), "nothing above the repository root counts");
        std::fs::write(api.join("Cargo.toml"), "[package]\nedition.workspace = true\n").unwrap();
        std::fs::write(repo.join("Cargo.toml"), "[workspace.package]\nedition = \"2024\"\n").unwrap();
        assert_eq!(edition(&api.join("lib.rs"), &repo), "2024", "a member inheriting its edition gets the workspace's");
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn a_projects_own_install_is_found_only_when_allowed_and_named_for_approval() {
        let root = std::env::temp_dir().join(format!("drift-fmt-local-{}", crate::random_hex(4)));
        let bin = root.join("node_modules").join(".bin");
        std::fs::create_dir_all(&bin).unwrap();
        std::fs::create_dir_all(root.join("src")).unwrap();
        let shim = if cfg!(windows) { "prettier.cmd" } else { "prettier" };
        std::fs::write(bin.join(shim), "").unwrap();
        std::fs::write(root.join("package.json"), r#"{ "devDependencies": { "prettier": "^3" } }"#).unwrap();
        let file = root.join("src/a.ts");
        let formatters = resolve(&BTreeMap::new());
        assert!(matches!(locate("prettier", &file, &root), Some(Found::Project(found)) if found == bin.join(shim)), "a devDependency in node_modules/.bin");
        assert!(locate("definitely-not-installed-anywhere", &file, &root).is_none());
        let lines = project_programs(std::slice::from_ref(&file), &root, &formatters);
        assert_eq!(lines.len(), 1);
        assert!(lines[0].starts_with(&format!("formatter prettier: {} (", bin.join(shim).display())), "the project's binary goes on the approval card: {}", lines[0]);
        assert!(chosen(&file, &root, &formatters, &[]).is_none(), "never the project's copy unless allowed, and never a global one in its place");
        assert_eq!(chosen(&file, &root, &formatters, &lines).map(|(_, program)| program), Some(bin.join(shim)));
        std::fs::write(bin.join(shim), "replaced").unwrap();
        assert!(chosen(&file, &root, &formatters, &lines).is_none(), "a program replaced at the same path needs allowing again");
        assert!(project_programs(&[root.join("notes.txt")], &root, &formatters).is_empty());
        std::fs::remove_dir_all(root).ok();
    }

    #[tokio::test]
    async fn a_stdin_formatter_rewrites_only_the_file_it_was_given() {
        let dir = std::env::temp_dir().join(format!("drift-fmt-stdin-{}", crate::random_hex(4)));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("a.txt");
        std::fs::write(&file, "b\na\n").unwrap();
        let filter: Vec<String> = if cfg!(windows) { vec!["findstr".into(), "a".into()] } else { vec!["grep".into(), "a".into()] };
        let keep_a = vec![Formatter { stdin: true, ..Formatter::custom("keep-a", &filter, &[".txt".into()]) }];
        assert_eq!(format(&file, &dir, &keep_a).await.as_deref(), Some("keep-a"));
        assert_eq!(std::fs::read_to_string(&file).unwrap().replace("\r\n", "\n"), "a\n", "its output replaced the file");
        std::fs::remove_dir_all(dir).ok();
    }
}
