use std::{borrow::Cow, collections::HashSet, mem, ops::Range};

use super::{Comment, Kind};

/// What Ruby's lexer expects next, as far as it decides whether `/`, `%`,
/// `?`, and `<<` start a literal.
#[derive(Clone, Copy, PartialEq)]
enum State {
    /// The start of an expression.
    Begin,
    /// After `return`, `break`, or `next`: as at the start of an expression,
    /// but `if` there is a modifier.
    Middle,
    /// After a value, where these are operators.
    End,
    /// After the name of a method that may take arguments without
    /// parentheses: a blank before and none after makes them a literal.
    Argument,
    /// After `.`, `&.`, or `::`, where a word names a method.
    Dot,
    /// After `def`, `alias`, or `undef`, where an operator names a method.
    Name,
    /// After `class`, where `<<` opens the singleton class.
    Class,
}

struct Heredoc {
    label: Range<usize>,
    /// `<<-` and `<<~` allow blanks before the label that ends them.
    indented: bool,
    interpolated: bool,
}

/// What the text at the current place belongs to.
enum Frame {
    /// Ruby code. Inside `#{ ... }` of a literal, it holds the braces open in
    /// it; at the top it holds `None`.
    Code(Option<usize>),
    /// A string, symbol, regular expression, command, or `%` literal. A pair
    /// of brackets nests; other delimiters do not.
    Literal {
        open: Option<u8>,
        close: u8,
        depth: usize,
        interpolated: bool,
        regexp: bool,
        /// Where the contents start.
        contents: usize,
        /// Whether no `#{ ... }` has been read in it.
        plain: bool,
    },
    /// The lines of a here-document, read after the line that opens it.
    Body { heredoc: Heredoc, line_start: bool },
}

/// Reads Ruby as its lexer does, in a single pass: a `#` outside literals
/// starts a comment, and so does `=begin` at the start of a line. Ruby tells
/// some literals from operators by what precedes them, including whether a
/// word is a local variable. The reader follows the scopes of local variables
/// as Ruby does: a `def`, `class`, or `module` starts one that sees nothing
/// outside it, and a block or lambda one that sees the variables outside it.
/// A word is a local variable after it has been assigned, also as one of
/// several targets; named as a parameter or as the variable of `for` or
/// `rescue`; bound in a pattern; or named by a group of a regular expression
/// before `=~`.
pub(super) fn parse(text: &str) -> Vec<Comment> {
    let (text, removed) = line_feeds(text);
    let text = text.as_ref();
    let mut reader = Reader {
        bytes: text.as_bytes(),
        text,
        at: 0,
        stack: vec![Frame::Code(None)],
        state: State::Begin,
        blank: false,
        line_start: true,
        pending: Vec::new(),
        visible: HashSet::new(),
        scopes: vec![Scope {
            declared: Vec::new(),
            outside: None,
        }],
        openers: Vec::new(),
        braces: Vec::new(),
        condition: false,
        endless: false,
        header: None,
        def_parameters: None,
        parameters: Vec::new(),
        loop_variables: false,
        statement: true,
        targets: None,
        rescue: false,
        declare_next: false,
        pattern: None,
        pinned: false,
        captures: None,
        previous: Previous::Other,
        comments: Vec::new(),
    };
    // A shebang is not a comment to translate.
    if text.starts_with("#!") {
        reader.at = line_end(reader.bytes, 0);
    }
    while reader.at < reader.bytes.len() && reader.step() {}
    let mut comments = reader.comments;
    // Each CR removed lies before the LF at its place. The comments come in
    // order, and so do the places asked for here, so the count of CRs before
    // each goes on from the last.
    let mut before = 0;
    let mut original = |at: usize| {
        while removed.get(before).is_some_and(|place| *place < at) {
            before += 1;
        }
        at + before
    };
    if !removed.is_empty() {
        for comment in &mut comments {
            let start = original(comment.range.start);
            if let Kind::Contents(ranges) = &mut comment.kind {
                for range in ranges {
                    *range = original(range.start)..original(range.end);
                }
            }
            comment.range = start..original(comment.range.end);
        }
    }
    comments
}

/// Ruby reads CRLF as LF, and a CR alone as a blank; but the editor ends a
/// line at a CR alone, as Ruby that runs never does. Returns `text` with LF
/// for both, and the places in it where a CR was removed, in order.
fn line_feeds(text: &str) -> (Cow<'_, str>, Vec<usize>) {
    if !text.contains('\r') {
        return (Cow::Borrowed(text), Vec::new());
    }
    let bytes = text.as_bytes();
    let mut source = Vec::with_capacity(bytes.len());
    let mut removed = Vec::new();
    for (index, &byte) in bytes.iter().enumerate() {
        match byte {
            b'\r' if bytes.get(index + 1) == Some(&b'\n') => removed.push(source.len()),
            b'\r' => source.push(b'\n'),
            _ => source.push(byte),
        }
    }
    let source = String::from_utf8(source).expect("CR and LF are both ASCII");
    (Cow::Owned(source), removed)
}

/// What to undo when a scope of local variables closes.
struct Scope<'a> {
    /// The variables that the scope added to those visible.
    declared: Vec<&'a str>,
    /// For a scope that hides the variables outside it, as a `def` does, those
    /// variables.
    outside: Option<HashSet<&'a str>>,
}

/// What a list of parameters belongs to.
#[derive(Clone, Copy, PartialEq)]
enum Owner {
    Def,
    /// A block, between `|`s.
    Block,
    /// A lambda, from `->` to its body.
    Lambda,
}

