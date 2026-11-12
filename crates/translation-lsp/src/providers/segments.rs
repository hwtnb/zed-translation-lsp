//! Texts for services that translate without a prompt, such as DeepL and Google
//! Cloud Translation. A text is
//! taken apart into the segments to translate: the lines of a paragraph are
//! joined into one, and a heading, a list item, a quote, and a table cell are
//! each a segment of their own. Code spans and URLs in a segment are marked so
//! that the service leaves them as they are, and code blocks are not sent. The
//! translations are put back after the marks that began their lines.
use std::mem;

/// How a service is told to leave a part of a segment untranslated.
#[derive(Clone, Copy, Debug)]
pub(crate) enum Markup {
    /// `<x i="0">…</x>`, which DeepL keeps with `tag_handling=xml` and
    /// `ignore_tags=x`.
    Xml,
    /// `<span translate="no" class="notranslate" id="i0">…</span>`, which
    /// Google Cloud Translation keeps in HTML.
    Html,
}

impl Markup {
    /// The opening tag of the part at `index`.
    fn open(self, index: usize) -> String {
        match self {
            Self::Xml => format!("<x i=\"{index}\">"),
            Self::Html => {
                format!("<span translate=\"no\" class=\"notranslate\" id=\"i{index}\">")
            }
        }
    }

    fn element(self) -> &'static str {
        match self {
            Self::Xml => "x",
            Self::Html => "span",
        }
    }

    /// The index that the attributes of an opening tag carry. A service may
    /// add attributes of its own, such as the direction of a script.
    fn index(self, attributes: &str) -> Option<usize> {
        let (name, prefix) = match self {
            Self::Xml => (" i=\"", ""),
            Self::Html => (" id=\"", "i"),
        };
        let value = &attributes[attributes.find(name)? + name.len()..];
        value[..value.find('"')?].strip_prefix(prefix)?.parse().ok()
    }
}

/// The texts in groups that each fit in one request: at most `bytes` bytes
/// and `count` texts. A text longer than `bytes` is a group of its own, which
/// the service refuses.
pub(crate) fn batches(texts: &[String], bytes: usize, count: usize) -> Vec<&[String]> {
    let mut batches = Vec::new();
    let mut start = 0;
    let mut size = 0;
    for (index, text) in texts.iter().enumerate() {
        if index > start && (size + text.len() > bytes || index - start == count) {
            batches.push(&texts[start..index]);
            (start, size) = (index, 0);
        }
        size += text.len();
    }
    if start < texts.len() {
        batches.push(&texts[start..]);
    }
    batches
}

/// A translation of one segment, with the language that the service detected
/// in it, which it gives only when no source language was sent.
pub(crate) struct Detected {
    pub text: String,
    pub language: Option<String>,
}

/// Whether the service detected `target` in every segment of `segments`. No
/// segment, or one without a detected language, is not `target`. `aliases`
/// map the lowercase codes of a service to the code it means, such as `zh`
/// to `zh-cn`.
pub(crate) fn all_in(segments: &[Detected], target: &str, aliases: &[(&str, &str)]) -> bool {
    !segments.is_empty()
        && segments.iter().all(|segment| {
            segment
                .language
                .as_deref()
                .is_some_and(|language| same_language(language, target, aliases))
        })
}

/// Whether two language codes name the same language, by the language subtag
/// alone, ignoring case and `-` or `_`: `pt-BR` is `PT`.
fn same_language(a: &str, b: &str, aliases: &[(&str, &str)]) -> bool {
    let normal = |code: &str| {
        let code = code.replace('_', "-").to_ascii_lowercase();
        aliases
            .iter()
            .find(|(alias, _)| *alias == code)
            .map_or(code, |(_, meant)| (*meant).to_owned())
    };
    let (a, b) = (normal(a), normal(b));
    let language = |code: &str| code.split('-').next().unwrap_or_default().to_owned();
    if language(&a) != language(&b) {
        return false;
    }
    // A reader of one Chinese script may not read the other, so Chinese codes
    // match only in full: `zh-Hans` is not a detected `zh`, which names
    // neither.
    language(&a) != "zh" || a == b
}

/// A text taken apart into segments, and the lines to put their translations
/// back into.
#[derive(Debug)]
pub(crate) struct Segmented {
    lines: Vec<Vec<Part>>,
    segments: Vec<Vec<Piece>>,
}

#[derive(Debug)]
enum Part {
    Text(String),
    Segment(usize),
}

