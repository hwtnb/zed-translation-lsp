use super::Comment;

/// Zig decides comments by tokens alone: outside string and character
/// literals and the lines of multiline strings, `//` starts a comment that
/// runs to the end of the line. `///` lines, but not `////` lines, are
/// documentation comments, and `//!` lines document their container.
pub(super) fn parse(text: &str) -> Vec<Comment> {
    let bytes = text.as_bytes();
    let mut comments = Vec::new();
    let mut index = 0;
    while index < bytes.len() {
        let start = index;
        match bytes[index] {
            // A string or character ends at its line, as the tokenizer reads
            // an unclosed one.
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
            // A line of a multiline string.
            b'\\' if bytes.get(index + 1) == Some(&b'\\') => index = line_end(bytes, index),
            b'/' if bytes.get(index + 1) == Some(&b'/') => {
                index = line_end(bytes, index);
                let rest = &bytes[start..index];
                let documentation = rest.starts_with(b"//!")
                    || rest.starts_with(b"///") && !rest.starts_with(b"////");
                // The tokenizer leaves the CR of a CRLF out of a comment.
                let end = if bytes[..index].ends_with(b"\r") {
                    index - 1
                } else {
                    index
                };
                comments.push(Comment::line(start..end, if documentation { 3 } else { 2 }));
            }
            _ => index += 1,
        }
    }
    comments
}

/// Zig ends lines with LF only.
fn line_end(bytes: &[u8], from: usize) -> usize {
    bytes[from..]
        .iter()
        .position(|byte| *byte == b'\n')
        .map_or(bytes.len(), |length| from + length)
}
