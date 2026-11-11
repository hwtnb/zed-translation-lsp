use std::time::{Duration, Instant};

use pulldown_cmark::{Event, Parser, Tag, TagEnd};

use super::*;

const SOURCE_URL: &str = "https://example.com/docs";

fn urls() -> Urls {
    Urls::of(&format!("// See {SOURCE_URL} for the details."))
}

fn rendered(translation: &str) -> String {
    render(translation, &urls())
}

/// What Zed shows for `markdown`: its text and code, with a line break for
/// each break, line ending, and end of a block, and a space for a soft break.
fn display(markdown: &str) -> String {
    let mut shown = String::new();
    for event in Parser::new_ext(markdown, ZED_OPTIONS) {
        match event {
            Event::Text(text) | Event::Code(text) => {
                shown.push_str(&text.replace("\r\n", "\n").replace('\r', "\n"));
            }
            Event::SoftBreak => shown.push(' '),
            Event::Start(Tag::Emphasis | Tag::Strong | Tag::Link { .. } | Tag::TableCell)
            | Event::End(TagEnd::Emphasis | TagEnd::Strong | TagEnd::Link | TagEnd::TableCell) => {}
            Event::Start(_) | Event::End(_) | Event::HardBreak | Event::Rule => shown.push('\n'),
            _ => (),
        }
    }
    shown
}

/// The display without blank lines and the whitespace around lines.
fn display_lines(markdown: &str) -> Vec<String> {
    display(markdown)
        .split('\n')
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::to_owned)
        .collect()
}

/// The events of `markdown` that are not text or breaks.
fn structure(markdown: &str) -> Vec<Event<'_>> {
    Parser::new_ext(markdown, ZED_OPTIONS)
        .filter(|event| {
            !matches!(
                event,
                Event::Text(_) | Event::SoftBreak | Event::HardBreak | Event::End(_)
            )
        })
        .collect()
}

#[test]
fn accidental_markdown_shows_as_written() {
    for translation in [
        "__init__ と _private_ を使う",
        "--verbose を付ける",
        r#"It's "quoted"..."#,
        r"\d+\.\d+ に一致する",
        "~a~、~~a~~、a^b^",
        "[^1]、[note]",
        "<!-- x -->",
        "Vec<T>、<div>",
        "#include <stdio.h>",
        "&amp; と &#42; と \\* と \\",
        "+++",
        "[ ] task と [x]",
        "見出し {#id}",
        "<a@example.com>",
        "$x$ と $$y$$",
        "1.5 倍と 2) 番目",
    ] {
        let markdown = rendered(translation);
        assert_eq!(display(&markdown).trim(), translation, "{markdown:?}");
        assert!(
            structure(&markdown)
                .iter()
                .all(|event| matches!(event, Event::Start(Tag::Paragraph))),
            "{translation:?} → {markdown:?}"
        );
    }
    // A note of GitHub is an ordinary block quote.
    let markdown = rendered("> [!NOTE]");
    assert_eq!(display_lines(&markdown), ["[!NOTE]"]);
    assert!(structure(&markdown).contains(&Event::Start(Tag::BlockQuote(None))));
}

#[test]
fn line_breaks_inside_paragraphs_show() {
    let translation = "@param a 最初の値\n@param b 次の値\n@return 結果";
    let markdown = rendered(translation);
    assert_eq!(display(&markdown).trim(), translation);
    assert_eq!(
        Parser::new_ext(&markdown, ZED_OPTIONS)
            .filter(|event| matches!(event, Event::HardBreak))
            .count(),
        2
    );
    // The last line of a paragraph keeps no break, which would show a `\`.
    assert_eq!(
        rendered("一行目\n二行目\n\n三行目"),
        "一行目\\\n二行目\n\n三行目"
    );
    for translation in ["一行目\r\n二行目", "一行目\r二行目"] {
        let markdown = rendered(translation);
        assert_eq!(markdown, "一行目\\\n二行目");
        assert_eq!(display_lines(&markdown), ["一行目", "二行目"]);
    }
    // A trailing backslash and two trailing spaces each end one line.
    let markdown = rendered("C:\\path\\\n次  \n最後");
    assert_eq!(display_lines(&markdown), ["C:\\path\\", "次", "最後"]);
}

/// Whether an event is the formatting that a case keeps.
type Is = fn(&Event) -> bool;