#[derive(Debug, PartialEq)]
enum Piece {
    Prose(String),
    /// A code span or a URL, which is sent marked and put back as it was.
    Kept(String),
}

impl Segmented {
    pub fn new(text: &str) -> Self {
        let lines: Vec<&str> = text
            .split('\n')
            .map(|line| line.strip_suffix('\r').unwrap_or(line))
            .collect();
        let decorated = decorated(&lines);
        let mut builder = Builder::default();
        let mut fence = None;
        for line in lines {
            let (decoration, content) = if decorated {
                split_decoration(line)
            } else {
                ("", line)
            };
            if let Some((mark, length)) = fence {
                builder.keep(line);
                if closes_fence(content, mark, length) {
                    fence = None;
                }
                continue;
            }
            if let Some(opened) = opens_fence(content) {
                fence = Some(opened);
                builder.keep(line);
            } else if content.trim().is_empty() || is_break(content) {
                builder.keep(line);
            } else if let Some(prefix) = heading(content) {
                builder.start(decoration, prefix, &content[prefix.len()..], Block::Single);
            } else if let Some(row) = table_row(content) {
                match row {
                    Row::Cells(cells) => builder.row(decoration, cells),
                    Row::Delimiter => builder.keep(line),
                }
            } else if let Some(prefix) = list_item(content) {
                builder.start(decoration, prefix, &content[prefix.len()..], Block::Item);
            } else if let Some(prefix) = quote(content) {
                let rest = &content[prefix.len()..];
                if builder.continues(&Block::Quote(prefix.to_owned())) {
                    builder.join(rest);
                } else {
                    builder.start(decoration, prefix, rest, Block::Quote(prefix.to_owned()));
                }
            } else if !content.trim_start().starts_with('@') && builder.continues(&Block::Paragraph)
            {
                builder.join(content);
            } else {
                let rest = content.trim_start();
                let indent = &content[..content.len() - rest.len()];
                builder.start(decoration, indent, rest, Block::Paragraph);
            }
        }
        Self {
            lines: builder.lines,
            segments: builder.segments.iter().map(|text| pieces(text)).collect(),
        }
    }

    /// The segments that hold something to translate, as the service reads
    /// them, in the order of the text.
    pub fn marked(&self, markup: Markup) -> Vec<String> {
        self.translated()
            .map(|pieces| marked(pieces, markup))
            .collect()
    }

    /// The text with `translations`, one for each of [`Self::marked`], in
    /// place of their segments. `None` when a translation lost or repeated a
    /// marked part.
    pub fn assemble(&self, markup: Markup, translations: &[String]) -> Option<String> {
        let mut translations = translations.iter();
        let mut segments = Vec::with_capacity(self.segments.len());
        for pieces in &self.segments {
            segments.push(if translatable(pieces) {
                restore(translations.next()?, pieces, markup)?
            } else {
                pieces.iter().map(Piece::text).collect()
            });
        }
        if translations.next().is_some() {
            return None;
        }
        let lines: Vec<String> = self
            .lines
            .iter()
            .map(|parts| {
                parts
                    .iter()
                    .map(|part| match part {
                        Part::Text(text) => text.as_str(),
                        Part::Segment(index) => &segments[*index],
                    })
                    .collect()
            })
            .collect();
        Some(lines.join("\n"))
    }

    fn translated(&self) -> impl Iterator<Item = &[Piece]> {
        self.segments
            .iter()
            .map(Vec::as_slice)
            .filter(|pieces| translatable(pieces))
    }
}

impl Piece {
    fn text(&self) -> &str {
        match self {
            Self::Prose(text) | Self::Kept(text) => text,
        }
    }
}

/// What a segment that is still open takes in from the next line.
#[derive(Debug, PartialEq)]
enum Block {
    /// Lines of prose, until one ends with a colon.
    Paragraph,
    /// A list item and the lines after it.
    Item,
    /// Lines with the same quote marks.
    Quote(String),
    /// A heading: nothing.
    Single,
}

#[derive(Default)]
struct Builder {
    lines: Vec<Vec<Part>>,
    segments: Vec<String>,
    open: Option<Block>,
}

impl Builder {
    fn keep(&mut self, line: &str) {
        self.open = None;
        self.lines.push(vec![Part::Text(line.to_owned())]);
    }

    fn start(&mut self, decoration: &str, prefix: &str, rest: &str, block: Block) {
        let rest = rest.trim_end();
        if rest.is_empty() {
            return self.keep(&format!("{decoration}{prefix}"));
        }
        self.segments.push(rest.to_owned());
        self.lines.push(vec![
            Part::Text(format!("{decoration}{prefix}")),
            Part::Segment(self.segments.len() - 1),
        ]);
        self.open = Some(block);
    }

