use std::collections::HashSet;

use super::Comment;

/// A comment is a line whose first character after its indent is `#`. Read
/// instructions only as far as that takes: the lines they continue on, and the
/// here-documents that follow them, whose lines are not comments.
///
/// BuildKit has no here-document in some places where one was meant. A line that
/// one of the two readings hides can start a here-document in the other, so
/// neither reading holds the other. Where they differ, the file is read in both
/// ways, and a comment is a line that both take for one.
pub(super) fn parse(text: &str) -> Vec<Comment> {
    let (mut comments, same) = read(text, false);
    if !same {
        let (meant, _) = read(text, true);
        let meant: HashSet<_> = meant.iter().map(|comment| comment.range.start).collect();
        comments.retain(|comment| meant.contains(&comment.range.start));
    }
    comments
}

/// The here-documents after one instruction, in order: for each, whether leading
/// tabs are stripped from the line that ends it, and that line.
type Documents = Vec<(bool, String)>;

/// The comments of one reading, and whether the other reading has the same
/// here-documents after every instruction that this one came to.
fn read(text: &str, meant: bool) -> (Vec<Comment>, bool) {
    let mut comments = Vec::new();
    let mut same = true;
    let mut lines = Lines { text, offset: 0 };
    let mut comment = |start: usize, line: &str| {
        let end = start + line.len();
        comments.push(Comment::line(end - line.trim_start().len()..end, 1));
    };
    // Parser directives come first, after a shebang if there is one.
    let mut directives = true;
    let mut escape = '\\';
    while let Some((start, line)) = lines.next() {
        let content = line.trim_start();
        if content.starts_with('#') {
            if let Some(value) = directive(content).filter(|_| directives) {
                if let Some(value) = value.filter(|value| matches!(*value, "\\" | "`")) {
                    escape = value.chars().next().unwrap_or(escape);
                }
            } else if !(start == 0 && content.starts_with("#!")) {
                directives = false;
                comment(start, line);
            }
            continue;
        }
        directives = false;
        let mut instruction = String::new();
        let mut continues = add_line(&mut instruction, content, escape);
        while continues && let Some((start, line)) = lines.next() {
            let content = line.trim_start();
            if content.starts_with('#') {
                comment(start, line);
            } else if !content.is_empty() {
                continues = add_line(&mut instruction, line, escape);
            }
        }
        let (buildkit, intended) = here_documents(&instruction);
        same &= intended.as_ref() == Some(&buildkit);
        // Without the end of a here-document, no later line is known to be a comment.
        let Some(documents) = (if meant { intended } else { Some(buildkit) }) else {
            break;
        };
        for (strip_tabs, delimiter) in documents {
            for (_, line) in lines.by_ref() {
                let line = if strip_tabs {
                    line.trim_start_matches('\t')
                } else {
                    line
                };
                if line == delimiter {
                    break;
                }
            }
        }
    }
    (comments, same)
}

struct Lines<'a> {
    text: &'a str,
    offset: usize,
}