#[test]
fn intended_markdown_stays() {
    let cases: [(&str, Is, &[&str]); 16] = [
        (
            "`retry_count` を使う",
            |e| matches!(e, Event::Code(_)),
            &["retry_count を使う"],
        ),
        (
            "例:\n\n```rust\nlet x = a_b * 2;\n```",
            |e| matches!(e, Event::Start(Tag::CodeBlock(_))),
            &["例:", "let x = a_b * 2;"],
        ),
        (
            "例:\n\n    let x = a_b * 2;",
            |e| matches!(e, Event::Start(Tag::CodeBlock(_))),
            &["例:", "let x = a_b * 2;"],
        ),
        (
            "# 見出し #",
            |e| matches!(e, Event::Start(Tag::Heading { .. })),
            &["見出し"],
        ),
        (
            "見出し\n===",
            |e| matches!(e, Event::Start(Tag::Heading { .. })),
            &["見出し"],
        ),
        (
            "見出し\n---",
            |e| matches!(e, Event::Start(Tag::Heading { .. })),
            &["見出し"],
        ),
        (
            "- 一\n- 二",
            |e| matches!(e, Event::Start(Tag::List(None))),
            &["一", "二"],
        ),
        (
            "* 一\n+ 二",
            |e| matches!(e, Event::Start(Tag::List(None))),
            &["一", "二"],
        ),
        (
            "1. 一\n2. 二",
            |e| matches!(e, Event::Start(Tag::List(Some(1)))),
            &["一", "二"],
        ),
        (
            "> 引用\n> 続き",
            |e| matches!(e, Event::Start(Tag::BlockQuote(None))),
            &["引用", "続き"],
        ),
        (
            "| a | b |\n|---|:-:|\n| 1 | 2 \\| 3 |",
            |e| matches!(e, Event::Start(Tag::Table(_))),
            &["ab", "12 \\| 3"],
        ),
        (
            "前\n\n---\n\n後",
            |e| matches!(e, Event::Rule),
            &["前", "後"],
        ),
        (
            "*強調* の文",
            |e| matches!(e, Event::Start(Tag::Emphasis)),
            &["強調 の文"],
        ),
        (
            "**太字** の文",
            |e| matches!(e, Event::Start(Tag::Strong)),
            &["太字 の文"],
        ),
        (
            "[資料](https://example.com/docs) を見る",
            |e| matches!(e, Event::Start(Tag::Link { .. })),
            &["資料 を見る"],
        ),
        (
            "- 一\n  - 入れ子\n- 二",
            |e| matches!(e, Event::Start(Tag::List(None))),
            &["一", "入れ子", "二"],
        ),
    ];
    for (translation, expected, lines) in cases {
        let markdown = rendered(translation);
        assert!(
            structure(&markdown).iter().any(expected),
            "{translation:?} → {markdown:?}"
        );
        assert_eq!(display_lines(&markdown), lines, "{markdown:?}");
    }
    // Formatting inside formatting stays too.
    let markdown = rendered("> - *一* と `a_b`\n>   続き");
    assert_eq!(markdown, "> - *一* と `a_b`\\\n>   続き");
}

/// The destinations of the links and images of `markdown`.
fn links(markdown: &str) -> Vec<String> {
    Parser::new_ext(markdown, ZED_OPTIONS)
        .filter_map(|event| match event {
            Event::Start(Tag::Link { dest_url, .. } | Tag::Image { dest_url, .. }) => {
                Some(dest_url.into_string())
            }
            _ => None,
        })
        .collect()
}

/// `translation` with [`LINK_BREAK`] after the `:` of each `://`.
fn broken(translation: &str) -> String {
    translation.replace("://", ":\u{2060}//")
}

