mod javascript;
mod rust;
#[cfg(test)]
use javascript::source_type;
use std::{
    cell::Cell,
    ops::Range,
    panic::{self, AssertUnwindSafe},
    sync::Once,
    thread,
    time::{Duration, Instant},
};

use lsp_types::Uri;
use rust_lexer::DocStyle;

use crate::language::Language;

mod c_family;
mod csharp;
mod css;
mod data;
mod dockerfile;
mod go;
mod html;
mod php;
mod powershell;
mod python;
mod ruby;
mod shell;
mod syntax;

#[derive(Clone, Copy, PartialEq, Eq)]
enum LineKind {
    Rust(Option<DocStyle>),
    JavaScript(u8),
    CSharp(u8),
    CFamily(&'static str),
    Python,
    /// Ends at CR or LF, after an opening of this many bytes.
    Plain(u8),
}

enum Kind {
    Line(LineKind),
    Block {
        opening: u8,
        closed: bool,
    },
    /// The text of a comment or a docstring lies in these ranges, without the
    /// marks around and between them.
    Contents(Vec<Range<usize>>),
}

struct Comment {
    range: Range<usize>,
    kind: Kind,
}

impl Comment {
    fn line(range: Range<usize>, opening: u8) -> Self {
        Self {
            range,
            kind: Kind::Line(LineKind::Plain(opening)),
        }
    }
}

/// Store only source byte spans and syntax kinds. Bodies and LSP coordinates are
/// produced for the requested group, rather than for every comment in a document.
#[derive(Default)]
pub(crate) struct Comments(Vec<Comment>);

/// Why reading a document gave no comments.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Failure {
    TimedOut,
    /// Where the panic was raised, as `file:line`.
    Panicked(Option<String>),
}

thread_local! {
    static READING: Cell<bool> = const { Cell::new(false) };
    static PANICKED_AT: Cell<Option<String>> = const { Cell::new(None) };
}

/// The stack of the thread that reads comments. Oxc parses nested code by
/// recursion, with no limit of its own on the depth, so a deeply nested
/// script would overflow the stack of the thread that handles requests,
/// which is 1 MiB on Windows. Only the part of the stack in use takes memory.
const READING_STACK: usize = 256 << 20;

/// A panic message can quote the text being read. While a thread reads
/// comments, keep only the place of its panic; report every other panic as before.
fn keep_reading_panics_quiet() {
    static HOOK: Once = Once::new();
    HOOK.call_once(|| {
        let report = panic::take_hook();
        panic::set_hook(Box::new(move |panic| {
            if READING.get() {
                PANICKED_AT.set(
                    panic
                        .location()
                        .map(|place| format!("{}:{}", place.file(), place.line())),
                );
            } else {
                report(panic);
            }
        }));
    });
}

impl Comments {
    /// The limit starts here. It stops the Tree-sitter languages; no other
    /// reader can be stopped part way.
    pub fn parse(
        language: Language,
        uri: &Uri,
        text: &str,
        limit: Duration,
    ) -> Result<Self, Failure> {
        let deadline = Instant::now() + limit;
        keep_reading_panics_quiet();
        // Reads, and gives the place of a panic if the reading raised one.
        let read = move || {
            READING.set(true);
            let read = panic::catch_unwind(AssertUnwindSafe(|| {
                Self::read(language, uri, text, deadline)
            }));
            READING.set(false);
            read.map_err(|_| PANICKED_AT.take())
        };
        let read = thread::scope(|scope| {
            thread::Builder::new()
                .name("comments".into())
                .stack_size(READING_STACK)
                .spawn_scoped(scope, read)
                .map(|reader| reader.join().unwrap_or(Err(None)))
        })
        // Where no such thread can start, read with the stack there is.
        .unwrap_or_else(|_| read());
        match read {
            Ok(Some(comments)) => Ok(Self(comments)),
            Ok(None) => Err(Failure::TimedOut),
            Err(place) => Err(Failure::Panicked(place)),
        }
    }

    fn read(language: Language, uri: &Uri, text: &str, deadline: Instant) -> Option<Vec<Comment>> {
        match language {
            Language::Python => python::parse(text, deadline),
            Language::C | Language::Cpp => c_family::parse(language, text, deadline),
            Language::CSharp => csharp::parse(text, deadline),
            Language::Rust => Some(rust::parse(text)),
            Language::Go => Some(go::parse(text)),
            Language::Html => Some(html::parse(text)),
            Language::Css => Some(css::parse(text)),
            Language::Php => Some(php::parse(text)),
            Language::Ruby => Some(ruby::parse(text)),
            Language::Json
            | Language::Jsonc
            | Language::Yaml
            | Language::Toml
            | Language::DockerCompose => Some(data::parse(language, text)),
            Language::ShellScript | Language::PowerShell | Language::Dockerfile => {
                // Read these as if a byte order mark were not there.
                let mark = if text.starts_with('\u{feff}') { 3 } else { 0 };
                let text = &text[mark..];
                let mut comments = match language {
                    Language::ShellScript => shell::parse(text),
                    Language::PowerShell => powershell::parse(text),
                    _ => dockerfile::parse(text),
                };
                for comment in &mut comments {
                    comment.range = comment.range.start + mark..comment.range.end + mark;
                }
                Some(comments)
            }
            Language::JavaScript | Language::TypeScript | Language::TypeScriptReact => {
                Some(javascript::parse(language, uri, text))
            }
            Language::Markdown | Language::PlainText => Some(Vec::new()),
        }
    }

