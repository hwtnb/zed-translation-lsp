use std::ops::Range;

use super::{Comment, Kind};

/// Lua decides comments by tokens alone: outside strings, `--` starts a
/// comment. After `--`, a long bracket such as `[[` or `[==[` starts a long
/// comment that runs to the closing bracket of the same level, `]]` or `]==]`;
/// otherwise the comment runs to the end of the line. `---` lines are
/// documentation comments, as for LuaCATS. As `lua` does with a file, a byte
/// order mark and a first line that starts with `#` are skipped.
pub(super) fn parse(text: &str) -> Vec<Comment> {
    let bytes = text.as_bytes();
    let mut comments = Vec::new();
    let mut index = if text.starts_with('\u{feff}') { 3 } else { 0 };
    // Only a line feed ends that line, as `lua` reads it.
    if bytes.get(index) == Some(&b'#') {
        index = bytes[index..]
            .iter()
            .position(|byte| *byte == b'\n')
            .map_or(bytes.len(), |length| index + length);
    }
    while index < bytes.len() {
        let start = index;
        match bytes[index] {
            b'-' if bytes.get(index + 1) == Some(&b'-') => {
                if let Some(level) = long_bracket(bytes, index + 2) {
                    let opening = index + 2 + level + 2;
                    // An unclosed comment hides the rest of the file.
                    let Some(closing) = long_end(bytes, opening, level) else {
                        break;
                    };
                    index = closing + level + 2;
                    // A line break right after the bracket is not part of
                    // the comment; a space is not shown either.
                    let mut first = opening;
                    if bytes[first..].starts_with(b"\r\n") || bytes[first..].starts_with(b"\n\r") {
                        first += 2;
                    } else if matches!(bytes.get(first), Some(b'\n' | b'\r' | b' ')) {
                        first += 1;
                    }
                    comments.push(Comment {
                        range: start..index,
                        kind: Kind::Contents(vec![Range {
                            start: first.min(closing),
                            end: closing,
                        }]),
                    });
                } else {
                    index = line_end(bytes, index);
                    let opening = if bytes.get(start + 2) == Some(&b'-') {
                        3
                    } else {
                        2
                    };
                    comments.push(Comment::line(start..index, opening));
                }
            }
            b'[' => match long_bracket(bytes, index) {
                Some(level) => {
                    // An unclosed long string hides the rest of the file.
                    let Some(closing) = long_end(bytes, index + level + 2, level) else {
                        break;
                    };
                    index = closing + level + 2;
                }
                None => index += 1,
            },
            quote @ (b'"' | b'\'') => index = string(bytes, index + 1, quote),
            _ => index += 1,
        }
    }
    comments
}

fn line_end(bytes: &[u8], from: usize) -> usize {
    bytes[from..]
        .iter()
        .position(|byte| matches!(byte, b'\n' | b'\r'))
        .map_or(bytes.len(), |length| from + length)
}

/// The level of the opening long bracket at `at`, such as 0 for `[[` and 2 for
/// `[==[`.
fn long_bracket(bytes: &[u8], at: usize) -> Option<usize> {
    if bytes.get(at) != Some(&b'[') {
        return None;
    }
    let level = bytes[at + 1..]
        .iter()
        .take_while(|byte| **byte == b'=')
        .count();
    (bytes.get(at + 1 + level) == Some(&b'[')).then_some(level)
}

/// Where the closing long bracket of `level` after `from` starts.
fn long_end(bytes: &[u8], from: usize, level: usize) -> Option<usize> {
    let mut at = from;
    while let Some(found) = bytes[at..].iter().position(|byte| *byte == b']') {
        let start = at + found;
        let equals = bytes[start + 1..]
            .iter()
            .take_while(|byte| **byte == b'=')
            .count();
        if equals == level && bytes.get(start + 1 + equals) == Some(&b']') {
            return Some(start);
        }
        at = start + 1 + equals;
    }
    None
}

/// The end of a short string whose opening quote is before `from`. A string
/// that a line break ends unescaped, which Lua refuses, ends there.
fn string(bytes: &[u8], from: usize, quote: u8) -> usize {
    let mut index = from;
    while let Some(&byte) = bytes.get(index) {
        match byte {
            b'\n' | b'\r' => return index,
            b'\\' => {
                index += 1;
                match bytes.get(index) {
                    // `\z` skips the white space after it, line breaks too.
                    Some(b'z') => {
                        index += 1;
                        while matches!(
                            bytes.get(index),
                            Some(b' ' | b'\t' | b'\n' | b'\r' | b'\x0b' | b'\x0c')
                        ) {
                            index += 1;
                        }
                    }
                    // A line break after a backslash is part of the string;
                    // `\r\n` and `\n\r` are one line break.
                    Some(b'\n' | b'\r') => {
                        let pair =
                            matches!(&bytes[index..], [b'\r', b'\n', ..] | [b'\n', b'\r', ..]);
                        index += if pair { 2 } else { 1 };
                    }
                    Some(_) => index += 1,
                    None => {}
                }
            }
            _ if byte == quote => return index + 1,
            _ => index += 1,
        }
    }
    index
}