#[test]
fn links_open_only_urls_of_the_original() {
    let docs = Urls::of("See https://example.com/docs.");
    for (translation, link, shown) in [
        (
            "詳細はhttps://example.com/docsを参照。",
            Some(SOURCE_URL),
            None,
        ),
        ("https://example.com/docs。", Some(SOURCE_URL), None),
        ("(https://example.com/docs).", Some(SOURCE_URL), None),
        (
            "**https://example.com/docs**",
            Some(SOURCE_URL),
            Some("https://example.com/docs"),
        ),
        (
            "[資料](https://example.com/docs)",
            Some(SOURCE_URL),
            Some("資料"),
        ),
        (
            "<https://example.com/docs>",
            Some(SOURCE_URL),
            Some("https://example.com/docs"),
        ),
        ("https://example.com/docs^evil", None, None),
        ("https://example.com/docs[evil]", None, None),
        ("https://example.com/docs{evil}", None, None),
        ("https://example.com/docs\\evil", None, None),
        ("https://example.com/docs2", None, None),
        ("https://example.com/docs/extra", None, None),
        ("https://example.com/docs?x=1", None, None),
        ("https://example.com/docs.html", None, None),
        ("https://example.com/docs_v2", None, None),
        (
            "https://example.com/docs?u=https://example.com/docs",
            None,
            None,
        ),
        ("[資料](https://example.com/docs!)", None, None),
        ("[資料](https://example.com/docs.)", None, None),
        ("[資料](https://evil.example/)", None, None),
        ("![画像](https://example.com/docs)", None, None),
        ("<https://evil.example/>", None, None),
        ("xhttps://example.com/docs", None, None),
    ] {
        let markdown = render(translation, &docs);
        assert_eq!(
            links(&markdown),
            link.into_iter().collect::<Vec<_>>(),
            "{translation:?} → {markdown:?}"
        );
        let shown = shown.map_or_else(
            || {
                if link.is_some() {
                    translation.to_owned()
                } else {
                    broken(translation)
                }
            },
            str::to_owned,
        );
        assert_eq!(display(&markdown).trim(), shown, "{markdown:?}");
    }
    // A URL whose end is not punctuation keeps it.
    let source = "https://en.wikipedia.org/wiki/Rust_(programming_language)";
    let markdown = render(&format!("({source})"), &Urls::of(source));
    assert_eq!(links(&markdown), [source]);
    assert_eq!(display(&markdown).trim(), format!("({source})"));
    // A character outside ASCII ends a URL of the original.
    let markdown = render("https://src.co。evil.com", &Urls::of("See https://src.co"));
    assert_eq!(links(&markdown), ["https://src.co"]);
    assert_eq!(display(&markdown).trim(), "https://src.co。evil.com");
    // Code, and the text of a link that stays, show URLs as written.
    for (translation, shown) in [
        ("`https://evil.example/`", "https://evil.example/"),
        (
            "[https://evil.example/](https://example.com/docs)",
            "https://evil.example/",
        ),
    ] {
        assert_eq!(display(&render(translation, &docs)).trim(), shown);
    }
    // A plain line shows no URL as a link.
    let markdown = literal("https://example.com/docs", false).markdown;
    assert_eq!(markdown, "https\\:\u{2060}\\/\\/example\\.com\\/docs");
}

#[test]
fn urls_of_the_original_leave_out_the_sentence_around_them() {
    let urls = Urls::of(
        "(see https://example.com/a). https://example.com/b, https://example.com/c(d) \
         https://example.com/e?f=g; ftp://x.example/h! x://short \
         https://example.com/i?u=https://example.com/j",
    );
    let mut written: Vec<_> = urls.written.into_iter().collect();
    written.sort();
    assert_eq!(
        written,
        [
            "ftp://x.example/h",
            "https://example.com/a",
            "https://example.com/b",
            "https://example.com/c(d)",
            "https://example.com/e?f=g",
        ]
    );
}

