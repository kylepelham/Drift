use std::path::{Path, PathBuf};

#[derive(Clone, Debug, PartialEq)]
pub enum Shell {
    Bash(PathBuf),
    PowerShell(PathBuf),
}

/// `DRIFT_SHELL` names the shell outright. Otherwise Git's bash is checked before PATH, because
/// Windows ships a WSL stub named bash.exe in System32; PATH is read as it is now, not at startup.
pub(super) fn detect_shell() -> Shell {
    if let Some(chosen) = std::env::var_os("DRIFT_SHELL")
        .map(PathBuf::from)
        .filter(|path| path.is_file())
    {
        return shell_for(chosen);
    }

    let which = crate::platform::process::which;
    let real = |path: PathBuf| (!path.to_string_lossy().to_ascii_lowercase().contains("system32")).then_some(path);
    if let Some(bash) = git_bash().or_else(|| which("bash").and_then(real)) {
        return Shell::Bash(bash);
    }

    let powershell = which("pwsh")
        .or_else(|| which("powershell"))
        .unwrap_or_else(|| "pwsh".into());
    Shell::PowerShell(powershell)
}

/// A shell named by path: anything called bash, sh or zsh speaks bash; anything else is taken for PowerShell.
pub(super) fn shell_for(path: PathBuf) -> Shell {
    let stem = path
        .file_stem()
        .map(|stem| stem.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();

    if matches!(stem.as_str(), "bash" | "sh" | "zsh") {
        Shell::Bash(path)
    } else {
        Shell::PowerShell(path)
    }
}

/// Git for Windows wherever it is installed: beside the `git` on PATH (Git puts only `cmd` there),
/// then the machine-wide, per-user and Scoop locations.
fn git_bash() -> Option<PathBuf> {
    if !cfg!(windows) {
        return None;
    }

    let environment_path = |name: &str| std::env::var_os(name).map(PathBuf::from);
    let scoop = environment_path("SCOOP").or_else(|| environment_path("USERPROFILE").map(|home| home.join("scoop")));
    let known = [
        Some(PathBuf::from("C:/Program Files/Git/bin/bash.exe")),
        Some(PathBuf::from("C:/Program Files (x86)/Git/bin/bash.exe")),
        environment_path("LOCALAPPDATA").map(|local| local.join("Programs/Git/bin/bash.exe")),
        scoop.map(|scoop| scoop.join("apps/git/current/bin/bash.exe")),
    ];

    crate::platform::process::which("git")
        .and_then(|git| bash_beside(&git))
        .or_else(|| known.into_iter().flatten().find(|candidate| candidate.is_file()))
}

/// `git.exe` sits in `<root>/cmd`, `<root>/bin` or `<root>/mingw64/bin`; its bash is `<root>/bin/bash.exe`.
pub(super) fn bash_beside(git: &Path) -> Option<PathBuf> {
    git.ancestors()
        .skip(1)
        .take(3)
        .map(|directory| directory.join("bin").join("bash.exe"))
        .find(|candidate| candidate.is_file())
}

/// What the model must know about the shell, which differs most on Windows: Git's bash there is still Unix bash.
pub(super) fn shell_note(shell: &Shell, windows: bool) -> &'static str {
    match shell {
        Shell::Bash(_) if windows => concat!(
            "Git Bash on Windows. It is Unix bash, not cmd: discard output with `/dev/null`, never `NUL`; ",
            "change directory with `cd`, never `cd /d`; write paths as `C:/dir/file` or `/c/dir/file`"
        ),
        Shell::Bash(_) => "bash",
        Shell::PowerShell(path) if windows_powershell(path) => {
            "Windows PowerShell 5.1 (powershell.exe, not PowerShell 7); use PowerShell 5.1 syntax, not bash"
        }
        Shell::PowerShell(_) => "PowerShell 7 (pwsh); use PowerShell syntax, not bash",
    }
}

/// How dependent steps are chained: Windows PowerShell 5.1 has no `&&`.
pub(super) fn chain_note(shell: &Shell) -> &'static str {
    match shell {
        Shell::PowerShell(path) if windows_powershell(path) => concat!(
            "separate dependent steps with `;` and stop on failure yourself ",
            "(`if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }`), since `&&` does not exist in this shell"
        ),
        _ => "chain dependent steps with `&&`",
    }
}

/// `powershell.exe` is Windows PowerShell 5.1; PowerShell 7 is `pwsh`.
fn windows_powershell(path: &Path) -> bool {
    path.file_stem()
        .is_some_and(|stem| stem.eq_ignore_ascii_case("powershell"))
}