    /// A group is a range of comment indices, not a copy of the source or body.
    pub fn at(&self, text: &str, offset: usize) -> Option<Range<usize>> {
        let index = self
            .0
            .partition_point(|comment| comment.range.start <= offset)
            .checked_sub(1)?;
        let comment = &self.0[index];
        let line_end = matches!(comment.kind, Kind::Line(kind)
            if offset == comment.range.end
                && text[offset..].chars().next().is_none_or(|character| line_break(character, kind)));
        if offset > comment.range.end
            || (offset == comment.range.end && !line_end)
            || matches!(comment.kind, Kind::Block { closed: false, .. })
        {
            return None;
        }
        let mut group = index..index + 1;
        while group.start > 0 && self.adjacent(text, group.start - 1, group.start) {
            group.start -= 1;
        }
        while group.end < self.0.len() && self.adjacent(text, group.end - 1, group.end) {
            group.end += 1;
        }
        Some(group)
    }

    pub fn range(&self, group: Range<usize>) -> Range<usize> {
        self.0[group.start].range.start..self.0[group.end - 1].range.end
    }

    pub fn body(&self, text: &str, group: Range<usize>) -> String {
        let mut body = String::new();
        for index in group.clone() {
            let comment = &self.0[index];
            if index > group.start {
                let gap = &text[self.0[index - 1].range.end..comment.range.start];
                // Adjacent groups contain exactly one physical newline, followed
                // by the next comment's indent. Keep that original newline.
                body.push_str(&gap[..newline_len(gap).unwrap()]);
            }
            let opening = match comment.kind {
                Kind::Line(LineKind::Rust(style)) => {
                    if style.is_some() {
                        3
                    } else {
                        2
                    }
                }
                Kind::Contents(ref contents) => {
                    for range in contents {
                        body.push_str(&text[range.clone()]);
                    }
                    continue;
                }
                Kind::Line(LineKind::Python) => 1,
                Kind::Line(LineKind::CFamily(prefix)) => prefix.len() as u8,
                Kind::Line(
                    LineKind::JavaScript(opening)
                    | LineKind::CSharp(opening)
                    | LineKind::Plain(opening),
                )
                | Kind::Block { opening, .. } => opening,
            } as usize;
            let closing = if matches!(comment.kind, Kind::Block { .. }) {
                2
            } else {
                0
            };
            let end = comment.range.end - closing;
            let content = &text[(comment.range.start + opening).min(end)..end];
            body.push_str(content.strip_prefix(' ').unwrap_or(content));
        }
        body
    }

    fn adjacent(&self, text: &str, left: usize, right: usize) -> bool {
        let (Kind::Line(left_kind), Kind::Line(right_kind)) =
            (&self.0[left].kind, &self.0[right].kind)
        else {
            return false;
        };
        if left_kind != right_kind {
            return false;
        }
        let indent = |comment: &Comment| {
            let prefix = &text[..comment.range.start];
            let line_start = prefix
                .rfind(|character| line_break(character, *left_kind))
                .map_or(0, |offset| {
                    offset + text[offset..].chars().next().unwrap().len_utf8()
                });
            &text[line_start..comment.range.start]
        };
        let left_indent = indent(&self.0[left]);
        let right_indent = indent(&self.0[right]);
        if left_indent != right_indent || !left_indent.chars().all(char::is_whitespace) {
            return false;
        }
        let gap = &text[self.0[left].range.end..self.0[right].range.start];
        newline_len(gap).is_some_and(|length| &gap[length..] == right_indent)
    }
}

fn line_break(character: char, kind: LineKind) -> bool {
    matches!(character, '\r' | '\n')
        || matches!(kind, LineKind::JavaScript(_) | LineKind::CSharp(_))
            && matches!(character, '\u{2028}' | '\u{2029}')
        || matches!(kind, LineKind::CSharp(_)) && character == '\u{85}'
}

fn newline_len(text: &str) -> Option<usize> {
    if text.starts_with("\r\n") {
        Some(2)
    } else {
        text.chars()
            .next()
            .filter(|&character| {
                matches!(character, '\r' | '\n' | '\u{85}' | '\u{2028}' | '\u{2029}')
            })
            .map(char::len_utf8)
    }
}

#[cfg(test)]
mod tests;
