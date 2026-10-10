use super::{Comment, Kind};

// How PowerShell reads a character depends on whether it starts a token, and
// that can depend on what the parser expects. This reader does not follow the
// parser. It tells apart only the places where every reading agrees.
#[derive(Clone, Copy, PartialEq)]
enum Place {
    // A token starts here, whatever the parser expects.
    Start,
    // After other text: inside one argument of a command, or at the start of
    // another token of an expression.
    Glued,
    // Inside one argument of a command, whatever the parser expects.
    Argument,
}

// Reads one character or one quoted piece at a time, in a single pass and
// without a stack: nothing that nests changes what a `#` or a quote means.
pub(super) fn parse(text: &str) -> Vec<Comment> {
    let mut reader = Reader {
        text,
        at: 0,
        place: Place::Start,
        quiet: 0,
        comments: Vec::new(),
    };
    // Where the readings may differ from here on, the comments found so far
    // are the result.
    while reader.step().is_some() {}
    reader.comments
}

struct Reader<'a> {
    text: &'a str,
    at: usize,
    place: Place,
    // The end of a line whose `#` may or may not have started a comment. No
    // comment is taken from the text before it, and nothing may reach past it.
    quiet: usize,
    comments: Vec<Comment>,
}

fn blank(character: char) -> bool {
    matches!(
        character,
        '\t' | '\u{b}' | '\u{c}' | ' ' | '\u{85}' | '\u{a0}'
    ) || character > '\u{ff}' && character.is_whitespace()
}

// PowerShell takes these for `'`, `"`, and `-`.
fn single(character: char) -> bool {
    matches!(character, '\'' | '\u{2018}'..='\u{201b}')
}

fn double(character: char) -> bool {
    matches!(character, '"' | '\u{201c}'..='\u{201e}')
}

fn dash(character: char) -> bool {
    matches!(character, '-' | '\u{2013}'..='\u{2015}')
}

// These end every token before them.
fn ends_token(character: char) -> bool {
    matches!(
        character,
        '\0' | '\n' | '\r' | '&' | '(' | ')' | ',' | ';' | '{' | '|' | '}'
    ) || blank(character)
}

// A type name such as ``List`1`` holds a backtick that escapes nothing.
fn names_type(character: char) -> bool {
    character.is_alphanumeric() || matches!(character, '_' | '.' | '\\' | '+' | '#' | '`')
}

