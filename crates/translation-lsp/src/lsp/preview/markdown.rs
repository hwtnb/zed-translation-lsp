//! The Markdown that Hover shows a translation in. The formatting that the
//! translation writes on purpose stays: code, headings, lists, block quotes,
//! tables, rules, `*` emphasis, and links to URLs of the original. Every other
//! character shows as written, and each line break of the translation shows as
//! a line break.
//!
//! Zed makes a link of every URL in text, and ends a URL where it sees fit.
//! So the server makes the links itself, `<URL>` for the URLs of the original,
//! and puts [`LINK_BREAK`] in every other URL so that Zed makes no link of it.

use std::{
    collections::{HashMap, HashSet},
    iter::from_fn,
    ops::Range,
};

use pulldown_cmark::{Event, LinkType, Options, Parser, Tag, TagEnd};

/// The options of Zed's Markdown parser: `PARSE_OPTIONS` in its
/// `crates/markdown/src/parser.rs`.
const ZED_OPTIONS: Options = Options::ENABLE_TABLES
    .union(Options::ENABLE_FOOTNOTES)
    .union(Options::ENABLE_STRIKETHROUGH)
    .union(Options::ENABLE_TASKLISTS)
    .union(Options::ENABLE_SMART_PUNCTUATION)
    .union(Options::ENABLE_HEADING_ATTRIBUTES)
    .union(Options::ENABLE_PLUSES_DELIMITED_METADATA_BLOCKS)
    .union(Options::ENABLE_OLD_FOOTNOTES)
    .union(Options::ENABLE_GFM)
    .union(Options::ENABLE_SUPERSCRIPT)
    .union(Options::ENABLE_SUBSCRIPT);

/// The options that read the formatting a translation keeps. Every other
/// extension is left out, so that its markup reads as text and is escaped.
const KEPT_OPTIONS: Options = Options::ENABLE_TABLES;

/// A word joiner, put after the `:` of a URL so that Zed makes no link of it.
const LINK_BREAK: char = '\u{2060}';

/// The longest URL, in bytes, that can be a link.
const MAX_URL: usize = 2048;

/// How many characters of punctuation after a URL of the translation the
/// matching tries to leave out of it.
const MAX_TRAILING: usize = 64;

/// The URLs of an original text, which the links of its translation may open.
#[derive(Clone, Default)]
pub(crate) struct Urls {
    /// Each URL written in the text, without the punctuation of the sentence
    /// after it.
    written: HashSet<String>,
    /// The destinations of the links and images of a Markdown text.
    destinations: HashSet<String>,
}

impl Urls {
    /// The URLs of `original`, a text that is not Markdown.
    pub fn of(original: &str) -> Self {
        Self {
            written: written_urls(original),
            destinations: HashSet::new(),
        }
    }

    /// The URLs of `original`, a Markdown text.
    pub fn of_markdown(original: &str) -> Self {
        let destinations = Parser::new_ext(original, ZED_OPTIONS)
            .filter_map(|event| match event {
                Event::Start(Tag::Link { dest_url, .. } | Tag::Image { dest_url, .. }) => {
                    Some(dest_url.into_string())
                }
                _ => None,
            })
            .collect();
        Self {
            written: written_urls(original),
            destinations,
        }
    }

    fn written(&self, url: &str) -> bool {
        self.written.contains(url)
    }

    fn destination(&self, url: &str) -> bool {
        self.written.contains(url) || self.destinations.contains(url)
    }
}

/// `translation` as Markdown that Zed shows with the formatting that stays and
/// every other character as written. The only character shown that the
/// translation does not write is [`LINK_BREAK`].
pub(crate) fn render(translation: &str, urls: &Urls) -> String {
    rendered(translation, urls, false).markdown
}

