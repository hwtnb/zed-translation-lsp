use std::ops::Range as ByteRange;

use lsp_types::{Position, Range, TextDocumentContentChangeEvent, Uri};

use crate::text::{comments::Comments, markdown::prose_ranges};

mod indexed_text;

use indexed_text::IndexedText;

pub(crate) struct TranslationInput {
    pub text: String,
    pub is_comment: bool,
}

pub(crate) struct Document {
    pub version: i32,
    // Versions can repeat after didClose/didOpen. This identifies an open lifetime.
    pub session: u64,
    pub language_id: String,
    text: Option<IndexedText>,
    comments: Option<Comments>,
    markdown_prose: Option<Vec<ByteRange<usize>>>,
}

impl Document {
    pub fn new(version: i32, session: u64, language_id: String, text: String) -> Self {
        Self {
            version,
            session,
            language_id,
            text: Some(IndexedText::new(text)),
            comments: None,
            markdown_prose: None,
        }
    }

    pub fn change(
        &mut self,
        version: i32,
        changes: Vec<TextDocumentContentChangeEvent>,
    ) -> Result<(), &'static str> {
        self.comments = None;
        self.markdown_prose = None;
        if version <= self.version {
            self.text = None;
            return Err("Document versions must increase; reopen or send a newer full update");
        }
        self.version = version;
        let mut text = self.text.take();
        for change in changes {
            if let Some(range) = change.range {
                let current = text.as_mut().ok_or("A full document update is required")?;
                current.replace(range, &change.text)?;
            } else {
                text = Some(IndexedText::new(change.text));
            }
        }
        self.text = text;
        Ok(())
    }

    pub fn text(&self) -> Option<&str> {
        self.text.as_ref().map(IndexedText::as_str)
    }

    pub fn supports_document_preview(&self) -> bool {
        matches!(self.language_id.as_str(), "markdown" | "plaintext")
    }

    pub fn full_range(&self) -> Option<Range> {
        let text = self.text.as_ref()?;
        Some(Range::new(
            Position::new(0, 0),
            text.position(text.as_str().len()).ok()?,
        ))
    }

    pub fn offset(&self, position: Position) -> Result<usize, &'static str> {
        self.text
            .as_ref()
            .ok_or("Document is not synchronized")?
            .offset(position)
    }

    pub fn selection(&self, range: Range) -> Result<&str, &'static str> {
        let indexed = self.text.as_ref().ok_or("Document is not synchronized")?;
        let text = indexed.as_str();
        let selected = &text[indexed.range(range)?];
        if selected.trim().is_empty() {
            return Err("Select a nonempty range of text");
        }
        Ok(selected)
    }

    pub fn markdown_selection_is_translatable(
        &mut self,
        range: Range,
    ) -> Result<bool, &'static str> {
        let indexed = self.text.as_ref().ok_or("Document is not synchronized")?;
        let text = indexed.as_str();
        let selected = indexed.range(range)?;
        if self.markdown_prose.is_none() {
            self.markdown_prose = Some(prose_ranges(text)?);
        }
        let prose = self.markdown_prose.as_ref().unwrap();
        let first = prose.partition_point(|span| span.end <= selected.start);
        let expected: Vec<_> = prose[first..]
            .iter()
            .take_while(|span| span.start < selected.end)
            .filter_map(|span| {
                let start = span.start.max(selected.start);
                let end = span.end.min(selected.end);
                let raw = &text[start..end];
                let trimmed = raw.trim();
                if trimmed.is_empty() {
                    return None;
                }
                let start = start + raw.len() - raw.trim_start().len() - selected.start;
                Some(start..start + trimmed.len())
            })
            .collect();
        // Avoid automatically translating fragments whose Markdown interpretation
        // differs from the document, such as the inside of an inline code span.
        Ok(!expected.is_empty() && prose_ranges(&text[selected])? == expected)
    }

    pub fn comment_range(&mut self, uri: &Uri, offset: usize) -> Option<Range> {
        let indexed = self.text.as_ref()?;
        let text = indexed.as_str();
        let comments = self
            .comments
            .get_or_insert_with(|| Comments::parse(&self.language_id, uri, text));
        let bytes = comments.range(comments.at(text, offset)?);
        Some(Range::new(
            indexed.position(bytes.start).ok()?,
            indexed.position(bytes.end).ok()?,
        ))
    }

    pub fn translation_input(
        &mut self,
        uri: &Uri,
        range: Range,
    ) -> Result<TranslationInput, &'static str> {
        let indexed = self.text.as_ref().ok_or("Document is not synchronized")?;
        let text = indexed.as_str();
        let bytes = indexed.range(range)?;
        let comments = self
            .comments
            .get_or_insert_with(|| Comments::parse(&self.language_id, uri, text));
        if let Some(group) = comments.at(text, bytes.start)
            && comments.range(group.clone()) == bytes
        {
            return Ok(TranslationInput {
                text: comments.body(text, group),
                is_comment: true,
            });
        }
        // Exact comment groups use the same body for Hover and Code Action, so
        // coalescing cannot depend on which operation arrived first.
        Ok(TranslationInput {
            text: text[bytes].to_owned(),
            is_comment: false,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn comment_cache_is_lazy_and_invalidated_by_edits_and_broken_sync() {
        let uri: Uri = "untitled:sample".parse().unwrap();
        let original = "🚀(); /* `id` */\r\n  // first\r\n  // second";
        let mut document = Document::new(1, 1, "rust".into(), original.into());
        assert!(document.comments.is_none());
        let group = document
            .comment_range(&uri, original.find("// second").unwrap())
            .unwrap();
        assert_eq!(group, Range::new(Position::new(1, 2), Position::new(2, 11)));
        assert_eq!(
            document.translation_input(&uri, group).unwrap().text,
            "first\r\nsecond"
        );
        assert_eq!(document.text(), Some(original));
        assert!(document.comments.is_some());
        let block = document
            .comment_range(&uri, original.find("/*").unwrap())
            .unwrap();
        assert_eq!(block.start, Position::new(0, 6));
        assert_eq!(
            document.translation_input(&uri, block).unwrap().text,
            "`id` "
        );
        for character in 0..=8 {
            let point = Position::new(0, character);
            if let Ok(offset) = document.offset(point) {
                assert_eq!(
                    document.text.as_ref().unwrap().position(offset).unwrap(),
                    point
                );
            }
        }
        assert!(
            document
                .text
                .as_ref()
                .unwrap()
                .position(original.find('\n').unwrap())
                .is_err()
        );
        document
            .change(
                2,
                vec![TextDocumentContentChangeEvent {
                    range: None,
                    range_length: None,
                    text: "let x = \"// hidden\";\n// new".into(),
                }],
            )
            .unwrap();
        assert!(document.comments.is_none());
        assert!(document.comment_range(&uri, 9).is_none());
        let new_range = document
            .comment_range(&uri, document.text().unwrap().find("// new").unwrap())
            .unwrap();
        assert_eq!(
            document.translation_input(&uri, new_range).unwrap().text,
            "new"
        );
        assert!(document.change(2, Vec::new()).is_err());
        assert!(document.comments.is_none());
        assert!(document.comment_range(&uri, 0).is_none());
    }

    #[test]
    fn markdown_prose_cache_is_lazy_and_invalidated_by_edits_and_broken_sync() {
        for ending in ["\n", "\r\n", "\r"] {
            let text =
                format!("```rust{ending}let retry_count = 3;{ending}```{ending}Read 🚀 the guide.");
            let mut document = Document::new(1, 1, "markdown".into(), text.clone());
            let code = Range::new(Position::new(1, 4), Position::new(1, 15));
            assert!(document.markdown_prose.is_none());
            assert!(!document.markdown_selection_is_translatable(code).unwrap());
            let cached = document.markdown_prose.as_ref().unwrap().as_ptr();
            let prose = Range::new(Position::new(3, 5), Position::new(3, 11));
            assert!(document.markdown_selection_is_translatable(prose).unwrap());
            assert_eq!(document.markdown_prose.as_ref().unwrap().as_ptr(), cached);
            assert_eq!(document.selection(prose).unwrap(), "🚀 the");
            document
                .change(
                    2,
                    vec![
                        TextDocumentContentChangeEvent {
                            range: Some(Range::new(Position::new(2, 0), Position::new(2, 3))),
                            range_length: None,
                            text: "End.".into(),
                        },
                        TextDocumentContentChangeEvent {
                            range: Some(Range::new(Position::new(0, 0), Position::new(0, 7))),
                            range_length: None,
                            text: "Opening.".into(),
                        },
                    ],
                )
                .unwrap();
            assert!(document.markdown_prose.is_none());
            assert!(document.markdown_selection_is_translatable(code).unwrap());
            assert!(document.change(2, Vec::new()).is_err());
            assert!(document.markdown_prose.is_none());
            assert!(document.markdown_selection_is_translatable(code).is_err());
            document
                .change(
                    3,
                    vec![TextDocumentContentChangeEvent {
                        range: None,
                        range_length: None,
                        text,
                    }],
                )
                .unwrap();
            assert!(document.markdown_prose.is_none());
            assert!(!document.markdown_selection_is_translatable(code).unwrap());
        }
    }

    #[test]
    fn js_comment_ranges_preserve_utf16_positions_and_unicode_separators() {
        let uri = "file:///sample.jsx".parse().unwrap();
        for ending in ["\n", "\r\n", "\r", "\u{2028}", "\u{2029}"] {
            let text = format!("const s = '🚀';{ending}  // `id`{ending}  // keep 🚀");
            let mut document = Document::new(1, 1, "javascript".into(), text.clone());
            let range = document
                .comment_range(&uri, text.find("// keep").unwrap())
                .unwrap();
            let start = if matches!(ending, "\u{2028}" | "\u{2029}") {
                Position::new(0, 18)
            } else {
                Position::new(1, 2)
            };
            assert_eq!(range.start, start);
            assert_eq!(
                &text[document.text.as_ref().unwrap().range(range).unwrap()],
                format!("// `id`{ending}  // keep 🚀")
            );
            assert_eq!(
                document.translation_input(&uri, range).unwrap().text,
                format!("`id`{ending}keep 🚀")
            );
        }
    }

    #[test]
    fn dense_document_above_one_mebibyte_has_no_document_size_cap() {
        let text = "// line\n/// doc\n".repeat(70_000);
        assert!(text.len() > 1024 * 1024);
        let uri = "file:///dense.rs".parse().unwrap();
        let mut document = Document::new(1, 1, "rust".into(), text);
        let end = document.text().unwrap().len();
        let range = document
            .comment_range(&uri, end - "/// doc\n".len())
            .unwrap();
        assert_eq!(
            range,
            Range::new(Position::new(139_999, 0), Position::new(139_999, 7))
        );
        assert_eq!(document.translation_input(&uri, range).unwrap().text, "doc");
    }

    #[test]
    fn utf16_crlf_and_multiline_selection_preserve_bytes() {
        let text = "前🚀x\r\n\tsecond `name`\r\nlast";
        let indexed = IndexedText::new(text.into());
        let range = Range::new(Position::new(0, 1), Position::new(1, 14));
        assert_eq!(
            &text[indexed.range(range).unwrap()],
            "🚀x\r\n\tsecond `name`"
        );
        assert_eq!(indexed.offset(Position::new(0, 4)).unwrap(), 8);
        assert!(indexed.offset(Position::new(0, 2)).is_err());
        assert!(indexed.offset(Position::new(0, 5)).is_err());
        assert!(indexed.offset(Position::new(3, 0)).is_err());
    }

    #[test]
    fn line_endings_and_eof_are_valid_positions() {
        for ending in ["\n", "\r\n", "\r"] {
            let text = format!("a{ending}🚀{ending}");
            let indexed = IndexedText::new(text.clone());
            assert_eq!(
                indexed.offset(Position::new(1, 2)).unwrap(),
                1 + ending.len() + 4
            );
            assert_eq!(indexed.offset(Position::new(2, 0)).unwrap(), text.len());
        }
        assert_eq!(
            IndexedText::new(String::new())
                .offset(Position::new(0, 0))
                .unwrap(),
            0
        );
        assert!(
            IndexedText::new("abc".into())
                .range(Range::new(Position::new(0, 2), Position::new(0, 1)))
                .is_err()
        );
    }

    #[test]
    fn incremental_edits_use_each_previous_change() {
        let mut document = Document::new(1, 1, "markdown".into(), "前🚀\r\nold".into());
        document
            .change(
                2,
                vec![
                    TextDocumentContentChangeEvent {
                        range: Some(Range::new(Position::new(1, 0), Position::new(1, 3))),
                        range_length: None,
                        text: "new🚀".into(),
                    },
                    TextDocumentContentChangeEvent {
                        range: Some(Range::new(Position::new(1, 3), Position::new(1, 5))),
                        range_length: None,
                        text: "!".into(),
                    },
                ],
            )
            .unwrap();
        assert_eq!(document.text(), Some("前🚀\r\nnew!"));
        assert_eq!(document.version, 2);
    }

    #[test]
    fn invalid_change_discards_partial_edits_and_full_update_recovers_positions() {
        let mut document = Document::new(1, 1, "markdown".into(), "前🚀\r\nold".into());
        let changed = |range, text: &str| TextDocumentContentChangeEvent {
            range,
            range_length: None,
            text: text.into(),
        };
        assert!(
            document
                .change(
                    2,
                    vec![
                        changed(
                            Some(Range::new(Position::new(1, 0), Position::new(1, 3))),
                            "new"
                        ),
                        changed(
                            Some(Range::new(Position::new(0, 2), Position::new(0, 3))),
                            "bad"
                        ),
                    ],
                )
                .is_err()
        );
        assert!(document.text().is_none());
        assert!(document.offset(Position::new(0, 0)).is_err());
        assert!(
            document
                .change(3, vec![changed(Some(Range::default()), "bad")])
                .is_err()
        );
        document.change(4, vec![changed(None, "a\rX\nb")]).unwrap();
        document
            .change(
                5,
                vec![changed(
                    Some(Range::new(Position::new(1, 0), Position::new(1, 1))),
                    "",
                )],
            )
            .unwrap();
        assert_eq!(document.text(), Some("a\r\nb"));
        assert_eq!(document.offset(Position::new(1, 0)).unwrap(), 3);
        assert!(document.change(5, Vec::new()).is_err());
        assert!(document.text().is_none());
    }

    #[test]
    fn bad_edits_disable_selection_until_a_newer_full_update() {
        let mut document = Document::new(1, 1, "markdown".into(), "🚀".into());
        let range = Range::new(Position::new(0, 0), Position::new(0, 2));
        assert!(
            document
                .change(
                    2,
                    vec![TextDocumentContentChangeEvent {
                        range: Some(Range::new(Position::new(0, 1), Position::new(0, 2))),
                        range_length: None,
                        text: String::new(),
                    }]
                )
                .is_err()
        );
        assert!(document.selection(range).is_err());
        document
            .change(
                3,
                vec![TextDocumentContentChangeEvent {
                    range: None,
                    range_length: None,
                    text: "ok".into(),
                }],
            )
            .unwrap();
        assert_eq!(document.selection(range).unwrap(), "ok");
        assert!(document.change(3, Vec::new()).is_err());
        assert!(document.selection(range).is_err());
    }
}
