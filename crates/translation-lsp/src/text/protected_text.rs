use std::borrow::Cow;
use std::fmt;
use std::ops::Range;

use pulldown_cmark::{Event, LinkType, Options, Parser, Tag, TagEnd};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// Source slices partition the entire input; only plain prose may be replaced.
pub(crate) struct ProtectedText<'a> {
    source: &'a str,
    segments: Vec<Segment<'a>>,
}

#[derive(Serialize)]
pub(crate) struct Segment<'a> {
    pub(crate) text: &'a str,
    pub(crate) protected: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ProtectionError {
    InvalidSourceRange,
    InvalidResponse,
    WrongTranslationCount,
    IntroducedLineBreaks,
    InvalidTranslation,
}

impl fmt::Display for ProtectionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::InvalidSourceRange => "Invalid Markdown source range",
            Self::InvalidResponse => "invalid translation response format",
            Self::WrongTranslationCount => "wrong number of translations",
            Self::IntroducedLineBreaks => "inserted line breaks in translated prose",
            Self::InvalidTranslation => "changed protected Markdown structure",
        })
    }
}

impl std::error::Error for ProtectionError {}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Response {
    translations: Vec<String>,
}

impl<'a> ProtectedText<'a> {
    pub(crate) fn new(source: &'a str) -> Result<Self, ProtectionError> {
        let parsing_source = parsing_source(source);
        let mut segments = Vec::new();
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
                        .ok_or(ProtectionError::InvalidSourceRange)?;
                    // Entities and other parser substitutions retain their spelling.
                    if raw != text.as_ref() {
                        continue;
                    }
                    if range.start < cursor {
                        return Err(ProtectionError::InvalidSourceRange);
                    }
                    // OffsetIter leaves an escape's backslash in the preceding gap.
                    // Keep the escaped character with it, using the parser's boundary.
                    if range.start > cursor
                        && source.as_bytes().get(range.start - 1) == Some(&b'\\')
                    {
                        range.start += raw.chars().next().map_or(0, char::len_utf8);
                    }
                    let raw = &source[range.clone()];
                    let trimmed = raw.trim_matches(char::is_whitespace);
                    if trimmed.is_empty() {
                        continue;
                    }
                    range.start += raw.len() - raw.trim_start_matches(char::is_whitespace).len();
                    range.end = range.start + trimmed.len();

                    push_segment(&mut segments, &source[cursor..range.start], true);
                    push_segment(&mut segments, &source[range.clone()], false);
                    cursor = range.end;
                }
                _ => {}
            }
        }
        push_segment(&mut segments, &source[cursor..], true);
        Ok(Self { source, segments })
    }

    pub(crate) fn segments(&self) -> &[Segment<'a>] {
        &self.segments
    }

    pub(crate) fn response_schema(&self) -> Value {
        let count = self
            .segments
            .iter()
            .filter(|segment| !segment.protected)
            .count();
        json!({
            "type": "object",
            "properties": {
                "translations": {
                    "type": "array",
                    "items": {"type": "string", "pattern": "^[^\\r\\n]*$"},
                    "minItems": count,
                    "maxItems": count
                }
            },
            "required": ["translations"],
            "additionalProperties": false
        })
    }

    pub(crate) fn restore(&self, response_json: &str) -> Result<String, ProtectionError> {
        let response: Response =
            serde_json::from_str(response_json).map_err(|_| ProtectionError::InvalidResponse)?;
        let mut translations = response.translations.into_iter();
        let mut restored = String::with_capacity(self.source.len());
        let mut protected_ranges = Vec::new();

        for segment in &self.segments {
            if segment.protected {
                let start = restored.len();
                restored.push_str(segment.text);
                protected_ranges.push(start..restored.len());
                continue;
            }
            let translation = translations
                .next()
                .ok_or(ProtectionError::WrongTranslationCount)?;
            if translation.contains(['\r', '\n']) {
                return Err(ProtectionError::IntroducedLineBreaks);
            }
            restored.push_str(&translation);
        }
        if translations.next().is_some() {
            return Err(ProtectionError::WrongTranslationCount);
        }
        if !same_structure(self.source, &restored)
            || !protected_spans_remain(&restored, &protected_ranges)?
        {
            return Err(ProtectionError::InvalidTranslation);
        }
        Ok(restored)
    }
}