/// The translation of a Markdown document as its own Markdown, with links
/// only to the URLs of the original, or `None` if it links elsewhere.
pub(crate) fn document(translation: &str, urls: &Urls) -> Option<String> {
    let bytes = translation.as_bytes();
    let mut kinds = vec![Kind::Other; bytes.len()];
    let mut links = 0usize;
    let mut images = 0usize;
    let mut in_code = false;
    for (event, range) in Parser::new_ext(translation, ZED_OPTIONS).into_offset_iter() {
        match event {
            Event::Start(Tag::Link {
                link_type,
                dest_url,
                ..
            }) => {
                if !stays(link_type, &dest_url, urls, false) {
                    return None;
                }
                links += 1;
            }
            Event::Start(Tag::Image {
                link_type,
                dest_url,
                ..
            }) => {
                if !stays(link_type, &dest_url, urls, false) {
                    return None;
                }
                images += 1;
            }
            Event::End(TagEnd::Link) => links -= 1,
            Event::End(TagEnd::Image) => images -= 1,
            Event::Start(Tag::CodeBlock(_)) => in_code = true,
            Event::End(TagEnd::CodeBlock) => in_code = false,
            Event::Text(_) if links == 0 && !in_code => {
                kinds[range].fill(if images == 0 { Kind::Text } else { Kind::Inert });
            }
            Event::Html(_) | Event::InlineHtml(_) => kinds[range].fill(Kind::Inert),
            _ => (),
        }
    }
    let mut links = Vec::new();
    let mut breaks = Vec::new();
    let starts = scheme_starts(translation);
    for (colon, _) in translation.match_indices("://") {
        match kinds[colon] {
            Kind::Text => match starts.get(&colon).and_then(|&start| {
                link_end(translation, start, colon, urls, |index| {
                    kinds[index] == Kind::Text
                })
                .map(|end| start..end)
            }) {
                Some(link) => links.push(link),
                None => breaks.push(colon),
            },
            Kind::Inert => breaks.push(colon),
            Kind::Other => (),
        }
    }
    let mut markdown = Writer::new(false);
    let mut links = links.into_iter().peekable();
    let mut index = 0;
    while index < bytes.len() {
        if let Some(link) = links.next_if(|link| link.start == index) {
            markdown.added('<');
            markdown.kept_str(&translation[link.clone()], link.start);
            markdown.added('>');
            index = link.end;
            continue;
        }
        let character = translation[index..]
            .chars()
            .next()
            .expect("index is a boundary");
        markdown.kept(index, character);
        if character == ':' && breaks.binary_search(&index).is_ok() {
            markdown.added(LINK_BREAK);
        }
        index += character.len_utf8();
    }
    links_stay(&markdown.markdown, urls, false).then_some(markdown.markdown)
}

/// What Zed does with a byte of a Markdown document.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    /// Text outside links and code, where Zed makes links of URLs and the
    /// server may make links of the URLs of the original.
    Text,
    /// Text where Zed makes links but the server makes none: the text of an
    /// image, and HTML.
    Inert,
    Other,
}

/// Whether a link or image of a translation may stay. `bare`: the links are
/// those of [`render`], to URLs written in the original. Otherwise the
/// destination must be one of the original's or a `#section`: Zed opens any
/// other destination as a URL, a file, or a path on the network.
fn stays(link_type: LinkType, destination: &str, urls: &Urls, bare: bool) -> bool {
    if bare {
        matches!(link_type, LinkType::Inline | LinkType::Autolink) && urls.written(destination)
    } else {
        destination.starts_with('#') || urls.destination(destination)
    }
}

/// [`render`], with the origin of each byte when `trace`.
fn rendered(translation: &str, urls: &Urls, trace: bool) -> Writer {
    // The parser reads some blocks differently before a CR alone.
    let translation = translation.replace("\r\n", "\n").replace('\r', "\n");
    formatted(&translation, urls, trace).unwrap_or_else(|| literal(&translation, trace))
}

/// `translation` with the formatting that stays, or `None` where Zed would not
/// show it as the roles promise. The roles come from reading the translation
/// without Zed's extensions, and escaping can join lines that the translation
/// kept apart.
fn formatted(translation: &str, urls: &Urls, trace: bool) -> Option<Writer> {
    let roles = roles(translation, urls);
    let (links, breaks) = bare_urls(translation, &roles, urls);
    let markdown = hard_breaks(escaped(translation, &roles, &links, &breaks, trace));
    (links_stay(&markdown.markdown, urls, true)
        && shown(&markdown.markdown)? == expected(translation, &roles))
    .then_some(markdown)
}

/// Markdown being written, with the byte of the translation that each of its
/// bytes comes from, when traced for tests.
struct Writer {
    markdown: String,
    origins: Option<Vec<Option<usize>>>,
}

impl Writer {
    fn new(trace: bool) -> Self {
        Self {
            markdown: String::new(),
            origins: trace.then(Vec::new),
        }
    }

