use lsp_types::{Hover, HoverContents, MarkupContent, MarkupKind, Position, Range};

pub(crate) struct Preview {
    pub context: crate::providers::Context,
    pub range: Range,
    pub translation: String,
    pub is_comment: bool,
}

impl Preview {
    pub fn hover(&self, position: Position) -> Option<Hover> {
        // Zed keyboard hover uses the selection head, including the exclusive end.
        // Only this document's last translated range can match; no nearest-range fallback.
        if position < self.range.start || position > self.range.end {
            return None;
        }
        let header = if matches!(self.context, crate::providers::Context::Mock) {
            "> **Zed Code Translation**\n>\n> 模擬翻訳（固定サンプル）"
        } else {
            "> **Zed Code Translation**"
        };
        Some(Hover {
            contents: HoverContents::Markup(MarkupContent {
                kind: MarkupKind::Markdown,
                value: format!("{header}\n\n---\n\n{}", fenced_text(&self.translation)),
            }),
            range: Some(self.range),
        })
    }
}

fn fenced_text(text: &str) -> String {
    let mut longest = 0;
    let mut current = 0;
    for character in text.chars() {
        current = if character == '`' { current + 1 } else { 0 };
        longest = longest.max(current);
    }
    let fence = "`".repeat(3.max(longest + 1));
    format!("{fence}text\n{text}\n{fence}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fences_preserve_whitespace_identifiers_and_markdown() {
        let source = "\t`retry_count`  🚀\r\n````\r\n<script>**text**</script>";
        assert_eq!(fenced_text(source), format!("`````text\n{source}\n`````"));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn header_and_rule_leave_both_provider_bodies_literal() {
        let fixture = crate::providers::chatgpt::test_support::Fixture::new().await;
        let auth = fixture.auth();
        fixture.connected(&auth, 3600).await;
        let source =
            "\t`id` 🚀\r\n````\r\n<script>**text**</script> ![image](https://example.invalid/)";
        for context in [
            crate::providers::Context::Mock,
            crate::providers::Context::ChatGpt(auth.key().unwrap()),
        ] {
            let is_mock = matches!(context, crate::providers::Context::Mock);
            let preview = Preview {
                context,
                range: Range::new(Position::new(0, 0), Position::new(0, 1)),
                translation: source.into(),
                is_comment: false,
            };
            let HoverContents::Markup(content) =
                preview.hover(preview.range.start).unwrap().contents
            else {
                panic!("Expected Markdown");
            };
            assert!(content.value.starts_with("> **Zed Code Translation**"));
            assert!(!content.value.contains("ChatGPT 翻訳"));
            assert_eq!(content.value.contains("模擬翻訳（固定サンプル）"), is_mock);
            assert!(content.value.ends_with(&fenced_text(source)));
            let events: Vec<_> = pulldown_cmark::Parser::new(&content.value).collect();
            assert_eq!(
                events
                    .iter()
                    .filter(|event| matches!(event, pulldown_cmark::Event::Rule))
                    .count(),
                1
            );
            assert_eq!(
                events
                    .iter()
                    .filter(|event| matches!(
                        event,
                        pulldown_cmark::Event::Start(pulldown_cmark::Tag::BlockQuote(_))
                    ))
                    .count(),
                1
            );
            assert_eq!(
                events
                    .iter()
                    .filter(|event| matches!(
                        event,
                        pulldown_cmark::Event::Start(pulldown_cmark::Tag::CodeBlock(_))
                    ))
                    .count(),
                1
            );
            assert!(!events.iter().any(|event| matches!(
                event,
                pulldown_cmark::Event::Html(_)
                    | pulldown_cmark::Event::InlineHtml(_)
                    | pulldown_cmark::Event::Start(pulldown_cmark::Tag::Image { .. })
            )));
        }
        assert!(fixture.state.lock().unwrap().records.is_empty());
    }

    #[test]
    fn hover_includes_only_its_own_range_and_exact_end() {
        let range = Range::new(Position::new(1, 4), Position::new(2, 35));
        let preview = Preview {
            range,
            translation: "模擬訳".into(),
            context: crate::providers::Context::Mock,
            is_comment: false,
        };
        for position in [range.start, Position::new(2, 10), range.end] {
            assert_eq!(preview.hover(position).unwrap().range, Some(range));
        }
        assert!(preview.hover(Position::new(1, 3)).is_none());
        assert!(preview.hover(Position::new(2, 36)).is_none());
    }
}