impl Reader<'_> {
    fn peek(&self, at: usize) -> Option<char> {
        self.text[at..].chars().next()
    }

    fn next(&mut self) -> Option<char> {
        let character = self.peek(self.at)?;
        self.at += character.len_utf8();
        Some(character)
    }

    fn line_end(&self, from: usize) -> usize {
        self.text[from..]
            .find(['\r', '\n'])
            .map_or(self.text.len(), |length| from + length)
    }

    // `None` ends the reading: at the end of the text, inside something that
    // is not closed, or where the readings differ.
    fn step(&mut self) -> Option<()> {
        let start = self.at;
        let character = self.next()?;
        let next = self.peek(self.at);
        let place = self.place;
        match character {
            '`' => self.backtick(start, next?)?,
            '#' if place == Place::Start => {
                self.at = self.line_end(start);
                let source = &self.text[start..self.at];
                let requires = source
                    .get(1..9)
                    .is_some_and(|name| name.eq_ignore_ascii_case("requires"));
                if start >= self.quiet && !requires && !(start == 0 && source.starts_with("#!")) {
                    self.comments.push(Comment::line(start..self.at, 1));
                }
            }
            // After other text, the rest of the line is a comment to one
            // reading and more of an argument to the other.
            '#' if place == Place::Glued && start >= self.quiet => {
                self.quiet = self.line_end(start);
            }
            '<' if next == Some('#') && place != Place::Argument => {
                if place == Place::Glued {
                    return None;
                }
                self.at += self.text[self.at + 1..].find("#>")? + 3;
                if start >= self.quiet {
                    self.comments.push(Comment {
                        range: start..self.at,
                        kind: Kind::Block {
                            opening: 2,
                            closed: true,
                        },
                    });
                }
            }
            _ if single(character) || double(character) => self.quoted(character)?,
            '@' if next.is_some_and(|quote| single(quote) || double(quote)) => match place {
                Place::Start => self.here_string(next.is_some_and(double))?,
                Place::Glued => return None,
                Place::Argument => {}
            },
            '$' if next == Some('(') => {
                self.at += 1;
                if place != Place::Start {
                    self.group(place == Place::Argument)?;
                }
            }
            '$' if next == Some('{') => {
                // `$$` can be one variable before a block.
                if place == Place::Glued && self.text[..start].ends_with('$') {
                    return None;
                }
                while self.next()? != '}' {
                    if self.text[..self.at].ends_with('`') {
                        self.next()?;
                    }
                }
            }
            _ if dash(character) && place != Place::Argument && self.stops_parsing() => {
                if place == Place::Glued {
                    return None;
                }
                self.verbatim();
            }
            // A byte order mark is a blank at the start of a token only.
            '\u{feff}' => {}
            _ if ends_token(character) => self.place = Place::Start,
            _ if place == Place::Start => self.place = Place::Glued,
            _ => {}
        }
        // What the text holds after a `#` that may have started a comment
        // must end with its line.
        (start >= self.quiet || self.at <= self.quiet).then_some(())
    }

    // A backtick continues the line, joins a blank to an argument, or escapes
    // the next character and so makes the text around it an argument.
    fn backtick(&mut self, start: usize, next: char) -> Option<()> {
        if matches!(next, '\n' | '\r') {
            return Some(());
        }
        match self.place {
            Place::Start if blank(next) => return Some(()),
            Place::Glued if blank(next) => {}
            Place::Glued if self.text[..start].ends_with(names_type) => {
                // Inside a type name, the next character is not escaped.
                if single(next) || double(next) || dash(next) || matches!(next, '$' | '@' | '<') {
                    return None;
                }
            }
            _ => self.place = Place::Argument,
        }
        self.at += next.len_utf8();
        Some(())
    }

    // The rest of a string after its opening quote. Two quotes in a row stand
    // for one, which reads the same as one string after another. In double
    // quotes, a backtick escapes, and PowerShell finds the end of a `$( ... )`
    // by counting parentheses: a quote in there does not end the string, and
    // no comment is taken from there.
    fn quoted(&mut self, quote: char) -> Option<()> {
        let closes = if double(quote) { double } else { single };
        loop {
            let character = self.next()?;
            if closes(character) {
                return Some(());
            } else if double(quote) && character == '`' {
                self.next()?;
            } else if double(quote) && character == '$' && self.peek(self.at) == Some('(') {
                self.at += 1;
                self.group(true)?;
            }
        }
    }

    // A `$( ... )` whose end is found by counting parentheses. Outside a
    // string, the same text can be commands with strings and comments of
    // their own. Unless it is `counted` for sure, it may hold nothing that
    // the two readings take differently: a string in it holds no parenthesis.
    fn group(&mut self, counted: bool) -> Option<()> {
        let mut depth = 1_usize;
        while depth > 0 {
            let Some(character) = self.next() else {
                break;
            };
            let start = self.at - character.len_utf8();
            let before = &self.text[..start];
            match character {
                '(' => depth += 1,
                ')' => depth -= 1,
                '`' if counted => {
                    self.next();
                }
                _ if counted => {}
                '`' | '#' => return None,
                '{' if before.ends_with('$') => return None,
                '%' if before.trim_end_matches(dash).len() + 2 <= before.len() => return None,
                _ if single(character) || double(character) => {
                    if before.ends_with('@') {
                        return None;
                    }
                    self.quoted(character)?;
                    if self.text[start..self.at].contains(['(', ')']) {
                        return None;
                    }
                }
                _ => {}
            }
        }
        Some(())
    }

    // A here-string ends at a line that starts with its quote and `@`.
    fn here_string(&mut self, expands: bool) -> Option<()> {
        self.next();
        // PowerShell reads `$( ... )` in a here-string to its last
        // parenthesis, also over a line that would end the here-string.
        let mut depth = 0_usize;
        loop {
            let character = self.next()?;
            match character {
                '\n' | '\r' => {
                    let quote = self.peek(self.at);
                    let closes = quote.is_some_and(if expands { double } else { single });
                    if closes && self.peek(self.at + quote.map_or(0, char::len_utf8)) == Some('@') {
                        self.at += quote.map_or(0, char::len_utf8) + 1;
                        return (depth == 0).then_some(());
                    }
                }
                '$' if expands && depth == 0 && self.peek(self.at) == Some('(') => {
                    self.at += 1;
                    depth = 1;
                }
                '(' if depth > 0 => depth += 1,
                ')' if depth > 0 => depth -= 1,
                '`' if depth > 0 && matches!(self.peek(self.at), Some('(' | ')')) => self.at += 1,
                _ => {}
            }
        }
    }

    // Whether the dash before this place starts `--%`.
    fn stops_parsing(&self) -> bool {
        let mut rest = self.text[self.at..].chars();
        rest.next().is_some_and(dash) && rest.next() == Some('%')
    }

    // After `--%`, the text up to the end of the line, or up to `|` or `&&`
    // outside double quotes, is passed on as written.
    fn verbatim(&mut self) {
        self.next();
        self.next();
        let mut quoted = false;
        while let Some(character) = self.peek(self.at) {
            let ends = match character {
                '\n' | '\r' => true,
                '|' => !quoted,
                '&' => !quoted && self.text[self.at + 1..].starts_with('&'),
                _ => false,
            };
            if ends {
                break;
            }
            quoted ^= double(character);
            self.at += character.len_utf8();
        }
        self.place = Place::Start;
    }
}
