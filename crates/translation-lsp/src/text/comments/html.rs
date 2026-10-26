use std::ops::Range;

use super::{Comment, Kind, css, javascript};

/// The elements whose text runs to their end tag without markup in it.
const RAW_TEXT: [&[u8]; 7] = [
    b"style",
    b"xmp",
    b"iframe",
    b"noembed",
    b"noframes",
    b"textarea",
    b"title",
];
/// The types of `<script>` that hold a classic script, besides an empty one.
const JAVASCRIPT: [&str; 16] = [
    "application/ecmascript",
    "application/javascript",
    "application/x-ecmascript",
    "application/x-javascript",
    "text/ecmascript",
    "text/javascript",
    "text/javascript1.0",
    "text/javascript1.1",
    "text/javascript1.2",
    "text/javascript1.3",
    "text/javascript1.4",
    "text/javascript1.5",
    "text/jscript",
    "text/livescript",
    "text/x-ecmascript",
    "text/x-javascript",
];

/// Reads HTML as its tokenizer does, without building the tree: `<!-- ... -->`
/// outside tags and outside the text of `<script>`, `<style>`, and the other
/// elements of raw text is a comment. The comments of a script are read as
/// JavaScript, and those of a style sheet as CSS.
pub(super) fn parse(text: &str) -> Vec<Comment> {
    let bytes = text.as_bytes();
    let mut comments = Vec::new();
    let mut index = 0;
    while let Some(found) = bytes[index..].iter().position(|byte| *byte == b'<') {
        index += found;
        let rest = &bytes[index..];
        if rest.starts_with(b"<!--") {
            let after = index + 4;
            // `<!-->` and `<!--->` are empty comments.
            let (end, closing) = if bytes.get(after) == Some(&b'>') {
                (after, 1)
            } else if bytes[after..].starts_with(b"->") {
                (after, 2)
            } else {
                // An unclosed comment hides the rest of the document. Each
                // dash after the first can start the `--` of the end.
                let mut at = after;
                loop {
                    let Some(end) = find(bytes, at, b"--") else {
                        return comments;
                    };
                    if let Some(closing) = comment_end(&bytes[end..]) {
                        break (end, closing);
                    }
                    at = end + 1;
                }
            };
            let start = (after + usize::from(bytes.get(after) == Some(&b' '))).min(end);
            comments.push(Comment {
                range: index..end + closing,
                kind: Kind::Contents(vec![Range { start, end }]),
            });
            index = end + closing;
        } else if rest.starts_with(b"<![CDATA[") {
            // This runs to `]]>` in SVG and MathML, where it belongs, and to
            // the first `>` elsewhere, where it is an error.
            let Some(end) = find(bytes, index, b"]]>") else {
                break;
            };
            index = end + 3;
        } else if rest.starts_with(b"<!")
            || rest.starts_with(b"<?")
            || rest.starts_with(b"</") && !rest.get(2).is_some_and(u8::is_ascii_alphabetic)
        {
            // A doctype, `</>`, or other text that the tokenizer reads to `>`
            // as a comment it does not show.
            let Some(end) = find(bytes, index + 2, b">") else {
                break;
            };
            index = end + 1;
        } else if rest
            .get(1)
            .is_some_and(|byte| byte.is_ascii_alphabetic() || *byte == b'/')
        {
            let Some(tag) = tag(bytes, index) else {
                // A tag runs to the end of the document.
                break;
            };
            index = tag.end;
            if tag.closing {
                continue;
            }
            let name = tag.name.to_ascii_lowercase();
            if name == b"plaintext" {
                break;
            }
            if name == b"script" {
                let end = script_end(bytes, index);
                if let Some(module) = script_type(&tag) {
                    shifted(
                        &mut comments,
                        javascript::script(&text[index..end], module),
                        index,
                    );
                }
                index = end;
            } else if RAW_TEXT.contains(&name.as_slice()) {
                let end = end_tag(bytes, index, &name).unwrap_or(bytes.len());
                if name == b"style" {
                    shifted(&mut comments, css::parse(&text[index..end]), index);
                }
                index = end;
            }
        } else {
            index += 1;
        }
    }
    comments
}

/// How many bytes from `--` end a comment: `-->` and `--!>`.
fn comment_end(dashes: &[u8]) -> Option<usize> {
    if dashes[2..].starts_with(b">") {
        Some(3)
    } else if dashes[2..].starts_with(b"!>") {
        Some(4)
    } else {
        None
    }
}

