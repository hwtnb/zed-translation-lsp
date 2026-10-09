use super::*;

pub(super) struct Selection {
    pub target: TranslationTarget,
    pub context: Option<Context>,
    pub start: Option<Instant>,
}

impl<P: TranslationProvider> Server<P> {
    pub(super) fn observe_selection(&mut self, params: &CodeActionParams, state: AuthUiState) {
        if self
            .previous_selection
            .as_ref()
            .is_some_and(|target| target.uri != params.text_document.uri)
        {
            self.previous_selection = None;
            self.adjusting_ticket = None;
        }
        let target = self
            .documents
            .get_mut(&params.text_document.uri)
            .and_then(|document| {
                if !self
                    .settings
                    .features
                    .enables(Feature::AutomaticSelection, &document.language_id)
                    || !P::capabilities().automatic_selection
                    || state != AuthUiState::Ready
                    || !document.language_id.eq_ignore_ascii_case("markdown")
                {
                    return None;
                }
                let source = document.selection(params.range).ok()?;
                if source.len() > self.settings.translation_limits.input_kib.hover.bytes()
                    || !document
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
            // Empty/ineligible selections end the candidate, not an accepted job or adjustment chain.
            self.selection = None;
            return;
        };
        let context = self.provider.context().ok();
        if self
            .selection
            .as_ref()
            .is_some_and(|s| s.target == target && (context.is_none() || s.context == context))
        {
            return;
        }
        if !self.previous_selection.as_ref().is_some_and(|previous| {
            previous.uri == target.uri && overlaps(previous.range, target.range)
        }) {
            self.adjusting_ticket = None;
        }
        self.previous_selection = Some(target.clone());
        self.selection = Some(Selection {
            target,
            start: context.as_ref().map(|_| {
                Instant::now() + Duration::from_millis(self.settings.selection_debounce_ms)
            }),
            context,
        });
    }

    // Automatic selection only ever joins a range operation.
    pub(super) fn range_ticket(&self, target: &TranslationTarget) -> Option<u64> {
        self.scheduler.ticket(target, Output::Range, &self.settings)
    }

    pub(super) fn can_start_selection(&self, selection: &Selection) -> bool {
        if !self.administrations.is_empty() {
            return false;
        }
        if self.range_ticket(&selection.target).is_some() {
            return true;
        }
        let replacing = self
            .adjusting_ticket
            .and_then(|t| self.scheduler.translations.get(&t))
            .is_some_and(|w| w.automatic && !w.comment_hover && !w.manual && !w.document);
        self.scheduler.admitted() < self.settings.translation_limits.work.admitted
            && (replacing
                || self.scheduler.automatic_count()
                    < self.settings.translation_limits.work.automatic)
    }

    pub(super) fn start_selection(&mut self) {
        if !self.selection.as_ref().is_some_and(|s| {
            self.can_start_selection(s) && s.start.is_some_and(|start| start <= Instant::now())
        }) {
            return;
        }
        let selection = self.selection.as_mut().unwrap();
        selection.start = None;
        let target = selection.target.clone();
        let Some(context) = selection.context.clone() else {
            return;
        };
        if self.provider.context().ok().as_ref() != Some(&context)
            || self
                .provider
                .admit(&context, crate::providers::Intent::Automatic)
                .is_err()
        {
            return;
        }
        if let Some(previous) = self.adjusting_ticket.take()
            && self.range_ticket(&target) != Some(previous)
        {
            self.scheduler.retire_automatic(previous);
        }
        if self
            .queue_translation(target.clone(), TranslationRequest::Selection)
            .is_ok()
        {
            self.adjusting_ticket = self.range_ticket(&target);
        }
    }

    pub(super) fn retire_selection(&mut self) {
        self.selection = None;
        self.previous_selection = None;
        self.adjusting_ticket = None;
    }

    pub(super) fn suppress_selection(&mut self) {
        if let Some(selection) = &mut self.selection {
            selection.start = None;
        }
        self.adjusting_ticket = None;
    }
}