/// A list of parameters being read. A word in it names a parameter where a
/// name can stand, and is code in a default value.
struct Parameters {
    owner: Owner,
    /// Whether a name can come next: at the start of the list, after `,`, and
    /// in parentheses that take a parameter apart.
    name_next: bool,
    /// The parentheses open around names: those of the list and those that
    /// take a parameter apart.
    groups: usize,
    /// The brackets open in a default value.
    nested: usize,
}

impl Parameters {
    fn new(owner: Owner) -> Self {
        Self {
            owner,
            name_next: true,
            groups: 0,
            nested: 0,
        }
    }
}

/// The targets of an assignment that may be being read at the start of a
/// statement, as in `a, (b, *c) = list`.
#[derive(Default)]
struct Targets<'a> {
    /// The local variables among them.
    names: Vec<&'a str>,
    /// Whether a target has just been read, so that `,`, `)`, `.`, or `=` can
    /// follow.
    after: bool,
    /// Whether that target is the last of `names`, until `.`, `::`, or `[`
    /// makes it a receiver.
    named: bool,
    /// The brackets open in an index, as in `a[i], b = list`.
    index: usize,
}

/// The token before the current one, where it decides what `|` starts.
#[derive(Clone, Copy, PartialEq)]
enum Previous {
    /// `{` or `do`, after which `|` opens the parameters of a block.
    BlockStart,
    Other,
}

struct Reader<'a> {
    text: &'a str,
    bytes: &'a [u8],
    at: usize,
    stack: Vec<Frame>,
    state: State,
    /// Whether a blank precedes the current token.
    blank: bool,
    line_start: bool,
    /// The here-documents opened on the current line, read after it.
    pending: Vec<Heredoc>,
    /// The local variables visible at the current place, in one set, so that
    /// a word is looked up in the same time at any depth of scopes.
    visible: HashSet<&'a str>,
    scopes: Vec<Scope<'a>>,
    /// Whether each keyword open until its `end` started a scope.
    openers: Vec<bool>,
    /// Whether each `{` open in the code started a block.
    braces: Vec<bool>,
    /// Whether a `do` on this line belongs to `while`, `until`, or `for`.
    condition: bool,
    /// Whether the current `def` has `=` instead of an `end`, and so ends
    /// with its line.
    endless: bool,
    /// The parentheses open in the parameters of a `def`, until its line ends.
    header: Option<usize>,
    /// Where the parameters of the current `def` lie in `parameters`.
    def_parameters: Option<usize>,
    /// The lists of parameters being read, the innermost last, as a block in
    /// a default value opens one inside another.
    parameters: Vec<Parameters>,
    /// Whether the variables of a `for` loop are being read, up to `in`.
    loop_variables: bool,
    /// Whether the next token starts a statement, and so can start the
    /// targets of an assignment.
    statement: bool,
    /// The targets of an assignment, while what has been read can be them.
    targets: Option<Targets<'a>>,
    /// Whether a `rescue` clause is being read, where `=>` names its variable.
    rescue: bool,
    /// Whether the next word is a variable declared, as after `rescue =>`.
    declare_next: bool,
    /// The brackets open in a pattern after `in`, where a word binds a
    /// variable; `None` outside one.
    pattern: Option<usize>,
    /// Whether the next word is pinned with `^` in a pattern, and so binds none.
    pinned: bool,
    /// The contents of a regular expression just read without interpolation,
    /// and whether it has the `x` option. `=~` after it declares the names of
    /// its groups as variables.
    captures: Option<(Range<usize>, bool)>,
    previous: Previous,
    comments: Vec<Comment>,
}

fn word_start(byte: u8) -> bool {
    byte.is_ascii_alphabetic() || byte == b'_' || byte >= 0x80
}

fn word_byte(byte: u8) -> bool {
    word_start(byte) || byte.is_ascii_digit()
}

/// Where the line of `from` ends, before LF.
fn line_end(bytes: &[u8], from: usize) -> usize {
    bytes[from..]
        .iter()
        .position(|byte| *byte == b'\n')
        .map_or(bytes.len(), |length| from + length)
}

/// Where the line after the one of `from` starts.
fn next_line(bytes: &[u8], from: usize) -> usize {
    bytes[from..]
        .iter()
        .position(|byte| *byte == b'\n')
        .map_or(bytes.len(), |length| from + length + 1)
}

/// Whether a line ends at `at`.
fn ends_line(bytes: &[u8], at: usize) -> bool {
    matches!(bytes.get(at), None | Some(b'\n'))
}

/// Whether `word` can name a local variable: it starts with a lowercase
/// letter, `_`, or a character without case.
fn local_name(word: &str) -> bool {
    word.chars().next().is_some_and(|first| {
        first == '_' || first.is_ascii_lowercase() || !first.is_ascii() && !first.is_uppercase()
    })
}

