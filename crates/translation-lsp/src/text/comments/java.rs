use super::{Comment, Kind};

/// Java decides comments by tokens alone: outside string, text block, and
/// character literals, `//` and `/*` start a comment. `///` lines and `/**`
/// comments are documentation comments.
pub(super) fn parse(text: &str) -> Vec<Comment> {
    let bytes = text.as_bytes();
    let mut comments = Vec::new();
    let mut index = 0;
    while index < bytes.len() {
        let start = index;
        match bytes[index] {
            // A text block ends at the first `"""` that no backslash escapes.
            b'"' if bytes[index..].starts_with(b"\"\"\"") => {
                index += 3;
                while index < bytes.len() {
                    if bytes[index] == b'\\' {
                        index += 2;
                    } else if bytes[index..].starts_with(b"\"\"\"") {
                        index += 3;
                        break;
                    } else {
                        index += 1;
                    }
                }
            }
            // An unclosed string or character ends at its line, as javac reads it.
            quote @ (b'"' | b'\'') => {
                let in_line = |at: usize| !matches!(bytes.get(at), None | Some(b'\r' | b'\n'));
                index += 1;
                while in_line(index) {
                    let byte = bytes[index];
                    index += 1;
                    if byte == quote {
                        break;
                    }
                    if byte == b'\\' && in_line(index) {
                        index += 1;
                    }
                }
            }
            b'/' if bytes.get(index + 1) == Some(&b'/') => {
                index = bytes[index..]
                    .iter()
                    .position(|byte| matches!(byte, b'\r' | b'\n'))
                    .map_or(bytes.len(), |length| index + length);
                let opening = if bytes[start..].starts_with(b"///") {
                    3
                } else {
                    2
                };
                comments.push(Comment::line(start..index, opening));
            }
            b'/' if bytes.get(index + 1) == Some(&b'*') => {
                // An unclosed comment hides the rest of the file.
                let Some(length) = text[index + 2..].find("*/") else {
                    break;
                };
                index += length + 4;
                // `/**/` is an empty documentation comment.
                let opening = if bytes[start..].starts_with(b"/**") {
                    3
                } else {
                    2
                };
                comments.push(Comment {
                    range: start..index,
                    kind: Kind::Block {
                        opening,
                        closed: true,
                    },
                });
            }
            _ => index += 1,
        }
    }
    comments
}
