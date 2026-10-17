use std::mem::replace;

use super::Comment;

mod heredoc;

use heredoc::HereDocument;

/// Reads sh, bash, and zsh alike, and follows bash where the shells differ.
/// What bash reads as data is never returned; a comment may be missed instead.
pub(super) fn parse(text: &str) -> Vec<Comment> {
    let mut reader = Reader {
        bytes: text.as_bytes(),
        at: 0,
        limit: text.len(),
        stack: Vec::new(),
        stream: Stream::new(0, false, true),
        here_documents: Vec::new(),
        comments: Vec::new(),
    };
    // The reader stops at the end, and where it cannot tell how a shell reads
    // on. The comments found before that stand either way.
    while reader.step().is_some() {}
    reader.comments
}

/// What the next word of a command can be. bash decides it from the token
/// before, and reads a reserved word only in some of these places.
#[derive(Clone, Copy, PartialEq)]
enum Position {
    /// A reserved word or an assignment.
    Command,
    /// As `Command`, and the word after it starts a command too.
    Coproc,
    /// As `Command`, with `-p` or `--` after `time`, or `--` after `time -p`.
    Time,
    TimeOption,
    /// An assignment, after another one.
    Assigned,
    /// The target of a redirection that came before any word of a command,
    /// and the place after it: for an assignment, but for no reserved word.
    Target,
    Redirected,
    /// The name after `function`.
    Function,
    /// The name after `for` or `select`, then `in` or `do`.
    For,
    Loop,
    Argument,
}

#[derive(Clone, Copy)]
enum Word {
    None,
    /// Only letters, digits, and `_` since this offset, and no digit first.
    Name(usize),
    /// Nothing quoted or expanded since this offset.
    Plain(usize),
    /// A name and its subscript, which `=` turns into an assignment.
    Subscript,
    Assignment,
    Other,
}

impl Word {
    fn is_none(self) -> bool {
        matches!(self, Self::None)
    }
}

/// The commands that one parser reads: the file, or one substitution.
struct Stream {
    position: Position,
    word: Word,
    /// A `#` here starts a comment in every shell: at the start, and after a
    /// blank, a line break, `;`, or `&`.
    comment: bool,
    /// Inside `[[ ]]`, where no word is reserved.
    condition: bool,
    /// The next word is a pattern inside `[[ ]]`, which holds `(` and `|`.
    pattern: bool,
    /// Read for `$( )`, `<( )`, or `>( )`.
    substitution: bool,
    /// Where its own entries start in `Reader::here_documents`.
    here_documents: usize,
}

impl Stream {
    fn new(here_documents: usize, substitution: bool, comment: bool) -> Self {
        Self {
            position: Position::Command,
            word: Word::None,
            comment,
            condition: false,
            pattern: false,
            substitution,
            here_documents,
        }
    }
}

enum Case {
    Subject,
    In,
    /// Before a list of patterns, in its leading `( )`, and after its start.
    Start,
    Leading,
    Pattern,
    Body,
}

/// What is open around the current offset. The file itself has no frame.
enum Frame {
    /// `$( )`, `<( )`, or `>( )`, with the stream around it.
    Substitution(Stream),
    Backquote {
        outer: Stream,
        /// The offset after the closing backquote.
        resume: usize,
        here_documents: usize,
    },
    Subshell,
    /// The words of `name=( )`, none of which is reserved, with what the word
    /// of the assignment could be.
    Array(Position),
    Case(Case),
    Double,
    Parameter,
    /// Text that ends where its brackets balance: an arithmetic expression, a
    /// subscript, or a group inside a pattern. No comment starts in it.
    Group {
        close: u8,
        depth: usize,
        /// Whether `$( )`, `${ }`, and `$[ ]` are read inside.
        expands: bool,
        /// For `(( ))`: the number of comments found before it, and whether the
        /// text so far reads the same as commands do.
        arithmetic: Option<(usize, bool)>,
    },
}