impl<'a> Iterator for Lines<'a> {
    type Item = (usize, &'a str);

    fn next(&mut self) -> Option<Self::Item> {
        let start = self.offset;
        let rest = self.text.get(start..).filter(|rest| !rest.is_empty())?;
        let length = rest.find(['\r', '\n']).unwrap_or(rest.len());
        let (line, ending) = rest.split_at(length);
        self.offset += length
            + if ending.starts_with("\r\n") {
                2
            } else {
                ending.len().min(1)
            };
        Some((start, line))
    }
}

/// The value of the escape directive, or `None` for another directive.
fn directive(comment: &str) -> Option<Option<&str>> {
    let (name, value) = comment[1..].split_once('=')?;
    if value.is_empty() {
        return None;
    }
    let (name, value) = (name.trim(), value.trim());
    if name.eq_ignore_ascii_case("escape") {
        return Some(Some(value));
    }
    ["syntax", "check"]
        .iter()
        .any(|known| name.eq_ignore_ascii_case(known))
        .then_some(None)
}

/// Adds a line without its continuation character, and tells whether the
/// instruction continues on a later line.
fn add_line(instruction: &mut String, line: &str, escape: char) -> bool {
    let continued = line
        .trim_end_matches([' ', '\t'])
        .strip_suffix(escape)
        .filter(|line| !line.ends_with(escape));
    instruction.push_str(continued.unwrap_or(line));
    continued.is_some()
}

/// The here-documents that follow an instruction: first the ones of BuildKit,
/// and then the ones that were meant. `None` where a delimiter cannot be read,
/// so that the end of its here-document is unknown.
///
/// BuildKit splits the instruction into words, and then reads the word after
/// `<<` once more. It has no here-document where a blank follows `<<-`, where
/// the delimiter is empty or holds `<`, and anywhere in an instruction that its
/// lexer rejects. Those were still meant as here-documents.
fn here_documents(instruction: &str) -> (Documents, Option<Documents>) {
    let mut words = instruction.split_whitespace();
    let mut keyword = words.next().unwrap_or_default();
    if keyword.eq_ignore_ascii_case("onbuild") {
        keyword = words.next().unwrap_or_default();
    }
    let known = ["run", "copy", "add"]
        .iter()
        .any(|known| keyword.eq_ignore_ascii_case(known));
    if !known || !instruction.contains("<<") {
        return (Vec::new(), Some(Vec::new()));
    }
    let read = Words::read(instruction, true);
    // After an error of the lexer, the words before it were still meant. The
    // word that it stopped in gives no delimiter.
    let stopped = !read.complete && (opening(&read.word).is_some() || read.waits);
    let mut buildkit = Vec::new();
    let mut meant = (!read.lost && !stopped).then(Vec::new);
    let mut words = read.words.iter().peekable();
    while let Some(word) = words.next() {
        let Some((strip_tabs, mut delimiter)) = opening(word) else {
            continue;
        };
        let own = !delimiter.is_empty();
        if !own {
            // The delimiter was meant to be the next word, unless that is an operator itself.
            match words.next_if(|next| opening(next).is_none()) {
                Some(next) => delimiter = next,
                None => continue,
            }
        }
        let mut names = Words::read(delimiter, false);
        let name = names.words.pop();
        if !names.complete || !names.words.is_empty() {
            meant = None;
            continue;
        }
        if let Some(name) = &name
            && read.complete
            && own
            && !delimiter.contains('<')
        {
            buildkit.push((strip_tabs, name.clone()));
        }
        // An empty delimiter, as in `<<""`, was meant to end at an empty line.
        if let Some(meant) = &mut meant {
            meant.push((strip_tabs, name.unwrap_or_default()));
        }
    }
    (buildkit, meant)
}

/// The rest of a word that opens a here-document: what follows its digits,
/// `<<`, an optional `-`, and blanks. A third `<` makes the operator a here-string.
fn opening(word: &str) -> Option<(bool, &str)> {
    let word = word.trim_start_matches(|digit: char| digit.is_ascii_digit());
    let word = word.strip_prefix("<<")?;
    let (strip_tabs, word) = match word.strip_prefix('-') {
        Some(word) => (true, word),
        None => (false, word),
    };
    let rest = word.trim_start_matches([' ', '\t', '\n', '\r', '\u{c}']);
    (!rest.starts_with('<')).then_some((strip_tabs, rest))
}

#[derive(Clone, Copy)]
enum Frame {
    /// The text of `${name ... }` up to this character. Another part follows
    /// the pattern of `${name/pattern/replacement}`.
    Part {
        stop: char,
        escapes: bool,
        patterns: bool,
        replacement: bool,
    },
    Double {
        escapes: bool,
        patterns: bool,
    },
}

/// The words of BuildKit's shell lexer, which does not depend on the escape
/// directive. It keeps quotes and backslashes when it splits an instruction,
/// and removes them when it reads a delimiter. The lexer calls itself for each
/// `${name ... }` and each string; the stack takes the place of those calls.
struct Words<'a> {
    characters: std::iter::Peekable<std::str::Chars<'a>>,
    keeps: bool,
    stack: Vec<Frame>,
    words: Vec<String>,
    word: String,
    /// Where in the word a string outside every `${ }` began.
    string: usize,
    /// Whether the lexer read the text to its end without an error.
    complete: bool,
    /// Whether the lexer dropped the start of a word that `<<` began or followed.
    lost: bool,
    /// Whether the last whole word is an operator whose delimiter is the word
    /// after it. Decided once, where the word ends.
    waits: bool,
}

impl<'a> Words<'a> {
    fn read(text: &'a str, keeps: bool) -> Self {
        let mut words = Self {
            // The lexer skips a byte order mark at the start of its text.
            characters: text
                .strip_prefix('\u{feff}')
                .unwrap_or(text)
                .chars()
                .peekable(),
            keeps,
            stack: Vec::new(),
            words: Vec::new(),
            word: String::new(),
            string: 0,
            complete: false,
            lost: false,
            waits: false,
        };
        while let Some(character) = words.characters.next() {
            if words.step(character).is_none() {
                return words;
            }
        }
        // Whatever is open at the end is an error. The end closes the last word.
        if words.stack.is_empty() {
            words.split(' ');
            words.complete = true;
        }
        words
    }

    fn split(&mut self, character: char) {
        if !character.is_whitespace() {
            self.word.push(character);
        } else if !self.word.is_empty() {
            self.waits = opening(&self.word).is_some_and(|(_, rest)| rest.is_empty());
            self.words.push(std::mem::take(&mut self.word));
        }
    }

    /// The text of `${name ... }` outside every string is split into words once
    /// more, whatever quotes it holds. Other quoted or escaped text is not.
    fn put(&mut self, character: char) {
        match self.stack.first() {
            Some(Frame::Part { .. }) => self.split(character),
            _ => self.word.push(character),
        }
    }

