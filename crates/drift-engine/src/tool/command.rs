//! Reading a shell command line well enough to decide on it: the simple commands it runs, or a
//! refusal to guess when it uses constructs whose effect cannot be read off the text.

#[cfg(test)]
mod tests;
mod tokenizer;

use tokenizer::Tokenizer;

/// Programs whose first arguments name what they do (`git push`, `npm run build`), so approving one
/// such command can extend to the same subcommand with other arguments. Everything else is approved
/// only exactly as written.
const SUBCOMMANDS: [(&str, usize); 27] = [
    ("git", 2),
    ("cargo", 2),
    ("rustup", 2),
    ("go", 2),
    ("dotnet", 2),
    ("deno", 2),
    ("make", 2),
    ("npm", 2),
    ("pnpm", 2),
    ("yarn", 2),
    ("bun", 2),
    ("pip", 2),
    ("uv", 2),
    ("poetry", 2),
    ("docker", 2),
    ("kubectl", 2),
    ("terraform", 2),
    ("mvn", 2),
    ("gradle", 2),
    ("npm run", 3),
    ("pnpm run", 3),
    ("yarn run", 3),
    ("docker compose", 3),
    ("gh", 3),
    ("az", 3),
    ("aws", 3),
    ("gcloud", 3),
];
/// Subcommands that run arbitrary code or fetch and install it (`cargo run`, `uv run`, `docker run`,
/// `npm install`, `pip install`). The same subcommand with other arguments is a different program or
/// package, so approving one never widens: each is approved exactly as written.
const NEVER_WIDEN: [&str; 11] = [
    "run", "exec", "x", "dlx", "install", "i", "add", "ci", "get", "update", "upgrade",
];
/// Runners whose next word is a script the project itself defines, so approving it names the code.
const SCRIPT_RUNNERS: [&str; 3] = ["npm run", "pnpm run", "yarn run"];
/// PowerShell's built-in aliases for cmdlets a rule is likely to name, so `rm x` meets a rule for
/// `Remove-Item *`.
const POWERSHELL_ALIASES: [(&str, &str); 31] = [
    ("ls", "Get-ChildItem"),
    ("dir", "Get-ChildItem"),
    ("gci", "Get-ChildItem"),
    ("rm", "Remove-Item"),
    ("del", "Remove-Item"),
    ("erase", "Remove-Item"),
    ("ri", "Remove-Item"),
    ("rmdir", "Remove-Item"),
    ("rd", "Remove-Item"),
    ("cp", "Copy-Item"),
    ("copy", "Copy-Item"),
    ("cpi", "Copy-Item"),
    ("mv", "Move-Item"),
    ("move", "Move-Item"),
    ("mi", "Move-Item"),
    ("cat", "Get-Content"),
    ("gc", "Get-Content"),
    ("type", "Get-Content"),
    ("sls", "Select-String"),
    ("sc", "Set-Content"),
    ("ac", "Add-Content"),
    ("ni", "New-Item"),
    ("iwr", "Invoke-WebRequest"),
    ("curl", "Invoke-WebRequest"),
    ("wget", "Invoke-WebRequest"),
    ("irm", "Invoke-RestMethod"),
    ("kill", "Stop-Process"),
    ("spps", "Stop-Process"),
    ("start", "Start-Process"),
    ("saps", "Start-Process"),
    ("icm", "Invoke-Command"),
];
/// Words that hand the rest of the line to another interpreter or program, hiding what really runs.
const LAUNCHERS: [&str; 22] = [
    "eval",
    "exec",
    "source",
    ".",
    "bash",
    "sh",
    "zsh",
    "fish",
    "pwsh",
    "powershell",
    "cmd",
    "xargs",
    "env",
    "sudo",
    "nohup",
    "time",
    "timeout",
    "nice",
    "iex",
    "invoke-expression",
    "start-process",
    "invoke-command",
];
/// Redirection targets that discard or pass output on rather than write a file. Git Bash has no `nul`
/// device: `> nul` there creates a file.
const BASH_SINKS: [&str; 5] = ["/dev/null", "/dev/stdout", "/dev/stderr", "/dev/fd/1", "/dev/fd/2"];
const POWERSHELL_SINKS: [&str; 1] = ["$null"];
/// Programs that only read, whatever their arguments; `cd` and friends only move.
const READERS: [&str; 34] = [
    "ls",
    "dir",
    "cat",
    "type",
    "head",
    "tail",
    "wc",
    "pwd",
    "echo",
    "printf",
    "grep",
    "rg",
    "which",
    "where",
    "whoami",
    "date",
    "file",
    "stat",
    "du",
    "df",
    "tree",
    "uname",
    "hostname",
    "cd",
    "pushd",
    "popd",
    "get-childitem",
    "get-content",
    "get-location",
    "select-string",
    "test-path",
    "get-item",
    "write-output",
    "set-location",
];
/// Git subcommands that only read.
const GIT_READERS: [&str; 9] = [
    "status",
    "diff",
    "log",
    "show",
    "rev-parse",
    "ls-files",
    "blame",
    "describe",
    "grep",
];
/// Arguments that make an otherwise reading program change or write files, or run another program:
/// `find`'s actions and file outputs (`-fprint`, `-fprint0`, `-fprintf`, `-fls`), `tree -o`, `rg --pre`.
const WRITING_ARGS: [(&str, &[&str]); 3] = [
    (
        "find",
        &["-delete", "-exec", "-execdir", "-ok", "-okdir", "-fprint", "-fls"],
    ),
    ("tree", &["-o"]),
    ("rg", &["--pre"]),
];
/// Programs that print a file they are given, so a run that succeeded has shown the model that file.
const PRINTERS: [&str; 6] = ["cat", "type", "get-content", "head", "tail", "sed"];
/// Commands that change the directory later words are read against.
const MOVES: [&str; 7] = [
    "cd",
    "chdir",
    "pushd",
    "popd",
    "set-location",
    "push-location",
    "pop-location",
];

