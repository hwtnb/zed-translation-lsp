use super::*;

pub(super) struct Selection {
    pub(super) target: TranslationTarget,
    pub(super) context: Option<Context>,
    pub(super) start: Option<Instant>,
}

impl<P: TranslationProvider> Server<P> {
    pub(super) fn observe_selection(&mut self, params: &CodeActionParams, state: AuthUiState) {
        let target = self
            .documents
            .get_mut(&params.text_document.uri)
            .and_then(|document| {
                if !self.settings.markdown_auto_translate
                    || !P::capabilities().automatic_selection
                    || state != AuthUiState::Ready
                    || !document.language_id.eq_ignore_ascii_case("markdown")
                {
                    return None;
                }
                let source = document.selection(params.range).ok()?;
                if source.len() > self.settings.translation_limits.input_kib.hover.bytes() {
                    return None;
                }
                if !document
                    .markdown_selection_is_translatable(params.range)
                    .ok()?
                {
                    return None;
                }
                Some(TranslationTarget {
                    uri: params.text_document.uri.clone(),
                    version: document.version,
                    session: document.session,
                    range: params.range,
                    target_language: self.settings.target_language.clone(),
                })
            });
        let Some(target) = target else {
            self.retire_selection();
            return;
        };
        let context = self.provider.context().ok();
        if self.selection.as_ref().is_some_and(|selection| {
            selection.target == target && (context.is_none() || selection.context == context)
        }) {
            return;
        }
        self.retire_selection();
        let start = context
            .is_some()
            .then(|| Instant::now() + Duration::from_millis(self.settings.selection_debounce_ms));
        self.selection = Some(Selection {
            target,
            context,
            start,
        });
    }

    pub(super) fn can_start_selection(&self, selection: &Selection) -> bool {
        self.administrations.is_empty()
            && self.scheduler.owner_count() < MAX_PENDING
            && (self.scheduler.ticket(&selection.target).is_some()
                || (self.scheduler.translations.len() < MAX_JOBS
                    && !self
                        .scheduler
                        .translations
                        .values()
                        .any(|work| work.target.uri == selection.target.uri)))
    }

    pub(super) fn retire_selection(&mut self) {
        if self.selection.is_some() {
            self.clear_problem();
        }
        let Some(selection) = self.selection.take() else {
            return;
        };
        let ticket = self.scheduler.ticket(&selection.target);
        if let Some(ticket) = ticket {
            self.scheduler
                .translations
                .get_mut(&ticket)
                .unwrap()
                .background_deadline = None;
            self.scheduler.update_deadline(ticket);
        }
    }
}
