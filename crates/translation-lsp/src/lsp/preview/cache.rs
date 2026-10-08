use super::{Preview, fenced_text};
use crate::{config::limits::TranslationLimits, providers::Context};
use lsp_types::{Hover, HoverContents, MarkupContent, MarkupKind, Position, Range, Uri};

struct Entry {
    uri: Uri,
    ticket: u64,
    preview: Preview,
    tab: bool,
    failed: bool,
    used: u64,
}

pub(crate) struct Cache {
    entries: Vec<Entry>,
    clock: u64,
    count_limit: usize,
    byte_limit: usize,
}

pub(crate) fn contains(outer: Range, inner: Range) -> bool {
    outer.start <= inner.start && inner.end <= outer.end
}

pub(crate) fn overlaps(a: Range, b: Range) -> bool {
    if a.start == a.end {
        return b.start <= a.start && a.start <= b.end;
    }
    if b.start == b.end {
        return a.start <= b.start && b.start <= a.end;
    }
    a.start < b.end && b.start < a.end
}

impl Cache {
    #[cfg(test)]
    pub fn latest(&self, uri: &Uri) -> Option<&Preview> {
        self.entries
            .iter()
            .filter(|e| &e.uri == uri && !e.failed)
            .max_by_key(|e| e.ticket)
            .map(|e| &e.preview)
    }
    #[cfg(test)]
    pub fn successes(&self) -> usize {
        self.entries.iter().filter(|e| !e.failed).count()
    }

    pub fn new(limits: TranslationLimits) -> Self {
        Self {
            entries: Vec::new(),
            clock: 0,
            count_limit: limits.cache.entries,
            byte_limit: limits
                .cache
                .capacity_kib
                .bytes()
                .max(limits.output_kib.bytes()),
        }
    }

    pub fn clear(&mut self) {
        self.entries.clear();
    }
    pub fn remove_document(&mut self, uri: &Uri) {
        self.entries.retain(|e| &e.uri != uri);
    }
    pub fn retain_context(&mut self, context: &Context) {
        self.entries.retain(|e| &e.preview.context == context);
    }
    pub fn exact(&mut self, uri: &Uri, range: Range, tab: bool) -> Option<Preview> {
        let index = self
            .entries
            .iter()
            .enumerate()
            .filter(|(_, e)| &e.uri == uri && e.preview.range == range && e.tab == tab && !e.failed)
            .max_by_key(|(_, e)| e.ticket)
            .map(|(i, _)| i)?;
        self.clock += 1;
        self.entries[index].used = self.clock;
        Some(self.entries[index].preview.clone())
    }

    pub fn hover(
        &mut self,
        uri: &Uri,
        position: Position,
        failed: bool,
        eligible: impl Fn(&Preview) -> bool,
    ) -> Option<Hover> {
        let index = self
            .entries
            .iter()
            .enumerate()
            .filter(|(_, e)| {
                &e.uri == uri
                    && !e.tab
                    && e.failed == failed
                    && e.preview.range.start <= position
                    && position <= e.preview.range.end
                    && eligible(&e.preview)
            })
            .max_by_key(|(_, e)| e.ticket)
            .map(|(i, _)| i)?;
        self.clock += 1;
        let entry = &mut self.entries[index];
        entry.used = self.clock;
        if failed {
            Some(Hover {
                range: Some(entry.preview.range),
                contents: HoverContents::Markup(MarkupContent {
                    kind: MarkupKind::Markdown,
                    value: format!(
                        "> **Translation LSP: translation failed**\n\n{}\n\nRun Translate selection to retry.",
                        fenced_text(&entry.preview.translation)
                    ),
                }),
            })
        } else {
            entry.preview.hover(position)
        }
    }