    /// Writes `character`, byte `index` of the translation.
    fn kept(&mut self, index: usize, character: char) {
        self.markdown.push(character);
        if let Some(origins) = &mut self.origins {
            origins.extend((index..index + character.len_utf8()).map(Some));
        }
    }

    /// Writes `text`, which starts at byte `start` of the translation.
    fn kept_str(&mut self, text: &str, start: usize) {
        self.markdown.push_str(text);
        if let Some(origins) = &mut self.origins {
            origins.extend((start..start + text.len()).map(Some));
        }
    }

    /// Writes `character`, which the translation does not have.
    fn added(&mut self, character: char) {
        self.markdown.push(character);
        if let Some(origins) = &mut self.origins {
            origins.extend((0..character.len_utf8()).map(|_| None));
        }
    }

    /// Writes `character`, byte `index` of the translation, so that it shows as
    /// written.
    fn text(&mut self, index: usize, character: char, breaks: &[usize]) {
        if character.is_ascii_punctuation() {
            self.added('\\');
        }
        self.kept(index, character);
        if character == ':' && breaks.binary_search(&index).is_ok() {
            self.added(LINK_BREAK);
        }
    }
}

/// What a byte of the translation is in the Markdown.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Role {
    /// Text that shows as written: its ASCII punctuation is escaped. The
    /// server makes links of the URLs of the original in it.
    Text,
    /// Text inside a link that stays, where Zed makes no links.
    LinkText,
    /// Text inside formatting that does not stay, such as a link to another
    /// URL: none of its URLs is a link.
    Inert,
    /// Text of code, which shows as written without escapes.
    Code,
    /// Formatting that stays. It is copied and does not show.
    Markup,
}

impl Role {
    fn escaped(self) -> bool {
        matches!(self, Self::Text | Self::LinkText | Self::Inert)
    }
}

/// The construct that a byte outside every event belongs to.
#[derive(Clone, Copy)]
enum Owner {
    /// The document, or a block whose own bytes are only `>` of block quotes.
    Block,
    /// A list item whose marker ends before `marker_end`.
    Item {
        marker_end: usize,
    },
    Heading,
    Table,
    CodeBlock,
    /// Emphasis or strong emphasis written with `*`.
    Star,
    /// A link `[text](URL)` to one of the URLs of the original, whose `[` is
    /// at `start`.
    Link {
        start: usize,
    },
    /// A link `<URL>` to one of the URLs of the original.
    Autolink,
    /// Formatting that does not stay, such as `_` emphasis, images, and links
    /// to other URLs.
    Inert,
}

impl Owner {
    fn of(tag: &Tag, start: usize, text: &[u8], urls: &Urls) -> Self {
        match tag {
            Tag::Item => Self::Item {
                marker_end: start + marker_length(&text[start..]),
            },
            Tag::Heading { .. } => Self::Heading,
            Tag::Table(_) | Tag::TableHead | Tag::TableRow | Tag::TableCell => Self::Table,
            Tag::CodeBlock(_) => Self::CodeBlock,
            Tag::Emphasis | Tag::Strong if text[start] == b'*' => Self::Star,
            Tag::Link {
                link_type: LinkType::Inline,
                dest_url,
                ..
            } if urls.written(dest_url) => Self::Link { start },
            Tag::Link {
                link_type: LinkType::Autolink,
                dest_url,
                ..
            } if urls.written(dest_url) => Self::Autolink,
            Tag::Paragraph | Tag::BlockQuote(_) | Tag::List(_) | Tag::HtmlBlock => Self::Block,
            _ => Self::Inert,
        }
    }

    /// The role of byte `index`, `byte`, that this construct owns. `prefix`:
    /// only whitespace and markers of block quotes and list items precede it
    /// on its line. `closing`: it follows the last event inside the construct.
    fn role(self, index: usize, byte: u8, prefix: bool, closing: bool) -> Role {
        // A `>` that is text is inside a text event, so an owned `>` that
        // starts a line marks a block quote.
        if byte == b'>' && prefix {
            return Role::Markup;
        }
        let markup = match self {
            Self::Block | Self::Inert => false,
            Self::Item { marker_end } => index < marker_end,
            Self::Heading => matches!(byte, b'#' | b'=' | b'-'),
            Self::Table => matches!(byte, b'|' | b'-' | b':'),
            Self::CodeBlock | Self::Autolink => true,
            Self::Star => byte == b'*',
            Self::Link { start } => closing || index == start,
        };
        if markup { Role::Markup } else { Role::Text }
    }
}