#[test]
fn markdown_documents_keep_links_only_to_urls_of_the_original() {
    let urls = Urls::of_markdown(
        "See [the docs](https://example.com/docs), [usage](docs/usage.md), and \
         https://example.com/a.\n\n![logo](https://example.com/logo.png)",
    );
    for (translation, shown) in [
        ("[資料](https://example.com/docs)", None),
        ("![ロゴ](https://example.com/logo.png)", None),
        ("[使い方](docs/usage.md) と [節](#part)", None),
        ("`https://evil.example/`", None),
        ("```\nhttps://evil.example/\n```", None),
        ("[https://evil.example/](https://example.com/docs)", None),
        (
            "https://example.com/a を見る",
            Some("<https://example.com/a> を見る"),
        ),
        (
            "https://evil.example/ を見る",
            Some("https:\u{2060}//evil.example/ を見る"),
        ),
        (
            "<div>https://evil.example/</div>",
            Some("<div>https:\u{2060}//evil.example/</div>"),
        ),
        // Zed makes links in the text of images too.
        (
            "![https://evil.example/x](https://example.com/logo.png)",
            Some("![https:\u{2060}//evil.example/x](https://example.com/logo.png)"),
        ),
        // A footnote shows its label as text, not as a URL.
        (
            "注[^https://evil.example/]\n\n[^https://evil.example/]: 説明",
            None,
        ),
    ] {
        assert_eq!(
            document(translation, &urls).as_deref(),
            Some(shown.unwrap_or(translation)),
            "{translation:?}"
        );
    }
    for translation in [
        "[x](https://evil.example/)",
        "[x](https://example.com/docs!)",
        "![x](https://evil.example/i.png)",
        "<https://evil.example/>",
        "<a@evil.example>",
        "[x][r]\n\n[r]: https://evil.example/",
        "[x](ms-msdt:foo)",
        // Zed opens a destination without a scheme as a file or a path.
        "[a](//evil.example/x)",
        "[b](/etc/passwd)",
        "[c](calc.exe)",
        "[d](\\\\evil\\share)",
        "[x][r]\n\n[r]: other.md",
        "[x]()",
        // Zed joins the text that escapes and character references divide.
        "https&#58;//evil.example/x",
        "https:\\//evil.example/x",
        "x&colon;//evil.example/x",
        "https:/\\/evil.example/x",
        "https:&#x2F;/evil.example/x",
        "https:&#47;/evil.example/x",
        "> https&#58;//evil.example/x",
        "| a |\n|---|\n| https&#58;//evil.example/x |",
        "[r]: \\^^*&#47;_\n\n[r]",
    ] {
        assert_eq!(document(translation, &urls), None, "{translation:?}");
    }
}

#[test]
fn escapes_and_character_references_make_no_links_in_text() {
    let docs = Urls::of("See https://example.com/docs.");
    for translation in [
        "https&#58;//evil.example/x",
        "https:\\//evil.example/x",
        "x&colon;//evil.example/x",
        "https:/\\/evil.example/x",
        "https:&#x2F;/evil.example/x",
        "https:&#47;/evil.example/x",
        "> https&#58;//evil.example/x",
        "| a |\n|---|\n| https&#58;//evil.example/x |",
    ] {
        let markdown = render(translation, &docs);
        assert!(
            links(&markdown).is_empty(),
            "{translation:?} → {markdown:?}"
        );
        assert!(
            links_stay(&markdown, &docs, true),
            "{translation:?} → {markdown:?}"
        );
    }
    // The check finds a URL that escapes divide.
    assert!(!links_stay("https\\://evil.example/", &docs, true));
    assert!(!links_stay("https&#58;//evil.example/", &docs, true));
}

#[test]
fn empty_and_symbol_only_translations() {
    assert_eq!(rendered(""), "");
    assert_eq!(rendered(" \n\t"), " \n\t");
    for translation in ["!?#", "<>", "\\", "[]()", "~~", "a_b_c"] {
        assert_eq!(display(&rendered(translation)).trim(), translation);
    }
    // These are formatting the translation can mean.
    assert_eq!(display_lines(&rendered("***")), Vec::<String>::new());
    assert_eq!(display_lines(&rendered("- ")), Vec::<String>::new());
}

#[test]
fn plain_lines_fall_back_with_breaks_inside_paragraphs() {
    let text = "  1. a_b \n> c\n\n#d  ";
    let markdown = literal(text, false).markdown;
    assert_eq!(markdown, "1\\. a\\_b\\\n\\> c\n\n\\#d");
    assert_eq!(display_lines(&markdown), ["1. a_b", "> c", "#d"]);
    assert!(
        structure(&markdown)
            .iter()
            .all(|event| matches!(event, Event::Start(Tag::Paragraph)))
    );
    assert!(check(text, &literal(text, true)).is_ok());
}

/// A fixed sequence of pseudorandom numbers.
struct Numbers(u64);

impl Numbers {
    fn next(&mut self, bound: usize) -> usize {
        // xorshift64
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 % bound as u64) as usize
    }
}

const PIECES: &[&str] = &[
    "a",
    "語",
    "b c",
    " ",
    "  ",
    "\t",
    "\n",
    "\n",
    "\n\n",
    "\r\n",
    "\r",
    "*",
    "**",
    "_",
    "__",
    "`",
    "```\n",
    "~~~\n",
    "~",
    "~~",
    "^",
    "#",
    "# ",
    "## ",
    "- ",
    "+ ",
    "* ",
    "1. ",
    "2) ",
    "> ",
    ">",
    "|",
    " | ",
    "|---|",
    "---",
    "===",
    "\\",
    "[",
    "]",
    "(",
    ")",
    "!",
    "<",
    ">",
    "<!--",
    "-->",
    "&amp;",
    "&",
    "'",
    "\"",
    "...",
    "--",
    ":",
    "{#id}",
    "[^1]",
    "[^1]: ",
    "[a]: /u",
    "+++",
    "$",
    "<div>",
    "\0",
    "\u{2060}",
    "    ",
    "https://example.com/docs",
    "https://evil.example/",
    "[x](https://example.com/docs)",
    "[y](https://evil.example/)",
    "[z]",
    "<https://e.x>",
];