fn find(bytes: &[u8], from: usize, needle: &[u8]) -> Option<usize> {
    bytes[from..]
        .windows(needle.len())
        .position(|window| window == needle)
        .map(|at| from + at)
}

fn shifted(comments: &mut Vec<Comment>, found: Vec<Comment>, offset: usize) {
    comments.extend(found.into_iter().map(|comment| Comment {
        range: comment.range.start + offset..comment.range.end + offset,
        kind: comment.kind,
    }));
}

/// The blanks that end the name of a tag or of an attribute. The tokenizer
/// reads a CR as LF.
fn blank(byte: u8) -> bool {
    matches!(byte, b'\t' | b'\n' | b'\x0c' | b'\r' | b' ')
}

struct Tag<'a> {
    name: &'a [u8],
    closing: bool,
    /// The values of `type` and `language`, as written.
    script_type: Option<&'a [u8]>,
    language: Option<&'a [u8]>,
    /// Where the text after the tag starts.
    end: usize,
}

/// Reads the tag that starts at `start`, with its attributes, as the
/// tokenizer does. `None` when the document ends inside it.
fn tag(bytes: &[u8], start: usize) -> Option<Tag<'_>> {
    let closing = bytes[start + 1] == b'/';
    let mut index = start + 1 + usize::from(closing);
    let name_start = index;
    while index < bytes.len() && !blank(bytes[index]) && !matches!(bytes[index], b'/' | b'>') {
        index += 1;
    }
    let mut tag = Tag {
        name: &bytes[name_start..index],
        closing,
        script_type: None,
        language: None,
        end: 0,
    };
    loop {
        while index < bytes.len() && (blank(bytes[index]) || bytes[index] == b'/') {
            index += 1;
        }
        match bytes.get(index)? {
            b'>' => {
                tag.end = index + 1;
                return Some(tag);
            }
            // A name may start with `=`.
            _ => {
                let name_start = index;
                index += 1;
                while index < bytes.len()
                    && !blank(bytes[index])
                    && !matches!(bytes[index], b'/' | b'>' | b'=')
                {
                    index += 1;
                }
                let name = &bytes[name_start..index];
                while index < bytes.len() && blank(bytes[index]) {
                    index += 1;
                }
                if bytes.get(index) != Some(&b'=') {
                    tag.attribute(name, &[]);
                    continue;
                }
                index += 1;
                while index < bytes.len() && blank(bytes[index]) {
                    index += 1;
                }
                let value = match *bytes.get(index)? {
                    quote @ (b'"' | b'\'') => {
                        let value_start = index + 1;
                        index = value_start
                            + bytes[value_start..]
                                .iter()
                                .position(|byte| *byte == quote)?;
                        let value = &bytes[value_start..index];
                        index += 1;
                        value
                    }
                    // `>` ends the tag; the value is empty.
                    b'>' => &[],
                    _ => {
                        let value_start = index;
                        while index < bytes.len() && !blank(bytes[index]) && bytes[index] != b'>' {
                            index += 1;
                        }
                        &bytes[value_start..index]
                    }
                };
                tag.attribute(name, value);
            }
        }
    }
}

impl<'a> Tag<'a> {
    /// Keeps the value of `type` or `language`. The first of the attributes of
    /// one name counts, also one without a value.
    fn attribute(&mut self, name: &[u8], value: &'a [u8]) {
        if name.eq_ignore_ascii_case(b"type") {
            self.script_type.get_or_insert(value);
        } else if name.eq_ignore_ascii_case(b"language") {
            self.language.get_or_insert(value);
        }
    }
}

/// Where the end tag of `name` starts, from `from` on: `</` and the name in any
/// case, followed by a blank, `/`, or `>`.
fn end_tag(bytes: &[u8], from: usize, name: &[u8]) -> Option<usize> {
    let mut at = from;
    while let Some(start) = find(bytes, at, b"</") {
        if is_tag(bytes, start + 2, name) {
            return Some(start);
        }
        at = start + 2;
    }
    None
}

