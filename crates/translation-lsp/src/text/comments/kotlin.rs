use super::{Comment, Kind};

/// What the reader is inside, kept on a stack of its own.
enum Frame {
    /// The code of a `${…}` template, with the `{` opened in it.
    Template { braces: usize },
    /// A string. `dollars` start a template: `$"…"` and `"…"` need one,
    /// `$$"…"` two.
    Text { raw: bool, dollars: usize },
}

/// Kotlin's lexer: block comments nest, and the code of a `${…}` template in a
/// string is read as code, comments included.
pub(super) fn parse(text: &str) -> Vec<Comment> {
    let bytes = text.as_bytes();
    let line_end = |at: usize| {
        bytes[at..]
            .iter()
            .position(|byte| matches!(byte, b'\r' | b'\n'))
            .map_or(bytes.len(), |length| at + length)
    };
    let mut comments = Vec::new();
    let mut stack = Vec::new();
    // A shebang line is no comment to translate.
    let mut index = if text.starts_with("#!") {
        line_end(0)
    } else {
        0
    };
    while index < bytes.len() {
        let start = index;
        if let Some(&Frame::Text { raw, dollars }) = stack.last() {
            match bytes[index] {
                b'"' if raw => {
                    let quotes = bytes[index..]
                        .iter()
                        .take_while(|&&byte| byte == b'"')
                        .count();
                    index += quotes;
                    // The last three of three or more quotes close the string.
                    if quotes >= 3 {
                        stack.pop();
                    }
                }
                b'"' => {
                    index += 1;
                    stack.pop();
                }
                // The line ends an unclosed string.
                b'\r' | b'\n' if !raw => {
                    stack.pop();
                }
                b'\\' if !raw => {
                    index += 1;
                    if !matches!(bytes.get(index), None | Some(b'\r' | b'\n')) {
                        index += 1;
                    }
                }
                b'$' => {
                    let count = bytes[index..]
                        .iter()
                        .take_while(|&&byte| byte == b'$')
                        .count();
                    index += count;
                    if count >= dollars && bytes.get(index) == Some(&b'{') {
                        index += 1;
                        stack.push(Frame::Template { braces: 0 });
                    }
                }
                _ => index += 1,
            }
            continue;
        }
        match bytes[index] {
            b'$' | b'"' => {
                let count = bytes[index..]
                    .iter()
                    .take_while(|&&byte| byte == b'$')
                    .count();
                index += count;
                if bytes.get(index) == Some(&b'"') {
                    let raw = bytes[index..].starts_with(b"\"\"\"");
                    index += if raw { 3 } else { 1 };
                    stack.push(Frame::Text {
                        raw,
                        dollars: count.max(1),
                    });
                }
            }
            // A character literal may hold several characters, and ends at its line.
            b'\'' => {
                index += 1;
                while let Some(&byte) = bytes.get(index) {
                    match byte {
                        b'\r' | b'\n' => break,
                        b'\'' => {
                            index += 1;
                            break;
                        }
                        b'\\' => {
                            index += 1;
                            if matches!(bytes.get(index), None | Some(b'\r' | b'\n')) {
                                break;
                            }
                            index += 1;
                        }
                        _ => index += 1,
                    }
                }
            }
            // A name between backticks, which ends at its line.
            b'`' => {
                index += 1;
                if let Some(length) = bytes[index..]
                    .iter()
                    .position(|byte| matches!(byte, b'`' | b'\r' | b'\n'))
                    .filter(|&length| length > 0 && bytes[index + length] == b'`')
                {
                    index += length + 1;
                }
            }
            b'{' => {
                if let Some(Frame::Template { braces }) = stack.last_mut() {
                    *braces += 1;
                }
                index += 1;
            }
            b'}' => {
                index += 1;
                match stack.last_mut() {
                    Some(Frame::Template { braces: 0 }) => {
                        stack.pop();
                    }
                    Some(Frame::Template { braces }) => *braces -= 1,
                    _ => {}
                }
            }
            b'/' if bytes.get(index + 1) == Some(&b'/') => {
                index = line_end(index);
                comments.push(Comment::line(start..index, 2));
            }
            b'/' if bytes.get(index + 1) == Some(&b'*') => {
                // `/**/` is an empty comment, and `/**` opens a KDoc comment.
                let opening =
                    if bytes[index..].starts_with(b"/**") && !bytes[index..].starts_with(b"/**/") {
                        3
                    } else {
                        2
                    };
                index += 2;
                let mut depth = 0;
                let mut closed = false;
                while index < bytes.len() {
                    if bytes[index..].starts_with(b"/*") {
                        depth += 1;
                        index += 2;
                    } else if bytes[index..].starts_with(b"*/") {
                        index += 2;
                        if depth == 0 {
                            closed = true;
                            break;
                        }
                        depth -= 1;
                    } else {
                        index += 1;
                    }
                }
                comments.push(Comment {
                    range: start..index,
                    kind: Kind::Block { opening, closed },
                });
            }
            _ => index += 1,
        }
    }
    comments
}