/// The names of the groups of a regular expression, as Ruby finds them: not
/// in a character class, in a comment group `(?#...)`, or, in extended mode,
/// after `#` to the end of the line. Extended mode is `extended` at first;
/// `(?x)` and `(?-x)` turn it on and off to the end of the group they are in,
/// and `(?x: ...)` and `(?-x: ...)` inside their group.
fn group_names(pattern: &str, mut extended: bool) -> Vec<&str> {
    let bytes = pattern.as_bytes();
    let mut names = Vec::new();
    // The mode outside each group open, to go back to at its end.
    let mut groups = Vec::new();
    // The brackets of character classes open, in which `(` and `#` are
    // characters.
    let mut classes = 0;
    let mut index = 0;
    while index < bytes.len() {
        let rest = &bytes[index..];
        match bytes[index] {
            b'\\' => index += 1,
            b'[' => classes += 1,
            b']' if classes > 0 => classes -= 1,
            _ if classes > 0 => {}
            b'#' if extended => {
                index += rest
                    .iter()
                    .position(|byte| *byte == b'\n')
                    .unwrap_or(rest.len());
            }
            b'(' if rest.starts_with(b"(?#") => {
                index += rest
                    .iter()
                    .position(|byte| *byte == b')')
                    .unwrap_or(rest.len());
            }
            b'(' => {
                groups.push(extended);
                let options = rest
                    .iter()
                    .skip(2)
                    .take_while(|byte| b"imx-".contains(byte))
                    .count();
                let after = rest.get(2 + options).copied();
                if rest.starts_with(b"(?") && options > 0 && matches!(after, Some(b')' | b':')) {
                    let options = &rest[2..2 + options];
                    let off = options.iter().position(|byte| *byte == b'-');
                    let (on, off) = options.split_at(off.unwrap_or(options.len()));
                    if on.contains(&b'x') {
                        extended = true;
                    }
                    if off.contains(&b'x') {
                        extended = false;
                    }
                    // `(?x)` opens no group: its options apply to the rest
                    // of the group around it.
                    if after == Some(b')') {
                        groups.pop();
                    }
                    index += 2 + options.len();
                } else if let Some(open) = [&b"(?<"[..], b"(?'"]
                    .into_iter()
                    .find(|open| rest.starts_with(open))
                {
                    let close = if open[2] == b'<' { b'>' } else { b'\'' };
                    let length = rest[3..]
                        .iter()
                        .take_while(|byte| word_byte(**byte))
                        .count();
                    if length > 0 && rest.get(3 + length) == Some(&close) {
                        names.push(&pattern[index + 3..index + 3 + length]);
                    }
                }
            }
            b')' => {
                if let Some(outside) = groups.pop() {
                    extended = outside;
                }
            }
            _ => {}
        }
        index += 1;
    }
    names
}

/// Keywords after which an expression starts, besides those that `word` reads
/// on their own.
const KEYWORDS_BEFORE_VALUE: [&str; 8] = [
    "and", "else", "elsif", "ensure", "or", "when", "BEGIN", "END",
];
/// Keywords that are values.
const KEYWORDS_OF_VALUE: [&str; 9] = [
    "self",
    "nil",
    "true",
    "false",
    "redo",
    "retry",
    "__FILE__",
    "__LINE__",
    "__ENCODING__",
];
/// Operators that a symbol or the name of a method can be, longest first.
const OPERATORS: [&[u8]; 28] = [
    b"[]=", b"<=>", b"===", b"[]", b"**", b"!=", b"!~", b"+@", b"-@", b"<<", b"<=", b">>", b">=",
    b"==", b"=~", b"!", b"+", b"-", b"*", b"/", b"%", b"<", b">", b"~", b"&", b"|", b"^", b"`",
];

impl<'a> Reader<'a> {
    /// Reads one token or one character, and returns `false` where the rest
    /// of the text is not Ruby.
    fn step(&mut self) -> bool {
        match self.stack.last_mut().unwrap() {
            Frame::Code(_) => self.code(),
            Frame::Literal { .. } => {
                self.literal();
                true
            }
            Frame::Body { .. } => {
                self.body();
                true
            }
        }
    }

    fn byte(&self, offset: usize) -> Option<u8> {
        self.bytes.get(self.at + offset).copied()
    }

    /// Takes LF, and starts the here-documents that the line opened.
    fn line_feed(&mut self) {
        self.at += 1;
        for heredoc in self.pending.drain(..).rev() {
            self.stack.push(Frame::Body {
                heredoc,
                line_start: true,
            });
        }
    }