/// Whether `name` in any case starts at `at`, followed by a blank, `/`, or `>`.
fn is_tag(bytes: &[u8], at: usize, name: &[u8]) -> bool {
    bytes
        .get(at..at + name.len())
        .is_some_and(|word| word.eq_ignore_ascii_case(name))
        && bytes
            .get(at + name.len())
            .is_some_and(|byte| blank(*byte) || matches!(byte, b'/' | b'>'))
}

/// Where the text of a `<script>` ends. After `<!--`, a `<script>` in the text
/// makes the next `</script>` part of the text, until `-->`.
fn script_end(bytes: &[u8], from: usize) -> usize {
    enum State {
        Plain,
        Escaped,
        DoubleEscaped,
    }
    let mut state = State::Plain;
    let mut index = from;
    while index < bytes.len() {
        let rest = &bytes[index..];
        match state {
            State::Plain if rest.starts_with(b"<!--") => {
                state = State::Escaped;
                // The dashes can also be the start of `-->`, as in `<!-->`.
                index += 2;
                continue;
            }
            State::Escaped | State::DoubleEscaped if rest.starts_with(b"-->") => {
                state = State::Plain;
                index += 3;
                continue;
            }
            State::Plain | State::Escaped
                if rest.starts_with(b"</") && is_tag(bytes, index + 2, b"script") =>
            {
                return index;
            }
            State::Escaped if rest.starts_with(b"<") && is_tag(bytes, index + 1, b"script") => {
                state = State::DoubleEscaped;
                index += 7;
                continue;
            }
            State::DoubleEscaped
                if rest.starts_with(b"</") && is_tag(bytes, index + 2, b"script") =>
            {
                state = State::Escaped;
                index += 8;
                continue;
            }
            _ => {}
        }
        index += 1;
    }
    bytes.len()
}

/// Whether a `<script>` holds JavaScript, and if so, whether a module.
fn script_type(tag: &Tag<'_>) -> Option<bool> {
    let written = match (tag.script_type, tag.language) {
        (Some(b""), _) | (None, None | Some(b"")) => return Some(false),
        (Some(script_type), _) => attribute_text(script_type),
        (None, Some(language)) => format!("text/{}", attribute_text(language)),
    };
    // An empty type is told apart before blanks are removed.
    let written = written
        .trim_matches(|character: char| character.is_ascii_whitespace())
        .to_ascii_lowercase();
    if JAVASCRIPT.contains(&written.as_str()) {
        Some(false)
    } else if written == "module" {
        Some(true)
    } else {
        None
    }
}

/// The text of an attribute value, as far as a script type can tell: a
/// character reference to an ASCII character is replaced by it, and any other
/// by U+FFFD, which no type has.
fn attribute_text(value: &[u8]) -> String {
    let mut text = String::new();
    let mut index = 0;
    while index < value.len() {
        let (character, length) =
            reference(&value[index..]).unwrap_or((char::from(value[index]), 1));
        text.push(character);
        index += length;
    }
    text
}

/// The character that a reference at the start of `text` stands for, with the
/// length of the reference. Of the named references, only those to ASCII
/// characters that a script type or the blanks around it can hold are known.
fn reference(text: &[u8]) -> Option<(char, usize)> {
    for (name, character) in [
        (&b"&sol;"[..], '/'),
        (b"&period;", '.'),
        (b"&Tab;", '\t'),
        (b"&NewLine;", '\n'),
    ] {
        if text.starts_with(name) {
            return Some((character, name.len()));
        }
    }
    let rest = text.strip_prefix(b"&#")?;
    let (radix, rest) = match rest.strip_prefix(b"x").or_else(|| rest.strip_prefix(b"X")) {
        Some(rest) => (16, rest),
        None => (10, rest),
    };
    let digits = rest
        .iter()
        .take_while(|byte| char::from(**byte).is_digit(radix))
        .count();
    if digits == 0 {
        return None;
    }
    let code = rest[..digits].iter().fold(0u32, |code, digit| {
        code.saturating_mul(radix)
            .saturating_add(char::from(*digit).to_digit(radix).unwrap())
    });
    let character = u8::try_from(code)
        .ok()
        .filter(|byte| (1..0x80).contains(byte))
        .map_or(char::REPLACEMENT_CHARACTER, char::from);
    let length = text.len() - rest.len() + digits;
    // The `;` is optional after the digits.
    Some((
        character,
        length + usize::from(rest.get(digits) == Some(&b';')),
    ))
}