fn push_segment<'a>(segments: &mut Vec<Segment<'a>>, text: &'a str, protected: bool) {
    if !text.is_empty() {
        segments.push(Segment { text, protected });
    }
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

fn same_structure(source: &str, restored: &str) -> bool {
    let source = parsing_source(source);
    let restored = parsing_source(restored);
    // Empty prose may remove a paragraph. Literal code, breaks, links and
    // formatting must still produce exactly the same non-prose events.
    let structural = |event: &Event<'_>| {
        !matches!(
            event,
            Event::Text(_) | Event::Start(Tag::Paragraph) | Event::End(TagEnd::Paragraph)
        )
    };
    parser(&source)
        .filter(structural)
        .eq(parser(&restored).filter(structural))
}

fn protected_spans_remain(
    restored: &str,
    protected_ranges: &[Range<usize>],
) -> Result<bool, ProtectionError> {
    let reparsed = ProtectedText::new(restored)?;
    let mut expected = protected_ranges.iter().peekable();
    let mut cursor = 0;
    for segment in reparsed.segments() {
        let end = cursor + segment.text.len();
        while expected.peek().is_some_and(|range| range.end <= cursor) {
            expected.next();
        }
        if !segment.protected && expected.peek().is_some_and(|range| range.start < end) {
            return Ok(false);
        }
        cursor = end;
    }
    Ok(true)
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};

    use super::*;

    #[test]
    fn response_contract_restores_ordered_prose_without_returned_ids() {
        let protected = ProtectedText::new(
            "Use `retry_count` unchanged.\r\nRead the [guide](https://example.invalid).",
        )
        .unwrap();
        let translated = protected.restore(
            r#"{"translations":["", "を変更せずに使ってください。", "", "ガイド", "を読んでください。"]}"#,
        ).unwrap();
        assert_eq!(
            translated,
            " `retry_count` を変更せずに使ってください。\r\n [ガイド](https://example.invalid)を読んでください。"
        );
    }

    fn response(
        protected: &ProtectedText<'_>,
        mut translate: impl FnMut(&str) -> String,
    ) -> String {
        let segments: Vec<_> = protected
            .segments()
            .iter()
            .filter(|segment| !segment.protected)
            .map(|segment| json!(translate(segment.text)))
            .collect();
        json!({"translations": segments}).to_string()
    }

    #[test]
    fn source_order_and_serialization_use_only_source_slices() {
        let source = "Use `retry_count` unchanged.";
        let protected = ProtectedText::new(source).unwrap();
        let segments = serde_json::to_value(protected.segments()).unwrap();
        assert_eq!(
            segments,
            json!([
                {"text":"Use","protected":false},
                {"text":" `retry_count` ","protected":true},
                {"text":"unchanged.","protected":false}
            ])
        );
        assert_eq!(
            protected
                .restore(r#"{"translations":["","の値を維持する。"]}"#)
                .unwrap(),
            " `retry_count` の値を維持する。"
        );
    }

    #[test]
    fn changing_prose_keeps_code_formatting_and_line_endings() {
        for ending in ["\n", "\r\n", "\r"] {
            let source = format!(
                "  # Retry policy{ending}{ending}Use **the identifier** and `🦀_id` unchanged.{ending}{ending}```rust{ending}let x = 3; // Keep bytes.{ending}```{ending}{ending}> Read [the guide](https://example.com?q=retry#policy).{ending}{ending}- Retry once.{ending}"
            );
            let protected = ProtectedText::new(&source).unwrap();
            assert_eq!(
                protected
                    .restore(&response(&protected, str::to_owned))
                    .unwrap(),
                source
            );
            let restored = protected
                .restore(&response(&protected, |_| "訳文".into()))
                .unwrap();
            for literal in [
                "  # ".to_owned(),
                "**".to_owned(),
                "`🦀_id`".to_owned(),
                format!("```rust{ending}let x = 3; // Keep bytes.{ending}```"),
                "https://example.com?q=retry#policy".to_owned(),
                format!("{ending}{ending}> "),
                format!("{ending}{ending}- "),
            ] {
                assert!(restored.contains(&literal));
            }
            assert!(!restored.contains("Retry policy"));
            assert_eq!(
                source.matches(ending).count(),
                restored.matches(ending).count()
            );
        }
    }

    #[test]
    fn code_only_and_empty_sources_have_no_translation_targets() {
        for source in [
            "",
            " \t\r\n\r\n",
            "`🦀_id`",
            "```rust\nlet x = \"// not a comment\";\n```\n",
            "~~~rs\rlet x = 1;\r~~~\r",
            "    // literal code\r\n",
            "<div>Literal HTML</div>\n",
            "<https://example.com> <dev@example.com>",
        ] {
            let protected = ProtectedText::new(source).unwrap();
            assert!(protected.segments().iter().all(|segment| segment.protected));
            assert_eq!(protected.restore(r#"{"translations":[]}"#).unwrap(), source);
        }
    }

    #[test]
    fn literal_html_destinations_and_reference_keys_are_protected() {
        let source = "Read <span title=\"Keep\">the guide</span> at <https://example.com/code> or <dev@example.com>. Use [a guide][id] and ![an image](a.png \"Title\"), [id] and [id][].\n\n[id]: https://example.com \"Keep title\"\n";
        let protected = ProtectedText::new(source).unwrap();
        let restored = protected
            .restore(&response(&protected, |_| "訳文".into()))
            .unwrap();
        for literal in [
            "<span title=\"Keep\">",
            "</span>",
            "<https://example.com/code>",
            "<dev@example.com>",
            "][id]",
            "](a.png \"Title\")",
            "[id]",
            "[id][]",
            "[id]: https://example.com \"Keep title\"\n",
        ] {
            assert!(restored.contains(literal));
        }
        assert!(!restored.contains("the guide"));
    }

    #[test]
    fn escape_and_entity_spellings_survive_changed_prose() {
        let source = r"Read \*the guide\* and &amp; the \_word and \\path.";
        let protected = ProtectedText::new(source).unwrap();
        assert_eq!(
            protected
                .restore(&response(&protected, str::to_owned))
                .unwrap(),
            source
        );
        let restored = protected
            .restore(&response(&protected, |_| "訳文".into()))
            .unwrap();
        for literal in [r"\*", "&amp;", r"\_", r"\\"] {
            assert!(restored.contains(literal));
        }
        assert!(!restored.contains("the guide"));
    }

    #[test]
    fn gfm_syntax_is_preserved_after_changing_prose() {
        let source = "| Heading | Value |\n| --- | ---: |\n| Body | ~~old~~ |\n\n- [x] Retry once.\n\n> [!NOTE]\n> Read this.\n\nRead footnote[^id].\n\n[^id]: The note.\n";
        let protected = ProtectedText::new(source).unwrap();
        assert_eq!(
            protected
                .restore(&response(&protected, str::to_owned))
                .unwrap(),
            source
        );
        let restored = protected
            .restore(&response(&protected, |_| "訳文".into()))
            .unwrap();
        for literal in [
            "| --- | ---: |",
            "~~",
            "- [x] ",
            "> [!NOTE]",
            "[^id]",
            "[^id]: ",
        ] {
            assert!(restored.contains(literal));
        }
    }

    #[test]
    fn whitespace_and_hard_break_bytes_survive() {
        let source = "  Read  `id`\t next  \r\nAgain\\\rLast\n";
        let protected = ProtectedText::new(source).unwrap();
        assert_eq!(
            protected
                .restore(&response(&protected, str::to_owned))
                .unwrap(),
            source
        );
        assert_eq!(
            protected
                .restore(&response(&protected, |_| "訳文".into()))
                .unwrap(),
            "  訳文  `id`\t 訳文  \r\n訳文\\\r訳文\n"
        );
    }

    #[test]
    fn missing_and_extra_translations_are_rejected() {
        let protected = ProtectedText::new("Use `id` unchanged.").unwrap();
        for translations in [json!([]), json!(["訳"]), json!(["訳", "訳", "訳"])] {
            assert_eq!(
                protected
                    .restore(&json!({"translations":translations}).to_string())
                    .err(),
                Some(ProtectionError::WrongTranslationCount)
            );
        }
    }

    #[test]
    fn invalid_json_shapes_and_types_are_rejected_without_fallback() {
        let protected = ProtectedText::new("Read this.").unwrap();
        for invalid in [
            "Raw translated text",
            "{\"translations\":[",
            "[]",
            "{}",
            r#"{"translations":null}"#,
            r#"{"translations":["訳"],"unknown":true}"#,
            r#"{"translations":[{"id":0,"text":"訳"}]}"#,
            r#"{"translations":[0]}"#,
            r#"{"translations":[true]}"#,
            r#"{"translations":[null]}"#,
            r#"{"translations":[["訳"]]}"#,
            r#"{"translations":[],"translations":[]}"#,
            r#"{"translations":["訳"]} trailing"#,
        ] {
            assert_eq!(
                protected.restore(invalid).err(),
                Some(ProtectionError::InvalidResponse)
            );
        }
        let valid: Value = json!({"translations":[""]});
        assert_eq!(protected.restore(&valid.to_string()).unwrap(), "");
    }

    #[test]
    fn inserted_line_endings_and_markdown_structure_are_rejected() {
        let protected = ProtectedText::new("Read this.").unwrap();
        for invalid in [
            "new\nline",
            "new\rline",
            "new\r\nline",
            "`new code`",
            "```new fence",
            "**bold**",
            "[label](https://example.com)",
            "<script>code</script>",
            "# heading",
            "- new item",
            "    indented code",
            "---",
        ] {
            assert_eq!(
                protected
                    .restore(&response(&protected, |_| invalid.into()))
                    .err(),
                Some(if invalid.contains(['\r', '\n']) {
                    ProtectionError::IntroducedLineBreaks
                } else {
                    ProtectionError::InvalidTranslation
                })
            );
        }
        assert_eq!(
            protected
                .restore(&response(&protected, |_| "普通の句読点、(例)！ 🦀".into()))
                .unwrap(),
            "普通の句読点、(例)！ 🦀"
        );
        let formatted = ProtectedText::new("**Read** this.").unwrap();
        assert_eq!(
            formatted
                .restore(&response(&formatted, |text| if text == "Read" {
                    "".into()
                } else {
                    "訳".into()
                }))
                .err(),
            Some(ProtectionError::InvalidTranslation)
        );
    }

    #[test]
    fn generated_syntax_cannot_replace_the_role_of_original_protected_bytes() {
        let protected = ProtectedText::new("Read`id`after.").unwrap();
        let counterfeit = response(&protected, |text| {
            if text == "Read" {
                "`id`\\".into()
            } else {
                text.into()
            }
        });
        assert_eq!(
            protected.restore(&counterfeit).err(),
            Some(ProtectionError::InvalidTranslation)
        );
        let protected = ProtectedText::new("Read&amp;after.").unwrap();
        let escaped_entity = response(&protected, |text| {
            if text == "Read" {
                "訳文\\".into()
            } else {
                text.into()
            }
        });
        assert_eq!(
            protected.restore(&escaped_entity).err(),
            Some(ProtectionError::InvalidTranslation)
        );
    }

    #[test]
    fn diagnostics_never_echo_source_or_response() {
        fn assert_error_trait(_: &dyn std::error::Error) {}
        let protected = ProtectedText::new("PRIVATE_SOURCE").unwrap();
        let error = protected.restore("PRIVATE_RESPONSE").err().unwrap();
        assert_error_trait(&error);
        assert_eq!(error.to_string(), "invalid translation response format");
        assert_eq!(format!("{error:?}"), "InvalidResponse");
    }
}
