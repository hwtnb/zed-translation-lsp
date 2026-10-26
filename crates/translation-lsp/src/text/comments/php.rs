use std::ops::Range;

use super::{Comment, Kind};

/// What the text at the current place belongs to.
enum Frame {
    /// PHP code after `<?php`, `<?=`, or `<?`.
    Code,
    /// The code of `{$ ... }` or `${ ... }` in a string, with the braces open
    /// in it.
    Interpolation(usize),
    /// A string in double quotes or backquotes.
    Quoted(u8),
    Heredoc {
        label: Range<usize>,
        nowdoc: bool,
    },
}

/// Reads PHP as its lexer does, in a single pass: the text outside the PHP
/// tags is not read, and in code, `#`, `//`, and `/*` start a comment outside
/// strings, here-documents, and attributes (`#[`). What is open is kept on a
/// stack, so that a string inside `{$ ... }` inside a string is read too.
pub(super) fn parse(text: &str) -> Vec<Comment> {
    let bytes = text.as_bytes();
    let mut comments = Vec::new();
    // Empty outside the PHP tags.
    let mut stack = Vec::new();
    let mut index = 0;
    while index < bytes.len() {
        let start = index;
        match stack.last_mut() {
            None => {
                let Some(open) = find(bytes, index, b"<?") else {
                    break;
                };
                index = open + 2;
                let rest = &bytes[index..];
                if rest.len() >= 3
                    && rest[..3].eq_ignore_ascii_case(b"php")
                    && rest.get(3).is_none_or(u8::is_ascii_whitespace)
                {
                    index += 3;
                }
                stack.push(Frame::Code);
            }
            Some(Frame::Code | Frame::Interpolation(_)) => match bytes[index] {
                b'?' if bytes.get(index + 1) == Some(&b'>') => {
                    // PHP leaves every string it is in.
                    stack.clear();
                    index += 2;
                }
                // An attribute.
                b'#' if bytes.get(index + 1) == Some(&b'[') => index += 2,
                b'#' | b'/' if bytes[index] == b'#' || bytes.get(index + 1) == Some(&b'/') => {
                    let opening = if bytes[index] == b'#' { 1 } else { 2 };
                    // `?>` ends the comment as well as the line.
                    index += opening;
                    while index < bytes.len()
                        && !matches!(bytes[index], b'\n' | b'\r')
                        && !bytes[index..].starts_with(b"?>")
                    {
                        index += 1;
                    }
                    comments.push(Comment::line(start..index, opening as u8));
                }
                b'/' if bytes.get(index + 1) == Some(&b'*') => {
                    // An unclosed comment hides the rest of the file.
                    let Some(end) = find(bytes, index + 2, b"*/") else {
                        break;
                    };
                    let documentation = bytes[index..].starts_with(b"/**")
                        && bytes.get(index + 3).is_some_and(u8::is_ascii_whitespace);
                    index = end + 2;
                    comments.push(Comment {
                        range: start..index,
                        kind: Kind::Block {
                            opening: if documentation { 3 } else { 2 },
                            closed: true,
                        },
                    });
                }
                // An unclosed string hides the rest of the file.
                b'\'' => {
                    index += 1;
                    loop {
                        match bytes.get(index) {
                            None => return comments,
                            Some(b'\\') => index += 2,
                            Some(b'\'') => break,
                            Some(_) => index += 1,
                        }
                    }
                    index += 1;
                }
                quote @ (b'"' | b'`') => {
                    stack.push(Frame::Quoted(quote));
                    index += 1;
                }
                b'<' if bytes[index..].starts_with(b"<<<") => match heredoc(bytes, index + 3) {
                    Some((frame, body)) => {
                        stack.push(frame);
                        index = body;
                    }
                    None => index += 3,
                },
                b'{' => {
                    if let Some(Frame::Interpolation(braces)) = stack.last_mut() {
                        *braces += 1;
                    }
                    index += 1;
                }
                b'}' => {
                    if let Some(Frame::Interpolation(braces)) = stack.last_mut() {
                        *braces -= 1;
                        if *braces == 0 {
                            stack.pop();
                        }
                    }
                    index += 1;
                }
                byte if label_start(byte) => {
                    let end = label_end(bytes, index);
                    let word = &bytes[index..end];
                    // The rest of the file is data, not PHP.
                    if word.eq_ignore_ascii_case(b"__halt_compiler")
                        && !bytes[..index].ends_with(b"$")
                        && !after_arrow(bytes, &comments, index)
                    {
                        break;
                    }
                    index = end;
                }
                _ => index += 1,
            },
            Some(&mut Frame::Quoted(quote)) => match bytes[index] {
                b'\\' => index += 2,
                byte if byte == quote => {
                    stack.pop();
                    index += 1;
                }
                _ => index = interpolation(bytes, index, &mut stack),
            },
            Some(Frame::Heredoc { label, nowdoc }) => {
                // The end is the label at the start of a line, after blanks,
                // with no more of a label after it.
                if matches!(bytes.get(index.wrapping_sub(1)), Some(b'\n' | b'\r')) {
                    let mut at = index;
                    while matches!(bytes.get(at), Some(b' ' | b'\t')) {
                        at += 1;
                    }
                    let end = at + label.len();
                    if bytes[at..].starts_with(&bytes[label.clone()])
                        && !bytes.get(end).is_some_and(|byte| label_byte(*byte))
                    {
                        stack.pop();
                        index = end;
                        continue;
                    }
                }
                if *nowdoc {
                    index = next_line(bytes, index);
                } else {
                    match bytes[index] {
                        // A backslash does not hide a line break from the end.
                        b'\\' if !matches!(bytes.get(index + 1), Some(b'\r' | b'\n')) => {
                            index += 2;
                        }
                        _ => index = interpolation(bytes, index, &mut stack),
                    }
                }
            }
        }
    }
    comments
}

