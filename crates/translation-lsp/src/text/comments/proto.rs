use super::{Comment, Kind};

/// Protocol Buffers as `protoc` reads it: outside strings between `'` or `"`,
/// `//` starts a comment that runs to the end of the line, and `/*` one that
/// runs to the next `*/`, without nesting.
pub(super) fn parse(text: &str) -> Vec<Comment> {
    let bytes = text.as_bytes();
    let mut comments = Vec::new();
    let mut index = 0;
    while index < bytes.len() {
        let start = index;
        match bytes[index] {
            // A string ends at its line, where `protoc` reports it unclosed.
            quote @ (b'"' | b'\'') => {
                index += 1;
                while let Some(&byte) = bytes.get(index) {
                    if byte == b'\n' {
                        break;
                    }
                    index += 1;
                    if byte == quote {
                        break;
                    }
                    if byte == b'\\' && bytes.get(index).is_some_and(|next| *next != b'\n') {
                        index += 1;
                    }
                }
            }
            b'/' if bytes.get(index + 1) == Some(&b'/') => {
                index = bytes[index..]
                    .iter()
                    .position(|byte| matches!(byte, b'\r' | b'\n'))
                    .map_or(bytes.len(), |length| index + length);
                comments.push(Comment::line(start..index, 2));
            }
            b'/' if bytes.get(index + 1) == Some(&b'*') => {
                // An unclosed comment hides the rest of the file.
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
