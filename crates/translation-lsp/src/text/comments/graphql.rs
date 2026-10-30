use super::Comment;

/// GraphQL decides comments by tokens alone: outside strings and block
/// strings, `#` starts a comment that runs to the end of the line. A
/// description is a string, not a comment.
pub(super) fn parse(text: &str) -> Vec<Comment> {
    let bytes = text.as_bytes();
    let mut comments = Vec::new();
    let mut index = 0;
    while index < bytes.len() {
        let start = index;
        match bytes[index] {
            b'"' if bytes[index..].starts_with(b"\"\"\"") => {
                // A block string ends at `"""` that no backslash escapes. An
                // unclosed one hides the rest of the document.
                index += 3;
                loop {
                    let Some(found) = bytes[index..].iter().position(|byte| *byte == b'"') else {
                        return comments;
                    };
                    index += found;
                    if bytes[index..].starts_with(b"\"\"\"") && bytes[index - 1] != b'\\' {
                        index += 3;
                        break;
                    }
                    if bytes[index..].starts_with(b"\"\"\"") {
                        index += 3;
                    } else {
                        index += 1;
                    }
                }
            }
            // A string ends at its line, where the lexer reports it unclosed.
            b'"' => {
                index += 1;
                while let Some(&byte) = bytes.get(index) {
                    if matches!(byte, b'\n' | b'\r') {
                        break;
                    }
                    index += 1;
                    if byte == b'"' {
                        break;
                    }
                    if byte == b'\\' && !matches!(bytes.get(index), None | Some(b'\n' | b'\r')) {
                        index += 1;
                    }
                }
            }
            b'#' => {
                index = bytes[index..]
                    .iter()
                    .position(|byte| matches!(byte, b'\r' | b'\n'))
                    .map_or(bytes.len(), |length| index + length);
                comments.push(Comment::line(start..index, 1));
            }
            _ => index += 1,
        }
    }
    comments
}
