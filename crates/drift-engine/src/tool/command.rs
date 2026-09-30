//! Reading a shell command line well enough to decide on it: the simple commands it runs, or a
//! refusal to guess when it uses constructs whose effect cannot be read off the text.

/// Which shell will run the line; quoting and operators differ.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Dialect {
    Bash,
    PowerShell,
}

/// Programs whose first arguments name what they do (`git push`, `npm run build`), so approving one
/// such command can extend to the same subcommand with other arguments. Everything else is approved
/// only exactly as written.
const SUBCOMMANDS: [(&str, usize); 30] = [
    ("git", 2), ("cargo", 2), ("rustup", 2), ("go", 2), ("dotnet", 2), ("deno", 2), ("make", 2),
    ("npm", 2), ("pnpm", 2), ("yarn", 2), ("bun", 2), ("npx", 2), ("pip", 2), ("uv", 2),
    ("poetry", 2), ("docker", 2), ("kubectl", 2), ("terraform", 2), ("mvn", 2), ("gradle", 2),
    ("npm run", 3), ("pnpm run", 3), ("yarn run", 3), ("bun run", 3), ("uv run", 3),
    ("docker compose", 3), ("gh", 3), ("az", 3), ("aws", 3), ("gcloud", 3),
];

/// Words that hand the rest of the line to another interpreter or program, hiding what really runs.
const LAUNCHERS: [&str; 22] = [
    "eval", "exec", "source", ".", "bash", "sh", "zsh", "fish", "pwsh", "powershell", "cmd", "xargs", "env",
    "sudo", "nohup", "time", "timeout", "nice", "iex", "invoke-expression", "start-process", "invoke-command",
];

/// The simple commands in `line`, each normalised to its words joined by single spaces, or `None`
/// when the line uses substitution, subshells, script blocks or a launcher, which only an exact
/// approval of the whole line may allow.
pub fn split(dialect: Dialect, line: &str) -> Option<Vec<String>> {
    let segments = tokenize(dialect, line)?;
    let commands: Vec<String> = segments.into_iter().filter(|words| !words.is_empty()).map(|words| words.join(" ")).collect();
    let hides_program = commands.iter().any(|command| {
        let first = command.split(' ').next().unwrap_or_default().to_ascii_lowercase();
        let program = first.rsplit(['/', '\\']).next().unwrap_or(&first).trim_end_matches(".exe");
        LAUNCHERS.contains(&program) || first.starts_with('&')
    });
    (!commands.is_empty() && !hides_program).then_some(commands)
}

/// The subcommand an approval of `command` may extend to (`cargo test` for `cargo test --release`),
/// for the programs above; `None` means approve exactly `command` and nothing else.
pub fn subcommand(command: &str) -> Option<String> {
    let words: Vec<&str> = command.split(' ').collect();
    let arity = SUBCOMMANDS
        .iter()
        .filter(|(prefix, arity)| words.len() >= *arity && words[..prefix.split(' ').count()].join(" ") == *prefix)
        .map(|(_, arity)| *arity)
        .max()?;
    // A flag where the subcommand belongs (`git -C elsewhere push`) says too little to widen on.
    words[1..arity].iter().all(|word| !word.starts_with('-')).then(|| words[..arity].join(" "))
}

/// Splits into simple commands of unquoted words; `None` for anything it will not guess about.
fn tokenize(dialect: Dialect, line: &str) -> Option<Vec<Vec<String>>> {
    let mut segments = vec![Vec::new()];
    let mut word = String::new();
    let mut in_word = false;
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\'' => {
                in_word = true;
                single_quoted(dialect, &mut chars, &mut word)?;
            }
            '"' => {
                in_word = true;
                double_quoted(dialect, &mut chars, &mut word)?;
            }
            '\\' if dialect == Dialect::Bash => {
                in_word = true;
                word.push(chars.next()?);
            }
            '`' if dialect == Dialect::PowerShell => {
                in_word = true;
                word.push(chars.next()?);
            }
            // Substitution, backticks, subshells, groups and script blocks run code the text does not show.
            '`' | '(' | ')' | '{' | '}' => return None,
            '$' | '@' | '<' | '>' if chars.peek() == Some(&'(') => return None,
            // `2>&1` and `&>` are redirections, not a separator.
            '&' if word.ends_with('>') || chars.peek() == Some(&'>') => {
                in_word = true;
                word.push(c);
            }
            ';' | '|' | '&' | '\n' | '\r' => {
                if c == '&' && dialect == Dialect::PowerShell && !in_word && segments.last().is_some_and(Vec::is_empty) {
                    return None;
                }
                flush(&mut segments, &mut word, &mut in_word);
                if matches!(chars.peek(), Some('&' | '|')) && c != ';' {
                    chars.next();
                }
                segments.push(Vec::new());
            }
            c if c.is_whitespace() => flush(&mut segments, &mut word, &mut in_word),
            c => {
                in_word = true;
                word.push(c);
            }
        }
    }
    flush(&mut segments, &mut word, &mut in_word);
    Some(segments)
}

