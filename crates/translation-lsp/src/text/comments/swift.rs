use super::{Comment, Kind};

/// What the reader is inside, kept on a stack of its own.
enum Frame {
    /// The code of a `\(…)` interpolation, with the `(` opened in it.
    Interpolation { parens: usize },
    /// A string between `"`, `"""`, or `'`, with `pounds` `#` around it.
    Text {
        quote: u8,
        multiline: bool,
        pounds: usize,
    },
}

/// Swift's lexer: block comments nest, and the code of a `\(…)` interpolation
/// in a string is read as code, comments included. Of regular expression
/// literals, only those between `#/` and `/#` are followed.
pub(super) fn parse(text: &str) -> Vec<Comment> {
    let bytes = text.as_bytes();
    let line_end = |at: usize| {
        bytes[at..]
            .iter()
            .position(|byte| matches!(byte, b'\r' | b'\n'))
            .map_or(bytes.len(), |length| at + length)
    };
    let pounds_at = |at: usize| bytes[at..].iter().take_while(|&&byte| byte == b'#').count();
    let mut comments = Vec::new();
    let mut stack = Vec::new();
    // Where the line of the last `"""` after `#` ends, and the closings on it.
    let mut closings_end = 0;
    let mut closings = Vec::new();
    // A shebang line is no comment to translate.
    let mut index = if text.starts_with("#!") {
        line_end(0)
    } else {
        0
    };
    while index < bytes.len() {
        let start = index;
        if let Some(&Frame::Text {
            quote,
            multiline,
            pounds,
        }) = stack.last()
        {
            match bytes[index] {
                byte if byte == quote => {
                    let length = if multiline { 3 } else { 1 };
                    if bytes[index..]
                        .iter()
                        .take(length)
                        .all(|&byte| byte == quote)
                        && bytes[index..].len() >= length
                        && pounds_at(index + length) >= pounds
                    {
                        index += length + pounds;
                        stack.pop();
                    } else {
                        index += 1;
                    }
                }
                // The line ends an unclosed string, and an interpolation in it.
                b'\r' | b'\n' if !multiline => {
                    stack.pop();
                }
                b'\\' if pounds_at(index + 1) >= pounds => {
                    index += 1 + pounds;
                    match bytes.get(index) {
                        Some(b'(') => {
                            index += 1;
                            stack.push(Frame::Interpolation { parens: 0 });
                        }
                        Some(b'\r' | b'\n') | None => {}
                        Some(_) => index += 1,
                    }
                }
                _ => index += 1,
            }
            continue;
        }
        match bytes[index] {
            b'#' | b'"' | b'\'' => {
                let pounds = pounds_at(index);
                index += pounds;
                match bytes.get(index) {
                    Some(&quote @ (b'"' | b'\'')) => {
                        // With `#`, `"""` closed by `"#` on its line is a
                        // one-line string that starts with `""`.
                        let multiline = quote == b'"'
                            && bytes[index..].starts_with(b"\"\"\"")
                            && (pounds == 0 || {
                                let from = index + 2;
                                // The line is read once, for all the `"""` on it.
                                if from >= closings_end {
                                    closings_end = line_end(from);
                                    closings = closings_of(&bytes[..closings_end], from);
                                }
                                while closings.last().is_some_and(|&(at, _)| at < from) {
                                    closings.pop();
                                }
                                closings.last().is_none_or(|&(_, most)| most < pounds)
                            });
                        index += if multiline { 3 } else { 1 };
                        stack.push(Frame::Text {
                            quote,
                            multiline,
                            pounds,
                        });
                    }
                    // A regular expression between `#/` and `/#`, over several
                    // lines when a line break follows `#/`.
                    Some(b'/') if pounds > 0 => {
                        index += 1;
                        let multiline = matches!(
                            bytes[index..]
                                .iter()
                                .find(|&&byte| !matches!(byte, b' ' | b'\t')),
                            Some(b'\r' | b'\n')
                        );
                        while index < bytes.len() {
                            match bytes[index] {
                                b'\r' | b'\n' if !multiline => break,
                                b'\\' => index += 2,
                                b'/' if pounds_at(index + 1) >= pounds => {
                                    index += 1 + pounds;
                                    break;
                                }
                                _ => index += 1,
                            }
                        }
                    }
                    _ => {}
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
            b'(' => {
                if let Some(Frame::Interpolation { parens }) = stack.last_mut() {
                    *parens += 1;
                }
                index += 1;
            }
            b')' => {
                index += 1;
                match stack.last_mut() {
                    Some(Frame::Interpolation { parens: 0 }) => {
                        stack.pop();
                    }
                    Some(Frame::Interpolation { parens }) => *parens -= 1,
                    _ => {}
                }
            }
            // A line break in the interpolation of a one-line string ends both.
            b'\r' | b'\n'
                if matches!(
                    stack.iter().rev().nth(1),
                    Some(Frame::Text {
                        multiline: false,
                        ..
                    })
                ) =>
            {
                stack.truncate(stack.len() - 2);
            }
            b'/' if bytes.get(index + 1) == Some(&b'/') => {
                index = line_end(index);
                let opening = if bytes[start..].starts_with(b"///") {
                    3
                } else {
                    2
                };
                comments.push(Comment::line(start..index, opening));
            }
            b'/' if bytes.get(index + 1) == Some(&b'*') => {
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

/// The `"` from `from` on that have more `#` after them than any later `"`,
/// with their `#`, the last first: a string with up to that many `#` can
/// close at or after the `"`.
fn closings_of(bytes: &[u8], from: usize) -> Vec<(usize, usize)> {
    let mut closings = Vec::new();
    let mut pounds = 0;
    for at in (from..bytes.len()).rev() {
        match bytes[at] {
            b'#' => pounds += 1,
            byte => {
                if byte == b'"' && closings.last().is_none_or(|&(_, most)| pounds > most) {
                    closings.push((at, pounds));
                }
                pounds = 0;
            }
        }
    }
    closings
}
