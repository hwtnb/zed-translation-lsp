use std::ops::Range;

use oxc_span::SourceType;

use super::{Comment, Kind, LineKind, css, html, javascript};

/// The kinds of component file.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Dialect {
    Vue,
    Svelte,
    Astro,
}

/// How deep elements and expressions nest before the rest goes unread. Each
/// level takes some of the stack.
const NESTING: usize = 512;

/// The elements that have no end tag.
const VOID: [&[u8]; 14] = [
    b"area", b"base", b"br", b"col", b"embed", b"hr", b"img", b"input", b"link", b"meta", b"param",
    b"source", b"track", b"wbr",
];

/// Reads a component file as its compiler does: `<!-- ... -->` in the markup is
/// a comment, as are the comments of the scripts and style sheets, and in
/// Svelte and Astro those of the `{ ... }` expressions. A Vue file holds blocks,
/// of which only its template, scripts, and styles are read. An Astro file can
/// start with a script between `---` lines. An unclosed part hides the rest.
pub(super) fn parse(dialect: Dialect, text: &str) -> Vec<Comment> {
    let mut reader = Reader {
        dialect,
        text,
        bytes: text.as_bytes(),
        comments: Vec::new(),
        no_regex_before: 0,
    };
    match dialect {
        Dialect::Vue => reader.blocks(),
        Dialect::Svelte => reader.markup(0, 0, false, NESTING),
        Dialect::Astro => reader
            .frontmatter()
            .and_then(|index| reader.markup(index, 0, false, NESTING)),
    };
    reader.comments
}

struct Reader<'a> {
    dialect: Dialect,
    text: &'a str,
    bytes: &'a [u8],
    comments: Vec<Comment>,
    /// Up to here, no regular expression can end on its line.
    no_regex_before: usize,
}

/// A start tag, with the attributes that decide how its text is read.
#[derive(Default)]
struct Tag<'a> {
    name: &'a [u8],
    lang: Option<&'a [u8]>,
    script_type: Option<&'a [u8]>,
    /// `is:raw` in Astro.
    raw: bool,
    /// Whether it ends with `/>`.
    closed: bool,
    /// Where the text after the tag starts.
    end: usize,
}

impl<'a> Tag<'a> {
    /// The first of the attributes of one name counts.
    fn attribute(&mut self, name: &[u8], value: &'a [u8]) {
        match name {
            b"lang" => {
                self.lang.get_or_insert(value);
            }
            b"type" => {
                self.script_type.get_or_insert(value);
            }
            b"is:raw" => self.raw = true,
            _ => {}
        }
    }
}

impl<'a> Reader<'a> {
    /// The top level of a Vue file, to the end of the file. The text of a block
    /// is read only when the block is a template, a script, or a style.
    fn blocks(&mut self) -> Option<usize> {
        let mut index = 0;
        loop {
            index += self.bytes[index..].iter().position(|byte| *byte == b'<')?;
            let rest = &self.bytes[index..];
            if rest.starts_with(b"<!--") {
                index = html::comment(self.bytes, index, &mut self.comments)?;
            } else if rest.get(1).is_some_and(u8::is_ascii_alphabetic) {
                let tag = self.tag(index, NESTING)?;
                index = tag.end;
                if tag.closed {
                    continue;
                }
                match tag.name {
                    b"template" if matches!(tag.lang, None | Some(b"html")) => {
                        index = self.markup(index, 1, true, NESTING)?;
                    }
                    b"script" | b"style" => index = self.embedded(&tag),
                    name => index = html::end_tag(self.bytes, index, name)?,
                }
            } else {
                index += 1;
            }
        }
    }

    /// The script between the `---` lines of an Astro file, when one comes
    /// before any element or expression. Gives where the markup starts.
    fn frontmatter(&mut self) -> Option<usize> {
        let mut index = 0;
        while let Some(&byte) = self.bytes.get(index) {
            let rest = &self.bytes[index..];
            if rest.starts_with(b"---") {
                let start = index + 3;
                let end = self.fence(start)?;
                self.script(start..end, SourceType::ts());
                return Some(end + 3);
            } else if rest.starts_with(b"<!--") {
                index = html::comment(self.bytes, index, &mut self.comments)?;
            } else if matches!(byte, b'<' | b'{') {
                break;
            } else {
                index += 1;
            }
        }
        Some(index)
    }