/// The length of the marker that starts a list item: a bullet, or digits and
/// `.` or `)`.
fn marker_length(item: &[u8]) -> usize {
    let digits = item.iter().take_while(|b| b.is_ascii_digit()).count();
    if digits > 0 {
        digits + usize::from(matches!(item.get(digits), Some(b'.' | b')')))
    } else {
        usize::from(matches!(item.first(), Some(b'-' | b'+' | b'*')))
    }
}

/// Assigns roles to the bytes of a text in order.
struct Roles<'a> {
    text: &'a [u8],
    roles: Vec<Role>,
    cursor: usize,
    prefix: bool,
    /// How many links that stay, and how many constructs that do not, are
    /// open around the cursor.
    links: usize,
    inert: usize,
}

impl Roles<'_> {
    /// Assigns roles up to `end`, from where the previous call stopped.
    fn fill(&mut self, end: usize, role: impl Fn(usize, u8, bool) -> Role) {
        while self.cursor < end.min(self.text.len()) {
            let index = self.cursor;
            let byte = self.text[index];
            let assigned = match role(index, byte, self.prefix) {
                Role::Text if self.inert > 0 => Role::Inert,
                Role::Text if self.links > 0 => Role::LinkText,
                role => role,
            };
            self.roles[index] = assigned;
            self.prefix = match byte {
                b'\n' | b'\r' => true,
                b' ' | b'\t' => self.prefix,
                b'>' | b'-' | b'+' | b'*' | b'.' | b')' | b'0'..=b'9' => {
                    self.prefix && assigned == Role::Markup
                }
                _ => false,
            };
            self.cursor += 1;
        }
    }

    fn open(&mut self, owner: Owner, by: isize) {
        let count = match owner {
            Owner::Link { .. } | Owner::Autolink => &mut self.links,
            Owner::Inert => &mut self.inert,
            _ => return,
        };
        *count = count.saturating_add_signed(by);
    }
}

/// The role of each byte of `text`, from reading it with [`KEPT_OPTIONS`].
fn roles(text: &str, urls: &Urls) -> Vec<Role> {
    let bytes = text.as_bytes();
    let mut roles = Roles {
        text: bytes,
        roles: vec![Role::Text; bytes.len()],
        cursor: 0,
        prefix: true,
        links: 0,
        inert: 0,
    };
    let mut owners: Vec<Owner> = Vec::new();
    for (event, range) in Parser::new_ext(text, KEPT_OPTIONS).into_offset_iter() {
        let owner = owners.last().copied().unwrap_or(Owner::Block);
        let own = |closing| move |index, byte, prefix| owner.role(index, byte, prefix, closing);
        match event {
            Event::Start(tag) => {
                roles.fill(range.start, own(false));
                let opened = Owner::of(&tag, range.start, bytes, urls);
                roles.open(opened, 1);
                owners.push(opened);
            }
            Event::End(_) => {
                roles.fill(range.end, own(true));
                roles.open(owner, -1);
                owners.pop();
            }
            leaf => {
                roles.fill(range.start, own(false));
                let source = &bytes[range.clone()];
                match leaf {
                    // The text of code, and the URL of `<URL>`, where escapes
                    // would show.
                    Event::Text(_) if matches!(owner, Owner::CodeBlock | Owner::Autolink) => {
                        roles.fill(range.end, |_, _, _| Role::Code);
                    }
                    // A code span across lines would show its line break as a
                    // space, so it shows as text instead.
                    Event::Code(_) if !source.contains(&b'\n') && !source.contains(&b'\r') => {
                        let ticks = source.iter().take_while(|b| **b == b'`').count();
                        let content = range.start + ticks..range.end - ticks;
                        roles.fill(range.end, |index, _, _| {
                            if content.contains(&index) {
                                Role::Code
                            } else {
                                Role::Markup
                            }
                        });
                    }
                    Event::Rule => roles.fill(range.end, |_, _, _| Role::Markup),
                    _ => roles.fill(range.end, |_, _, _| Role::Text),
                }
            }
        }
    }
    roles.fill(bytes.len(), |index, byte, prefix| {
        Owner::Block.role(index, byte, prefix, false)
    });
    roles.roles
}

