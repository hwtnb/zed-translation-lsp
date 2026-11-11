use lsp_types::{Hover, HoverContents, MarkupContent, MarkupKind, Position, Range};
use uuid::Uuid;

use crate::{
    config::hover::{HoverFormat, HoverSettings},
    providers::dictionary::inline,
};

pub(super) mod cache;
pub(super) mod files;
pub(super) mod markdown;

use markdown::Urls;

#[derive(Clone)]
pub(crate) struct Preview<C> {
    id: uuid::Uuid,
    pub context: C,
    pub range: Range,
    pub translation: String,
    pub form: Form,
    pub is_comment: bool,
    /// The provider's line under the Hover title.
    pub note: Option<&'static str>,
    /// The `hover` settings that the translation completed under.
    pub hover: HoverSettings,
}

/// What a translation is, which decides how Hover shows it.
#[derive(Clone)]
pub(crate) enum Form {
    /// A dictionary entry in Markdown.
    Entry,
    /// The translation of a Markdown document, which is Markdown, with the
    /// URLs of its original.
    Markdown(Urls),
    /// Any other translation, with the URLs of its original.
    Text(Urls),
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
            form: Form::Text(Urls::default()),
            is_comment,
            note,
            hover: HoverSettings::default(),
        }
    }

    pub fn hover(&self, position: Position) -> Option<Hover> {
        // Zed keyboard hover uses the selection head, including the exclusive end.
        // Cached ranges match their own positions; there is no nearest-range fallback.
        if position < self.range.start || position > self.range.end {
            return None;
        }
        let body = match (&self.form, self.hover.format) {
            (Form::Entry, _) => self.translation.clone(),
            // A link to elsewhere would be one the original does not have.
            (Form::Markdown(urls), HoverFormat::Markdown) => {
                markdown::document(&self.translation, urls)
                    .unwrap_or_else(|| fenced_text(&self.translation))
            }
            (Form::Text(urls), HoverFormat::Markdown) => markdown::render(&self.translation, urls),
            (Form::Markdown(_) | Form::Text(_), HoverFormat::Text) => {
                fenced_text(&self.translation)
            }
        };
        let value = if self.hover.header {
            let header = match self.note {
                Some(note) => format!("> **Translation LSP**\n>\n> {note}"),
                None => "> **Translation LSP**".into(),
            };
            format!("{header}\n\n---\n\n{body}")
        } else {
            body
        };
        Some(Hover {
            contents: HoverContents::Markup(MarkupContent {
                kind: MarkupKind::Markdown,
                value,
            }),
            range: Some(self.range),
        })
    }

    /// The translation as the Markdown of an editor message. Zed wraps the
    /// lines of a message but not those of a code block in it, so the text is
    /// escaped instead of fenced.
    pub fn message(&self) -> String {
        if matches!(self.form, Form::Entry) {
            self.translation.clone()
        } else {
            plain_text(&self.translation)
        }
    }
}