    /// The `---` that ends the script from `from`, outside its strings and
    /// comments.
    fn fence(&self, from: usize) -> Option<usize> {
        let bytes = self.bytes;
        let mut index = from;
        loop {
            let rest = bytes.get(index..)?;
            index = match *rest.first()? {
                b'-' if rest.starts_with(b"---") => return Some(index),
                b'"' | b'\'' => self.string(index),
                b'`' => {
                    let mut at = index + 1;
                    while *bytes.get(at)? != b'`' {
                        at += if bytes[at] == b'\\' { 2 } else { 1 };
                    }
                    at + 1
                }
                b'/' if rest.starts_with(b"//") => self.line_end(index),
                b'/' if rest.starts_with(b"/*") => html::find(bytes, index + 2, b"*/")? + 2,
                _ => index + 1,
            };
        }
    }

    /// Reads markup from `index`, with `open` elements open. When `nested`, it
    /// ends after the tag that closes the last of them, and gives where.
    /// Otherwise it reads to the end of the file.
    fn markup(
        &mut self,
        mut index: usize,
        mut open: usize,
        nested: bool,
        nesting: usize,
    ) -> Option<usize> {
        let nesting = nesting.checked_sub(1)?;
        loop {
            index += self.bytes[index..]
                .iter()
                .position(|byte| matches!(byte, b'<' | b'{'))?;
            let rest = &self.bytes[index..];
            if rest[0] == b'{' {
                index = match self.dialect {
                    Dialect::Vue if rest.starts_with(b"{{") => {
                        html::find(self.bytes, index + 2, b"}}")? + 2
                    }
                    Dialect::Vue => index + 1,
                    _ => self.expression(index, nesting)?,
                };
                continue;
            } else if rest.starts_with(b"<!--") {
                index = html::comment(self.bytes, index, &mut self.comments)?;
                continue;
            } else if rest.starts_with(b"<!") || rest.starts_with(b"<?") {
                index = html::find(self.bytes, index + 2, b">")? + 1;
                continue;
            } else if rest.starts_with(b"</") {
                let end = html::find(self.bytes, index + 2, b">")?;
                let name = self.bytes[index + 2..end].trim_ascii();
                if self.counts(name) {
                    open = open.saturating_sub(1);
                }
                index = end + 1;
            } else if self.dialect == Dialect::Astro && rest.starts_with(b"<>") {
                open += 1;
                index += 2;
            } else if rest.get(1).is_some_and(u8::is_ascii_alphabetic) {
                let tag = self.tag(index, nesting)?;
                index = tag.end;
                let name = tag.name.to_ascii_lowercase();
                if !tag.closed {
                    open += usize::from(self.counts(&name));
                    if !tag.raw && matches!(name.as_slice(), b"script" | b"style") {
                        index = self.embedded(&tag);
                    } else if tag.raw || self.raw_text(&name) {
                        index = html::end_tag(self.bytes, index, &name).unwrap_or(self.bytes.len());
                    }
                }
            } else {
                index += 1;
                continue;
            }
            if nested && open == 0 {
                return Some(index);
            }
        }
    }

    /// Whether an element of this name counts as open until its end tag: only
    /// the depth of `<template>` matters in Vue, and only inside an expression
    /// in Astro.
    fn counts(&self, name: &[u8]) -> bool {
        match self.dialect {
            Dialect::Vue => name == b"template",
            Dialect::Svelte => false,
            Dialect::Astro => !VOID.contains(&name),
        }
    }

    fn raw_text(&self, name: &[u8]) -> bool {
        match self.dialect {
            // Svelte reads the text of `<title>` as markup.
            Dialect::Svelte => name == b"textarea",
            Dialect::Vue | Dialect::Astro => html::RAW_TEXT.contains(&name),
        }
    }