/// Translations made of pieces that Markdown reads.
fn corpus(count: usize) -> Vec<String> {
    let mut numbers = Numbers(0x9E37_79B9_7F4A_7C15);
    (0..count)
        .map(|_| {
            let length = 1 + numbers.next(24);
            (0..length)
                .map(|_| PIECES[numbers.next(PIECES.len())])
                .collect()
        })
        .collect()
}

/// What a byte of rendered Markdown is to Zed, from reading it with Zed's
/// options.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Seen {
    /// Text or code that shows.
    Shown,
    /// Formatting that stays: markers of blocks, `*` emphasis, code
    /// delimiters, and the syntax of links to URLs of the original.
    Formatting,
    /// Anything else that does not show.
    Hidden,
}

/// What separates two characters that show.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Separator {
    Soft,
    Hard,
    Block,
    /// A line break inside a code block.
    Code,
}

/// The block that a construct of rendered Markdown owns its bytes as.
#[derive(Clone, Copy)]
enum Construct {
    Block,
    Item(usize),
    Heading,
    Table,
    CodeBlock,
    Star,
    Link,
    Other,
}

impl Construct {
    fn formatting(self, index: usize, byte: u8) -> bool {
        byte == b'>'
            || match self {
                Self::Block | Self::Other => false,
                Self::Item(marker_end) => index < marker_end,
                Self::Heading => matches!(byte, b'#' | b'=' | b'-'),
                Self::Table => matches!(byte, b'|' | b'-' | b':'),
                Self::CodeBlock | Self::Link => true,
                Self::Star => byte == b'*',
            }
    }
}