fn flush(segments: &mut [Vec<String>], word: &mut String, in_word: &mut bool) {
    if *in_word {
        segments.last_mut().unwrap().push(std::mem::take(word));
        *in_word = false;
    }
}

/// Bash single quotes are literal; PowerShell doubles a quote to escape it.
fn single_quoted(dialect: Dialect, chars: &mut std::iter::Peekable<std::str::Chars>, word: &mut String) -> Option<()> {
    loop {
        match chars.next()? {
            '\'' if dialect == Dialect::PowerShell && chars.peek() == Some(&'\'') => {
                chars.next();
                word.push('\'');
            }
            '\'' => return Some(()),
            c => word.push(c),
        }
    }
}

/// Double quotes still expand `$(...)` and backticks in bash and subexpressions in PowerShell.
fn double_quoted(dialect: Dialect, chars: &mut std::iter::Peekable<std::str::Chars>, word: &mut String) -> Option<()> {
    let escape = if dialect == Dialect::Bash { '\\' } else { '`' };
    loop {
        match chars.next()? {
            '"' => return Some(()),
            c if c == escape => word.push(chars.next()?),
            '`' => return None,
            '$' if chars.peek() == Some(&'(') => return None,
            c => word.push(c),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bash(line: &str) -> Option<Vec<String>> {
        split(Dialect::Bash, line)
    }

    fn pwsh(line: &str) -> Option<Vec<String>> {
        split(Dialect::PowerShell, line)
    }

    #[test]
    fn compound_lines_are_split_into_each_command() {
        assert_eq!(bash("cargo test && rm -rf target").unwrap(), ["cargo test", "rm -rf target"]);
        assert_eq!(bash("git status; git log | head -5").unwrap(), ["git status", "git log", "head -5"]);
        assert_eq!(bash("a || b &\nc").unwrap(), ["a", "b", "c"]);
        assert_eq!(bash("cargo build 2>&1 | tail -3").unwrap(), ["cargo build 2>&1", "tail -3"]);
        assert_eq!(pwsh("dotnet test 2>&1").unwrap(), ["dotnet test 2>&1"]);
        assert_eq!(pwsh("Get-ChildItem; Remove-Item x && echo done").unwrap(), ["Get-ChildItem", "Remove-Item x", "echo done"]);
    }

    #[test]
    fn quotes_keep_operators_inside_one_word() {
        assert_eq!(bash(r#"git commit -m "fix; rm -rf / && more""#).unwrap(), ["git commit -m fix; rm -rf / && more"]);
        assert_eq!(bash(r"echo 'a | b' c\ d").unwrap(), ["echo a | b c d"]);
        assert_eq!(pwsh("Write-Host 'it''s; fine'").unwrap(), ["Write-Host it's; fine"]);
    }

    #[test]
    fn constructs_that_hide_what_runs_are_not_guessed_at() {
        for line in ["echo $(rm -rf ~)", "echo `whoami`", "(cd x && make)", "{ a; b; }", "diff <(a) <(b)", r#"echo "$(id)""#, "eval \"$CMD\"", "bash -c 'rm x'", "xargs rm < list", "sudo apt install x", "/usr/bin/env python x.py"] {
            assert_eq!(bash(line), None, "{line}");
        }
        for line in ["iex (irm x)", "& $cmd", "& { rm x }", "Invoke-Expression $s", "pwsh -Command rm x", "echo $(Get-Date)", "Start-Process cmd"] {
            assert_eq!(pwsh(line), None, "{line}");
        }
    }

    #[test]
    fn approvals_widen_only_to_a_known_subcommand() {
        assert_eq!(subcommand("cargo test --release").as_deref(), Some("cargo test"));
        assert_eq!(subcommand("npm run build").as_deref(), Some("npm run build"));
        assert_eq!(subcommand("gh pr view 12").as_deref(), Some("gh pr view"));
        assert_eq!(subcommand("git -C elsewhere push"), None, "a flag says too little");
        assert_eq!(subcommand("cargo"), None);
        assert_eq!(subcommand("./deploy.sh prod"), None);
        assert_eq!(subcommand("Remove-Item build -Recurse"), None);
    }
}