/// Whether `->` precedes `at`, also with blanks and comments between, so that
/// a word there names a property.
fn after_arrow(bytes: &[u8], comments: &[Comment], mut at: usize) -> bool {
    let mut comments = comments.iter().rev();
    loop {
        while at > 0 && bytes[at - 1].is_ascii_whitespace() {
            at -= 1;
        }
        match comments.next() {
            Some(comment) if comment.range.end == at => at = comment.range.start,
            _ => return bytes[..at].ends_with(b"->"),
        }
    }
}

/// Reads one character of a string or here-document that has variables in
/// it, and returns where the next one starts.
fn interpolation(bytes: &[u8], index: usize, stack: &mut Vec<Frame>) -> usize {
    if bytes[index..].starts_with(b"{$") {
        stack.push(Frame::Interpolation(1));
        index + 1
    } else if bytes[index..].starts_with(b"${") {
        stack.push(Frame::Interpolation(1));
        index + 2
    } else {
        index + 1
    }
}

/// Reads the opening of a here-document after `<<<`, and returns it with
/// where its body starts. `None` when the text is no such opening.
fn heredoc(bytes: &[u8], mut index: usize) -> Option<(Frame, usize)> {
    while matches!(bytes.get(index), Some(b' ' | b'\t')) {
        index += 1;
    }
    let quote = bytes
        .get(index)
        .copied()
        .filter(|byte| matches!(byte, b'"' | b'\''));
    index += usize::from(quote.is_some());
    if !bytes.get(index).copied().is_some_and(label_start) {
        return None;
    }
    let label = index..label_end(bytes, index);
    index = label.end;
    if let Some(quote) = quote {
        if bytes.get(index) != Some(&quote) {
            return None;
        }
        index += 1;
    }
    let body = match bytes.get(index) {
        Some(b'\r') if bytes.get(index + 1) == Some(&b'\n') => index + 2,
        Some(b'\r' | b'\n') => index + 1,
        _ => return None,
    };
    let nowdoc = quote == Some(b'\'');
    Some((Frame::Heredoc { label, nowdoc }, body))
}

fn label_start(byte: u8) -> bool {
    byte.is_ascii_alphabetic() || byte == b'_' || byte >= 0x80
}

fn label_byte(byte: u8) -> bool {
    label_start(byte) || byte.is_ascii_digit()
}

fn label_end(bytes: &[u8], mut index: usize) -> usize {
    while bytes.get(index).is_some_and(|byte| label_byte(*byte)) {
        index += 1;
    }
    index
}

/// Where the line after the one of `index` starts.
fn next_line(bytes: &[u8], index: usize) -> usize {
    bytes[index..]
        .iter()
        .position(|byte| matches!(byte, b'\r' | b'\n'))
        .map_or(bytes.len(), |length| index + length + 1)
}

fn find(bytes: &[u8], from: usize, needle: &[u8]) -> Option<usize> {
    bytes[from..]
        .windows(needle.len())
        .position(|window| window == needle)
        .map(|at| from + at)
}