/// Checks `markdown`, rendered from `translation` with the origin of each
/// byte, against the conditions, from how Zed reads it and independently of
/// the roles that `render` uses:
///
/// - every character that shows comes from the translation in order, except
///   [`LINK_BREAK`], and the only bytes added are escapes, hard breaks, the
///   brackets of `<URL>`, and [`LINK_BREAK`];
/// - every line break of the translation stays, and inside a code block shows;
/// - a character of the translation that does not show is formatting at its
///   position, or whitespace that has no character that shows before it or
///   none after it on its line (in a table, in its cell), or whitespace inside
///   formatting such as a link destination;
/// - characters of one line of the translation show on one line, those of
///   different lines on different lines, and those of lines with a blank line
///   between them in different blocks. Counts of blank lines are not
///   compared.
fn check(translation: &str, rendered: &Writer) -> Result<(), String> {
    let markdown = &rendered.markdown;
    let origins = rendered.origins.as_ref().expect("traced");
    let fail = |message: String| Err(format!("{message}: {translation:?} → {markdown:?}"));
    let bytes = markdown.as_bytes();
    let mut kinds = vec![Seen::Hidden; bytes.len()];
    // The bytes that show, by the code block they are in, and separators.
    let mut sequence: Vec<Result<(usize, usize), Separator>> = Vec::new();
    let mut constructs = vec![Construct::Block];
    let mut cursor = 0;
    let mut code_blocks = 0;
    let mut code_block = None;
    let owned = |kinds: &mut [Seen], cursor: &mut usize, end: usize, construct: Construct| {
        while *cursor < end {
            if construct.formatting(*cursor, bytes[*cursor]) {
                kinds[*cursor] = Seen::Formatting;
            }
            *cursor += 1;
        }
    };
    for (event, range) in Parser::new_ext(markdown, ZED_OPTIONS).into_offset_iter() {
        let leaf = !matches!(event, Event::Start(_) | Event::End(_));
        let construct = *constructs.last().unwrap();
        owned(&mut kinds, &mut cursor, range.start, construct);
        match event {
            Event::Start(tag) => {
                constructs.push(match &tag {
                    Tag::Item => {
                        Construct::Item(range.start + marker_length(&bytes[range.start..]))
                    }
                    Tag::Heading { .. } => Construct::Heading,
                    Tag::Table(_) | Tag::TableHead | Tag::TableRow | Tag::TableCell => {
                        Construct::Table
                    }
                    Tag::CodeBlock(_) => Construct::CodeBlock,
                    Tag::Emphasis | Tag::Strong if bytes[range.start] == b'*' => Construct::Star,
                    Tag::Link {
                        link_type: LinkType::Inline | LinkType::Autolink,
                        dest_url,
                        ..
                    } if urls().written(dest_url) => Construct::Link,
                    Tag::Paragraph | Tag::BlockQuote(None) | Tag::List(_) => Construct::Block,
                    _ => Construct::Other,
                });
                if let Tag::CodeBlock(_) = tag {
                    code_blocks += 1;
                    code_block = Some(code_blocks);
                }
                if !matches!(
                    tag,
                    Tag::Emphasis | Tag::Strong | Tag::Link { .. } | Tag::TableCell
                ) {
                    sequence.push(Err(Separator::Block));
                }
            }
            Event::End(end) => {
                owned(&mut kinds, &mut cursor, range.end, construct);
                constructs.pop();
                if end == TagEnd::CodeBlock {
                    code_block = None;
                }
                if !matches!(
                    end,
                    TagEnd::Emphasis | TagEnd::Strong | TagEnd::Link | TagEnd::TableCell
                ) {
                    sequence.push(Err(Separator::Block));
                }
            }
            Event::Text(_) | Event::Code(_) => {
                let source = &bytes[range.clone()];
                let ticks = if matches!(event, Event::Code(_)) {
                    source.iter().take_while(|b| **b == b'`').count()
                } else {
                    0
                };
                for index in range.clone() {
                    kinds[index] = if index < range.start + ticks || index >= range.end - ticks {
                        Seen::Formatting
                    } else {
                        Seen::Shown
                    };
                    if kinds[index] == Seen::Shown && markdown.is_char_boundary(index) {
                        if bytes[index] == b'\n' {
                            sequence.push(Err(if code_block.is_some() {
                                Separator::Code
                            } else {
                                Separator::Soft
                            }));
                        } else if !bytes[index].is_ascii_whitespace() {
                            sequence.push(Ok((index, code_block.unwrap_or(0))));
                        }
                    }
                }
            }
            Event::SoftBreak => sequence.push(Err(Separator::Soft)),
            Event::HardBreak => sequence.push(Err(Separator::Hard)),
            Event::Rule => {
                kinds[range.clone()].fill(Seen::Formatting);
                sequence.push(Err(Separator::Block));
            }
            _ => (),
        }
        // A leaf event owns its bytes; a tag's own bytes follow its start.
        if leaf {
            cursor = cursor.max(range.end);
        }
    }
    owned(&mut kinds, &mut cursor, bytes.len(), Construct::Block);

    // What becomes of each character of the translation.
    let mut fate: Vec<Option<Seen>> = vec![None; translation.len()];
    for (index, origin) in origins.iter().enumerate() {
        match origin {
            Some(origin) => {
                let kind = kinds[index];
                fate[*origin] = Some(match (fate[*origin], kind) {
                    (Some(Seen::Shown), _) | (_, Seen::Shown) => Seen::Shown,
                    (Some(Seen::Hidden), _) | (_, Seen::Hidden) => Seen::Hidden,
                    _ => Seen::Formatting,
                });
            }
            // Only escapes, the brackets of `<URL>`, hard breaks, and
            // LINK_BREAK are added, and only LINK_BREAK shows.
            None if markdown.is_char_boundary(index) => {
                let character = markdown[index..].chars().next().unwrap();
                if !matches!(character, '\\' | '<' | '>' | LINK_BREAK) {
                    return fail(format!("{character:?} is added at {index}"));
                }
                if kinds[index] == Seen::Shown && character != LINK_BREAK {
                    return fail(format!("the added {character:?} at {index} shows"));
                }
            }
            None => (),
        }
    }
    // Every line break of the translation stays in the Markdown.
    for (index, _) in translation.match_indices('\n') {
        if fate[index].is_none() {
            return fail(format!("the line break at {index} is gone"));
        }
    }
    let lines: Vec<Range<usize>> = line_ranges(translation).collect();
    for line in &lines {
        let characters: Vec<(usize, char)> = translation[line.clone()]
            .char_indices()
            .map(|(offset, c)| (line.start + offset, c))
            .collect();
        let shows = |index: usize| fate[index] == Some(Seen::Shown);
        // A `|` of a table divides a line into cells.
        let cell_edge =
            |&(index, c): &(usize, char)| c == '|' && fate[index] == Some(Seen::Formatting);
        for (position, &(index, character)) in characters.iter().enumerate() {
            if shows(index) || character == LINK_BREAK {
                continue;
            }
            if character.is_whitespace() {
                // Whitespace may go only at the start or end of a line or
                // cell, or inside formatting such as a link destination.
                let before = characters[..position]
                    .iter()
                    .rev()
                    .take_while(|c| !cell_edge(c))
                    .any(|(index, c)| !c.is_whitespace() && shows(*index));
                let after = characters[position + 1..]
                    .iter()
                    .take_while(|c| !cell_edge(c))
                    .any(|(index, c)| !c.is_whitespace() && shows(*index));
                if before && after && fate[index] != Some(Seen::Formatting) {
                    return fail(format!("the space at {index} does not show"));
                }
            } else if fate[index] != Some(Seen::Formatting) {
                return fail(format!("{character:?} at {index} does not show"));
            }
        }
    }

    let line_of = |index: usize| lines.partition_point(|line| line.end < index);
    let blank_between = |from: usize, to: usize| {
        lines[from + 1..to]
            .iter()
            .any(|line| translation[line.clone()].trim().is_empty())
    };
    // Inside a code block, every line break of the translation shows.
    let mut code_spans: HashMap<usize, Range<usize>> = HashMap::new();
    for &(index, block) in sequence.iter().flatten() {
        if let (Some(origin), true) = (origins[index], block != 0) {
            let span = code_spans.entry(block).or_insert(origin..origin);
            span.start = span.start.min(origin);
            span.end = span.end.max(origin);
        }
    }
    for span in code_spans.values() {
        if let Some((offset, _)) = translation[span.clone()]
            .match_indices('\n')
            .find(|(offset, _)| fate[span.start + offset] != Some(Seen::Shown))
        {
            return fail(format!(
                "the line break at {} of a code block does not show",
                span.start + offset
            ));
        }
    }
    let mut previous: Option<(usize, usize)> = None;
    let mut separators = Vec::new();
    for item in sequence {
        let (index, block) = match item {
            Err(separator) => {
                separators.push(separator);
                continue;
            }
            Ok(item) => item,
        };
        let Some(origin) = origins[index] else {
            continue;
        };
        if let Some((last, last_block)) = previous {
            if origin <= last {
                return fail(format!("{origin} shows after {last}"));
            }
            let (from, to) = (line_of(last), line_of(origin));
            let breaks = separators
                .iter()
                .any(|s| matches!(s, Separator::Hard | Separator::Block | Separator::Code));
            if from == to && (breaks || separators.contains(&Separator::Soft)) {
                return fail(format!("line {from} shows on two lines"));
            }
            if from < to && !breaks {
                return fail(format!("lines {from} and {to} show on one line"));
            }
            let same_code_block = block != 0 && block == last_block;
            if from < to
                && blank_between(from, to)
                && !same_code_block
                && !separators.contains(&Separator::Block)
            {
                return fail(format!("paragraphs {from} and {to} show as one"));
            }
        }
        previous = Some((origin, block));
        separators.clear();
    }
    Ok(())
}