    fn code(&mut self) -> bool {
        let start = self.at;
        let line_start = mem::replace(&mut self.line_start, false);
        let byte = self.bytes[start];
        let blank = mem::replace(&mut self.blank, false);
        if line_start && matches!(self.stack.as_slice(), [Frame::Code(None)]) {
            let end = line_end(self.bytes, start);
            if &self.text[start..end] == "__END__" {
                return false;
            }
        }
        if line_start && self.bytes[start..].starts_with(b"=begin") && self.begin_block(start) {
            return true;
        }
        let state = self.state;
        // Blanks and comments leave the token before them in place.
        let previous = self.previous;
        let token = !matches!(byte, b' ' | b'\t' | b'\x0b' | b'\x0c' | b'\n' | b'#')
            && !(byte == b'\\' && self.byte(1) == Some(b'\n'));
        let (statement, declare, pinned, captures) = if token {
            self.previous = Previous::Other;
            (
                mem::take(&mut self.statement),
                mem::take(&mut self.declare_next),
                mem::take(&mut self.pinned),
                self.captures.take(),
            )
        } else {
            (false, false, false, None)
        };
        if statement {
            self.targets = Some(Targets::default());
        }
        if matches!(byte, b'(' | b'[' | b'{' | b')' | b']' | b'}' | b',' | b';') {
            self.punctuation(byte);
        }
        match byte {
            b' ' | b'\t' | b'\x0b' | b'\x0c' => {
                self.at += 1;
                self.blank = true;
            }
            b'\\' if self.byte(1) == Some(b'\n') => {
                // The statement goes on, but here-documents, `=begin`, and
                // `__END__` start on the next line as after any other.
                self.at += 1;
                self.line_feed();
                self.blank = true;
                self.line_start = true;
            }
            b'\n' => {
                self.line_feed();
                self.line_start = true;
                // After `,`, the targets of an assignment go on.
                if self.targets.as_ref().is_none_or(|targets| targets.after) {
                    self.targets = None;
                    self.statement = true;
                }
                self.end_of_statement();
                if matches!(self.state, State::End | State::Argument | State::Middle) {
                    self.state = State::Begin;
                }
            }
            b'#' => {
                let end = line_end(self.bytes, start);
                self.comments.push(Comment::line(start..end, 1));
                self.at = end;
            }
            b'\'' => self.open(None, b'\'', false, false, 1),
            b'"' => self.open(None, b'"', true, false, 1),
            b'`' if matches!(self.state, State::Name | State::Dot) => {
                self.at += 1;
                self.method_named(state);
            }
            b'`' => self.open(None, b'`', true, false, 1),
            b'/' if self.literal_starts(blank) => self.open(None, b'/', true, true, 1),
            b'%' if self.literal_starts(blank) && self.percent() => {}
            b'?' if self.character() => {}
            b'<' if self.byte(1) == Some(b'<') && self.heredoc(blank) => {}
            b':' if self.byte(1) == Some(b':') => {
                self.at += 2;
                self.state = State::Dot;
            }
            b':' => self.symbol(),
            b'$' => self.global(),
            b'@' => {
                self.at += 1 + usize::from(self.byte(1) == Some(b'@'));
                self.word_end();
                self.state = State::End;
            }
            byte if byte.is_ascii_digit() => {
                self.number();
                self.state = State::End;
            }
            byte if word_start(byte) => self.word(declare, pinned),
            b'{' => {
                if let Some(Frame::Code(Some(braces))) = self.stack.last_mut() {
                    *braces += 1;
                }
                // After a value, a method, or the parameters of a lambda, `{`
                // starts a block; elsewhere a hash.
                let block =
                    matches!(self.state, State::End | State::Argument) || self.lambda_body();
                if block {
                    self.open_block();
                }
                self.braces.push(block);
                self.at += 1;
                self.state = State::Begin;
                self.previous = Previous::BlockStart;
            }
            b'}' => {
                self.at += 1;
                match self.stack.last_mut() {
                    Some(Frame::Code(Some(1))) => {
                        self.stack.pop();
                    }
                    frame => {
                        if let Some(Frame::Code(Some(braces))) = frame {
                            *braces -= 1;
                        }
                        if self.braces.pop() == Some(true) {
                            self.close_scope();
                        }
                    }
                }
                self.state = State::End;
            }
            b'(' | b'[' => {
                if let Some(depth) = &mut self.header {
                    *depth += usize::from(byte == b'(');
                }
                self.at += 1;
                self.state = State::Begin;
            }
            b')' | b']' => {
                self.at += 1;
                self.state = State::End;
                if let Some(depth) = &mut self.header
                    && byte == b')'
                {
                    *depth = depth.saturating_sub(1);
                    // The body of the method starts after its parameters.
                    if *depth == 0 {
                        self.end_header();
                        self.state = State::Begin;
                        self.endless |= self.assignment_follows();
                    }
                }
            }
            b'.' if self.byte(1) == Some(b'.') => {
                self.at += 2 + usize::from(self.byte(2) == Some(b'.'));
                self.state = State::Begin;
            }
            b'.' => {
                self.at += 1;
                self.state = State::Dot;
            }
            b'&' if self.byte(1) == Some(b'.') => {
                self.at += 2;
                self.state = State::Dot;
            }
            b'|' => {
                if previous == Previous::BlockStart {
                    self.parameters.push(Parameters::new(Owner::Block));
                } else if self
                    .parameters
                    .last()
                    .is_some_and(|list| list.owner == Owner::Block)
                {
                    self.parameters.pop();
                    self.statement = true;
                }
                self.at += 1;
                self.state = State::Begin;
            }
            b';' => {
                self.end_header();
                self.end_of_statement();
                self.at += 1;
                self.state = State::Begin;
                self.statement = true;
            }
            // An operator after `def`, `.`, or `::` names a method.
            _ if matches!(self.state, State::Name | State::Dot) => {
                let length = OPERATORS
                    .iter()
                    .find(|operator| self.bytes[start..].starts_with(operator))
                    .map_or(1, |operator| operator.len());
                self.at += length;
                self.method_named(state);
            }
            b'-' if self.byte(1) == Some(b'>') => {
                self.at += 2;
                // The parameters belong to the scope of the body, and a
                // default value sees those before it.
                self.open_scope(false);
                self.parameters.push(Parameters::new(Owner::Lambda));
                self.state = State::Begin;
            }
            b'=' if self.byte(1) == Some(b'>') => {
                self.at += 2;
                self.declare_next = self.rescue;
                self.state = State::Begin;
            }
            b'=' if self.byte(1) == Some(b'~') => {
                self.at += 2;
                if let Some((contents, extended)) = captures {
                    let text = self.text;
                    for name in group_names(&text[contents], extended) {
                        self.declare(name);
                    }
                }
                self.state = State::Begin;
            }
            _ => {
                self.at += 1;
                self.pinned = byte == b'^' && self.pattern.is_some();
                self.state = State::Begin;
            }
        }
        if token && self.targets.is_some() {
            self.follow_targets(start, state);
        }
        // Parentheses around an expression hold statements, as in
        // `(a, b = list)`; those of a call or of targets do not.
        if byte == b'(' && matches!(state, State::Begin | State::Middle) && self.targets.is_none() {
            self.statement = true;
        }
        true
    }

