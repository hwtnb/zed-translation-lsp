use lsp_types::{Hover, HoverContents, MarkupContent, MarkupKind, Position, Range};
use uuid::Uuid;

pub(super) mod cache;
pub(super) mod files;

#[derive(Clone)]
pub(crate) struct Preview<C> {
    id: uuid::Uuid,
    pub context: C,
    pub range: Range,
    pub translation: String,
    /// The translation is a dictionary entry in Markdown.
    pub entry: bool,
    pub is_comment: bool,
    /// The provider's line under the Hover title.
    pub note: Option<&'static str>,
}

impl<C> Preview<C> {
    pub fn new(
        context: C,
        range: Range,
        translation: String,
        is_comment: bool,
        note: Option<&'static str>,
    ) -> Self {
        Self {
            id: Uuid::new_v4(),
            context,
            range,
            translation,
            entry: false,
            is_comment,
            note,
        }
    }

    pub fn hover(&self, position: Position) -> Option<Hover> {
        // Zed keyboard hover uses the selection head, including the exclusive end.
        // Cached ranges match their own positions; there is no nearest-range fallback.
        if position < self.range.start || position > self.range.end {
            return None;
        }
        let header = match self.note {
            Some(note) => format!("> **Translation LSP**\n>\n> {note}"),
            None => "> **Translation LSP**".into(),
        };
        let body = if self.entry {
            self.translation.clone()
        } else {
            fenced_text(&self.translation)
        };
        Some(Hover {
            contents: HoverContents::Markup(MarkupContent {
                kind: MarkupKind::Markdown,
                value: format!("{header}\n\n---\n\n{body}"),
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
    use pulldown_cmark::{Event, Parser, Tag};

    use crate::providers::chatgpt::test_support::Fixture;

    use super::*;

    #[test]
    fn fences_preserve_whitespace_identifiers_and_markdown() {
        let source = "\t`retry_count`  🚀\r\n````\r\n<script>**text**</script>";
        assert_eq!(fenced_text(source), format!("`````text\n{source}\n`````"));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn header_and_rule_leave_both_provider_bodies_literal() {
        let fixture = Fixture::new().await;
        let auth = fixture.auth();
        fixture.connected(&auth, 3600).await;
        let source =
            "\t`id` 🚀\r\n````\r\n<script>**text**</script> ![image](https://example.invalid/)";
        for note in [Some("Mock translation (fixed sample)"), None] {
            let preview = Preview::new(
                auth.key().unwrap(),
                Range::new(Position::new(0, 0), Position::new(0, 1)),
                source.into(),
                false,
                note,
            );
            let HoverContents::Markup(content) =
                preview.hover(preview.range.start).unwrap().contents
            else {
                panic!("Expected Markdown");
            };
            assert!(content.value.starts_with("> **Translation LSP**"));
            assert!(!content.value.contains("ChatGPT 翻訳"));
            assert_eq!(
                content.value.contains("Mock translation (fixed sample)"),
                note.is_some()
            );
            assert!(content.value.ends_with(&fenced_text(source)));
            let events: Vec<_> = Parser::new(&content.value).collect();
            assert_eq!(
                events
                    .iter()
                    .filter(|event| matches!(event, Event::Rule))
                    .count(),
                1
            );
            assert_eq!(
                events
                    .iter()
                    .filter(|event| matches!(event, Event::Start(Tag::BlockQuote(_))))
                    .count(),
                1
            );
            assert_eq!(
                events
                    .iter()
                    .filter(|event| matches!(event, Event::Start(Tag::CodeBlock(_))))
                    .count(),
                1
            );
            assert!(!events.iter().any(|event| matches!(
                event,
                Event::Html(_) | Event::InlineHtml(_) | Event::Start(Tag::Image { .. })
            )));
        }
        assert!(fixture.state.lock().unwrap().records.is_empty());
    }

    #[test]
    fn hover_includes_only_its_own_range_and_exact_end() {
        let range = Range::new(Position::new(1, 4), Position::new(2, 35));
        let preview = Preview::new((), range, "模擬訳".into(), false, None);
        for position in [range.start, Position::new(2, 10), range.end] {
            assert_eq!(preview.hover(position).unwrap().range, Some(range));
        }
        assert!(preview.hover(Position::new(1, 3)).is_none());
        assert!(preview.hover(Position::new(2, 36)).is_none());
    }
}