    pub fn insert(&mut self, uri: Uri, ticket: u64, preview: Preview, tab: bool, failed: bool) {
        if self.entries.iter().any(|e| {
            e.uri == uri
                && e.tab == tab
                && e.ticket > ticket
                && !e.failed
                && (tab || contains(e.preview.range, preview.range))
        }) {
            return;
        }
        self.entries.retain(|e| {
            if e.uri != uri || e.tab != tab || e.ticket > ticket {
                return true;
            }
            if failed {
                return !e.failed || e.preview.range != preview.range;
            }
            !(tab || contains(preview.range, e.preview.range))
        });
        self.clock += 1;
        self.entries.push(Entry {
            uri,
            ticket,
            preview,
            tab,
            failed,
            used: self.clock,
        });
        while self.entries.len() > self.count_limit
            || self
                .entries
                .iter()
                .map(|e| e.preview.translation.len())
                .sum::<usize>()
                > self.byte_limit
        {
            let oldest = self
                .entries
                .iter()
                .enumerate()
                .min_by_key(|(_, e)| e.used)
                .unwrap()
                .0;
            self.entries.remove(oldest);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn range(start: u32, end: u32) -> Range {
        Range::new(Position::new(0, start), Position::new(0, end))
    }

    fn preview(range: Range, text: &str) -> Preview {
        Preview {
            context: Context::Mock,
            range,
            translation: text.into(),
            is_comment: false,
        }
    }

    fn value(hover: Hover) -> String {
        let HoverContents::Markup(markup) = hover.contents else {
            panic!("markup expected")
        };
        markup.value
    }

    #[test]
    fn ranges_survive_reverse_completion_failures_and_tab_results() {
        let uri: Uri = "file:///ranges.md".parse().unwrap();
        let mut cache = Cache::new(TranslationLimits::default());
        cache.insert(uri.clone(), 2, preview(range(0, 10), "new"), false, false);
        cache.insert(uri.clone(), 1, preview(range(0, 5), "old"), false, false);
        assert!(cache.exact(&uri, range(0, 5), false).is_none());
        cache.insert(
            uri.clone(),
            3,
            preview(range(12, 20), "second"),
            false,
            false,
        );
        cache.insert(
            uri.clone(),
            4,
            preview(range(0, 10), "failure"),
            false,
            true,
        );
        cache.insert(
            uri.clone(),
            5,
            preview(range(0, 30), "whole document"),
            true,
            false,
        );
        assert!(
            value(
                cache
                    .hover(&uri, Position::new(0, 3), false, |_| true)
                    .unwrap()
            )
            .contains("new")
        );
        assert!(
            value(
                cache
                    .hover(&uri, Position::new(0, 15), false, |_| true)
                    .unwrap()
            )
            .contains("second")
        );
        assert!(
            cache
                .hover(&uri, Position::new(0, 25), false, |_| true)
                .is_none()
        );
        cache.insert(uri.clone(), 6, preview(range(0, 10), "retry"), false, false);
        assert!(
            cache
                .hover(&uri, Position::new(0, 3), true, |_| true)
                .is_none()
        );
        assert_eq!(
            cache.exact(&uri, range(0, 10), false).unwrap().translation,
            "retry"
        );
        cache.remove_document(&uri);
        assert!(cache.entries.is_empty());
    }

    #[test]
    fn global_lru_enforces_entries_bytes_and_single_output_floor() {
        let mut limits: TranslationLimits = serde_json::from_value(serde_json::json!({
            "output_kib": 2, "cache": {"entries": 2, "capacity_kib": 1}
        }))
        .unwrap();
        let a: Uri = "file:///a".parse().unwrap();
        let b: Uri = "file:///b".parse().unwrap();
        let mut cache = Cache::new(limits);
        cache.insert(
            a.clone(),
            1,
            preview(range(0, 1), &"a".repeat(2048)),
            false,
            false,
        );
        assert!(cache.latest(&a).is_some());
        cache.insert(b.clone(), 2, preview(range(0, 1), "b"), false, false);
        assert!(cache.latest(&a).is_none());
        limits.cache.capacity_kib = 16.try_into().unwrap();
        let mut cache = Cache::new(limits);
        cache.insert(a.clone(), 1, preview(range(0, 1), "a"), false, false);
        cache.insert(b.clone(), 2, preview(range(0, 1), "b"), false, false);
        cache.exact(&a, range(0, 1), false).unwrap();
        cache.insert(a.clone(), 3, preview(range(2, 3), "c"), false, false);
        assert!(cache.latest(&b).is_none());
        assert!(cache.exact(&a, range(0, 1), false).is_some());
    }
}