    /// Follows brackets and separators in a list of parameters or a pattern.
    fn punctuation(&mut self, byte: u8) {
        if let Some(depth) = &mut self.pattern {
            match byte {
                b'(' | b'[' | b'{' => *depth += 1,
                b')' | b']' | b'}' => *depth = depth.saturating_sub(1),
                b';' => self.pattern = None,
                _ => {}
            }
        }
        // The body of a lambda ends its parameters, which `open_block` does.
        if byte == b'{' && self.lambda_body() {
            return;
        }
        if let Some(list) = self.parameters.last_mut() {
            match byte {
                // Parentheses that take a parameter apart hold names.
                b'(' if list.name_next => list.groups += 1,
                b'(' | b'[' | b'{' => list.nested += 1,
                b')' | b']' | b'}' if list.nested > 0 => list.nested -= 1,
                b')' => list.groups = list.groups.saturating_sub(1),
                b',' if list.nested == 0 => list.name_next = true,
                // Variables of the block alone follow `;`.
                b';' if list.owner != Owner::Def => list.name_next = true,
                _ => {}
            }
        }
    }

    /// Follows the targets of an assignment by the token just read, from
    /// `start`, and declares their variables at `=`.
    fn follow_targets(&mut self, start: usize, state: State) {
        let text = self.text;
        let token = &text[start..self.at];
        let next = self.byte(0);
        let mut assigned = Vec::new();
        let Some(targets) = &mut self.targets else {
            return;
        };
        let first = token.as_bytes()[0];
        // What an index holds is not a target.
        if targets.index > 0 {
            match token {
                "[" => targets.index += 1,
                "]" => {
                    targets.index -= 1;
                    targets.after = targets.index == 0;
                }
                _ => {}
            }
            return;
        }
        let goes_on = match token {
            "," if targets.after => {
                targets.after = false;
                true
            }
            "*" | "(" => !targets.after,
            ")" => targets.after,
            "." | "&." | "::" | "[" if targets.after => {
                if mem::take(&mut targets.named) {
                    targets.names.pop();
                }
                targets.after = false;
                targets.index = usize::from(token == "[");
                true
            }
            "=" if targets.after && !matches!(next, Some(b'=' | b'~' | b'>')) => {
                assigned = mem::take(&mut targets.names);
                false
            }
            // The name of a method after `.` or `::`.
            _ if state == State::Dot && word_start(first) => {
                targets.after = true;
                targets.named = false;
                true
            }
            _ if !targets.after && (first == b'@' || first == b'$') => {
                targets.after = true;
                targets.named = false;
                true
            }
            _ if !targets.after && word_start(first) && !token.ends_with(':') => {
                targets.named = local_name(token);
                if targets.named {
                    targets.names.push(token);
                }
                targets.after = true;
                true
            }
            _ => false,
        };
        if !goes_on {
            self.targets = None;
        }
        for name in assigned {
            self.declare(name);
        }
    }

    /// Reads `=begin` at the start of a line to its `=end` line, and returns
    /// whether it was one.
    fn begin_block(&mut self, start: usize) -> bool {
        let after = start + 6;
        if !matches!(self.bytes.get(after), None | Some(b' ' | b'\t' | b'\n')) {
            return false;
        }
        let contents = next_line(self.bytes, start);
        let mut line = contents;
        loop {
            if line >= self.bytes.len() {
                // An unclosed block hides the rest of the file.
                self.at = self.bytes.len();
                self.stack.truncate(1);
                return true;
            }
            if self.bytes[line..].starts_with(b"=end")
                && matches!(self.bytes.get(line + 4), None | Some(b' ' | b'\t' | b'\n'))
            {
                break;
            }
            line = next_line(self.bytes, line);
        }
        let end = line_end(self.bytes, line);
        self.comments.push(Comment {
            range: start..end,
            kind: Kind::Contents(vec![Range {
                start: contents.min(line),
                end: line,
            }]),
        });
        self.at = end;
        self.line_start = false;
        true
    }

    /// Whether `/` or `%` here starts a literal rather than an operator.
    fn literal_starts(&self, blank: bool) -> bool {
        match self.state {
            State::Begin | State::Middle | State::Class => true,
            State::Argument => {
                blank
                    && !matches!(
                        self.byte(1),
                        None | Some(b' ' | b'\t' | b'\n' | b'\x0b' | b'\x0c' | b'=')
                    )
            }
            State::End | State::Dot | State::Name => false,
        }
    }

    fn open(
        &mut self,
        open: Option<u8>,
        close: u8,
        interpolated: bool,
        regexp: bool,
        length: usize,
    ) {
        self.at += length;
        self.stack.push(Frame::Literal {
            open,
            close,
            depth: 1,
            interpolated,
            regexp,
            contents: self.at,
            plain: true,
        });
    }

    /// Reads `%` and the opening of a literal after it, and returns whether
    /// there was one.
    fn percent(&mut self) -> bool {
        let (kind, delimiter, length) = match self.byte(1) {
            Some(byte) if byte.is_ascii_alphabetic() => (byte, self.byte(2), 3),
            byte => (b'Q', byte, 2),
        };
        if !b"qQwWiIrsx".contains(&kind) {
            return false;
        }
        let Some(delimiter) =
            delimiter.filter(|byte| !byte.is_ascii_alphanumeric() && *byte < 0x80)
        else {
            return false;
        };
        let close = match delimiter {
            b'(' => b')',
            b'[' => b']',
            b'{' => b'}',
            b'<' => b'>',
            other => other,
        };
        let open = (close != delimiter).then_some(delimiter);
        self.open(open, close, b"QWIrx".contains(&kind), kind == b'r', length);
        true
    }

