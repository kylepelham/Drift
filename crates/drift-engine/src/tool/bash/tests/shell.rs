use super::super::shell::{bash_beside, shell_for};
use super::*;

#[test]
fn git_bash_is_found_beside_any_git_and_a_named_shell_is_taken_at_its_word() {
    let root = std::env::temp_dir().join(format!("drift-git-{}", crate::random_hex(4)));
    for directory in ["cmd", "bin", "mingw64/bin"] {
        std::fs::create_dir_all(root.join(directory)).unwrap();
    }
    std::fs::write(root.join("bin/bash.exe"), "").unwrap();

    for git in ["cmd/git.exe", "bin/git.exe", "mingw64/bin/git.exe"] {
        assert_eq!(
            bash_beside(&root.join(git)),
            Some(root.join("bin").join("bash.exe")),
            "{git}"
        );
    }
    assert_eq!(bash_beside(&std::env::temp_dir().join("elsewhere/git.exe")), None);
    std::fs::remove_dir_all(&root).unwrap();
    assert!(matches!(shell_for("D:/tools/Git/bin/bash.exe".into()), Shell::Bash(_)));
    assert!(matches!(shell_for("/usr/bin/zsh".into()), Shell::Bash(_)));
    assert!(matches!(
        shell_for("C:/Program Files/PowerShell/7/pwsh.exe".into()),
        Shell::PowerShell(_)
    ));
}

#[test]
fn the_model_is_told_git_bash_on_windows_is_unix_bash() {
    let bash = Shell::Bash("bash".into());
    let windows = shell_note(&bash, true);
    assert!(
        windows.contains("Unix bash")
            && windows.contains("/dev/null")
            && windows.contains("never `NUL`")
            && windows.contains("never `cd /d`"),
        "{windows}"
    );
    assert_eq!(shell_note(&bash, false), "bash");
    assert!(shell_note(&Shell::PowerShell("pwsh".into()), true).starts_with("PowerShell 7"));

    let legacy = Shell::PowerShell("C:/Windows/System32/WindowsPowerShell/v1.0/powershell.exe".into());
    assert!(shell_note(&legacy, true).starts_with("Windows PowerShell 5.1"));
    let legacy_spec = Bash::with(legacy).spec().description;
    assert!(
        legacy_spec.contains("`&&` does not exist") && !legacy_spec.contains("chain dependent steps with `&&`"),
        "{legacy_spec}"
    );
    assert!(
        Bash::with(Shell::PowerShell("pwsh".into()))
            .spec()
            .description
            .contains("chain dependent steps with `&&`")
    );
    let specification = Bash::with(bash).spec();
    assert!(
        specification.description.contains("/dev/null") == cfg!(windows),
        "{}",
        specification.description
    );
    assert!(
        !specification.description.contains('{'),
        "every placeholder is filled: {}",
        specification.description
    );
}
