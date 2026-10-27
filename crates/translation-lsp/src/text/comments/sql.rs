use super::{Comment, Kind};

/// SQL as Zed's SQL grammar reads it, whatever the dialect: `--` comments,
/// `/* */` comments that do not nest, and strings between `'`, `"`, or
/// backticks. A quote doubled in a string stands for itself; in `E'…'`, a
/// backslash escapes the next character. Text between `$$` or `$tag$` is read
/// as SQL, so that the comments in the body of a function are found.
pub(super) fn parse(text: &str) -> Vec<Comment> {
    let bytes = text.as_bytes();
    let mut comments = Vec::new();
    // Where no more `*/`, or no more of each quote, follows. An opening that
    // finds none is no comment or string, as in the grammar.
    let mut no_closing = bytes.len() + 1;
    let mut no_quote = [bytes.len() + 1; 3];
    let mut index = 0;
    while index < bytes.len() {
        let start = index;
        match bytes[index] {
            quote @ (b'\'' | b'"' | b'`') => {
                let slot = match quote {
                    b'\'' => 0,
                    b'"' => 1,
                    _ => 2,
                };
                let escapes = quote == b'\''
                    && index > 0
                    && matches!(bytes[index - 1], b'e' | b'E')
                    && !(index > 1 && word(bytes[index - 2]));
                index += 1;
                if index >= no_quote[slot] {
                    continue;
                }
                let mut end = index;
                let closed = loop {
                    match bytes.get(end) {
                        None => break false,
                        Some(b'\\') if escapes => end += 2,
                        Some(&byte) if byte == quote => break true,
                        Some(_) => end += 1,
                    }
                };
                if closed {
                    index = end + 1;
                } else if !escapes {
                    no_quote[slot] = index;
                }
            }
            b'-' if bytes.get(index + 1) == Some(&b'-') => {
                index = bytes[index..]
                    .iter()
                    .position(|byte| matches!(byte, b'\r' | b'\n'))
                    .map_or(bytes.len(), |length| index + length);
                comments.push(Comment::line(start..index, 2));
            }
            b'/' if bytes.get(index + 1) == Some(&b'*') && index < no_closing => {
                match text[index + 2..].find("*/") {
                    Some(length) => {
                        index += length + 4;
                        comments.push(Comment {
                            range: start..index,
                            kind: Kind::Block {
                                opening: 2,
                                closed: true,
                            },
                        });
                    }
                    None => {
                        no_closing = index;
                        index += 1;
                    }
                }
            }
            _ => index += 1,
        }
    }
    comments
}

/// Whether a byte may be part of a word, such as a keyword or a name.
fn word(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_' || byte >= 0x80
}