    fn row(&mut self, decoration: &str, cells: Vec<Cell>) {
        self.open = None;
        let mut parts = vec![Part::Text(decoration.to_owned())];
        for cell in cells {
            match cell {
                Cell::Text(text) => parts.push(Part::Text(text.to_owned())),
                Cell::Segment(text) => {
                    self.segments.push(text.to_owned());
                    parts.push(Part::Segment(self.segments.len() - 1));
                }
            }
        }
        self.lines.push(parts);
    }

    /// Whether a line of `block` joins the open segment.
    fn continues(&self, block: &Block) -> bool {
        let Some(open) = &self.open else {
            return false;
        };
        let after_colon = self
            .segments
            .last()
            .is_some_and(|segment| segment.ends_with(':'));
        match (open, block) {
            (Block::Single, _) => false,
            (Block::Paragraph | Block::Item, Block::Paragraph) => !after_colon,
            (Block::Quote(open), Block::Quote(next)) => open == next,
            _ => false,
        }
    }

    fn join(&mut self, line: &str) {
        let line = line.trim();
        let segment = self.segments.last_mut().expect("an open segment");
        if line.is_empty() {
            return;
        }
        let unspaced = segment.chars().next_back().is_some_and(unspaced)
            && line.chars().next().is_some_and(unspaced);
        if !unspaced {
            segment.push(' ');
        }
        segment.push_str(line);
    }
}

/// Whether the script of `character` writes no spaces between words, so that
/// a line break inside a sentence stands for nothing.
fn unspaced(character: char) -> bool {
    matches!(character,
        '\u{3000}'..='\u{30ff}'
        | '\u{3400}'..='\u{4dbf}'
        | '\u{4e00}'..='\u{9fff}'
        | '\u{f900}'..='\u{faff}'
        | '\u{ff00}'..='\u{ffef}')
}

/// Whether every line after the first starts with the indented `*` of a block
/// comment such as `/** … */`, which then decorates the lines instead of
/// marking items. A list item of Markdown starts at the margin.
fn decorated(lines: &[&str]) -> bool {
    let mut rest = lines.iter().skip(1).filter(|line| !line.trim().is_empty());
    let mut any = false;
    rest.all(|line| {
        any = true;
        let content = line.trim_start();
        content.len() < line.len()
            && content
                .strip_prefix('*')
                .is_some_and(|after| after.is_empty() || after.starts_with([' ', '\t']))
    }) && any
}

/// The indentation and the `*` of a decorated line, with one space after it.
fn split_decoration(line: &str) -> (&str, &str) {
    let content = line.trim_start();
    let Some(after) = content.strip_prefix('*') else {
        return ("", line);
    };
    let after = after.strip_prefix(' ').unwrap_or(after);
    line.split_at(line.len() - after.len())
}

fn indentation(line: &str) -> usize {
    line.len() - line.trim_start_matches(' ').len()
}

/// The mark and length of the fence that `line` opens, as in Markdown.
fn opens_fence(line: &str) -> Option<(char, usize)> {
    if indentation(line) > 3 {
        return None;
    }
    let content = line.trim_start_matches(' ');
    let mark = content
        .chars()
        .next()
        .filter(|mark| matches!(mark, '`' | '~'))?;
    let length = content.len() - content.trim_start_matches(mark).len();
    let info = &content[length..];
    (length >= 3 && !(mark == '`' && info.contains('`'))).then_some((mark, length))
}

fn closes_fence(line: &str, mark: char, length: usize) -> bool {
    if indentation(line) > 3 {
        return false;
    }
    let content = line.trim_start_matches(' ');
    let rest = content.trim_start_matches(mark);
    content.len() - rest.len() >= length && rest.trim().is_empty()
}

/// A thematic break, or the underline of a heading.
fn is_break(line: &str) -> bool {
    if indentation(line) > 3 {
        return false;
    }
    let marks: String = line.chars().filter(|c| !c.is_whitespace()).collect();
    let Some(mark) = marks.chars().next() else {
        return false;
    };
    match mark {
        '-' | '*' | '_' => marks.len() >= 3 && marks.chars().all(|c| c == mark),
        '=' => line.trim().chars().all(|c| c == '='),
        _ => false,
    }
}

