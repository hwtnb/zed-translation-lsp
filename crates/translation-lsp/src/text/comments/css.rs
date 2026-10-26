use super::{Comment, Kind};

/// Reads CSS as its tokenizer does, as far as comments depend on it: outside
/// strings and the contents of an unquoted `url( ... )`, every `/*` starts a
/// comment. Names and numbers are read whole, so that `url` is told apart
/// from a longer name, a unit, or a name written with escapes.
pub(super) fn parse(text: &str) -> Vec<Comment> {
    let bytes = text.as_bytes();
    let mut comments = Vec::new();
    let mut index = 0;
    while index < bytes.len() {
        let start = index;
        match bytes[index] {
            b'/' if bytes.get(index + 1) == Some(&b'*') => {
                // An unclosed comment hides the rest of the style sheet.
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
            quote @ (b'"' | b'\'') => index = string(bytes, index + 1, quote),
            _ if starts_number(bytes, index) => {
                index = number(bytes, index);
                // A unit is a name that belongs to the number; a percentage
                // has none.
                if bytes[index - 1] != b'%' && starts_name(bytes, index) {
                    index = name(bytes, index).0;
                }
            }
            b'-' if bytes[index..].starts_with(b"-->") => index += 3,
            _ if starts_name(bytes, index) => {
                let (end, url) = name(bytes, index);
                index = end;
                if url && bytes.get(index) == Some(&b'(') {
                    index = url_contents(bytes, index + 1);
                }
            }
            // The name of an at-rule or a hash is no function.
            b'@' if starts_name(bytes, index + 1) => index = name(bytes, index + 1).0,
            b'#' if name_byte(bytes.get(index + 1)) || escape(bytes, index + 1) => {
                index = name(bytes, index + 1).0;
            }
            _ => index += 1,
        }
    }
    comments
}

fn newline(byte: Option<&u8>) -> bool {
    matches!(byte, Some(b'\n' | b'\r' | b'\x0c'))
}

/// Whether a backslash at `index` escapes the character after it.
fn escape(bytes: &[u8], index: usize) -> bool {
    bytes.get(index) == Some(&b'\\') && index + 1 < bytes.len() && !newline(bytes.get(index + 1))
}

fn name_start_byte(byte: Option<&u8>) -> bool {
    byte.is_some_and(|byte| byte.is_ascii_alphabetic() || matches!(byte, b'_' | 0x80..))
}

fn name_byte(byte: Option<&u8>) -> bool {
    name_start_byte(byte) || byte.is_some_and(|byte| byte.is_ascii_digit() || *byte == b'-')
}

/// Whether a name starts at `index`, also with a dash or an escape.
fn starts_name(bytes: &[u8], index: usize) -> bool {
    match bytes.get(index) {
        Some(b'-') => {
            name_start_byte(bytes.get(index + 1))
                || bytes.get(index + 1) == Some(&b'-')
                || escape(bytes, index + 1)
        }
        Some(b'\\') => escape(bytes, index),
        byte => name_start_byte(byte),
    }
}

/// Reads a name from `index` and tells whether it is `url` in any case and
/// with any escapes.
fn name(bytes: &[u8], mut index: usize) -> (usize, bool) {
    // How many characters have been read, and whether they begin `url`.
    let mut length = 0;
    let mut url = true;
    loop {
        let character = if name_byte(bytes.get(index)) {
            index += 1;
            char::from(bytes[index - 1])
        } else if escape(bytes, index) {
            index += 1;
            let digits = bytes[index..]
                .iter()
                .take(6)
                .take_while(|byte| byte.is_ascii_hexdigit())
                .count();
            if digits == 0 {
                index += 1;
                char::from(bytes[index - 1])
            } else {
                let code = bytes[index..index + digits].iter().fold(0, |code, digit| {
                    code * 16 + char::from(*digit).to_digit(16).unwrap()
                });
                index += digits;
                // One blank after the digits belongs to the escape.
                if bytes[index..].starts_with(b"\r\n") {
                    index += 2;
                } else if bytes.get(index).is_some_and(u8::is_ascii_whitespace) {
                    index += 1;
                }
                char::from_u32(code).unwrap_or(char::REPLACEMENT_CHARACTER)
            }
        } else {
            return (index, url && length == 3);
        };
        url &= "url".chars().nth(length) == Some(character.to_ascii_lowercase());
        length += 1;
    }
}

fn starts_number(bytes: &[u8], index: usize) -> bool {
    let digit = |at: usize| bytes.get(at).is_some_and(u8::is_ascii_digit);
    match bytes.get(index) {
        Some(b'+' | b'-') => {
            digit(index + 1) || bytes.get(index + 1) == Some(&b'.') && digit(index + 2)
        }
        Some(b'.') => digit(index + 1),
        _ => digit(index),
    }
}

/// Reads a number from `index`, with its sign, fraction, exponent, and `%`.
fn number(bytes: &[u8], mut index: usize) -> usize {
    let digit = |at: usize| bytes.get(at).is_some_and(u8::is_ascii_digit);
    let digits = |mut at: usize| {
        while digit(at) {
            at += 1;
        }
        at
    };
    index += usize::from(matches!(bytes[index], b'+' | b'-'));
    index = digits(index);
    if bytes.get(index) == Some(&b'.') && digit(index + 1) {
        index = digits(index + 1);
    }
    if matches!(bytes.get(index), Some(b'e' | b'E')) {
        if digit(index + 1) {
            index = digits(index + 1);
        } else if matches!(bytes.get(index + 1), Some(b'+' | b'-')) && digit(index + 2) {
            index = digits(index + 2);
        }
    }
    index + usize::from(bytes.get(index) == Some(&b'%'))
}

/// Reads a string from after its opening quote. An unescaped line break ends
/// it, closed or not.
fn string(bytes: &[u8], mut index: usize, quote: u8) -> usize {
    while index < bytes.len() && !newline(bytes.get(index)) {
        let byte = bytes[index];
        index += 1;
        if byte == quote {
            break;
        }
        // An escaped line break continues the string.
        if byte == b'\\' {
            index += if bytes[index..].starts_with(b"\r\n") {
                2
            } else {
                usize::from(index < bytes.len())
            };
        }
    }
    index
}

/// Reads what follows `url(`: a string, or a URL to the first `)` that no
/// backslash escapes.
fn url_contents(bytes: &[u8], mut index: usize) -> usize {
    while bytes.get(index).is_some_and(u8::is_ascii_whitespace) {
        index += 1;
    }
    // `url("...")` is a function with a string in it.
    if matches!(bytes.get(index), Some(b'"' | b'\'')) {
        return index;
    }
    while index < bytes.len() && bytes[index] != b')' {
        index += if bytes[index] == b'\\' { 2 } else { 1 };
    }
    (index + 1).min(bytes.len())
}