    /// Reads the tag at `start`. `None` when the file ends inside it.
    fn tag(&mut self, start: usize, nesting: usize) -> Option<Tag<'a>> {
        let bytes = self.bytes;
        let mut index = start + 1;
        while index < bytes.len()
            && !html::blank(bytes[index])
            && !matches!(bytes[index], b'/' | b'>')
        {
            index += 1;
        }
        let mut tag = Tag {
            name: &bytes[start + 1..index],
            ..Tag::default()
        };
        let expressions = self.dialect != Dialect::Vue;
        loop {
            while index < bytes.len() && html::blank(bytes[index]) {
                index += 1;
            }
            match *bytes.get(index)? {
                b'>' => {
                    tag.closed = bytes[index - 1] == b'/';
                    tag.end = index + 1;
                    return Some(tag);
                }
                b'/' => index += 1,
                b'{' if expressions => index = self.expression(index, nesting)?,
                _ => {
                    let name_start = index;
                    index += 1;
                    while index < bytes.len()
                        && !html::blank(bytes[index])
                        && !matches!(bytes[index], b'/' | b'>' | b'=')
                    {
                        index += 1;
                    }
                    let name = &bytes[name_start..index];
                    while index < bytes.len() && html::blank(bytes[index]) {
                        index += 1;
                    }
                    if bytes.get(index) != Some(&b'=') {
                        tag.attribute(name, &[]);
                        continue;
                    }
                    index += 1;
                    while index < bytes.len() && html::blank(bytes[index]) {
                        index += 1;
                    }
                    let value_start = index;
                    match *bytes.get(index)? {
                        quote @ (b'"' | b'\'') => {
                            index += 1;
                            let value_start = index;
                            while *bytes.get(index)? != quote {
                                // Svelte reads an expression in a quoted value,
                                // which can hold the same quote.
                                index = if bytes[index] == b'{' && self.dialect == Dialect::Svelte {
                                    self.expression(index, nesting)?
                                } else {
                                    index + 1
                                };
                            }
                            tag.attribute(name, &bytes[value_start..index]);
                            index += 1;
                            continue;
                        }
                        b'{' if expressions => index = self.expression(index, nesting)?,
                        b'`' if self.dialect == Dialect::Astro => {
                            index = self.template(index, nesting)?;
                        }
                        _ => {
                            while index < bytes.len()
                                && !html::blank(bytes[index])
                                && bytes[index] != b'>'
                            {
                                index = if bytes[index] == b'{' && expressions {
                                    self.expression(index, nesting)?
                                } else {
                                    index + 1
                                };
                            }
                        }
                    }
                    tag.attribute(name, &bytes[value_start..index]);
                }
            }
        }
    }

    /// Reads the text of a `<script>` or `<style>`, and gives where it ends.
    fn embedded(&mut self, tag: &Tag<'_>) -> usize {
        let start = tag.end;
        let name = tag.name.to_ascii_lowercase();
        let end = html::end_tag(self.bytes, start, &name).unwrap_or(self.bytes.len());
        if name == b"style" {
            let text = &self.text[start..end];
            let found = match tag.lang {
                None | Some(b"css" | b"postcss") => css::parse(text),
                Some(b"scss" | b"less") => css::parse_with_line_comments(text),
                _ => Vec::new(),
            };
            html::shifted(&mut self.comments, found, start);
        } else if let Some(source_type) = self.source_type(tag) {
            self.script(start..end, source_type);
        }
        end
    }

    /// What a `<script>` holds, when that is JavaScript or TypeScript.
    fn source_type(&self, tag: &Tag<'_>) -> Option<SourceType> {
        if let Some(written) = tag.script_type
            && !written.is_empty()
        {
            html::script_kind(&String::from_utf8_lossy(written))?;
        }
        Some(match tag.lang {
            None if self.dialect == Dialect::Astro => SourceType::ts(),
            None | Some(b"js" | b"javascript") => SourceType::mjs(),
            Some(b"ts" | b"typescript") => SourceType::ts(),
            Some(b"jsx") => SourceType::jsx(),
            Some(b"tsx") => SourceType::tsx(),
            Some(_) => return None,
        })
    }

    fn script(&mut self, range: Range<usize>, source_type: SourceType) {
        let start = range.start;
        let found = javascript::comments(source_type, &self.text[range]);
        html::shifted(&mut self.comments, found, start);
    }

    /// Reads the expression whose `{` is at `start`, and gives where it ends.
    fn expression(&mut self, start: usize, nesting: usize) -> Option<usize> {
        let nesting = nesting.checked_sub(1)?;
        let bytes = self.bytes;
        let mut index = start + 1;
        // The closing tag of a Svelte block, such as `{/if}`.
        if self.dialect == Dialect::Svelte
            && bytes.get(index) == Some(&b'/')
            && !matches!(bytes.get(index + 1), Some(b'/' | b'*'))
        {
            return html::find(bytes, index, b"}").map(|end| end + 1);
        }
        let mut braces = 1usize;
        // The last byte of code, which tells a division from a regular expression.
        let mut last = b'{';
        loop {
            let byte = *bytes.get(index)?;
            let rest = &bytes[index..];
            match byte {
                b'{' => {
                    braces += 1;
                    index += 1;
                }
                b'}' => {
                    braces -= 1;
                    index += 1;
                    if braces == 0 {
                        return Some(index);
                    }
                }
                b'"' | b'\'' => index = self.string(index),
                b'`' => index = self.template(index, nesting)?,
                b'/' if rest.starts_with(b"//") => {
                    let end = self.line_end(index);
                    self.comments.push(Comment {
                        range: index..end,
                        kind: Kind::Line(LineKind::JavaScript(2)),
                    });
                    index = end;
                    continue;
                }
                b'/' if rest.starts_with(b"/*") => {
                    let end = html::find(bytes, index + 2, b"*/")? + 2;
                    let documentation = rest.starts_with(b"/**") && !rest.starts_with(b"/**/");
                    self.comments.push(Comment {
                        range: index..end,
                        kind: Kind::Block {
                            opening: if documentation { 3 } else { 2 },
                            closed: true,
                        },
                    });
                    index = end;
                    continue;
                }
                // Astro's own reading of an expression knows no regular expression.
                b'/' if self.dialect == Dialect::Svelte && starts_regex(last) => {
                    index = self.regex(index).unwrap_or(index + 1);
                }
                b'<' if self.dialect == Dialect::Astro
                    && rest
                        .get(1)
                        .is_some_and(|next| next.is_ascii_alphabetic() || *next == b'>') =>
                {
                    index = self.markup(index, 0, true, nesting)?;
                }
                _ if byte.is_ascii_whitespace() => {
                    index += 1;
                    continue;
                }
                _ => index += 1,
            }
            last = bytes[index - 1];
        }
    }

    /// Where the string at `start` ends: after its quote, or at the end of its line.
    fn string(&self, start: usize) -> usize {
        let quote = self.bytes[start];
        let mut index = start + 1;
        while let Some(&byte) = self.bytes.get(index) {
            match byte {
                b'\\' => index += 2,
                b'\n' | b'\r' => return index,
                _ if byte == quote => return index + 1,
                _ => index += 1,
            }
        }
        self.bytes.len()
    }

    /// Reads the template literal at `start`, with its substitutions.
    fn template(&mut self, start: usize, nesting: usize) -> Option<usize> {
        let mut index = start + 1;
        loop {
            match *self.bytes.get(index)? {
                b'\\' => index += 2,
                b'`' => return Some(index + 1),
                b'$' if self.bytes.get(index + 1) == Some(&b'{') => {
                    index = self.expression(index + 1, nesting)?;
                }
                _ => index += 1,
            }
        }
    }

    /// Where the regular expression at `start` ends, if one ends on its line.
    fn regex(&mut self, start: usize) -> Option<usize> {
        if start < self.no_regex_before {
            return None;
        }
        let mut class = false;
        let mut index = start + 1;
        loop {
            match self.bytes.get(index) {
                None | Some(b'\n' | b'\r') => {
                    self.no_regex_before = index;
                    return None;
                }
                Some(b'\\') => index += 1,
                Some(b'[') => class = true,
                Some(b']') => class = false,
                Some(b'/') if !class => return Some(index + 1),
                Some(_) => {}
            }
            index += 1;
        }
    }

    /// The end of the line comment at `start`, as JavaScript ends lines.
    fn line_end(&self, start: usize) -> usize {
        let rest = &self.bytes[start..];
        rest.iter()
            .enumerate()
            .position(|(at, byte)| {
                matches!(byte, b'\n' | b'\r')
                    || rest[at..].starts_with("\u{2028}".as_bytes())
                    || rest[at..].starts_with("\u{2029}".as_bytes())
            })
            .map_or(self.bytes.len(), |length| start + length)
    }
}

/// Whether a `/` after this byte of code starts a regular expression.
fn starts_regex(last: u8) -> bool {
    !last.is_ascii_alphanumeric()
        && !matches!(last, b'_' | b'$' | b')' | b']' | b'}' | b'"' | b'\'' | b'`')
}