    /// Reads a character literal such as `?a` or `?#`, and returns whether
    /// there was one.
    fn character(&mut self) -> bool {
        if self.state == State::End {
            return false;
        }
        match self.byte(1) {
            None | Some(b' ' | b'\t' | b'\n' | b'\x0b' | b'\x0c') => false,
            Some(b'\\') => {
                self.at += 2;
                // `\M-`, `\C-`, and `\c` take the character after them.
                while self.bytes[self.at..].starts_with(b"M-")
                    || self.bytes[self.at..].starts_with(b"C-")
                    || self.byte(0) == Some(b'c')
                {
                    self.at += if self.byte(0) == Some(b'c') { 1 } else { 2 };
                    if self.byte(0) == Some(b'\\') {
                        self.at += 1;
                    }
                }
                if self.byte(0) == Some(b'u') && self.byte(1) == Some(b'{') {
                    self.at = self.bytes[self.at..]
                        .iter()
                        .position(|byte| *byte == b'}')
                        .map_or(self.bytes.len(), |length| self.at + length + 1);
                } else {
                    self.skip_character();
                }
                self.state = State::End;
                true
            }
            Some(byte) if word_byte(byte) && byte < 0x80 && self.byte(2).is_some_and(word_byte) => {
                false
            }
            Some(_) => {
                self.at += 1;
                self.skip_character();
                self.state = State::End;
                true
            }
        }
    }

    /// Takes one character, of one or more bytes.
    fn skip_character(&mut self) {
        let length = self.text[self.at..]
            .chars()
            .next()
            .map_or(0, char::len_utf8);
        self.at += length;
    }

    /// Reads the opening of a here-document after `<<`, and returns whether
    /// there was one. Its lines are read after the current line.
    fn heredoc(&mut self, blank: bool) -> bool {
        if matches!(
            self.state,
            State::End | State::Dot | State::Class | State::Name
        ) || self.state == State::Argument && !blank
        {
            return false;
        }
        let mut at = self.at + 2;
        let indented = matches!(self.bytes.get(at), Some(b'-' | b'~'));
        at += usize::from(indented);
        let (label, interpolated, end) = match self.bytes.get(at) {
            Some(&quote @ (b'\'' | b'"' | b'`')) => {
                let start = at + 1;
                let Some(length) = self.bytes[start..]
                    .iter()
                    .position(|byte| *byte == quote || *byte == b'\n')
                else {
                    return false;
                };
                if self.bytes[start + length] != quote {
                    return false;
                }
                (start..start + length, quote != b'\'', start + length + 1)
            }
            Some(&byte) if word_byte(byte) => {
                let mut end = at;
                while self.bytes.get(end).copied().is_some_and(word_byte) {
                    end += 1;
                }
                (at..end, true, end)
            }
            _ => return false,
        };
        self.pending.push(Heredoc {
            label,
            indented,
            interpolated,
        });
        self.at = end;
        self.state = State::End;
        true
    }

    fn symbol(&mut self) {
        self.at += 1;
        match self.byte(0) {
            Some(b'"') => self.open(None, b'"', true, false, 1),
            Some(b'\'') => self.open(None, b'\'', false, false, 1),
            Some(byte) if word_start(byte) => {
                self.word_end();
                if matches!(self.byte(0), Some(b'?' | b'!' | b'='))
                    && !matches!(self.byte(1), Some(b'=' | b'~' | b'>'))
                {
                    self.at += 1;
                }
                self.state = State::End;
            }
            Some(b'@') => {
                self.at += 1 + usize::from(self.byte(1) == Some(b'@'));
                self.word_end();
                self.state = State::End;
            }
            Some(b'$') => self.global(),
            _ => match OPERATORS
                .iter()
                .find(|operator| self.bytes[self.at..].starts_with(operator))
            {
                Some(operator) => {
                    self.at += operator.len();
                    self.state = State::End;
                }
                None => self.state = State::Begin,
            },
        }
    }

    /// Reads a global variable, such as `$x`, `$1`, `$-w`, or `$'`.
    fn global(&mut self) {
        self.at += 1;
        match self.byte(0) {
            Some(byte) if word_byte(byte) => self.word_end(),
            Some(b'-') => {
                self.at += 1;
                self.skip_character();
            }
            Some(byte) if b"~*$?!@/\\;,.=:<>\"&`'+".contains(&byte) => self.at += 1,
            _ => {}
        }
        self.state = State::End;
    }

    fn word_end(&mut self) {
        while self.byte(0).is_some_and(word_byte) {
            self.at += 1;
        }
    }

    fn number(&mut self) {
        loop {
            self.word_end();
            if self.byte(0) == Some(b'.') && self.byte(1).is_some_and(|byte| byte.is_ascii_digit())
            {
                self.at += 1;
            } else {
                break;
            }
        }
    }