/// Which shell will run the line; quoting and operators differ.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Dialect {
    Bash,
    PowerShell,
}

/// A shell line read for permission: the simple commands it runs, each normalised to its words
/// joined by single spaces with its redirections last, and the files its redirections write.
/// `canonical` holds each command as deny rules see it: leading `NAME=value` assignments dropped and
/// PowerShell aliases spelt as their cmdlets. Approvals and allow rules see `commands` as written.
#[derive(Debug, PartialEq)]
pub struct Line {
    pub commands: Vec<String>,
    pub canonical: Vec<String>,
    pub writes: Vec<String>,
}

/// Whether a redirection target discards or passes on output rather than writing a file, in either shell.
pub fn is_sink(target: &str) -> bool {
    BASH_SINKS
        .iter()
        .chain(&POWERSHELL_SINKS)
        .any(|sink| sink.eq_ignore_ascii_case(target))
}

/// The file a redirection word of a split command names (`>~/.bashrc`, `2>>log`, `<>rw`, `<in`), or
/// `None` for an ordinary word; a stream duplication (`2>&1`) names an empty one.
pub fn redirect_target(word: &str) -> Option<&str> {
    let rest =
        word.trim_start_matches(|character: char| character.is_ascii_digit() || character == '*' || character == '&');
    let rest = rest.strip_prefix('<').or_else(|| rest.strip_prefix('>'))?;
    let target = rest.trim_start_matches(['>', '|']);

    Some(match target.strip_prefix('&') {
        Some(stream)
            if stream
                .chars()
                .all(|character| character.is_ascii_digit() || character == '-') =>
        {
            ""
        }
        Some(file) => file,
        None => target,
    })
}

/// `None` when the line uses substitution, subshells, script blocks or a launcher, which only an
/// exact approval of the whole line may allow.
pub fn split(dialect: Dialect, line: &str) -> Option<Line> {
    let segments = Tokenizer::new(dialect).run(line)?;
    let segments: Vec<_> = segments
        .into_iter()
        .filter(|segment| !segment.words.is_empty() || !segment.redirects.is_empty())
        .collect();
    let canonical: Vec<Vec<String>> = segments
        .iter()
        .map(|segment| canonical_words(dialect, &segment.words))
        .collect();
    let hides_program = canonical.iter().any(|words| {
        let first = words.first().map(|word| word.to_ascii_lowercase()).unwrap_or_default();
        let program = first
            .rsplit(['/', '\\'])
            .next()
            .unwrap_or(&first)
            .trim_end_matches(".exe");
        LAUNCHERS.contains(&program) || first.starts_with('&')
    });
    if segments.is_empty() || hides_program {
        return None;
    }

    let writes = segments
        .iter()
        .flat_map(|segment| segment.writes.iter().cloned())
        .collect();
    let canonical = canonical
        .into_iter()
        .zip(&segments)
        .map(|(words, segment)| {
            words
                .into_iter()
                .chain(segment.redirects.iter().cloned())
                .collect::<Vec<_>>()
                .join(" ")
        })
        .collect();
    let commands = segments
        .into_iter()
        .map(|segment| {
            segment
                .words
                .into_iter()
                .chain(segment.redirects)
                .collect::<Vec<_>>()
                .join(" ")
        })
        .collect();

    Some(Line {
        commands,
        canonical,
        writes,
    })
}

/// True only for a line that can be read, writes nothing by redirection, and runs nothing but
/// known readers; unclear lines count as writing.
pub fn reads_only(dialect: Dialect, line: &str) -> bool {
    let Some(read) = split(dialect, line) else { return false };

    read.writes.is_empty() && read.canonical.iter().all(|command| reader(command))
}