/// The URLs of the original in the text of `text`, which become links, and
/// the `:` of every other URL in its text, which gets [`LINK_BREAK`].
fn bare_urls(text: &str, roles: &[Role], urls: &Urls) -> (Vec<Range<usize>>, Vec<usize>) {
    let starts = scheme_starts(text);
    let mut links = Vec::new();
    let mut breaks = Vec::new();
    for (colon, _) in text.match_indices("://") {
        match roles[colon] {
            Role::Text => match starts.get(&colon).and_then(|&start| {
                link_end(text, start, colon, urls, |index| roles[index] == Role::Text)
                    .map(|end| start..end)
            }) {
                Some(link) => links.push(link),
                None => breaks.push(colon),
            },
            Role::Inert => breaks.push(colon),
            Role::LinkText | Role::Code | Role::Markup => (),
        }
    }
    (links, breaks)
}

/// The end of the longest URL of the original that the text at `start`
/// begins with and that no character of a URL follows: after the
/// punctuation that may follow it, the text ends or has whitespace or a
/// character outside ASCII. Every byte of the URL must be `linkable`.
fn link_end(
    text: &str,
    start: usize,
    colon: usize,
    urls: &Urls,
    linkable: impl Fn(usize) -> bool,
) -> Option<usize> {
    let bytes = text.as_bytes();
    let stretch = colon
        + bytes[colon..]
            .iter()
            .take(start + MAX_URL + MAX_TRAILING + 1 - colon)
            .take_while(|b| b.is_ascii_graphic())
            .count();
    if stretch - start > MAX_URL + MAX_TRAILING {
        return None;
    }
    let unlinkable = (start..stretch)
        .find(|&index| !linkable(index))
        .unwrap_or(stretch);
    let punctuation = bytes[..stretch]
        .iter()
        .rev()
        .take(MAX_TRAILING)
        .take_while(|b| b.is_ascii_punctuation())
        .count();
    (stretch - punctuation..=stretch)
        .rev()
        .filter(|&end| end > colon + 3 && end <= unlinkable && end - start <= MAX_URL)
        .find(|&end| urls.written(&text[start..end]))
}

/// The start of the scheme of each `://` of `text` that can start a URL, by
/// its `:`. A `://` without a scheme of 2 to 32 characters is left out, and
/// so is every `://` of a stretch of ASCII without whitespace that has more
/// than one, which keeps the matching linear.
fn scheme_starts(text: &str) -> HashMap<usize, usize> {
    let bytes = text.as_bytes();
    let colons: Vec<usize> = text.match_indices("://").map(|(colon, _)| colon).collect();
    let mut starts = HashMap::new();
    let mut index = 0;
    while let Some(&colon) = colons.get(index) {
        let end = colon
            + bytes[colon..]
                .iter()
                .take_while(|b| b.is_ascii_graphic())
                .count();
        let shared = colons[index + 1..]
            .iter()
            .take_while(|&&next| next < end)
            .count();
        if shared == 0 {
            let start = bytes[..colon]
                .iter()
                .rposition(|b| !is_scheme_byte(*b))
                .map_or(0, |before| before + 1);
            if (2..=32).contains(&(colon - start)) && bytes[start].is_ascii_alphabetic() {
                starts.insert(colon, start);
            }
        }
        index += shared + 1;
    }
    starts
}

fn is_scheme_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'+' | b'-' | b'.')
}

/// The URLs written in `text`, each without the punctuation of the sentence
/// after it: `(see https://example.com/docs).` writes `https://example.com/docs`.
fn written_urls(text: &str) -> HashSet<String> {
    let bytes = text.as_bytes();
    scheme_starts(text)
        .into_iter()
        .filter_map(|(colon, start)| {
            let end = colon
                + 3
                + bytes[colon + 3..]
                    .iter()
                    .take(MAX_URL)
                    .take_while(|b| is_url_byte(**b))
                    .count();
            let url = without_sentence_punctuation(&text[start..end]);
            (url.len() > colon - start + 3 && url.len() <= MAX_URL).then(|| url.to_owned())
        })
        .collect()
}

