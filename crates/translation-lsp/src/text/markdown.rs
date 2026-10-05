use std::{borrow::Cow, ops::Range};

use pulldown_cmark::{Event, LinkType, Options, Parser, Tag};

/// Locate prose for automatic selection eligibility; translation uses the original text.
pub(crate) fn prose_ranges(source: &str) -> Result<Vec<Range<usize>>, &'static str> {
    let parsing_source = parsing_source(source);
    let mut prose = Vec::new();
    let mut cursor = 0;
    let mut protected_until = 0;

    for (event, mut range) in parser(&parsing_source).into_offset_iter() {
        match event {
            Event::Start(Tag::CodeBlock(_) | Tag::HtmlBlock) => {
                protected_until = protected_until.max(range.end);
            }
            Event::Start(Tag::Link { link_type, .. } | Tag::Image { link_type, .. })
                if protected_link(link_type) =>
            {
                protected_until = protected_until.max(range.end);
            }
            Event::Text(text) if range.start >= protected_until => {
                let raw = source
                    .get(range.clone())
                    .ok_or("Invalid Markdown source range")?;
                // Entities and escaped punctuation alone are not prose selections.
                if raw != text.as_ref() {
                    continue;
                }
                if range.start < cursor {
                    return Err("Invalid Markdown source range");
                }
                // OffsetIter leaves an escape's backslash in the preceding gap.
                // Exclude the escaped character from the prose range.
                if range.start > cursor && source.as_bytes().get(range.start - 1) == Some(&b'\\') {
                    range.start += raw.chars().next().map_or(0, char::len_utf8);
                }
                let raw = &source[range.clone()];
                let trimmed = raw.trim_matches(char::is_whitespace);
                if trimmed.is_empty() {
                    continue;
                }
                range.start += raw.len() - raw.trim_start_matches(char::is_whitespace).len();
                range.end = range.start + trimmed.len();

                cursor = range.end;
                prose.push(range);
            }
            _ => {}
        }
    }
    Ok(prose)
}

fn protected_link(link_type: LinkType) -> bool {
    // A shortcut/collapsed label is also the reference key, so it is literal.
    matches!(
        link_type,
        LinkType::Autolink | LinkType::Email | LinkType::Shortcut | LinkType::Collapsed
    )
}

fn parser(source: &str) -> Parser<'_> {
    Parser::new_ext(
        source,
        Options::ENABLE_TABLES
            | Options::ENABLE_STRIKETHROUGH
            | Options::ENABLE_TASKLISTS
            | Options::ENABLE_FOOTNOTES
            | Options::ENABLE_GFM,
    )
}

fn parsing_source(source: &str) -> Cow<'_, str> {
    let bytes = source.as_bytes();
    let bare_cr = |index: usize| bytes[index] == b'\r' && bytes.get(index + 1) != Some(&b'\n');
    if !bytes.iter().enumerate().any(|(index, _)| bare_cr(index)) {
        return Cow::Borrowed(source);
    }
    // 0.13.4 does not recognize fenced info strings ending in bare CR.
    // This view changes only one-byte CR to LF; offsets and source bytes stay intact.
    Cow::Owned(
        source
            .char_indices()
            .map(|(index, character)| if bare_cr(index) { '\n' } else { character })
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn prose(source: &str) -> Vec<&str> {
        prose_ranges(source)
            .unwrap()
            .into_iter()
            .map(|range| &source[range])
            .collect()
    }

    #[test]
    fn code_and_literal_links_do_not_start_translation() {
        for source in [
            "",
            " \t\r\n",
            "`🦀_id`",
            "```rust\nlet x = 3;\n```",
            "~~~rs\rlet x = 1;\r~~~\r",
            "    // literal code\r\n",
            "<div>Literal HTML</div>",
            "<https://example.com> <dev@example.com>",
        ] {
            assert!(prose(source).is_empty(), "{source:?}");
        }
    }

    #[test]
    fn markup_and_code_are_excluded_from_prose_ranges() {
        let source = "# Retry policy\n\nUse **the identifier** and `🦀_id` unchanged.\n\nRead [the guide](https://example.com) and ![an image](a.png), [id] and [id][].\n\n[id]: https://example.com\n";
        assert_eq!(
            prose(source),
            [
                "Retry policy",
                "Use",
                "the identifier",
                "and",
                "unchanged.",
                "Read",
                "the guide",
                "and",
                "an image",
                ",",
                "and",
                "."
            ]
        );
    }

    #[test]
    fn escape_entity_and_line_ending_offsets_match_source_bytes() {
        for ending in ["\n", "\r\n", "\r"] {
            let source = format!(r"Read \*the guide\* and &amp; the \_word.{ending}Next");
            assert_eq!(
                prose(&source),
                ["Read", "the guide", "and", "the", "word.", "Next"]
            );
            let source = format!("```rs{ending}code{ending}```{ending}Read this.{ending}");
            assert_eq!(prose(&source), ["Read this."]);
        }
    }
}