    /// Reads a word: a keyword, or a name. `declare` tells that the name is
    /// a variable declared, and `pinned` that it is pinned with `^`.
    fn word(&mut self, declare: bool, pinned: bool) {
        let start = self.at;
        self.word_end();
        if matches!(self.byte(0), Some(b'?' | b'!'))
            && (self.byte(1) != Some(b'=') || matches!(self.byte(2), Some(b'=' | b'~' | b'>')))
        {
            self.at += 1;
        }
        let text = self.text;
        let word = &text[start..self.at];
        let state = self.state;
        // A label, as in `{name: value}`.
        if self.byte(0) == Some(b':')
            && self.byte(1) != Some(b':')
            && !matches!(state, State::Dot | State::Name)
        {
            self.at += 1;
            // A keyword parameter, or a key of a pattern without a value.
            if self.parameter_name() || self.pattern.is_some() && !pinned && self.label_alone() {
                self.declare(word);
            }
            self.state = State::Begin;
            return;
        }
        if matches!(state, State::Dot) {
            self.state = State::Argument;
            return;
        }
        if state == State::Name {
            // The name of a setter, as in `def name=(value)`.
            if self.byte(0) == Some(b'=') && !matches!(self.byte(1), Some(b'=' | b'~' | b'>')) {
                self.at += 1;
            }
            if self.byte(0) == Some(b'.') {
                // `def self.name`: the name follows.
                self.at += 1;
            } else {
                self.method_named(state);
            }
            return;
        }
        match word {
            "def" => {
                self.open_scope(true);
                self.openers.push(true);
                self.header = Some(0);
                self.state = State::Name;
            }
            "alias" | "undef" => self.state = State::Name,
            "class" | "module" => {
                self.open_scope(true);
                self.openers.push(true);
                self.state = if word == "class" {
                    State::Class
                } else {
                    State::Begin
                };
            }
            "begin" | "case" => {
                self.openers.push(false);
                self.state = State::Begin;
            }
            // At the start of an expression these take an `end`; after one,
            // they are modifiers.
            "if" | "unless" | "while" | "until" => {
                if matches!(state, State::Begin | State::Class) {
                    self.openers.push(false);
                    self.condition = matches!(word, "while" | "until");
                }
                // A guard ends a pattern.
                self.pattern = None;
                self.state = State::Begin;
            }
            "for" => {
                self.openers.push(false);
                self.condition = true;
                self.loop_variables = true;
                self.state = State::Begin;
            }
            "in" => {
                // After `for`, `in` ends its variables; elsewhere a pattern
                // follows.
                if !mem::take(&mut self.loop_variables) {
                    self.pattern = Some(0);
                }
                self.state = State::Begin;
            }
            "do" => {
                // `while x do` takes one `end`, and starts no block.
                if !mem::take(&mut self.condition) {
                    self.open_block();
                    self.openers.push(true);
                }
                self.state = State::Begin;
                self.previous = Previous::BlockStart;
            }
            "end" => {
                if self.openers.pop() == Some(true) {
                    self.close_scope();
                }
                self.state = State::End;
            }
            "rescue" => {
                self.rescue = true;
                self.state = State::Begin;
            }
            "then" => {
                self.pattern = None;
                self.rescue = false;
                self.state = State::Begin;
            }
            "return" | "break" | "next" => self.state = State::Middle,
            "yield" | "super" | "defined?" | "not" => self.state = State::Argument,
            _ if KEYWORDS_BEFORE_VALUE.contains(&word) => self.state = State::Begin,
            _ if KEYWORDS_OF_VALUE.contains(&word) => self.state = State::End,
            _ => {
                // The name of a parameter takes its place in the list first.
                if self.parameter_name()
                    || declare
                    || self.loop_variables
                    || self.pattern.is_some() && !pinned
                    || self.assigned()
                {
                    self.declare(word);
                }
                self.state = if self.visible.contains(word) {
                    State::End
                } else {
                    State::Argument
                };
            }
        }
        if matches!(word, "begin" | "do" | "else" | "ensure" | "then") {
            self.statement = true;
        }
    }

    /// Whether the word just read names a parameter, and if so, takes the
    /// place of a name in the list.
    fn parameter_name(&mut self) -> bool {
        self.parameters
            .last_mut()
            .is_some_and(|list| mem::take(&mut list.name_next))
    }

    /// Whether the label just read stands alone, as `name:` does in a pattern
    /// such as `{name:}`.
    fn label_alone(&self) -> bool {
        let mut at = self.at;
        while matches!(self.bytes.get(at), Some(b' ' | b'\t')) {
            at += 1;
        }
        matches!(
            self.bytes.get(at),
            None | Some(b',' | b'}' | b')' | b']' | b'\n' | b';')
        ) || self.bytes[at..].starts_with(b"then")
    }

    /// Whether a `{` or `do` here starts the body of a lambda, after its
    /// parameters.
    fn lambda_body(&self) -> bool {
        self.parameters
            .last()
            .is_some_and(|list| list.owner == Owner::Lambda && list.groups == 0 && list.nested == 0)
    }

    /// Starts the scope of a block. The body of a lambda goes on in the
    /// scope that its parameters started.
    fn open_block(&mut self) {
        if self.lambda_body() {
            self.parameters.pop();
        } else {
            self.open_scope(false);
        }
        self.statement = true;
    }

    /// Makes `word` a local variable of the current scope, if it can name one.
    fn declare(&mut self, word: &'a str) {
        if local_name(word) && self.visible.insert(word) {
            self.scopes.last_mut().unwrap().declared.push(word);
        }
    }