/// The rendered Markdown of `translation`, traced, after normalizing its line
/// endings as `render` does.
fn traced(translation: &str) -> (String, Writer) {
    let normalized = translation.replace("\r\n", "\n").replace('\r', "\n");
    let rendered = super::rendered(translation, &urls(), true);
    (normalized, rendered)
}

#[test]
fn shown_text_matches_the_translation_for_a_fixed_corpus() {
    let started = Instant::now();
    let corpus = corpus(3000);
    let mut fallbacks = 0;
    for translation in &corpus {
        let (normalized, rendered) = traced(translation);
        check(&normalized, &rendered).unwrap();
        if formatted(&normalized, &urls(), false).is_none() {
            fallbacks += 1;
        }
    }
    // Few translations fall back to plain lines: 3 of these when measured.
    assert!(fallbacks < corpus.len() / 100, "{fallbacks} fallbacks");
    assert!(started.elapsed() < Duration::from_secs(60));
}

#[test]
fn conditions_hold_for_the_examples() {
    for translation in [
        "<!-- c -->\n---",
        "<div>\n# x\n</div>",
        "- a\n<div>",
        "> a\n<div>\nb",
        "`a\nb`",
        "[a\nb](https://example.com/docs)",
        "[a](\nhttps://example.com/docs)",
        "> *a\n> b*",
        "[a]: <u>\n- [b]: /u\n> [c]:\n> /v",
        "| a | b |\n|---|---|\n| `x|y` | c |",
        "a | b\n-",
        "日本語の文。\r\n次の文。\r\n\r\n- 項目",
        "a\n\n\n\nb",
        "例:\n\n```\nx\n\ny\n```",
        "\0",
    ] {
        let (normalized, rendered) = traced(translation);
        check(&normalized, &rendered).unwrap();
    }
}

