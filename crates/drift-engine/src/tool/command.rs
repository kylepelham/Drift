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

/// Redirection targets that discard or pass output on rather than write a file. Git Bash has no `nul`
/// device: `> nul` there creates a file.
const BASH_SINKS: [&str; 5] = ["/dev/null", "/dev/stdout", "/dev/stderr", "/dev/fd/1", "/dev/fd/2"];
const POWERSHELL_SINKS: [&str; 1] = ["$null"];

/// A shell line read for permission: the simple commands it runs, each normalised to its words
/// joined by single spaces with its redirections last, and the files its redirections write.
#[derive(Debug, PartialEq)]
pub struct Line {
    pub commands: Vec<String>,
    pub writes: Vec<String>,
}

/// `None` when the line uses substitution, subshells, script blocks or a launcher, which only an
/// exact approval of the whole line may allow.
pub fn split(dialect: Dialect, line: &str) -> Option<Line> {
    let segments = Tokenizer::new(dialect).run(line)?;
    let segments: Vec<Segment> = segments.into_iter().filter(|s| !s.words.is_empty() || !s.redirects.is_empty()).collect();
    let hides_program = segments.iter().any(|segment| {
        let first = segment.words.first().map(|w| w.to_ascii_lowercase()).unwrap_or_default();
        let program = first.rsplit(['/', '\\']).next().unwrap_or(&first).trim_end_matches(".exe");
        LAUNCHERS.contains(&program) || first.starts_with('&')
    });
    if segments.is_empty() || hides_program {
        return None;
    }
    let writes = segments.iter().flat_map(|s| s.writes.iter().cloned()).collect();
    let commands = segments.into_iter().map(|s| s.words.into_iter().chain(s.redirects).collect::<Vec<_>>().join(" ")).collect();
    Some(Line { commands, writes })
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

/// One simple command: its words, its redirections as written, and the files they write.
#[derive(Default)]
struct Segment {
    words: Vec<String>,
    redirects: Vec<String>,
    writes: Vec<String>,
}

type Chars<'a> = std::iter::Peekable<std::str::Chars<'a>>;

/// Splits into simple commands of unquoted words; `None` for anything it will not guess about.
struct Tokenizer {
    dialect: Dialect,
    segments: Vec<Segment>,
    word: String,
    in_word: bool,
    /// Part of the current word came from quotes or an escape, so it cannot be a file descriptor.
    quoted: bool,
    /// A redirection waiting for its target word, as the operator written before it.
    pending: Option<String>,
}

impl Tokenizer {
    fn new(dialect: Dialect) -> Self {
        Self { dialect, segments: vec![Segment::default()], word: String::new(), in_word: false, quoted: false, pending: None }
    }

    fn run(mut self, line: &str) -> Option<Vec<Segment>> {
        let mut chars = line.chars().peekable();
        while let Some(c) = chars.next() {
            self.step(c, &mut chars)?;
        }
        self.flush();
        self.pending.is_none().then_some(self.segments)
    }

    fn step(&mut self, c: char, chars: &mut Chars) -> Option<()> {
        let dialect = self.dialect;
        let escaped = |word: &mut String, chars: &mut Chars| {
            word.push(chars.next()?);
            Some(())
        };
        match c {
            '\'' => self.quote(|word| single_quoted(dialect, chars, word))?,
            '"' => self.quote(|word| double_quoted(dialect, chars, word))?,
            '\\' if dialect == Dialect::Bash => self.quote(|word| escaped(word, chars))?,
            '`' if dialect == Dialect::PowerShell => self.quote(|word| escaped(word, chars))?,
            // Substitution, backticks, subshells, groups and script blocks run code the text does not show.
            '`' | '(' | ')' | '{' | '}' => return None,
            '$' | '@' | '<' | '>' if chars.peek() == Some(&'(') => return None,
            '>' => self.redirect(String::new(), chars)?,
            '&' if self.dialect == Dialect::Bash && chars.peek() == Some(&'>') => {
                chars.next();
                self.redirect("&".into(), chars)?;
            }
            '<' if self.dialect == Dialect::Bash && chars.peek() == Some(&'>') => {
                chars.next();
                self.target("<>".into());
            }
            ';' | '|' | '&' | '\n' | '\r' => self.separator(c, chars)?,
            c if c.is_whitespace() => self.flush(),
            c => {
                self.in_word = true;
                self.word.push(c);
            }
        }
        Some(())
    }

    fn quote(&mut self, read: impl FnOnce(&mut String) -> Option<()>) -> Option<()> {
        self.in_word = true;
        self.quoted = true;
        read(&mut self.word)
    }

    /// After `>`: a descriptor written right before it (`2>`, PowerShell `*>`) belongs to it; `>>`
    /// and `>|` still write; `>&2` duplicates a stream, while bash `>&file` writes the file.
    fn redirect(&mut self, prefix: String, chars: &mut Chars) -> Option<()> {
        let descriptor = prefix.is_empty() && self.in_word && !self.quoted && (self.word.chars().all(|c| c.is_ascii_digit()) || self.word == "*");
        let mut op = if descriptor {
            self.in_word = false;
            std::mem::take(&mut self.word)
        } else {
            self.flush();
            prefix
        };
        op.push('>');
        if let Some(next @ ('>' | '|')) = chars.peek().copied() {
            chars.next();
            op.push(next);
        }
        if chars.peek() == Some(&'&') {
            chars.next();
            let stream: String = std::iter::from_fn(|| chars.next_if(|c| c.is_ascii_digit() || *c == '-')).collect();
            if stream.is_empty() {
                self.target(format!("&{op}"));
            } else {
                self.segment().redirects.push(format!("{op}&{stream}"));
            }
            return Some(());
        }
        self.target(op);
        Some(())
    }

    /// The next word is where `op` writes.
    fn target(&mut self, op: String) {
        self.flush();
        self.pending = Some(op);
    }

    fn separator(&mut self, c: char, chars: &mut Chars) -> Option<()> {
        if c == '&' && self.dialect == Dialect::PowerShell && !self.in_word && self.segment().words.is_empty() {
            return None;
        }
        self.flush();
        if self.pending.is_some() {
            return None;
        }
        if matches!(chars.peek(), Some('&' | '|')) && c != ';' {
            chars.next();
        }
        self.segments.push(Segment::default());
        Some(())
    }

    fn flush(&mut self) {
        if !self.in_word {
            return;
        }
        let word = std::mem::take(&mut self.word);
        self.in_word = false;
        self.quoted = false;
        match self.pending.take() {
            Some(op) => {
                let sinks: &[&str] = if self.dialect == Dialect::Bash { &BASH_SINKS } else { &POWERSHELL_SINKS };
                if !sinks.iter().any(|sink| sink.eq_ignore_ascii_case(&word)) {
                    self.segment().writes.push(word.clone());
                }
                self.segment().redirects.push(format!("{op}{word}"));
            }
            None => self.segment().words.push(word),
        }
    }

    fn segment(&mut self) -> &mut Segment {
        self.segments.last_mut().unwrap()
    }
}

/// Bash single quotes are literal; PowerShell doubles a quote to escape it.
fn single_quoted(dialect: Dialect, chars: &mut Chars, word: &mut String) -> Option<()> {
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
fn double_quoted(dialect: Dialect, chars: &mut Chars, word: &mut String) -> Option<()> {
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
        split(Dialect::Bash, line).map(|line| line.commands)
    }

    fn pwsh(line: &str) -> Option<Vec<String>> {
        split(Dialect::PowerShell, line).map(|line| line.commands)
    }

    fn writes(dialect: Dialect, line: &str) -> Vec<String> {
        split(dialect, line).unwrap_or_else(|| panic!("{line} should split")).writes
    }

    #[test]
    fn redirections_that_write_files_are_found_in_both_shells() {
        for (line, target) in [
            ("git status > victim.txt", "victim.txt"),
            ("git status >> log.txt", "log.txt"),
            ("git status >| forced.txt", "forced.txt"),
            ("make 2> errors.txt", "errors.txt"),
            ("make 2>>errors.txt", "errors.txt"),
            ("make &> all.txt", "all.txt"),
            ("make &>> all.txt", "all.txt"),
            ("make >& both.txt", "both.txt"),
            (">first.txt echo hi", "first.txt"),
            ("echo hi>glued.txt", "glued.txt"),
            ("echo x > 'quoted name.txt'", "quoted name.txt"),
            ("cat <> rw.txt", "rw.txt"),
            ("echo hi > nul", "nul"),
        ] {
            assert_eq!(writes(Dialect::Bash, line), [target], "{line}");
        }
        for (line, target) in [("git status > victim.txt", "victim.txt"), ("dir >> list.txt", "list.txt"), ("build 2> err.txt", "err.txt"), ("build *> all.txt", "all.txt"), ("build *>> all.txt", "all.txt")] {
            assert_eq!(writes(Dialect::PowerShell, line), [target], "{line}");
        }
        assert_eq!(bash("git status > victim.txt").unwrap(), ["git status >victim.txt"]);
        assert_eq!(bash(">first.txt echo hi").unwrap(), ["echo hi >first.txt"], "the program stays first");
    }

    #[test]
    fn stream_sinks_and_quoted_operators_write_nothing() {
        for line in ["cargo build 2>&1 | tail -3", "make >/dev/null 2>&1", "make 2> /dev/null", "echo err >&2", "exec 3>&-", "echo 'a > b'", r#"echo "a >> b""#, r"echo a\>b", "echo \"2\"", "sort < input.txt"] {
            let found = split(Dialect::Bash, line);
            assert!(found.as_ref().is_none_or(|l| l.writes.is_empty()), "{line}: {found:?}");
        }
        for line in ["dotnet test 2>&1", "build > $null", "build *> $NULL", "Write-Host 'a > b'", "echo a`>b"] {
            assert_eq!(writes(Dialect::PowerShell, line), Vec::<String>::new(), "{line}");
        }
        assert_eq!(bash("echo '2'>x").unwrap(), ["echo 2 >x"], "a quoted 2 is an argument, not a descriptor");
        assert_eq!(bash("echo hi >"), None, "a redirection with no target is not guessed at");
    }

    #[test]
    fn compound_lines_are_split_into_each_command() {
        assert_eq!(bash("cargo test && rm -rf target").unwrap(), ["cargo test", "rm -rf target"]);
        assert_eq!(bash("git status; git log | head -5").unwrap(), ["git status", "git log", "head -5"]);
        assert_eq!(bash("a || b &\nc").unwrap(), ["a", "b", "c"]);
        assert_eq!(bash("cargo build 2>&1 | tail -3").unwrap(), ["cargo build 2>&1", "tail -3"]);
        assert_eq!(bash("a |& b").unwrap(), ["a", "b"]);
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