fn reader(command: &str) -> bool {
    let words: Vec<String> = command.split_whitespace().map(str::to_ascii_lowercase).collect();
    let Some(first) = words.first() else { return true };
    let program = first
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(first)
        .trim_end_matches(".exe");
    let writing = WRITING_ARGS
        .iter()
        .find(|(name, _)| *name == program)
        .map_or(&[][..], |(_, arguments)| *arguments);
    // Prefix matching also catches joined flag values such as --pre=cmd and -ofile.
    if words
        .iter()
        .skip(1)
        .any(|word| writing.iter().any(|argument| word.starts_with(argument)))
    {
        return false;
    }

    match program {
        "git" => {
            words
                .get(1)
                .is_some_and(|subcommand| GIT_READERS.contains(&subcommand.as_str()))
                && !words.iter().any(|word| word.starts_with("--output"))
                && !opens_pager(command)
        }
        "find" => true,
        "sed" => printed_range(&words[1..]),
        _ => READERS.contains(&program),
    }
}

/// `git grep -O<cmd>` / `--open-files-in-pager=<cmd>` runs the program it names. Read as written, since
/// `-O` and `-o` differ, short flags may be bundled (`-nOvim`) and git takes `--op` for the long form.
fn opens_pager(command: &str) -> bool {
    command.split_whitespace().skip(2).any(|word| {
        word.starts_with("--op") || (word.starts_with('-') && !word.starts_with("--") && word.contains('O'))
    })
}

/// `sed -n '<line>[,<line>]p' file...`: printing lines is all it does (no `-i`, no `w` script).
fn printed_range(arguments: &[String]) -> bool {
    let script = |word: &str| {
        let body = word.trim_matches(['\'', '"']).strip_suffix('p').unwrap_or("x");
        !body.is_empty()
            && body.split(',').all(|line| {
                !line.is_empty() && (line.chars().all(|character| character.is_ascii_digit()) || line == "$")
            })
    };

    matches!(arguments, [flag, range, ..] if flag == "-n" && script(range))
        && !arguments
            .iter()
            .any(|word| word.starts_with("-i") || word.starts_with("--in-place"))
}

/// The files a line that only reads prints (`cat a.rs`, `sed -n '1,80p' a.rs`, `Get-Content a.rs`),
/// as written, so a model that reads through the shell may then edit them. Empty for a line that
/// does anything else, or moves directory first, since its paths would no longer resolve from the
/// workspace. Words that are not files (flag values such as `-n 20`) are for the caller to drop.
pub fn files_read(dialect: Dialect, line: &str) -> Vec<String> {
    let Some(segments) = Tokenizer::new(dialect).run(line).filter(|_| reads_only(dialect, line)) else {
        return Vec::new();
    };
    let mut files = Vec::new();

    for segment in segments.iter().filter(|segment| !segment.words.is_empty()) {
        let words = canonical_words(dialect, &segment.words);
        let Some(first) = words.first().map(|word| word.to_ascii_lowercase()) else {
            continue;
        };
        let program = first
            .rsplit(['/', '\\'])
            .next()
            .unwrap_or(&first)
            .trim_end_matches(".exe")
            .to_string();
        if MOVES.contains(&program.as_str()) {
            return Vec::new();
        }
        // Piped output reached the model through the next command, not as the original file text.
        if !PRINTERS.contains(&program.as_str()) || segment.piped {
            continue;
        }

        let rest = if program == "sed" {
            &words[3.min(words.len())..]
        } else {
            &words[1..]
        };
        files.extend(rest.iter().filter(|word| !word.starts_with('-')).cloned());
    }

    files
}

/// The words as a deny rule should see them: what actually runs, not how it was spelt.
fn canonical_words(dialect: Dialect, words: &[String]) -> Vec<String> {
    let assignment = |word: &str| {
        word.split_once('=').is_some_and(|(name, _)| {
            !name.is_empty()
                && name
                    .chars()
                    .all(|character| character.is_ascii_alphanumeric() || character == '_')
                && !name.starts_with(|character: char| character.is_ascii_digit())
        })
    };
    let skip = if dialect == Dialect::Bash {
        words.iter().take_while(|word| assignment(word)).count()
    } else {
        0
    };
    let mut words = words[skip..].to_vec();

    if let (Dialect::PowerShell, Some(first)) = (dialect, words.first_mut())
        && let Some((_, cmdlet)) = POWERSHELL_ALIASES
            .iter()
            .find(|(alias, _)| alias.eq_ignore_ascii_case(first))
    {
        *first = (*cmdlet).to_string();
    }

    words
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
    let names = &words[1..arity];
    // An option in place of the subcommand does not identify the code an approval would cover.
    if names.iter().any(|word| word.starts_with('-')) {
        return None;
    }

    let runs_code =
        names.iter().any(|word| NEVER_WIDEN.contains(word)) && !SCRIPT_RUNNERS.contains(&words[..2].join(" ").as_str());
    (!runs_code).then(|| words[..arity].join(" "))
}