/// `text` as Markdown that shows the words of each line as written, with the
/// line breaks of `text`.
fn plain_text(text: &str) -> String {
    text.lines()
        .map(|line| {
            let line = inline(line);
            // A line of `=` under a line of text would make it a heading.
            if line.starts_with('=') {
                format!("\\{line}")
            } else {
                line
            }
        })
        .collect::<Vec<_>>()
        .join("  \n")
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
    use std::time::{Duration, Instant};

    use pulldown_cmark::{Event, Parser, Tag, TagEnd};

    use crate::providers::chatgpt::test_support::Fixture;

    use super::*;

    #[test]
    fn messages_escape_markdown_and_keep_line_breaks() {
        let text = "**retry** `count`\n\n===\n- item\nhttps://example.com";
        assert_eq!(
            plain_text(text),
            "\\*\\*retry\\*\\* \\`count\\`  \n  \n\\===  \n\\- item  \nhttps:\u{2060}//example.com"
        );
        let markdown = plain_text(text);
        let mut parsed = Parser::new(&markdown).filter_map(|event| match event {
            Event::Text(text) => Some(text.into_string()),
            Event::HardBreak => Some("\n".into()),
            Event::Start(Tag::Heading { .. } | Tag::List(_) | Tag::CodeBlock(_)) => {
                panic!("Unexpected {event:?}")
            }
            _ => None,
        });
        assert_eq!(parsed.next().unwrap(), "*");
        assert!(parsed.any(|text| text == "\n"));
    }

    /// The text of `markdown` when it is one paragraph in Zed, without the
    /// word joiners that keep URLs from being links.
    fn zed_text(markdown: &str) -> Option<String> {
        let mut text = String::new();
        for event in Parser::new_ext(markdown, markdown::ZED_OPTIONS) {
            match event {
                Event::Text(part) => text.push_str(&part),
                Event::Start(Tag::Paragraph) | Event::End(TagEnd::Paragraph) => (),
                _ => return None,
            }
        }
        Some(text.replace('\u{2060}', ""))
    }

    #[test]
    fn messages_show_dashes_quotes_and_dots_as_written() {
        for (text, markdown) in [
            ("--verbose", "\\-\\-verbose"),
            ("---", "\\-\\-\\-"),
            ("a - b", "a - b"),
            ("It's \"quoted\"...", "It\\'s \\\"quoted\\\"\\.\\.\\."),
            ("1.5", "1\\.5"),
            ("e.g.", "e.g."),
            ("a..b", "a..b"),
            (
                "`--verbose` を付けると、各試行を表示する。",
                "\\`\\-\\-verbose\\` を付けると、各試行を表示する。",
            ),
        ] {
            assert_eq!(inline(text), markdown);
            assert_eq!(zed_text(markdown).as_deref(), Some(text), "{markdown:?}");
        }
        // Lines of pieces from a fixed sequence of pseudorandom numbers.
        const PIECES: &[&str] = &[
            "-", "--", ".", "...", "'", "\"", "a", "語", " ", "1", "e.g.", "+", ")", "*", "`", "<",
            ":", "//", "x://y", "[", "]", "(", "!", "#", "=", "|", "~", "_", "&", ";", "\\",
        ];
        let started = Instant::now();
        let mut state: u64 = 0x9E37_79B9_7F4A_7C15;
        let mut next = |bound: usize| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            (state % bound as u64) as usize
        };
        for _ in 0..2000 {
            let length = 1 + next(12);
            let text: String = (0..length).map(|_| PIECES[next(PIECES.len())]).collect();
            let markdown = inline(&text);
            let written = text.split_whitespace().collect::<Vec<_>>().join(" ");
            assert_eq!(
                zed_text(&markdown).as_deref(),
                Some(written.as_str()),
                "{text:?} → {markdown:?}"
            );
        }
        assert!(started.elapsed() < Duration::from_secs(10));
    }

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
        for (note, format) in [
            (Some("Mock translation (fixed sample)"), HoverFormat::Text),
            (None, HoverFormat::Text),
            (
                Some("Mock translation (fixed sample)"),
                HoverFormat::Markdown,
            ),
            (None, HoverFormat::Markdown),
        ] {
            let mut preview = Preview::new(
                auth.key().unwrap(),
                Range::new(Position::new(0, 0), Position::new(0, 1)),
                source.into(),
                false,
                note,
            );
            preview.hover.format = format;
            let value = value(&preview);
            assert!(value.starts_with("> **Translation LSP**"));
            assert!(!value.contains("ChatGPT 翻訳"));
            assert_eq!(
                value.contains("Mock translation (fixed sample)"),
                note.is_some()
            );
            let body = match format {
                HoverFormat::Text => fenced_text(source),
                HoverFormat::Markdown => markdown::render(source, &Urls::default()),
            };
            assert!(value.ends_with(&format!("\n\n---\n\n{body}")));
            let events: Vec<_> = Parser::new(&value).collect();
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
            assert!(
                events
                    .iter()
                    .any(|event| matches!(event, Event::Start(Tag::CodeBlock(_))))
            );
            assert!(!events.iter().any(|event| matches!(
                event,
                Event::Html(_) | Event::InlineHtml(_) | Event::Start(Tag::Image { .. })
            )));
        }
        assert!(fixture.state.lock().unwrap().records.is_empty());
    }

    fn value<C>(preview: &Preview<C>) -> String {
        let HoverContents::Markup(content) = preview.hover(preview.range.start).unwrap().contents
        else {
            panic!("Expected Markdown");
        };
        content.value
    }

    #[test]
    fn hover_settings_choose_the_header_and_the_form_of_the_body() {
        let translation = "`a_b` を使う。\n次の行";
        let mut preview = Preview::new(
            (),
            Range::default(),
            translation.into(),
            false,
            Some("Note"),
        );
        let header = "> **Translation LSP**\n>\n> Note\n\n---\n\n";
        let markdown = "`a_b` を使う。\\\n次の行";
        let fenced = "```text\n`a_b` を使う。\n次の行\n```";
        for (shows_header, format, expected) in [
            (true, HoverFormat::Markdown, format!("{header}{markdown}")),
            (false, HoverFormat::Markdown, markdown.into()),
            // The format of earlier versions, unchanged.
            (true, HoverFormat::Text, format!("{header}{fenced}")),
            (false, HoverFormat::Text, fenced.into()),
        ] {
            preview.hover = HoverSettings {
                header: shows_header,
                format,
            };
            assert_eq!(value(&preview), expected);
        }
        // An entry is Markdown in both formats, and so is the translation of a
        // Markdown document in Markdown.
        for (form, format, body) in [
            (Form::Entry, HoverFormat::Markdown, translation),
            (Form::Entry, HoverFormat::Text, translation),
            (
                Form::Markdown(Urls::default()),
                HoverFormat::Markdown,
                translation,
            ),
            (Form::Markdown(Urls::default()), HoverFormat::Text, fenced),
        ] {
            preview.form = form;
            preview.hover = HoverSettings {
                header: false,
                format,
            };
            assert_eq!(value(&preview), body);
        }
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
