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
const SUBCOMMANDS: [(&str, usize); 27] = [
    ("git", 2), ("cargo", 2), ("rustup", 2), ("go", 2), ("dotnet", 2), ("deno", 2), ("make", 2),
    ("npm", 2), ("pnpm", 2), ("yarn", 2), ("bun", 2), ("pip", 2), ("uv", 2),
    ("poetry", 2), ("docker", 2), ("kubectl", 2), ("terraform", 2), ("mvn", 2), ("gradle", 2),
    ("npm run", 3), ("pnpm run", 3), ("yarn run", 3),
    ("docker compose", 3), ("gh", 3), ("az", 3), ("aws", 3), ("gcloud", 3),
];

/// Subcommands that run arbitrary code or fetch and install it (`cargo run`, `uv run`, `docker run`,
/// `npm install`, `pip install`). The same subcommand with other arguments is a different program or
/// package, so approving one never widens: each is approved exactly as written.
const NEVER_WIDEN: [&str; 11] = ["run", "exec", "x", "dlx", "install", "i", "add", "ci", "get", "update", "upgrade"];
/// Runners whose next word is a script the project itself defines, so approving it names the code.
const SCRIPT_RUNNERS: [&str; 3] = ["npm run", "pnpm run", "yarn run"];

/// PowerShell's built-in aliases for cmdlets a rule is likely to name, so `rm x` meets a rule for
/// `Remove-Item *`.
const POWERSHELL_ALIASES: [(&str, &str); 30] = [
    ("ls", "Get-ChildItem"), ("dir", "Get-ChildItem"), ("gci", "Get-ChildItem"),
    ("rm", "Remove-Item"), ("del", "Remove-Item"), ("erase", "Remove-Item"), ("ri", "Remove-Item"), ("rmdir", "Remove-Item"), ("rd", "Remove-Item"),
    ("cp", "Copy-Item"), ("copy", "Copy-Item"), ("cpi", "Copy-Item"),
    ("mv", "Move-Item"), ("move", "Move-Item"), ("mi", "Move-Item"),
    ("cat", "Get-Content"), ("gc", "Get-Content"), ("type", "Get-Content"),
    ("sc", "Set-Content"), ("ac", "Add-Content"), ("ni", "New-Item"),
    ("iwr", "Invoke-WebRequest"), ("curl", "Invoke-WebRequest"), ("wget", "Invoke-WebRequest"), ("irm", "Invoke-RestMethod"),
    ("kill", "Stop-Process"), ("spps", "Stop-Process"), ("start", "Start-Process"), ("saps", "Start-Process"), ("icm", "Invoke-Command"),
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
/// `canonical` holds each command as deny rules see it: leading `NAME=value` assignments dropped and
/// PowerShell aliases spelt as their cmdlets. Approvals and allow rules see `commands` as written.
#[derive(Debug, PartialEq)]
pub struct Line {
    pub commands: Vec<String>,
    pub canonical: Vec<String>,
    pub writes: Vec<String>,
}

/// `None` when the line uses substitution, subshells, script blocks or a launcher, which only an
/// exact approval of the whole line may allow.
pub fn split(dialect: Dialect, line: &str) -> Option<Line> {
    let segments = Tokenizer::new(dialect).run(line)?;
    let segments: Vec<Segment> = segments.into_iter().filter(|s| !s.words.is_empty() || !s.redirects.is_empty()).collect();
    let canonical: Vec<Vec<String>> = segments.iter().map(|s| canonical_words(dialect, &s.words)).collect();
    let hides_program = canonical.iter().any(|words| {
        let first = words.first().map(|w| w.to_ascii_lowercase()).unwrap_or_default();
        let program = first.rsplit(['/', '\\']).next().unwrap_or(&first).trim_end_matches(".exe");
        LAUNCHERS.contains(&program) || first.starts_with('&')
    });
    if segments.is_empty() || hides_program {
        return None;
    }
    let writes = segments.iter().flat_map(|s| s.writes.iter().cloned()).collect();
    let canonical = canonical.into_iter().zip(&segments).map(|(words, s)| words.into_iter().chain(s.redirects.iter().cloned()).collect::<Vec<_>>().join(" ")).collect();
    let commands = segments.into_iter().map(|s| s.words.into_iter().chain(s.redirects).collect::<Vec<_>>().join(" ")).collect();
    Some(Line { commands, canonical, writes })
}

/// Programs that only read, whatever their arguments; `cd` and friends only move.
const READERS: [&str; 34] = [
    "ls", "dir", "cat", "type", "head", "tail", "wc", "pwd", "echo", "printf", "grep", "rg", "which", "where", "whoami", "date", "file",
    "stat", "du", "df", "tree", "uname", "hostname", "cd", "pushd", "popd", "get-childitem", "get-content", "get-location", "select-string",
    "test-path", "get-item", "write-output", "set-location",
];
/// Git subcommands that only read.
const GIT_READERS: [&str; 9] = ["status", "diff", "log", "show", "rev-parse", "ls-files", "blame", "describe", "grep"];
/// Arguments that make an otherwise reading program change or write files, or run another program:
/// `find`'s actions and file outputs (`-fprint`, `-fprint0`, `-fprintf`, `-fls`), `tree -o`, `rg --pre`.
const WRITING_ARGS: [(&str, &[&str]); 3] = [
    ("find", &["-delete", "-exec", "-execdir", "-ok", "-okdir", "-fprint", "-fls"]),
    ("tree", &["-o"]),
    ("rg", &["--pre"]),
];

/// True only for a line that can be read, writes nothing by redirection, and runs nothing but
/// known readers; unclear lines count as writing.
pub fn reads_only(dialect: Dialect, line: &str) -> bool {
    let Some(read) = split(dialect, line) else { return false };
    read.writes.is_empty() && read.canonical.iter().all(|command| reader(command))
}

fn reader(command: &str) -> bool {
    let words: Vec<String> = command.split_whitespace().map(str::to_ascii_lowercase).collect();
    let Some(first) = words.first() else { return true };
    let program = first.rsplit(['/', '\\']).next().unwrap_or(first).trim_end_matches(".exe");
    let writing = WRITING_ARGS.iter().find(|(name, _)| *name == program).map_or(&[][..], |(_, args)| *args);
    // A prefix catches the variants and joined forms: `-fprint0`, `-fprintf`, `--pre=cmd`, `-ofile`.
    if words.iter().skip(1).any(|word| writing.iter().any(|arg| word.starts_with(arg))) {
        return false;
    }
    match program {
        "git" => words.get(1).is_some_and(|sub| GIT_READERS.contains(&sub.as_str())) && !words.iter().any(|w| w.starts_with("--output")),
        "find" => true,
        "sed" => printed_range(&words[1..]),
        _ => READERS.contains(&program),
    }
}

/// `sed -n '<line>[,<line>]p' file...`: printing lines is all it does (no `-i`, no `w` script).
fn printed_range(args: &[String]) -> bool {
    let script = |word: &str| {
        let body = word.trim_matches(['\'', '"']).strip_suffix('p').unwrap_or("x");
        !body.is_empty() && body.split(',').all(|line| !line.is_empty() && (line.chars().all(|c| c.is_ascii_digit()) || line == "$"))
    };
    matches!(args, [flag, range, ..] if flag == "-n" && script(range)) && !args.iter().any(|word| word.starts_with("-i") || word.starts_with("--in-place"))
}

/// Programs that print a file they are given, so a run that succeeded has shown the model that file.
const PRINTERS: [&str; 6] = ["cat", "type", "get-content", "head", "tail", "sed"];
/// Commands that change the directory later words are read against.
const MOVES: [&str; 7] = ["cd", "chdir", "pushd", "popd", "set-location", "push-location", "pop-location"];

/// The files a line that only reads prints (`cat a.rs`, `sed -n '1,80p' a.rs`, `Get-Content a.rs`),
/// as written, so a model that reads through the shell may then edit them. Empty for a line that
/// does anything else, or moves directory first, since its paths would no longer resolve from the
/// workspace. Words that are not files (flag values such as `-n 20`) are for the caller to drop.
pub fn files_read(dialect: Dialect, line: &str) -> Vec<String> {
    let Some(segments) = Tokenizer::new(dialect).run(line).filter(|_| reads_only(dialect, line)) else { return Vec::new() };
    let mut files = Vec::new();
    for segment in segments.iter().filter(|s| !s.words.is_empty()) {
        let words = canonical_words(dialect, &segment.words);
        let first = words[0].to_ascii_lowercase();
        let program = first.rsplit(['/', '\\']).next().unwrap_or(&first).trim_end_matches(".exe").to_string();
        if MOVES.contains(&program.as_str()) {
            return Vec::new();
        }
        if !PRINTERS.contains(&program.as_str()) {
            continue;
        }
        // sed's first two words are `-n` and its script; anything else's flags start with `-`.
        let rest = if program == "sed" { &words[3.min(words.len())..] } else { &words[1..] };
        files.extend(rest.iter().filter(|word| !word.starts_with('-')).cloned());
    }
    files
}

/// The words as a deny rule should see them: what actually runs, not how it was spelt.
fn canonical_words(dialect: Dialect, words: &[String]) -> Vec<String> {
    let assignment = |word: &str| word.split_once('=').is_some_and(|(name, _)| !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') && !name.starts_with(|c: char| c.is_ascii_digit()));
    let skip = if dialect == Dialect::Bash { words.iter().take_while(|w| assignment(w)).count() } else { 0 };
    let mut words: Vec<String> = words[skip..].to_vec();
    if let (Dialect::PowerShell, Some(first)) = (dialect, words.first_mut()) {
        if let Some((_, cmdlet)) = POWERSHELL_ALIASES.iter().find(|(alias, _)| alias.eq_ignore_ascii_case(first)) {
            *first = (*cmdlet).to_string();
        }
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
    // A flag where the subcommand belongs (`git -C elsewhere push`) says too little to widen on.
    if names.iter().any(|word| word.starts_with('-')) {
        return None;
    }
    let runs_code = names.iter().any(|word| NEVER_WIDEN.contains(word)) && !SCRIPT_RUNNERS.contains(&words[..2].join(" ").as_str());
    (!runs_code).then(|| words[..arity].join(" "))
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
    fn runners_and_installers_never_widen() {
        for exact in ["cargo run --release", "uv run python evil.py", "bun run x.ts", "docker run alpine sh", "npm install left-pad", "pip install requests", "cargo add serde", "go get example.com/x", "npx cowsay hi", "pnpm dlx create-app", "docker compose run web sh", "gh extension install owner/ext", "npm exec thing"] {
            assert_eq!(subcommand(exact), None, "{exact}");
        }
        assert_eq!(subcommand("npm run build --watch").as_deref(), Some("npm run build"), "a project script is named, so it widens");
        assert_eq!(subcommand("cargo test --lib").as_deref(), Some("cargo test"));
    }

    #[test]
    fn deny_rules_see_past_assignments_and_aliases() {
        let line = split(Dialect::Bash, "FIXTURE=1 LANG=C git status && ./b=c").unwrap();
        assert_eq!(line.commands, ["FIXTURE=1 LANG=C git status", "./b=c"], "approvals see the line as written");
        assert_eq!(line.canonical, ["git status", "./b=c"]);
        assert_eq!(split(Dialect::Bash, "X=1 bash -c 'rm -rf /'"), None, "an assignment does not hide a launcher");
        assert_eq!(split(Dialect::Bash, "A=$(id) ls"), None);
        let ps = split(Dialect::PowerShell, "rm build -Recurse; ls; iwr https://x | Out-Null").unwrap();
        assert_eq!(ps.canonical, ["Remove-Item build -Recurse", "Get-ChildItem", "Invoke-WebRequest https://x", "Out-Null"]);
        assert_eq!(split(Dialect::PowerShell, "saps cmd"), None, "an alias for a launcher is a launcher");
        assert_eq!(split(Dialect::Bash, "rm x").unwrap().canonical, ["rm x"], "aliases are PowerShell's only");
    }

    #[test]
    fn only_lines_of_known_readers_count_as_reading() {
        for line in ["git status", "git diff --stat && git log --oneline -5", "ls -la src | wc -l", "rg TODO src 2>/dev/null", "cd crates && cat Cargo.toml", "find . -name '*.rs'", "echo done"] {
            assert!(reads_only(Dialect::Bash, line), "{line}");
        }
        for line in ["git commit -m x", "git status > out.txt", "cargo test", "find . -name x -delete", "rm a", "ls && touch b", "echo $(rm x)", "git diff --output=patch", "sed -i s/a/b/ f", "find . -fprint0 out", "find . -fprintf out %p", "find . -fls out", "tree -o out.txt", "rg --pre ./script x", "rg --pre=./script x"] {
            assert!(!reads_only(Dialect::Bash, line), "{line}");
        }
        assert!(reads_only(Dialect::PowerShell, "Get-ChildItem src; Select-String -Path a.txt -Pattern x"));
        assert!(reads_only(Dialect::PowerShell, "ls; cat a.txt"), "aliases read as their cmdlets");
        assert!(!reads_only(Dialect::PowerShell, "Set-Content a.txt x"));
    }

    #[test]
    fn a_line_that_prints_files_names_them_and_anything_else_names_none() {
        assert_eq!(files_read(Dialect::Bash, "cat src/a.rs"), ["src/a.rs"]);
        assert_eq!(files_read(Dialect::Bash, "sed -n '1,80p' src/a.rs"), ["src/a.rs"]);
        assert_eq!(files_read(Dialect::Bash, "head -n 20 a.rs && tail b.rs"), ["20", "a.rs", "b.rs"], "flag values are dropped by the caller, which keeps only files");
        assert_eq!(files_read(Dialect::Bash, "cat \"my file.txt\" | grep x"), ["my file.txt"]);
        assert_eq!(files_read(Dialect::PowerShell, "Get-Content -Path a.rs; gc b.rs"), ["a.rs", "b.rs"]);
        assert!(files_read(Dialect::Bash, "cd src && cat a.rs").is_empty(), "paths after a move do not resolve from the workspace");
        assert!(files_read(Dialect::Bash, "sed -i 's/a/b/' a.rs").is_empty() && !reads_only(Dialect::Bash, "sed -i 's/a/b/' a.rs"));
        assert!(files_read(Dialect::Bash, "sed -n 'w out' a.rs").is_empty(), "a sed script that writes is not a read");
        assert!(files_read(Dialect::Bash, "cat a.rs > b.rs").is_empty(), "a line that writes is not a read");
        assert!(files_read(Dialect::Bash, "grep fn a.rs").is_empty(), "matches are not the file");
        assert!(reads_only(Dialect::Bash, "sed -n 1,200p a.rs"));
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