/// The marks and spaces before the text of a heading.
fn heading(line: &str) -> Option<&str> {
    if indentation(line) > 3 {
        return None;
    }
    let content = line.trim_start_matches(' ');
    let level = content.len() - content.trim_start_matches('#').len();
    let after = &content[level..];
    if !(1..=6).contains(&level) || !(after.is_empty() || after.starts_with([' ', '\t'])) {
        return None;
    }
    let text = after.trim_start();
    Some(&line[..line.len() - text.len()])
}

/// The indentation, mark, spaces, and task box before the text of a list item.
fn list_item(line: &str) -> Option<&str> {
    let content = line.trim_start();
    let marker = if content.starts_with(['-', '*', '+']) {
        1
    } else {
        let digits = content.len()
            - content
                .trim_start_matches(|c: char| c.is_ascii_digit())
                .len();
        if !(1..=9).contains(&digits) || !content[digits..].starts_with(['.', ')']) {
            return None;
        }
        digits + 1
    };
    let after = &content[marker..];
    if !after.starts_with([' ', '\t']) {
        return None;
    }
    let mut text = after.trim_start();
    for task in ["[ ] ", "[x] ", "[X] "] {
        if let Some(rest) = text.strip_prefix(task) {
            text = rest.trim_start();
        }
    }
    Some(&line[..line.len() - text.len()])
}

/// The indentation and quote marks before the text of a quote.
fn quote(line: &str) -> Option<&str> {
    if indentation(line) > 3 || !line.trim_start().starts_with('>') {
        return None;
    }
    let text = line.trim_start_matches([' ', '>']);
    Some(&line[..line.len() - text.len()])
}

enum Row<'a> {
    Cells(Vec<Cell<'a>>),
    /// The row under the header, which is all dashes.
    Delimiter,
}

enum Cell<'a> {
    Text(&'a str),
    Segment(&'a str),
}

/// The pipes and spaces of a table row, and its cells. `None` for a line that
/// is no row.
fn table_row(line: &str) -> Option<Row<'_>> {
    let content = line.trim_start();
    if !content.starts_with('|') {
        return None;
    }
    let mut cells = Vec::new();
    let mut start = 0;
    let mut code = None;
    let mut delimiter = true;
    for (index, character) in line.char_indices() {
        match character {
            '`' => code = if code.is_some() { None } else { Some(index) },
            '|' if code.is_none() && !line[..index].ends_with('\\') => {
                cell(&line[start..index], &mut cells, &mut delimiter);
                cells.push(Cell::Text(&line[index..index + 1]));
                start = index + 1;
            }
            _ => (),
        }
    }
    cell(&line[start..], &mut cells, &mut delimiter);
    Some(if delimiter {
        Row::Delimiter
    } else {
        Row::Cells(cells)
    })
}

/// Adds the spaces and text of one cell, and notes a text that is not all
/// dashes.
fn cell<'a>(text: &'a str, cells: &mut Vec<Cell<'a>>, delimiter: &mut bool) {
    let content = text.trim();
    if content.is_empty() {
        cells.push(Cell::Text(text));
        return;
    }
    let dashes = content.trim_matches(':');
    if dashes.is_empty() || !dashes.chars().all(|c| c == '-') {
        *delimiter = false;
    }
    let start = text.len() - text.trim_start().len();
    let end = start + content.len();
    cells.push(Cell::Text(&text[..start]));
    cells.push(Cell::Segment(content));
    cells.push(Cell::Text(&text[end..]));
}

/// A segment with its code spans and URLs apart from its prose.
fn pieces(text: &str) -> Vec<Piece> {
    let mut pieces = Vec::new();
    let mut prose = String::new();
    let mut rest = text;
    while let Some(character) = rest.chars().next() {
        let kept = match character {
            '`' => code_span(rest),
            'h' if !prose.ends_with(|c: char| c.is_alphanumeric()) => url(rest),
            _ => None,
        };
        if let Some(length) = kept {
            if !prose.is_empty() {
                pieces.push(Piece::Prose(mem::take(&mut prose)));
            }
            pieces.push(Piece::Kept(rest[..length].to_owned()));
            rest = &rest[length..];
        } else {
            let length = if character == '`' {
                rest.len() - rest.trim_start_matches('`').len()
            } else {
                character.len_utf8()
            };
            prose.push_str(&rest[..length]);
            rest = &rest[length..];
        }
    }
    if !prose.is_empty() {
        pieces.push(Piece::Prose(prose));
    }
    pieces
}