/// A change that breaks correct Markdown.
enum Edit {
    /// Removes the first byte of the first match.
    Remove(&'static str),
    /// Replaces the first byte of the first match with an added character.
    Replace(&'static str, char),
    /// Adds a character at the start or the end.
    Prepend(char),
    Append(char),
}

#[test]
fn the_check_rejects_broken_conversions() {
    // A letter, the space between two words, the space after formatting, one
    // line feed of a blank line between paragraphs, the backslash of a line
    // break inside a paragraph, or a line feed of a code block goes; that
    // backslash turns into a line feed; or a character is added that shows.
    for (translation, edit) in [
        ("alpha beta", Edit::Remove("l")),
        ("alpha beta", Edit::Remove(" ")),
        ("**alpha** beta", Edit::Remove(" ")),
        ("前の段落\n\n次の段落", Edit::Remove("\n")),
        ("一行目\n二行目", Edit::Remove("\\")),
        ("一行目\n二行目", Edit::Replace("\\", '\n')),
        ("```\nx\n\ny\n```", Edit::Remove("\n\n")),
        ("alpha beta", Edit::Prepend('<')),
        ("alpha beta", Edit::Append('>')),
        ("alpha beta", Edit::Append('\\')),
    ] {
        let (normalized, rendered) = traced(translation);
        check(&normalized, &rendered).unwrap();
        let mut markdown = rendered.markdown.clone();
        let mut origins = rendered.origins.clone().unwrap();
        match edit {
            Edit::Remove(found) | Edit::Replace(found, _) => {
                let index = markdown.find(found).unwrap();
                markdown.remove(index);
                origins.remove(index);
                if let Edit::Replace(_, added) = edit {
                    markdown.insert(index, added);
                    origins.insert(index, None);
                }
            }
            Edit::Prepend(added) => {
                markdown.insert(0, added);
                origins.insert(0, None);
            }
            Edit::Append(added) => {
                markdown.push(added);
                origins.push(None);
            }
        }
        let broken = Writer {
            markdown,
            origins: Some(origins),
        };
        assert!(
            check(&normalized, &broken).is_err(),
            "{translation:?} broken to {:?}",
            broken.markdown
        );
    }
}

fn hostile(unit: &str, bytes: usize) -> String {
    unit.repeat(bytes / unit.len())
}

/// Times conversions of hostile translations, as text and as the translation
/// of a Markdown document. Run with
/// `cargo test --release -- --ignored --nocapture conversion_time`.
#[test]
#[ignore = "measures time"]
fn conversion_time() {
    let markdown_urls = Urls::of_markdown(SOURCE_URL);
    for unit in [
        "_",
        "\\",
        "[",
        "*",
        "`",
        "<",
        "a_",
        "[a](",
        "*a ",
        "> ",
        "- ",
        "https://",
        "a://",
        "| a ",
        "\n",
        "\n\n",
        "&amp;",
        "<!--",
        "[^1]",
        "~",
        "    a\n",
        "1. ",
        "#",
        "https://example.com/docs ",
        "https://example.com/docs.",
        "https://example.com/docs/",
        "[a](https://example.com/docs)",
    ] {
        for bytes in [64 * 1024, 1024 * 1024] {
            let translation = hostile(unit, bytes);
            let started = Instant::now();
            let markdown = rendered(&translation);
            let text = started.elapsed();
            let started = Instant::now();
            let document = document(&translation, &markdown_urls);
            println!(
                "{unit:?} {} KiB: {text:?} as text, {} bytes; {:?} as Markdown, {}",
                bytes / 1024,
                markdown.len(),
                started.elapsed(),
                document.map_or("a code block".into(), |d| format!("{} bytes", d.len())),
            );
        }
    }
}