/// Whether a URL can have `byte` as written, without percent-encoding.
fn is_url_byte(byte: u8) -> bool {
    byte.is_ascii_graphic()
        && !matches!(
            byte,
            b'<' | b'>' | b'"' | b'`' | b'\\' | b'^' | b'{' | b'}' | b'|' | b'[' | b']'
        )
}

/// `url` without the punctuation at its end that ends a sentence or a
/// parenthesis around it. A `)` stays when it closes a `(` of the URL.
fn without_sentence_punctuation(url: &str) -> &str {
    let bytes = url.as_bytes();
    let opening = bytes.iter().filter(|b| **b == b'(').count();
    let mut closing = bytes.iter().filter(|b| **b == b')').count();
    let mut end = bytes.len();
    while let Some(&last) = bytes[..end].last() {
        match last {
            b'.' | b',' | b':' | b';' | b'!' | b'?' | b'\'' | b'*' | b'_' | b'~' => end -= 1,
            b')' if closing > opening => {
                closing -= 1;
                end -= 1;
            }
            _ => break,
        }
    }
    &url[..end]
}

/// `text` with its text escaped, the URLs of `links` made links, and its
/// code and formatting as written.
fn escaped(
    text: &str,
    roles: &[Role],
    links: &[Range<usize>],
    breaks: &[usize],
    trace: bool,
) -> Writer {
    let mut markdown = Writer::new(trace);
    markdown.markdown.reserve(text.len() * 2);
    let mut links = links.iter().peekable();
    let mut index = 0;
    while index < text.len() {
        if let Some(link) = links.next_if(|link| link.start == index) {
            markdown.added('<');
            markdown.kept_str(&text[link.clone()], link.start);
            markdown.added('>');
            index = link.end;
            continue;
        }
        let character = text[index..].chars().next().expect("index is a boundary");
        if roles[index].escaped() {
            markdown.text(index, character, breaks);
        } else {
            markdown.kept(index, character);
        }
        index += character.len_utf8();
    }
    markdown
}

/// `markdown` with each line break inside a paragraph made a hard break.
/// A backslash at the end of the last line of a paragraph would show, so
/// only the line breaks that Zed reads as soft breaks change.
fn hard_breaks(markdown: Writer) -> Writer {
    let text = &markdown.markdown;
    let positions: Vec<usize> = Parser::new_ext(text, ZED_OPTIONS)
        .into_offset_iter()
        .filter(|(event, _)| matches!(event, Event::SoftBreak))
        .filter_map(|(_, range)| Some(range.start + text[range].find(['\n', '\r'])?))
        .collect();
    if positions.is_empty() {
        return markdown;
    }
    let mut broken = Writer {
        markdown: String::with_capacity(text.len() + positions.len()),
        origins: markdown
            .origins
            .as_ref()
            .map(|origins| Vec::with_capacity(origins.len() + positions.len())),
    };
    let mut copied = 0;
    for position in positions.into_iter().chain([text.len()]) {
        broken.markdown.push_str(&text[copied..position]);
        if let (Some(origins), Some(from)) = (&mut broken.origins, &markdown.origins) {
            origins.extend_from_slice(&from[copied..position]);
        }
        if position < text.len() {
            broken.added('\\');
        }
        copied = position;
    }
    broken
}

/// Whether every link and image of `markdown` may stay, and no text outside
/// links and code has a URL that Zed would make a link of. Zed looks for URLs
/// in the text of consecutive text events, which escapes and character
/// references divide. `bare`: as in [`stays`]; such Markdown has no images.
fn links_stay(markdown: &str, urls: &Urls, bare: bool) -> bool {
    let mut links = 0usize;
    let mut in_code = false;
    let mut text = String::new();
    for event in Parser::new_ext(markdown, ZED_OPTIONS) {
        if let Event::Text(part) = &event
            && links == 0
            && !in_code
        {
            text.push_str(part);
            continue;
        }
        if text.contains("://") {
            return false;
        }
        text.clear();
        match event {
            Event::Start(Tag::Link {
                link_type,
                dest_url,
                ..
            }) => {
                if !stays(link_type, &dest_url, urls, bare) {
                    return false;
                }
                links += 1;
            }
            Event::Start(Tag::Image {
                link_type,
                dest_url,
                ..
            }) => {
                if bare || !stays(link_type, &dest_url, urls, bare) {
                    return false;
                }
            }
            Event::End(TagEnd::Link) => links -= 1,
            Event::Start(Tag::CodeBlock(_)) => in_code = true,
            Event::End(TagEnd::CodeBlock) => in_code = false,
            _ => (),
        }
    }
    !text.contains("://")
}

