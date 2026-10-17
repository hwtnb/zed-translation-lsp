use granit_parser::{Scanner, StrInput, TokenType};

use super::{Comment, Kind};
use crate::language::Language;

/// Comments in these formats are decided by tokens alone, so read tokens and
/// leave the structure unchecked. Errors in the structure then hide nothing.
pub(super) fn parse(language: Language, text: &str) -> Vec<Comment> {
    match language {
        Language::Yaml | Language::DockerCompose => yaml(text),
        Language::Toml => toml(text),
        _ => json(text),
    }
}

/// Where the line of this offset ends: at CR, LF, or the end of the text.
fn line_end(bytes: &[u8], from: usize) -> usize {
    bytes[from..]
        .iter()
        .position(|byte| matches!(byte, b'\r' | b'\n'))
        .map_or(bytes.len(), |length| from + length)
}

fn json(text: &str) -> Vec<Comment> {
    let bytes = text.as_bytes();
    let mut comments = Vec::new();
    let mut index = 0;
    while index < bytes.len() {
        let start = index;
        match bytes[index] {
            // An unclosed string ends at its line, as JSON strings cannot span
            // lines. Single quotes are not JSON, but nothing inside is a comment.
            quote @ (b'"' | b'\'') => {
                let in_line = |at: usize| !matches!(bytes.get(at), None | Some(b'\r' | b'\n'));
                index += 1;
                while in_line(index) {
                    let byte = bytes[index];
                    index += 1;
                    if byte == quote {
                        break;
                    }
                    // A backslash hides the next character, but not the line break.
                    if byte == b'\\' && in_line(index) {
                        index += 1;
                    }
                }
            }
            b'/' if bytes.get(index + 1) == Some(&b'/') => {
                index = line_end(bytes, index);
                comments.push(Comment::line(start..index, 2));
            }
            b'/' if bytes.get(index + 1) == Some(&b'*') => {
                // An unclosed block comment hides the rest of the document.
                let Some(length) = text[index + 2..].find("*/") else {
                    break;
                };
                index += length + 4;
                comments.push(Comment {
                    range: start..index,
                    kind: Kind::Block {
                        opening: 2,
                        closed: true,
                    },
                });
            }
            _ => index += 1,
        }
    }
    comments
}

/// Outside the four kinds of strings, every `#` starts a comment, and every
/// quote starts a string: also in the middle of a bare value, as in `r"# x"`.
fn toml(text: &str) -> Vec<Comment> {
    let bytes = text.as_bytes();
    let mut comments = Vec::new();
    let mut index = 0;
    while index < bytes.len() {
        let start = index;
        match bytes[index] {
            b'#' => {
                index = line_end(bytes, index);
                comments.push(Comment::line(start..index, 1));
            }
            quote @ (b'"' | b'\'') => {
                let delimiter = [quote; 3];
                let multiline = bytes[index..].starts_with(&delimiter);
                index += if multiline { 3 } else { 1 };
                loop {
                    match bytes.get(index) {
                        // An unclosed multi-line string hides the rest of the document.
                        None => break,
                        // An unclosed one-line string ends at its line.
                        Some(b'\r' | b'\n') if !multiline => break,
                        // In a basic string, a backslash hides a quote or another backslash.
                        Some(b'\\') if quote == b'"' => {
                            index += 1;
                            index += usize::from(matches!(bytes.get(index), Some(b'\\' | b'"')));
                        }
                        Some(&byte) if byte == quote && !multiline => {
                            index += 1;
                            break;
                        }
                        Some(_) if multiline && bytes[index..].starts_with(&delimiter) => {
                            index += 3;
                            // Up to two more quotes belong to the string.
                            index += bytes[index..]
                                .iter()
                                .take(2)
                                .take_while(|byte| **byte == quote)
                                .count();
                            break;
                        }
                        Some(_) => index += 1,
                    }
                }
            }
            _ => index += 1,
        }
    }
    comments
}

fn yaml(text: &str) -> Vec<Comment> {
    // The scanner reports its first error after the comments before it.
    Scanner::new(StrInput::new(text))
        .map_while(Result::ok)
        .filter(|token| matches!(token.token_type(), TokenType::Comment(_)))
        .filter_map(|token| token.span().byte_range())
        .map(|range| Comment::line(range, 1))
        .collect()
}