/// Whether a backslash and a line break between these two bytes come inside
/// what can be one token. bash removes the pair before it reads tokens, and the
/// reader does not look through it.
fn splits(before: Option<u8>, after: Option<u8>) -> bool {
    matches!(
        (before, after),
        (Some(b'&'), Some(b'&' | b'>' | b'|' | b'!'))
            | (Some(b'|'), Some(b'|' | b'&'))
            | (Some(b';'), Some(b';' | b'&' | b'|'))
            | (Some(b'<'), Some(b'<' | b'&' | b'>' | b'('))
            | (Some(b'>'), Some(b'>' | b'&' | b'|' | b'('))
            | (Some(b'(' | b'='), Some(b'('))
            | (Some(b'+'), Some(b'='))
            | (Some(b'$'), Some(b'(' | b'{' | b'[' | b'\'' | b'$' | b'\\'))
    )
}

struct Reader<'a> {
    bytes: &'a [u8],
    at: usize,
    /// The end of the text, or of the backquoted command being read.
    limit: usize,
    stack: Vec<Frame>,
    stream: Stream,
    here_documents: Vec<HereDocument>,
    comments: Vec<Comment>,
}

impl Reader<'_> {
    fn step(&mut self) -> Option<()> {
        if self.at >= self.limit {
            // The end of a backquoted command closes whatever it left open.
            loop {
                let Frame::Backquote {
                    outer,
                    resume,
                    here_documents,
                } = self.stack.pop()?
                else {
                    continue;
                };
                self.stream = outer;
                self.here_documents.truncate(here_documents);
                self.limit = self.bytes.len();
                self.at = resume;
                return Some(());
            }
        }
        match self.stack.last() {
            Some(Frame::Double) => self.double(),
            Some(Frame::Parameter) => self.parameter(),
            Some(&Frame::Group { close, expands, .. }) => self.group(close, expands),
            _ => self.command(),
        }
    }

    fn peek(&self, ahead: usize) -> Option<u8> {
        self.bytes[..self.limit].get(self.at + ahead).copied()
    }

    /// The offset of the line break at or after `from`.
    fn line_end(&self, from: usize) -> usize {
        self.bytes[from..self.limit]
            .iter()
            .position(|byte| matches!(byte, b'\n' | b'\r'))
            .map_or(self.limit, |length| from + length)
    }

    /// The length of the line break at `at`: LF, CRLF, or CR.
    fn line_break(&self, at: usize) -> usize {
        match self.bytes[..self.limit].get(at..) {
            Some([b'\r', b'\n', ..]) => 2,
            Some([b'\n' | b'\r', ..]) => 1,
            _ => 0,
        }
    }

    fn double(&mut self) -> Option<()> {
        match self.bytes[self.at] {
            b'"' => {
                self.stack.pop();
                self.at += 1;
            }
            b'\\' => self.at += 2,
            b'`' => self.backquote(),
            b'$' => return self.dollar(false),
            _ => self.at += 1,
        }
        Some(())
    }

    fn parameter(&mut self) -> Option<()> {
        match self.bytes[self.at] {
            b'}' => {
                self.stack.pop();
                self.at += 1;
            }
            b'\\' => self.at += 2,
            b'\'' => self.quote(1, false),
            b'"' => self.open(Frame::Double, 1),
            b'`' => self.backquote(),
            b'$' => return self.dollar(true),
            _ => self.at += 1,
        }
        Some(())
    }

    fn group(&mut self, close: u8, expands: bool) -> Option<()> {
        let byte = self.bytes[self.at];
        match byte {
            b'\\' => self.at += 2,
            b'\'' => self.quote(1, false),
            b'"' => self.open(Frame::Double, 1),
            b'`' => self.at = self.backquoted().1 + 1,
            b'$' if expands => return self.dollar(true),
            b'$' if self.peek(1) == Some(b'\'') => self.quote(2, true),
            b'$' if self.peek(1) == Some(b'\\') && self.line_break(self.at + 2) > 0 => return None,
            // zsh can begin a here-document inside the group of a pattern.
            b'<' if !expands && self.peek(1) == Some(b'<') => return None,
            _ => {
                // Commands read these another way: a comment, a word in
                // brackets, a here-document, and the patterns of a `case`.
                let commands = match byte {
                    b'#' | b'[' => false,
                    b'<' => self.peek(1) != Some(b'<'),
                    b'e' => !self.bytes[..self.at].ends_with(b"cas"),
                    _ => true,
                };
                self.at += 1;
                let Some(Frame::Group {
                    depth, arithmetic, ..
                }) = self.stack.last_mut()
                else {
                    return None;
                };
                if let Some((_, same)) = arithmetic {
                    *same &= commands;
                }
                let arithmetic = *arithmetic;
                if byte == close {
                    *depth -= 1;
                } else if byte == if close == b')' { b'(' } else { b'[' } {
                    *depth += 1;
                }
                if *depth > 0 {
                    return Some(());
                }
                self.stack.pop();
                let after = self.peek(0);
                if let Some(Frame::Case(case @ Case::Leading)) = self.stack.last_mut() {
                    // `(a|b)` is a whole list of patterns before a blank. Before
                    // anything else, zsh reads on in the same pattern, and bash
                    // reads a command.
                    *case = match after {
                        None | Some(b' ' | b'\t' | b'\n' | b'\r' | b';' | b'&') => {
                            self.stream.word = Word::None;
                            self.stream.position = Position::Command;
                            Case::Body
                        }
                        _ if self.stream.substitution => return None,
                        _ => Case::Pattern,
                    };
                }
                if let Some((found, same)) = arithmetic {
                    self.stream.position = Position::Command;
                    self.stream.comment = false;
                    if after == Some(b')') {
                        self.at += 1;
                    } else if same {
                        // Without a second `)`, bash reads `((` as two
                        // subshells, and the text again as commands. The
                        // reader goes on in the outer subshell where commands
                        // read that text the same.
                        self.stack.push(Frame::Subshell);
                    } else {
                        self.comments.truncate(found);
                        return None;
                    }
                }
            }
        }
        Some(())
    }

    fn open(&mut self, frame: Frame, length: usize) {
        self.stack.push(frame);
        self.at += length;
    }

    /// Skips a quotation that ends at the next `'`.
    fn quote(&mut self, opening: usize, escapes: bool) {
        self.at += opening;
        while self.at < self.limit {
            self.at += 1;
            match self.bytes[self.at - 1] {
                b'\'' => return,
                b'\\' if escapes => self.at += 1,
                _ => {}
            }
        }
    }

    /// Reads `$` and what it opens. `quote` allows `$'…'`.
    fn dollar(&mut self, quote: bool) -> Option<()> {
        let joined = self.line_break(self.at + 2);
        if self.peek(1) == Some(b'\\') && joined > 0 {
            let after = self.peek(2 + joined);
            if splits(Some(b'$'), after) {
                return None;
            }
        }
        let bracket = Frame::Group {
            close: b']',
            depth: 1,
            expands: true,
            arithmetic: None,
        };
        match (self.peek(1), self.peek(2)) {
            (Some(b'('), _) => self.substitution(),
            // bash 5.3 reads commands here, and older shells do not.
            (Some(b'{'), Some(b' ' | b'\t' | b'\n' | b'\r' | b'|' | b'\\')) => return None,
            (Some(b'{'), _) => self.open(Frame::Parameter, 2),
            (Some(b'['), _) => self.open(bracket, 2),
            (Some(b'\''), _) if quote => self.quote(2, true),
            _ => self.at += 1,
        }
        Some(())
    }

    /// Reads `$(`, `<(`, or `>(`.
    fn substitution(&mut self) {
        if self.peek(2) == Some(b'(') {
            // After a second `(`, bash ends the text where the parentheses
            // balance, and decides only when it runs whether the text is
            // arithmetic.
            return self.open(
                Frame::Group {
                    close: b')',
                    depth: 2,
                    expands: true,
                    arithmetic: None,
                },
                3,
            );
        }
        let inner = Stream::new(self.here_documents.len(), true, false);
        let outer = replace(&mut self.stream, inner);
        self.open(Frame::Substitution(outer), 2);
    }

    /// The offset up to which a backquoted command can be read as it is
    /// written, and the offset of the backquote that closes it.
    fn backquoted(&self) -> (usize, usize) {
        let mut end = self.at + 1;
        let mut stop = None;
        while end < self.limit && self.bytes[end] != b'`' {
            if self.bytes[end] == b'\\' {
                // The shell removes these backslashes before it reads the
                // command, so the text after one is read differently.
                if matches!(
                    self.bytes[..self.limit].get(end + 1),
                    Some(b'\\' | b'`' | b'$' | b'"')
                ) {
                    stop.get_or_insert(end);
                }
                end += 1;
            }
            end += 1;
        }
        let end = end.min(self.limit);
        (stop.unwrap_or(end), end)
    }

    /// Reads a backquoted command. It ends at the next backquote that no
    /// backslash escapes, whatever is open there, so no other one is open.
    fn backquote(&mut self) {
        let (stop, end) = self.backquoted();
        let here_documents = self.here_documents.len();
        let inner = Stream::new(here_documents, false, true);
        let outer = replace(&mut self.stream, inner);
        self.open(
            Frame::Backquote {
                outer,
                resume: (end + 1).min(self.limit),
                here_documents,
            },
            1,
        );
        self.limit = stop;
    }

    fn command(&mut self) -> Option<()> {
        let byte = self.bytes[self.at];
        match byte {
            b' ' | b'\t' => {
                self.end_word();
                self.stream.comment = true;
                self.at += 1;
            }
            b'\n' | b'\r' => {
                self.end_word();
                self.at += self.line_break(self.at);
                self.position(Position::Command);
                self.stream.comment = true;
                return self.bodies();
            }
            b'#' if self.stream.word.is_none() => self.hash(),
            b'\\' => match self.line_break(self.at + 1) {
                0 => {
                    self.quoted();
                    self.at += 2;
                }
                // A backslash joins two lines, inside a word too.
                mut length => {
                    let before = self.at.checked_sub(1).map(|at| self.bytes[at]);
                    while length > 0 {
                        self.at += 1 + length;
                        let again = self.peek(0) == Some(b'\\');
                        length = usize::from(again) * self.line_break(self.at + 1);
                    }
                    if splits(before, self.peek(0)) {
                        return None;
                    }
                }
            },
            b'\'' => {
                self.quoted();
                self.quote(1, false);
            }
            b'"' => {
                self.quoted();
                self.open(Frame::Double, 1);
            }
            b'`' => {
                self.quoted();
                self.backquote();
            }
            b'$' => {
                self.quoted();
                if self.peek(1) == Some(b'$') {
                    self.at += 2;
                } else {
                    return self.dollar(true);
                }
            }
            b'(' => self.parenthesis(),
            b')' => return self.close(),
            b'|' if self.stream.pattern => self.word(byte),
            b';' | b'&' | b'|' => self.control(byte),
            b'<' | b'>' => return self.redirection(byte),
            _ => self.word(byte),
        }
        Some(())
    }

    /// Sets what the next word can be. Neither `[[ ]]` nor an array holds a
    /// reserved word or an assignment.
    fn position(&mut self, position: Position) {
        let words = self.stream.condition || matches!(self.stack.last(), Some(Frame::Array(_)));
        self.stream.position = if words { Position::Argument } else { position };
    }

    /// Whether the next word can be a reserved one.
    fn reserves(&self) -> bool {
        matches!(
            self.stream.position,
            Position::Command | Position::Coproc | Position::Time | Position::TimeOption
        )
    }

    /// What the word after a redirection operator is.
    fn target(&self) -> Position {
        if self.reserves() || self.stream.position == Position::Redirected {
            Position::Target
        } else {
            Position::Argument
        }
    }

    /// Whether the next word can be an assignment.
    fn assigns(&self) -> bool {
        let position = self.stream.position;
        (self.reserves() || matches!(position, Position::Assigned | Position::Redirected))
            && !matches!(
                self.stack.last(),
                Some(Frame::Case(Case::Start | Case::Pattern))
            )
    }

    /// Marks the current word as holding something quoted or expanded.
    fn quoted(&mut self) {
        if !matches!(self.stream.word, Word::Assignment) {
            self.stream.word = Word::Other;
        }
    }

    fn word(&mut self, byte: u8) {
        let name = byte.is_ascii_alphanumeric() || byte == b'_';
        let equals = || byte == b'=' || (byte == b'+' && self.peek(1) == Some(b'='));
        self.stream.word = match self.stream.word {
            Word::None if name && !byte.is_ascii_digit() => Word::Name(self.at),
            Word::Name(start) if name => Word::Name(start),
            Word::Name(_) if equals() && self.assigns() => Word::Assignment,
            // bash reads the subscript of an assignment up to its `]`, blanks
            // and all: after a name, and at the start of a word of an array.
            word @ (Word::None | Word::Name(_))
                if byte == b'['
                    && if word.is_none() {
                        matches!(self.stack.last(), Some(Frame::Array(_)))
                    } else {
                        self.assigns()
                    } =>
            {
                self.stack.push(Frame::Group {
                    close: b']',
                    depth: 1,
                    expands: true,
                    arithmetic: None,
                });
                Word::Subscript
            }
            Word::None => Word::Plain(self.at),
            Word::Name(start) => Word::Plain(start),
            Word::Subscript if equals() => Word::Assignment,
            Word::Subscript => Word::Other,
            word => word,
        };
        self.at += 1;
    }

    /// Ends the word before this offset, and tells from it what the next word
    /// can be.
    fn end_word(&mut self) {
        let word = replace(&mut self.stream.word, Word::None);
        let bytes = self.bytes;
        let text = match word {
            Word::None => return,
            Word::Name(start) | Word::Plain(start) => &bytes[start..self.at],
            _ => &[],
        };
        // Such a word holds a backslash only where it joins two lines.
        let joined: Vec<u8>;
        let text = if text.contains(&b'\\') {
            joined = text
                .iter()
                .copied()
                .filter(|byte| !b"\\\r\n".contains(byte))
                .collect();
            &joined
        } else {
            text
        };
        self.stream.pattern = false;
        if self.stream.condition {
            match text {
                b"]]" => {
                    self.stream.condition = false;
                    self.stream.position = Position::Command;
                }
                // bash reads a group only after `=~`, and zsh after the others
                // too.
                b"=~" | b"=" | b"==" | b"!=" => self.stream.pattern = true,
                _ => {}
            }
            return;
        }
        let position = self.stream.position;
        let reserved = self.reserves();
        match self.stack.last_mut() {
            Some(Frame::Array(_)) => return,
            Some(Frame::Case(case @ Case::Subject)) => return *case = Case::In,
            Some(Frame::Case(case @ Case::In)) => {
                if text == b"in" {
                    *case = Case::Start;
                    self.stream.position = Position::Command;
                } else {
                    self.stack.pop();
                }
                return;
            }
            // Among patterns, only `esac` is reserved, and only at the start
            // of a list of them.
            Some(Frame::Case(case @ (Case::Start | Case::Pattern))) => {
                *case = Case::Pattern;
                self.stream.position = Position::Argument;
                if text == b"esac" && reserved {
                    self.stack.pop();
                    self.stream.position = Position::Command;
                }
                return;
            }
            _ => {}
        }
        self.stream.position = match text {
            b"if" | b"then" | b"else" | b"elif" | b"fi" | b"do" | b"done" | b"while" | b"until"
            | b"{" | b"}" | b"!"
                if reserved =>
            {
                Position::Command
            }
            b"time" if reserved => Position::Time,
            b"-p" if position == Position::Time => Position::TimeOption,
            b"--" if matches!(position, Position::Time | Position::TimeOption) => Position::Command,
            b"coproc" if reserved => Position::Coproc,
            b"function" if reserved => Position::Function,
            b"for" | b"select" if reserved => Position::For,
            b"case" if reserved => {
                self.stack.push(Frame::Case(Case::Subject));
                Position::Argument
            }
            b"esac" if reserved => {
                if matches!(self.stack.last(), Some(Frame::Case(Case::Body))) {
                    self.stack.pop();
                }
                Position::Command
            }
            b"[[" if reserved => {
                self.stream.condition = true;
                Position::Argument
            }
            b"do" if position == Position::Loop => Position::Command,
            _ => match position {
                Position::Function | Position::Coproc => Position::Command,
                Position::Target => Position::Redirected,
                Position::For => Position::Loop,
                Position::Command
                | Position::Time
                | Position::TimeOption
                | Position::Assigned
                | Position::Redirected
                    if matches!(word, Word::Assignment) =>
                {
                    Position::Assigned
                }
                _ => Position::Argument,
            },
        };
    }

    /// Reads `#` where a word could start. Right after `(`, `)`, `|`, `<`, or
    /// `>`, bash reads a comment too, and zsh can read a part of a pattern, as
    /// in `(#b)` and `(a|b)#`. There the line is skipped, and not returned.
    fn hash(&mut self) {
        let end = self.line_end(self.at);
        if self.stream.comment {
            // A comment that a backquote ends is left out, so that every
            // returned one runs to the end of its line.
            let whole = end < self.limit || self.limit == self.bytes.len();
            if whole && !(self.at == 0 && self.bytes.starts_with(b"#!")) {
                self.comments.push(Comment::line(self.at..end, 1));
            }
        }
        self.at = end;
    }

    fn parenthesis(&mut self) {
        let group = |expands, arithmetic| Frame::Group {
            close: b')',
            depth: 1,
            expands,
            arithmetic,
        };
        let word = !self.stream.word.is_none();
        if self.stream.pattern {
            self.quoted();
            return self.open(group(false, None), 1);
        }
        if word && self.bytes[self.at - 1] == b'=' {
            self.stream.word = Word::None;
            self.stream.comment = false;
            self.open(Frame::Array(self.stream.position), 1);
            return self.stream.position = Position::Argument;
        }
        let patterns = matches!(
            self.stack.last(),
            Some(Frame::Case(Case::Start | Case::Pattern))
        );
        // A subshell, the group of a condition, or the list of words that zsh
        // takes after the name of a loop.
        let commands = self.reserves()
            || self.stream.condition
            || matches!(self.stream.position, Position::For | Position::Loop);
        if word || patterns || !commands {
            let mut close = self.at + 1;
            while matches!(self.bytes[..self.limit].get(close), Some(b' ' | b'\t')) {
                close += 1;
            }
            if patterns || self.bytes[..self.limit].get(close) != Some(&b')') {
                // A group of a pattern, where no command can start: `@(a|b)`,
                // `*(.)`, and `(a|b)` of zsh. A list of `case` patterns can
                // start with one.
                if let (false, Some(Frame::Case(case @ Case::Start))) =
                    (word, self.stack.last_mut())
                {
                    *case = Case::Leading;
                }
                self.quoted();
                return self.open(group(!word, None), 1);
            }
            // The parentheses of a function definition.
            self.end_word();
            self.at = close;
        } else if self.peek(1) == Some(b'(')
            && (self.reserves() || self.stream.position == Position::For)
        {
            return self.open(group(true, Some((self.comments.len(), true))), 2);
        } else {
            self.stack.push(Frame::Subshell);
        }
        self.at += 1;
        self.position(Position::Command);
        self.stream.comment = false;
    }

    fn close(&mut self) -> Option<()> {
        self.end_word();
        self.at += 1;
        // A `)` that is not the end of a pattern closes what is around the
        // `case`, which bash rejects.
        let mut rejected = false;
        loop {
            match self.stack.last_mut() {
                Some(Frame::Case(case @ (Case::Start | Case::Pattern))) if !rejected => {
                    *case = Case::Body;
                    break;
                }
                Some(Frame::Case(_)) => {
                    self.stack.pop();
                    rejected = true;
                }
                Some(Frame::Subshell) => {
                    self.stack.pop();
                    break;
                }
                // The word of the assignment goes on after an array.
                Some(&mut Frame::Array(position)) => {
                    self.stack.pop();
                    self.stream.position = position;
                    self.stream.word = Word::Assignment;
                    return Some(());
                }
                Some(Frame::Substitution(_)) => {
                    // bash reads a here-document that is still pending from
                    // the next line on, and the rest of this line before it.
                    if rejected || self.here_documents.len() > self.stream.here_documents {
                        return None;
                    }
                    if let Some(Frame::Substitution(outer)) = self.stack.pop() {
                        self.stream = outer;
                    }
                    return Some(());
                }
                _ => break,
            }
        }
        self.position(Position::Command);
        self.stream.comment = false;
        Some(())
    }

    fn control(&mut self, byte: u8) {
        self.end_word();
        // The pattern after `=~` can start right behind it.
        if byte == b'|' && self.stream.pattern {
            return self.word(byte);
        }
        let next = self.peek(1);
        let mut position = Position::Command;
        self.stream.comment = byte != b'|';
        self.at += match (byte, next) {
            // The end of a `case` clause: `;;`, `;&`, `;;&`, and `;|` of zsh.
            (b';', Some(b';' | b'&' | b'|')) => {
                if let Some(Frame::Case(case @ Case::Body)) = self.stack.last_mut() {
                    *case = Case::Start;
                }
                2 + usize::from(next == Some(b';') && self.peek(2) == Some(b'&'))
            }
            (b'&', Some(b'>')) => {
                position = self.target();
                self.stream.comment = false;
                2 + usize::from(self.peek(2) == Some(b'>'))
            }
            (b'&', Some(b'&')) | (b'|', Some(b'|' | b'&')) => 2,
            _ => 1,
        };
        // `esac` after `|` is a pattern.
        if byte == b'|'
            && let Some(Frame::Case(case @ (Case::Start | Case::Pattern))) = self.stack.last_mut()
        {
            *case = Case::Pattern;
            position = Position::Argument;
        }
        self.position(position);
    }

    fn redirection(&mut self, byte: u8) -> Option<()> {
        let next = self.peek(1);
        if next == Some(b'(') {
            self.quoted();
            self.substitution();
            return Some(());
        }
        // `<1-9>` and `<->` of zsh match numbers.
        let digits = |from: usize| {
            let digits = self.bytes[from..self.limit].iter();
            from + digits.take_while(|byte| byte.is_ascii_digit()).count()
        };
        let dash = digits(self.at + 1);
        if byte == b'<' && self.bytes[..self.limit].get(dash) == Some(&b'-') {
            let close = digits(dash + 1);
            if self.bytes[..self.limit].get(close) == Some(&b'>') {
                self.quoted();
                self.at = close + 1;
                return Some(());
            }
        }
        self.end_word();
        self.stream.position = self.target();
        self.stream.comment = false;
        self.at += 1;
        match (byte, next) {
            (b'<', Some(b'<')) if self.peek(1) == Some(b'<') => self.at += 2,
            (b'<', Some(b'<')) => {
                self.at += 1;
                return self.here_document();
            }
            (b'<', Some(b'&' | b'>')) | (b'>', Some(b'>' | b'&' | b'|')) => self.at += 1,
            _ => {}
        }
        Some(())
    }
}