/// The lines of text that Zed shows for `markdown`, without whitespace and
/// [`LINK_BREAK`], or `None` if it shows formatting that does not stay, such
/// as HTML, images, or footnotes.
fn shown(markdown: &str) -> Option<Vec<String>> {
    let mut text = String::new();
    for event in Parser::new_ext(markdown, ZED_OPTIONS) {
        match event {
            Event::Text(part) | Event::Code(part) => text.push_str(&part),
            Event::SoftBreak => text.push(' '),
            Event::End(TagEnd::Emphasis | TagEnd::Strong | TagEnd::Link | TagEnd::TableCell) => (),
            Event::HardBreak | Event::Rule | Event::End(_) => text.push('\n'),
            Event::Start(Tag::Emphasis | Tag::Strong | Tag::TableCell | Tag::Link { .. }) => (),
            Event::Start(
                Tag::Paragraph
                | Tag::Heading { .. }
                | Tag::BlockQuote(None)
                | Tag::CodeBlock(_)
                | Tag::List(_)
                | Tag::Item
                | Tag::Table(_)
                | Tag::TableHead
                | Tag::TableRow,
            ) => text.push('\n'),
            _ => return None,
        }
    }
    Some(lines(
        text.split(['\n', '\r'])
            .map(|line| line.chars().map(|c| (c, true))),
    ))
}

/// The lines of text that the roles promise to show, without whitespace and
/// [`LINK_BREAK`].
fn expected(text: &str, roles: &[Role]) -> Vec<String> {
    lines(line_ranges(text).map(|line| {
        let start = line.start;
        text[line]
            .char_indices()
            .map(move |(index, c)| (c, roles[start + index] != Role::Markup))
    }))
}

/// The nonempty lines of the characters marked to show.
fn lines<L: Iterator<Item = (char, bool)>>(lines: impl Iterator<Item = L>) -> Vec<String> {
    lines
        .map(|line| {
            line.filter(|(c, show)| *show && !c.is_whitespace() && *c != LINK_BREAK)
                .map(|(c, _)| c)
                .collect::<String>()
        })
        .filter(|line| !line.is_empty())
        .collect()
}

/// The range of each line of `text`, without its line ending. Like Markdown,
/// this ends lines at CR LF, LF, and CR.
fn line_ranges(text: &str) -> impl Iterator<Item = Range<usize>> {
    let bytes = text.as_bytes();
    let mut start = Some(0);
    from_fn(move || {
        let line_start = start?;
        match bytes[line_start..]
            .iter()
            .position(|b| matches!(b, b'\n' | b'\r'))
        {
            Some(offset) => {
                let end = line_start + offset;
                let ending = if bytes[end..].starts_with(b"\r\n") {
                    2
                } else {
                    1
                };
                start = Some(end + ending);
                Some(line_start..end)
            }
            None => {
                start = None;
                Some(line_start..bytes.len())
            }
        }
    })
}

/// `text` as paragraphs of lines that show every character as written,
/// without the whitespace at the start and end of each line. No URL is a
/// link.
fn literal(text: &str, trace: bool) -> Writer {
    let breaks: Vec<usize> = text.match_indices("://").map(|(colon, _)| colon).collect();
    // The start of each line, and its text without whitespace around it.
    let lines: Vec<(usize, usize, &str)> = line_ranges(text)
        .map(|range| {
            let line = &text[range.clone()];
            let trimmed = line.trim_start();
            (
                range.start,
                range.start + line.len() - trimmed.len(),
                trimmed.trim_end(),
            )
        })
        .collect();
    let mut markdown = Writer::new(trace);
    markdown.markdown.reserve(text.len() * 2);
    for (number, &(line_start, start, line)) in lines.iter().enumerate() {
        if number > 0 {
            // The line feed before the line: the text has no other endings.
            markdown.kept(line_start - 1, '\n');
        }
        for (index, character) in line.char_indices() {
            markdown.text(start + index, character, &breaks);
        }
        if !line.is_empty()
            && lines
                .get(number + 1)
                .is_some_and(|(_, _, next)| !next.is_empty())
        {
            markdown.added('\\');
        }
    }
    markdown
}

#[cfg(test)]
mod tests;
