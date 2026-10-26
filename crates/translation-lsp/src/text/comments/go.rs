use super::{Comment, Kind};

/// Go decides comments by tokens alone: outside the three kinds of literals,
/// `//` and `/*` start a comment. Directives such as `//go:build` are left out.
pub(super) fn parse(text: &str) -> Vec<Comment> {
    let bytes = text.as_bytes();
    let mut comments = Vec::new();
    let mut index = 0;
    while index < bytes.len() {
        let start = index;
        match bytes[index] {
            // An unclosed string or rune ends at its line, as Go's scanner reads it.
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
            // An unclosed raw string hides the rest of the file.
            b'`' => match text[index + 1..].find('`') {
                Some(length) => index += length + 2,
                None => break,
            },
            b'/' if bytes.get(index + 1) == Some(&b'/') => {
                index = bytes[index..]
                    .iter()
                    .position(|byte| matches!(byte, b'\r' | b'\n'))
                    .map_or(bytes.len(), |length| index + length);
                if !directive(&text[start + 2..index]) {
                    comments.push(Comment::line(start..index, 2));
                }
            }
            b'/' if bytes.get(index + 1) == Some(&b'*') => {
                // An unclosed block comment hides the rest of the file.
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

/// Whether the text of a `//` comment is a directive, as `go/ast` tells them
/// apart: `line `, `extern `, `export `, or `[a-z0-9]+:[a-z0-9]`.
fn directive(text: &str) -> bool {
    if ["line ", "extern ", "export "]
        .iter()
        .any(|prefix| text.starts_with(prefix))
    {
        return true;
    }
    let plain = |byte: &u8| byte.is_ascii_lowercase() || byte.is_ascii_digit();
    match text.find(':') {
        Some(colon) if colon > 0 => {
            text.as_bytes()[..colon].iter().all(plain)
                && text.as_bytes().get(colon + 1).is_some_and(plain)
        }
        _ => false,
    }
}
