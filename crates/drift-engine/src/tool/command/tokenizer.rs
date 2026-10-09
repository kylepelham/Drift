use super::{BASH_SINKS, Dialect, POWERSHELL_SINKS};

type Chars<'a> = std::iter::Peekable<std::str::Chars<'a>>;

/// One simple command: its words, its redirections as written, and the files they write.
#[derive(Default)]
pub(super) struct Segment {
    pub(super) words: Vec<String>,
    pub(super) redirects: Vec<String>,
    pub(super) writes: Vec<String>,
    /// Its output feeds the next command (`a | b`), so what it printed was never shown as it was.
    pub(super) piped: bool,
}

/// Splits into simple commands of unquoted words; `None` for anything it will not guess about.
pub(super) struct Tokenizer {
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
    pub(super) fn new(dialect: Dialect) -> Self {
        Self {
            dialect,
            segments: vec![Segment::default()],
            word: String::new(),
            in_word: false,
            quoted: false,
            pending: None,
        }
    }

    pub(super) fn run(mut self, line: &str) -> Option<Vec<Segment>> {
        let mut characters = line.chars().peekable();
        while let Some(character) = characters.next() {
            self.step(character, &mut characters)?;
        }

        self.flush();
        self.pending.is_none().then_some(self.segments)
    }

    fn step(&mut self, character: char, characters: &mut Chars) -> Option<()> {
        let dialect = self.dialect;
        let escaped = |word: &mut String, characters: &mut Chars| {
            word.push(characters.next()?);
            Some(())
        };

        match character {
            '\'' => self.quote(|word| single_quoted(dialect, characters, word))?,
            '"' => self.quote(|word| double_quoted(dialect, characters, word))?,
            '\\' if dialect == Dialect::Bash => self.quote(|word| escaped(word, characters))?,
            '`' if dialect == Dialect::PowerShell => self.quote(|word| escaped(word, characters))?,
            // These constructs can run code that simple word tokenization cannot identify.
            '`' | '(' | ')' | '{' | '}' => return None,
            '$' | '@' | '<' | '>' if characters.peek() == Some(&'(') => return None,
            '>' => self.redirect(String::new(), characters)?,
            '&' if self.dialect == Dialect::Bash && characters.peek() == Some(&'>') => {
                characters.next();
                self.redirect("&".into(), characters)?;
            }
            '<' if self.dialect == Dialect::Bash && characters.peek() == Some(&'>') => {
                characters.next();
                self.target("<>".into());
            }
            ';' | '|' | '&' | '\n' | '\r' => self.separator(character, characters)?,
            character if character.is_whitespace() => self.flush(),
            character => {
                self.in_word = true;
                self.word.push(character);
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
    fn redirect(&mut self, prefix: String, characters: &mut Chars) -> Option<()> {
        let descriptor = prefix.is_empty()
            && self.in_word
            && !self.quoted
            && (self.word.chars().all(|character| character.is_ascii_digit()) || self.word == "*");
        let mut operator = if descriptor {
            self.in_word = false;
            std::mem::take(&mut self.word)
        } else {
            self.flush();
            prefix
        };
        operator.push('>');
        if let Some(next @ ('>' | '|')) = characters.peek().copied() {
            characters.next();
            operator.push(next);
        }

        if characters.peek() == Some(&'&') {
            characters.next();
            let stream: String =
                std::iter::from_fn(|| characters.next_if(|character| character.is_ascii_digit() || *character == '-'))
                    .collect();
            if stream.is_empty() {
                self.target(format!("&{operator}"));
            } else {
                self.segment().redirects.push(format!("{operator}&{stream}"));
            }
            return Some(());
        }

        self.target(operator);
        Some(())
    }

    /// The next word is where `op` writes.
    fn target(&mut self, operator: String) {
        self.flush();
        self.pending = Some(operator);
    }

    fn separator(&mut self, character: char, characters: &mut Chars) -> Option<()> {
        if character == '&' && self.dialect == Dialect::PowerShell && !self.in_word && self.segment().words.is_empty() {
            return None;
        }
        self.flush();
        if self.pending.is_some() {
            return None;
        }

        let doubled = matches!(characters.peek(), Some('&' | '|')) && character != ';';
        if doubled {
            characters.next();
        }
        self.segment().piped = character == '|' && !doubled;
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
            Some(operator) => {
                let sinks: &[&str] = if self.dialect == Dialect::Bash {
                    &BASH_SINKS
                } else {
                    &POWERSHELL_SINKS
                };
                if !sinks.iter().any(|sink| sink.eq_ignore_ascii_case(&word)) {
                    self.segment().writes.push(word.clone());
                }
                self.segment().redirects.push(format!("{operator}{word}"));
            }
            None => self.segment().words.push(word),
        }
    }

    fn segment(&mut self) -> &mut Segment {
        self.segments.last_mut().unwrap()
    }
}

/// Bash single quotes are literal; PowerShell doubles a quote to escape it.
fn single_quoted(dialect: Dialect, characters: &mut Chars, word: &mut String) -> Option<()> {
    loop {
        match characters.next()? {
            '\'' if dialect == Dialect::PowerShell && characters.peek() == Some(&'\'') => {
                characters.next();
                word.push('\'');
            }
            '\'' => return Some(()),
            character => word.push(character),
        }
    }
}

/// Double quotes still expand `$(...)` and backticks in bash and subexpressions in PowerShell.
fn double_quoted(dialect: Dialect, characters: &mut Chars, word: &mut String) -> Option<()> {
    let escape = if dialect == Dialect::Bash { '\\' } else { '`' };

    loop {
        match characters.next()? {
            '"' => return Some(()),
            character if character == escape => word.push(characters.next()?),
            '`' => return None,
            '$' if characters.peek() == Some(&'(') => return None,
            character => word.push(character),
        }
    }
}