/// The length of the code span at the start of `text`: a run of backticks up
/// to the next run of the same length.
fn code_span(text: &str) -> Option<usize> {
    let ticks = text.len() - text.trim_start_matches('`').len();
    let mut offset = ticks;
    while let Some(found) = text[offset..].find('`') {
        let start = offset + found;
        let run = text[start..].len() - text[start..].trim_start_matches('`').len();
        if run == ticks {
            return Some(start + run);
        }
        offset = start + run;
    }
    None
}

/// The length of the URL at the start of `text`, without the punctuation that
/// ends the sentence around it.
fn url(text: &str) -> Option<usize> {
    if !(text.starts_with("https://") || text.starts_with("http://")) {
        return None;
    }
    let mut url = text
        .split(|c: char| c.is_whitespace() || matches!(c, '<' | '>' | '"' | '`'))
        .next()?;
    loop {
        let trimmed = url.trim_end_matches(['.', ',', ';', ':', '!', '?', '\'', '*', '_']);
        let unbalanced = |open: char, close: char| {
            trimmed.ends_with(close)
                && trimmed.matches(open).count() < trimmed.matches(close).count()
        };
        if unbalanced('(', ')') || unbalanced('[', ']') {
            url = &trimmed[..trimmed.len() - 1];
        } else if trimmed.len() < url.len() {
            url = trimmed;
        } else {
            break;
        }
    }
    (url.len() > "https://".len()).then_some(url.len())
}

/// Whether a segment holds a letter outside its code spans and URLs.
fn translatable(pieces: &[Piece]) -> bool {
    pieces.iter().any(|piece| match piece {
        Piece::Prose(text) => text.chars().any(char::is_alphabetic),
        Piece::Kept(_) => false,
    })
}

fn marked(pieces: &[Piece], markup: Markup) -> String {
    let mut text = String::new();
    let mut kept = 0;
    for piece in pieces {
        match piece {
            Piece::Prose(prose) => escape(prose, &mut text),
            Piece::Kept(code) => {
                text.push_str(&markup.open(kept));
                escape(code, &mut text);
                text.push_str(&format!("</{}>", markup.element()));
                kept += 1;
            }
        }
    }
    text
}

fn escape(text: &str, into: &mut String) {
    for character in text.chars() {
        match character {
            '&' => into.push_str("&amp;"),
            '<' => into.push_str("&lt;"),
            '>' => into.push_str("&gt;"),
            _ => into.push(character),
        }
    }
}

/// The translation with each marked part as it was in `pieces`.
fn restore(translation: &str, pieces: &[Piece], markup: Markup) -> Option<String> {
    let kept: Vec<&str> = pieces
        .iter()
        .filter_map(|piece| match piece {
            Piece::Kept(code) => Some(code.as_str()),
            Piece::Prose(_) => None,
        })
        .collect();
    let mut seen = vec![false; kept.len()];
    let mut text = String::new();
    let mut rest = translation;
    let opening = format!("<{} ", markup.element());
    let closing = format!("</{}>", markup.element());
    while let Some(start) = rest.find(&opening) {
        text.push_str(&unescape(&rest[..start])?);
        let tag = &rest[start..];
        let end = tag.find('>')?;
        let index = markup.index(&tag[..end])?;
        let after = if tag[..end].ends_with('/') {
            &tag[end + 1..]
        } else {
            let close = tag.find(&closing)?;
            &tag[close + closing.len()..]
        };
        if mem::replace(seen.get_mut(index)?, true) {
            return None;
        }
        text.push_str(kept[index]);
        rest = after;
    }
    text.push_str(&unescape(rest)?);
    seen.iter().all(|seen| *seen).then_some(text)
}

/// The text that the character references in `text` stand for. `None` when a
/// tag is left, which the service added or broke.
fn unescape(text: &str) -> Option<String> {
    if text.contains('<') {
        return None;
    }
    let mut result = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find('&') {
        result.push_str(&rest[..start]);
        rest = &rest[start..];
        let reference = rest.find(';').map(|end| &rest[1..end]);
        let character = reference.and_then(|name| match name {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" => Some('\''),
            "nbsp" => Some('\u{a0}'),
            _ => {
                let number = name.strip_prefix('#')?;
                let code = match number.strip_prefix(['x', 'X']) {
                    Some(hex) => u32::from_str_radix(hex, 16).ok()?,
                    None => number.parse().ok()?,
                };
                char::from_u32(code)
            }
        });
        match (character, reference) {
            (Some(character), Some(name)) => {
                result.push(character);
                rest = &rest[name.len() + 2..];
            }
            _ => {
                result.push('&');
                rest = &rest[1..];
            }
        }
    }
    result.push_str(rest);
    Some(result)
}

#[cfg(test)]
mod tests;