    /// Takes the name of a method read in `state`. After `def`, the
    /// parameters follow, or `=` and a body without `end`.
    fn method_named(&mut self, state: State) {
        self.state = State::Argument;
        if state != State::Name || self.header.is_none() {
            return;
        }
        if self.byte(0) != Some(b'(') && self.assignment_follows() {
            self.endless = true;
        } else {
            self.def_parameters = Some(self.parameters.len());
            self.parameters.push(Parameters::new(Owner::Def));
        }
    }

    /// Ends the parameters of a `def`, with any list left open in them.
    fn end_header(&mut self) {
        self.header = None;
        if let Some(index) = self.def_parameters.take() {
            self.parameters.truncate(index);
        }
    }

    fn open_scope(&mut self, closed: bool) {
        let outside = closed.then(|| mem::take(&mut self.visible));
        self.scopes.push(Scope {
            declared: Vec::new(),
            outside,
        });
    }

    fn close_scope(&mut self) {
        // The outermost scope stays, also after an `end` too many.
        if self.scopes.len() > 1 {
            let scope = self.scopes.pop().unwrap();
            match scope.outside {
                Some(outside) => self.visible = outside,
                None => {
                    for word in scope.declared {
                        self.visible.remove(word);
                    }
                }
            }
        }
    }

    /// Ends what a line break or `;` ends: the parameters of a `def`, a
    /// pattern, a `rescue` clause, the condition of a loop, and a `def` without
    /// `end`.
    fn end_of_statement(&mut self) {
        if self.header == Some(0) {
            self.end_header();
        }
        if self.pattern == Some(0) {
            self.pattern = None;
        }
        self.rescue = false;
        self.condition = false;
        if mem::take(&mut self.endless) && self.openers.pop() == Some(true) {
            self.close_scope();
        }
    }

    /// Whether `=` follows, after blanks, as in `def name = value`.
    fn assignment_follows(&self) -> bool {
        let mut at = self.at;
        while matches!(self.bytes.get(at), Some(b' ' | b'\t')) {
            at += 1;
        }
        self.bytes.get(at) == Some(&b'=')
            && !matches!(self.bytes.get(at + 1), Some(b'=' | b'~' | b'>'))
    }

    /// Whether the word just read is assigned to, as in `x = 1` or `x += 1`.
    fn assigned(&self) -> bool {
        let mut at = self.at;
        while matches!(self.bytes.get(at), Some(b' ' | b'\t')) {
            at += 1;
        }
        let rest = &self.bytes[at..];
        let operator = [
            &b"**="[..],
            b"||=",
            b"&&=",
            b"<<=",
            b">>=",
            b"+=",
            b"-=",
            b"*=",
            b"/=",
            b"%=",
            b"|=",
            b"&=",
            b"^=",
        ]
        .iter()
        .any(|operator| rest.starts_with(operator));
        operator || rest.starts_with(b"=") && !matches!(rest.get(1), Some(b'=' | b'~' | b'>'))
    }

    fn literal(&mut self) {
        let Some(Frame::Literal {
            open,
            close,
            depth,
            interpolated,
            regexp,
            contents,
            plain,
        }) = self.stack.last_mut()
        else {
            unreachable!();
        };
        let byte = self.bytes[self.at];
        // A backslash escapes, unless it is the delimiter, as in `%\ ... \`.
        if byte == *close {
            *depth -= 1;
            self.at += 1;
            if *depth == 0 {
                let regexp = *regexp;
                let captures = (*plain).then(|| *contents..self.at - 1);
                self.stack.pop();
                if regexp {
                    let options = self.at;
                    while self.byte(0).is_some_and(|byte| byte.is_ascii_alphabetic()) {
                        self.at += 1;
                    }
                    let extended = self.bytes[options..self.at].contains(&b'x');
                    self.captures = captures.map(|contents| (contents, extended));
                }
                self.state = State::End;
            }
        } else if byte == b'\\' {
            self.at += 2;
        } else if Some(byte) == *open {
            *depth += 1;
            self.at += 1;
        } else if *interpolated && self.bytes[self.at..].starts_with(b"#{") {
            *plain = false;
            self.at += 2;
            self.stack.push(Frame::Code(Some(1)));
            self.state = State::Begin;
        } else if byte == b'\n' {
            self.line_feed();
        } else {
            self.at += 1;
        }
        self.at = self.at.min(self.bytes.len());
    }

    fn body(&mut self) {
        let Some(Frame::Body {
            heredoc,
            line_start,
        }) = self.stack.last_mut()
        else {
            unreachable!();
        };
        if mem::replace(line_start, false) {
            let mut at = self.at;
            if heredoc.indented {
                while matches!(self.bytes.get(at), Some(b' ' | b'\t')) {
                    at += 1;
                }
            }
            let label = &self.bytes[heredoc.label.clone()];
            if self.bytes[at..].starts_with(label) && ends_line(self.bytes, at + label.len()) {
                self.at = next_line(self.bytes, at);
                self.stack.pop();
                if matches!(self.stack.last(), Some(Frame::Code(_))) {
                    self.line_start = true;
                }
                return;
            }
        }
        if !heredoc.interpolated {
            self.at = next_line(self.bytes, self.at);
            *line_start = true;
            return;
        }
        let byte = self.bytes[self.at];
        if byte == b'\\' {
            self.at += 2;
        } else if self.bytes[self.at..].starts_with(b"#{") {
            self.at += 2;
            self.stack.push(Frame::Code(Some(1)));
            self.state = State::Begin;
        } else if byte == b'\n' {
            self.at += 1;
            *line_start = true;
        } else {
            self.at += 1;
        }
        self.at = self.at.min(self.bytes.len());
    }
}
