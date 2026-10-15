use super::{Comment, Kind};

/// How PowerShell reads a character depends on whether it starts a token, and
/// that can depend on what the parser expects. This reader does not follow the
/// parser. It tells apart only the places where every reading agrees, and
/// follows PowerShell's own tokenizer there, also where that reads on although
/// the text looks finished: after a backtick before a line break, and inside
/// `$( ... )` and `${ ... }` in a string.
#[derive(Clone, Copy, PartialEq)]
enum Place {
    /// A token starts here, whatever the parser expects.
    Start,
    /// After other text: inside one argument of a command, or at the start of
    /// another token of an expression.
    Glued,
    /// Inside one argument of a command, whatever the parser expects.
    Argument,
}

/// Reads one character or one quoted piece at a time, in a single pass and
/// without a stack: nothing that nests changes what a `#` or a quote means.
pub(super) fn parse(text: &str) -> Vec<Comment> {
    let mut reader = Reader {
        text,
        at: 0,
        place: Place::Start,
        quiet: 0,
        types: 0,
        named: false,
        listed: false,
        parameter: false,
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
    /// The end of a line whose `#` may or may not have started a comment. No
    /// comment is taken from the text before it, and nothing may reach past it.
    quiet: usize,
    /// The brackets open since a `[` that can start the name of a type, while
    /// nothing but such names followed it, whether the name after the last of
    /// them has begun, and whether a `,` followed.
    types: usize,
    named: bool,
    listed: bool,
    /// Whether a `-name` has begun since the last start of a token. A command
    /// reads its parameter up to a blank, a quote, or one of `{}();,|&.[:`, and
    /// a backtick or `$` in it is plain text.
    parameter: bool,
    comments: Vec<Comment>,
}

fn blank(character: char) -> bool {
    matches!(
        character,
        '\t' | '\u{b}' | '\u{c}' | ' ' | '\u{85}' | '\u{a0}'
    ) || character > '\u{ff}' && character.is_whitespace()
}

/// PowerShell takes these for `'`, `"`, and `-`.
fn single(character: char) -> bool {
    matches!(character, '\'' | '\u{2018}'..='\u{201b}')
}

fn double(character: char) -> bool {
    matches!(character, '"' | '\u{201c}'..='\u{201e}')
}

fn dash(character: char) -> bool {
    matches!(character, '-' | '\u{2013}'..='\u{2015}')
}

/// These end every token before them.
fn ends_token(character: char) -> bool {
    matches!(
        character,
        '\0' | '\n' | '\r' | '&' | '(' | ')' | ',' | ';' | '{' | '|' | '}'
    ) || blank(character)
}

/// A type name such as ``List`1`` holds a backtick that escapes nothing.
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

    /// `None` ends the reading: at the end of the text, inside something that
    /// is not closed, or where the readings differ.
    fn step(&mut self) -> Option<()> {
        let start = self.at;
        let character = self.next()?;
        let next = self.peek(self.at);
        let place = self.place;
        self.type_name(character)?;
        let before = &self.text[..start];
        let names = |next: char| next.is_alphabetic() || matches!(next, '_' | '?');
        self.parameter = place != Place::Start && self.parameter
            || dash(character) && next.is_some_and(names) && !before.ends_with(dash);
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
            // `@$` and `$$` are one variable each, so a `(` or `{` after them
            // is one of its own.
            '@' | '$' if next == Some('$') && (character == '$' || place == Place::Start) => {
                self.at += 1;
                if place == Place::Start {
                    self.place = Place::Glued;
                }
            }
            '$' if next == Some('(') => {
                self.at += 1;
                match place {
                    Place::Start => {}
                    Place::Glued => self.group()?,
                    Place::Argument => self.counted()?,
                }
            }
            '$' if next == Some('{') => {
                self.at += 1;
                // In a parameter, and after `@`, the `{` can open a block.
                let block = place == Place::Glued && (self.parameter || before.ends_with('@'));
                self.braced()?;
                let name = &self.text[start..self.at];
                let opens =
                    |inside: char| single(inside) || double(inside) || "#`%".contains(inside);
                if block && name.contains(opens) {
                    return None;
                }
                // A command takes text right after the `}` into the same argument.
                if place == Place::Start {
                    self.place = Place::Glued;
                }
            }
            // A NUL ends a token and starts an argument. A byte order mark
            // starts one too, and is no blank.
            '\0' => self.place = Place::Argument,
            '\u{feff}' if place == Place::Start => self.place = Place::Argument,
            _ if ends_token(character) => self.place = Place::Start,
            _ => {
                if place == Place::Start {
                    self.place = Place::Glued;
                }
                if character == '%' && self.stops(start) {
                    self.verbatim()?;
                }
            }
        }
        // What the text holds after a `#` that may have started a comment
        // must end with its line.
        (start >= self.quiet || self.at <= self.quiet).then_some(())
    }

    /// After `[name,`, PowerShell can read the name of an assembly up to the
    /// next blank, `,`, `=`, or `]`: quotes, `#`, and parentheses open nothing
    /// in it. Whether it does depends on the parser, so such a character stops
    /// the reading.
    fn type_name(&mut self, character: char) -> Option<()> {
        match character {
            // Also after text that looks like it takes an index, as in `-is[`.
            '[' if self.types > 0 || self.place != Place::Argument => {
                self.types += 1;
                self.named = false;
            }
            _ if self.types == 0 => {}
            ']' => {
                self.types -= 1;
                self.listed &= self.types > 0;
            }
            '\n' | '\r' => (self.types, self.listed) = (0, false),
            ',' if self.named => self.listed = true,
            _ if blank(character) => {}
            // The name of a type starts with a letter. An index does not have to.
            _ if !self.named && !self.listed => {
                self.named = character.is_alphabetic() || character == '_';
                self.types *= usize::from(self.named);
            }
            _ if self.listed => {
                let opens = matches!(
                    character,
                    '#' | '`' | '$' | '<' | '@' | '(' | ')' | '{' | '}' | ';' | '|' | '&'
                );
                if opens || single(character) || double(character) {
                    return None;
                }
            }
            _ if names_type(character) => {}
            _ => self.types = 0,
        }
        Some(())
    }

    /// Whether the `%` at this offset ends `--%`. PowerShell looks at an
    /// argument without its quotes and backticks, so `-'-%'` counts as well.
    fn stops(&self, percent: usize) -> bool {
        let skipped = |character: char| single(character) || double(character) || character == '`';
        let before = self.text[..percent].trim_end_matches(skipped);
        before
            .strip_suffix('-')
            .is_some_and(|before| before.trim_end_matches(skipped).ends_with('-'))
    }

    /// After `--%`, a command takes the rest of its line as it is written, up
    /// to `|` or `&&`. Elsewhere the same text is read as usual. So nothing is
    /// taken from that line, and both readings must end with it.
    fn verbatim(&mut self) -> Option<()> {
        let end = self.line_end(self.at);
        let rest = &self.text[self.at..end];
        if self.at < self.quiet || rest.contains('|') || rest.contains("&&") {
            return None;
        }
        self.quiet = end;
        Some(())
    }

    /// A backtick continues the line, joins a blank or a line break to an
    /// argument, or escapes the next character and so makes the text around it
    /// an argument.
    fn backtick(&mut self, start: usize, next: char) -> Option<()> {
        let line_break = matches!(next, '\n' | '\r');
        // `u{...}` names a character, and its braces open and close nothing.
        let unicode = next == 'u' && self.peek(self.at + 1) == Some('{');
        // Inside a type name or a parameter, the backtick is plain text.
        let plain = self.parameter || self.text[..start].ends_with(names_type);
        match self.place {
            Place::Start if line_break || blank(next) => return Some(()),
            // The backtick takes only the CR of CRLF, and the LF ends the line.
            _ if next == '\r' && self.peek(self.at + 1) == Some('\n') => return Some(()),
            // The line break becomes a part of the argument.
            Place::Argument if line_break => {}
            // The next line starts inside the argument, or starts a token.
            _ if line_break => self.place = Place::Glued,
            Place::Glued if blank(next) => {}
            Place::Glued if plain => {
                let opens = single(next) || double(next) || dash(next) || "$@<".contains(next);
                if opens || unicode || self.types > 0 && !next.is_alphanumeric() {
                    return None;
                }
            }
            _ => self.place = Place::Argument,
        }
        self.at += next.len_utf8();
        if unicode {
            self.unicode();
        }
        if next == '%' && self.stops(self.at - 1) {
            self.verbatim()?;
        }
        Some(())
    }

    /// The rest of `u{...}` after a backtick: up to six digits and a `}`.
    fn unicode(&mut self) {
        self.at += 1;
        let digits = self.text[self.at..]
            .bytes()
            .take(6)
            .take_while(u8::is_ascii_hexdigit);
        self.at += digits.count();
        self.at += usize::from(self.peek(self.at) == Some('}'));
    }

    /// The rest of a string after its opening quote. Two quotes in a row stand
    /// for one, which reads the same as one string after another. In double
    /// quotes, a backtick escapes, and `$` can open a `$( ... )` or `${ ... }`
    /// that a quote does not end. No comment is taken from there.
    fn quoted(&mut self, quote: char) -> Option<()> {
        let closes = if double(quote) { double } else { single };
        loop {
            let at = self.at;
            let character = self.next()?;
            if closes(character) {
                // An argument that reads `--%` without its quotes works as that.
                let stops = self.text[..at].ends_with('%') && self.stops(at - 1);
                return if stops { self.verbatim() } else { Some(()) };
            }
            if double(quote) {
                self.expansion(character)?;
            }
        }
    }

    /// What a backtick or `$` does inside double quotes and inside a
    /// here-string in double quotes.
    fn expansion(&mut self, character: char) -> Option<()> {
        match (character, self.peek(self.at)) {
            ('`', _) => {
                self.next()?;
            }
            ('$', Some('(')) => {
                self.at += 1;
                self.counted()?;
            }
            ('$', Some('{')) => {
                self.at += 1;
                self.braced()?;
            }
            // `$$` is one variable, and what follows it opens nothing.
            ('$', Some('$')) => self.at += 1,
            _ => {}
        }
        Some(())
    }

    /// The rest of `${ ... }` after its `{`: up to the next `}` that no
    /// backtick escapes, whatever else it holds.
    fn braced(&mut self) -> Option<()> {
        loop {
            match self.next()? {
                '}' => return Some(()),
                '`' => {
                    let escaped = self.next()?;
                    if escaped == 'u' && self.peek(self.at) == Some('{') {
                        self.unicode();
                    }
                }
                _ => {}
            }
        }
    }

    /// The rest of a `$( ... )` inside a string or an argument. PowerShell
    /// finds its end by counting parentheses, and nothing in it is escaped or
    /// quoted for that count.
    fn counted(&mut self) -> Option<()> {
        let mut depth = 1_usize;
        while depth > 0 {
            match self.next()? {
                '(' => depth += 1,
                ')' => depth -= 1,
                _ => {}
            }
        }
        Some(())
    }

    /// The rest of a `$( ... )` after other text. Its end is found by counting
    /// parentheses if the text is an argument, and it holds commands with
    /// strings and comments of their own if a token starts at it. So it may
    /// hold nothing that the two readings take differently: a string in it
    /// holds no parenthesis.
    fn group(&mut self) -> Option<()> {
        let mut depth = 1_usize;
        // A `[` and then a `,` on this line: see `type_name`.
        let (mut bracket, mut listed) = (false, false);
        while depth > 0 {
            let Some(character) = self.next() else {
                break;
            };
            let start = self.at - character.len_utf8();
            let before = &self.text[..start];
            match character {
                '(' => depth += 1,
                ')' => depth -= 1,
                '`' | '#' => return None,
                '{' if before.ends_with('$') => return None,
                '%' if self.stops(start) => return None,
                '[' => bracket = true,
                ',' => listed = bracket,
                ']' | '\n' | '\r' => (bracket, listed) = (false, false),
                _ if single(character) || double(character) => {
                    if listed || before.ends_with('@') {
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

    /// A here-string starts after its opening line, and ends at a line that
    /// starts with its quote and `@`.
    fn here_string(&mut self, expands: bool) -> Option<()> {
        let closes = if expands { double } else { single };
        self.next();
        while self.peek(self.at).is_some_and(blank) {
            self.next();
        }
        // Other text after the opening is a mistake. PowerShell then ends the
        // here-string at a plain `"@` or `'@` on the same line, or at the end
        // of that line.
        let plain = if expands { '"' } else { '\'' };
        while !matches!(self.peek(self.at)?, '\n' | '\r') {
            if self.next() == Some(plain) && self.peek(self.at) == Some('@') {
                self.at += 1;
                return Some(());
            }
            if self
                .peek(self.at)
                .is_none_or(|next| matches!(next, '\n' | '\r'))
            {
                return Some(());
            }
        }
        let mut character = self.next()?;
        loop {
            match character {
                '\n' | '\r' => {
                    if character == '\r' && self.peek(self.at) == Some('\n') {
                        self.at += 1;
                    }
                    let quote = self.peek(self.at).filter(|quote| closes(*quote));
                    let length = quote.map_or(0, char::len_utf8);
                    if quote.is_some() && self.peek(self.at + length) == Some('@') {
                        self.at += length + 1;
                        return Some(());
                    }
                }
                // A backtick escapes the next character, also a line break
                // before what looks like the ending. It takes only the CR of
                // CRLF. A `$( ... )` and a `${ ... }` run to their ends over
                // any line.
                _ if expands => self.expansion(character)?,
                _ => {}
            }
            character = self.next()?;
        }
    }
}