    fn step(&mut self, character: char) -> Option<()> {
        let (stop, escapes, patterns) = match self.stack.last() {
            None => (None, self.keeps, false),
            Some(&Frame::Part {
                stop,
                escapes,
                patterns,
                ..
            }) => (Some(stop), escapes, patterns),
            Some(&Frame::Double { escapes, patterns }) => {
                return self.double(character, escapes, patterns);
            }
        };
        match character {
            _ if Some(character) == stop => {
                let Some(Frame::Part { replacement, .. }) = self.stack.pop() else {
                    return None;
                };
                if replacement {
                    self.put('/');
                    self.stack.push(Frame::Part {
                        stop: '}',
                        escapes: true,
                        patterns: false,
                        replacement: false,
                    });
                } else {
                    self.put('}');
                }
            }
            '$' => self.dollar()?,
            '<' => {
                self.put('<');
                if self.characters.next_if_eq(&'<').is_some() {
                    self.put('<');
                    while let Some(blank) = self
                        .characters
                        .next_if(|next| matches!(next, ' ' | '\t' | '\r'))
                    {
                        self.put(blank);
                    }
                }
            }
            '\'' => {
                if self.keeps {
                    self.put('\'');
                }
                loop {
                    let quoted = self.characters.next()?;
                    if quoted == '\'' {
                        break;
                    }
                    if patterns && quoted == '\\' {
                        self.put('\\');
                    }
                    self.put(quoted);
                }
                if self.keeps {
                    self.put('\'');
                }
            }
            '"' => {
                if self.stack.is_empty() {
                    self.string = self.word.len();
                }
                if self.keeps {
                    self.put('"');
                }
                self.stack.push(Frame::Double { escapes, patterns });
            }
            '\\' => {
                if escapes {
                    self.put('\\');
                }
                // A backslash at the end of the text is dropped.
                if let Some(escaped) = self.characters.next() {
                    self.put(escaped);
                }
            }
            _ if self.stack.is_empty() => self.split(character),
            _ => self.put(character),
        }
        Some(())
    }

    fn double(&mut self, character: char, escapes: bool, patterns: bool) -> Option<()> {
        match character {
            '"' => {
                if self.keeps {
                    self.put('"');
                }
                self.stack.pop();
            }
            '$' => self.dollar()?,
            // A backslash hides only these three characters, and stays before others.
            '\\' => match self.characters.peek() {
                None => {}
                Some('"' | '$' | '\\') => {
                    if escapes {
                        self.put('\\');
                    }
                    let escaped = self.characters.next()?;
                    self.put(escaped);
                }
                Some(_) => {
                    if escapes && (!self.keeps || patterns) {
                        self.put('\\');
                    }
                    self.put('\\');
                }
            },
            _ => self.put(character),
        }
        Some(())
    }

    /// After `$`: a name, or `${name}`, or `${name ... }` with more text.
    fn dollar(&mut self) -> Option<()> {
        if self.characters.next_if_eq(&'{').is_none() {
            self.put('$');
            let name = self.name();
            name.chars().for_each(|character| self.put(character));
            return Some(());
        }
        if matches!(self.characters.peek(), None | Some('{' | '}' | ':')) {
            return None;
        }
        let name = self.name();
        let mut operator = String::from(self.characters.next()?);
        let part = match operator.as_str() {
            "}" => None,
            ":" => {
                let modifier = self.characters.next()?;
                operator.push(modifier);
                if matches!(modifier, '#' | '%') {
                    return None;
                }
                Some(('}', false, false))
            }
            "+" | "-" | "?" => Some(('}', false, false)),
            "#" | "%" => Some(('}', true, false)),
            "/" => {
                self.characters.next_if_eq(&'/');
                Some(('/', true, true))
            }
            _ => return None,
        };
        if let Some((stop, escapes, replacement)) = part {
            // The lexer starts its word again where it reads the text of
            // `${name ... }`. Only a string around it stays in the word.
            let dropped = match self.stack.first() {
                None => self.word.len(),
                Some(Frame::Double { .. }) => std::mem::take(&mut self.string),
                Some(Frame::Part { .. }) => 0,
            };
            // The dropped text is read here once, and is gone after that.
            self.lost |= dropped > 0 && (self.waits || opening(&self.word[..dropped]).is_some());
            self.word.drain(..dropped);
            self.stack.push(Frame::Part {
                stop,
                escapes,
                patterns: escapes,
                replacement,
            });
        }
        "${".chars()
            .chain(name.chars())
            .chain(operator.chars())
            .for_each(|character| self.put(character));
        Some(())
    }

    /// A run of digits, one special parameter, or letters, digits, and `_`.
    fn name(&mut self) -> String {
        let mut name = String::new();
        match self.characters.peek() {
            Some(first) if first.is_numeric() => {
                while let Some(digit) = self.characters.next_if(|next| next.is_numeric()) {
                    name.push(digit);
                }
            }
            Some('@' | '*' | '#' | '?' | '-' | '$' | '!') => name.extend(self.characters.next()),
            _ => {
                while let Some(letter) = self
                    .characters
                    .next_if(|next| next.is_alphanumeric() || *next == '_')
                {
                    name.push(letter);
                }
            }
        }
        name
    }
}
