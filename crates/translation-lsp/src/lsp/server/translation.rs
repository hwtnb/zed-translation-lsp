use super::*;
use crate::{
    config::limits::SizeSetting,
    lsp::preview::{Form, markdown::Urls},
    providers::{Intent, Kind, Source, error::Notification as ErrorNotification},
};

impl<P: TranslationProvider> Server<P> {
    pub(super) fn dispatch(&mut self) -> RequestResult<()> {
        if let Some(jobs) = &self.jobs {
            self.scheduler
                .dispatch(jobs, &self.settings, Instant::now())?;
        }
        Ok(())
    }

    pub(super) fn queue(
        &mut self,
        id: RequestId,
        target: TranslationTarget,
        reply: PendingReply,
    ) -> RequestResult<Option<Value>> {
        self.queue_translation(target, TranslationRequest::Reply(id, reply))
    }

    pub(super) fn queue_translation(
        &mut self,
        target: TranslationTarget,
        request: TranslationRequest,
    ) -> RequestResult<Option<Value>> {
        let context = self.check_cache()?;
        self.check_input(&target, request.output())?;
        let running = self
            .scheduler
            .ticket(&target, request.output(), self.sharing());
        // Work in progress on the range, such as an explicit translation after
        // the text was found in the target language, answers it instead.
        if running.is_none()
            && !request.explicit()
            && self.cache.same_language(&target.uri, target.range)
        {
            return Ok(matches!(request, TranslationRequest::Reply(..)).then_some(Value::Null));
        }
        if request.reuses_completed()
            && let Some(preview) = self.completed(&target, &request)
        {
            return self.reuse_completed(target, request, preview);
        }
        let immediate = matches!(
            request,
            TranslationRequest::Reply(_, PendingReply::Command { .. } | PendingReply::Retry { .. })
        );
        match running {
            Some(ticket) => self.join_running(ticket, request, &context)?,
            None => self.admit_new(target, request, context)?,
        }
        Ok(immediate.then_some(Value::Null))
    }

    /// Checks that the target still belongs to the open document and that its
    /// text fits the input limit of the output.
    fn check_input(&mut self, target: &TranslationTarget, output: Output) -> RequestResult<()> {
        let document = self
            .documents
            .get_mut(&target.uri)
            .ok_or_else(|| failure(ErrorCode::ContentModified, "Document was closed"))?;
        if document.version != target.version || document.session != target.session {
            return Err(failure(
                ErrorCode::ContentModified,
                "Translation target belongs to an older document",
            ));
        }
        let original = document
            .selection(target.range)
            .map_err(|m| failure(ErrorCode::InvalidParams, m))?;
        let limits = &self.settings.translation_limits.input_kib;
        let (limit, setting) = match output {
            Output::Document => (limits.document, SizeSetting::DocumentInput),
            Output::Range => (limits.hover, SizeSetting::HoverInput),
        };
        limit
            .check(original.len(), setting)
            .map_err(|m| failure(ErrorCode::InvalidParams, &m.to_string()))
    }

    /// A completed translation of the target that the request can use.
    fn completed(
        &mut self,
        target: &TranslationTarget,
        request: &TranslationRequest,
    ) -> Option<Preview<P::Context>> {
        let output = request.output();
        self.cache
            .exact(&target.uri, target.range, output)
            .or_else(|| {
                // Each output keeps its own results when tabs have their own settings.
                if request.explicit() && !self.settings.provider.separates_documents() {
                    let other = match output {
                        Output::Document => Output::Range,
                        Output::Range => Output::Document,
                    };
                    self.cache.exact(&target.uri, target.range, other)
                } else {
                    None
                }
            })
    }

    fn reuse_completed(
        &mut self,
        target: TranslationTarget,
        request: TranslationRequest,
        preview: Preview<P::Context>,
    ) -> RequestResult<Option<Value>> {
        self.next_ticket += 1;
        self.cache.insert(
            target.uri.clone(),
            self.next_ticket,
            Stored::Translation(request.output(), preview.clone()),
        );
        match request {
            TranslationRequest::Reply(_, PendingReply::Hover(position, _)) => {
                to_value(preview.hover(position)).map(Some)
            }
            TranslationRequest::Document => {
                let markdown = self.documents[&target.uri].language == Some(Language::Markdown);
                self.preview_files.open(&self.ui, &preview, markdown)?;
                Ok(None)
            }
            TranslationRequest::Reply(
                _,
                PendingReply::Command { notice: true } | PendingReply::Retry { notice: true },
            ) => {
                self.ui
                    .inform(&format!("{}: {TRANSLATION_READY}", location(&target)))
                    .map_err(|_| failure(ErrorCode::InternalError, "Client disconnected"))?;
                Ok(Some(Value::Null))
            }
            TranslationRequest::Reply(
                _,
                PendingReply::Command { notice: false } | PendingReply::Retry { notice: false },
            ) => Ok(Some(Value::Null)),
            TranslationRequest::Selection => Ok(None),
        }
    }

    /// Adds the request to the translation already running for its target.
    fn join_running(
        &mut self,
        ticket: u64,
        request: TranslationRequest,
        context: &P::Context,
    ) -> RequestResult<()> {
        let generation = if request.explicit() {
            Some(
                self.provider
                    .admit(context, Intent::Explicit)
                    .map_err(provider_failure)?,
            )
        } else {
            None
        };
        self.scheduler
            .join(ticket, request, &self.settings, Instant::now())?;
        if let Some(fresh) = generation {
            self.scheduler.set_generation(ticket, fresh);
        }
        Ok(())
    }

    /// Accepts a new translation of the target and queues it.
    fn admit_new(
        &mut self,
        target: TranslationTarget,
        request: TranslationRequest,
        context: P::Context,
    ) -> RequestResult<()> {
        if !self
            .scheduler
            .available(&self.settings, !request.explicit())
        {
            return Err(failure(
                ErrorCode::RequestFailed,
                &format!(
                    "{}: Translation queue is full; this range was not accepted",
                    location(&target)
                ),
            ));
        }
        let intent = if request.explicit() {
            Intent::Explicit
        } else {
            Intent::Automatic
        };
        let generation = self
            .provider
            .admit(&context, intent)
            .map_err(provider_failure)?;
        let document = self
            .documents
            .get_mut(&target.uri)
            .expect("check_input found the document");
        let input = document.translation_input(
            &target.uri,
            target.range,
            Duration::from_millis(self.settings.comment_parse_timeout_ms),
        );
        report_comment_failure(&self.ui, &target.uri, document)?;
        let input = input.map_err(|m| failure(ErrorCode::InvalidParams, m))?;
        // The text sent, so that a comment's closing mark is not read as part
        // of a URL.
        let sent = match &input.source {
            Source::Passage(text) => text,
            Source::Term { term, .. } => term,
        };
        let urls = if document.language == Some(Language::Markdown) {
            Urls::of_markdown(sent)
        } else {
            Urls::of(sent)
        };
        let task = Task::Translation {
            intent,
            output: request.output(),
            source: input.source,
            target_language: target.target_language.clone(),
            context: context.clone(),
            generation,
        };
        self.next_ticket += 1;
        self.scheduler.insert(
            self.next_ticket,
            scheduler::PreparedTranslation {
                target,
                context,
                is_comment: input.is_comment,
                urls,
                task,
            },
            request,
            &self.settings,
            Instant::now(),
        )?;
        self.clear_problem();
        self.dispatch()
    }

    pub(super) fn finish(
        &mut self,
        connection: &Connection,
        completion: Finished<P::Context>,
    ) -> ServerResult<()> {
        self.scheduler.terminated(completion.ticket());
        self.expire(connection)?;
        match completion {
            Finished::Translation(completion) => self.finish_translation(connection, completion),
            Finished::Administration(completion) => {
                self.finish_administration(connection, completion)
            }
        }
    }

    fn finish_translation(
        &mut self,
        connection: &Connection,
        completion: Completion<Translated<P::Context>>,
    ) -> ServerResult<()> {
        let Some(work) = self.scheduler.remove(completion.ticket) else {
            return Ok(());
        };
        let silent = completion
            .result
            .as_ref()
            .err()
            .is_some_and(|e| e.notification() == ErrorNotification::Suppress);
        let result = (|| {
            if !self.target_is_current(&work.target) {
                return Err(failure(
                    ErrorCode::ContentModified,
                    "Document changed during translation",
                ));
            }
            let translation = completion.result.map_err(provider_failure)?;
            if self.provider.context().map_err(provider_failure)? != translation.context {
                return Err(failure(
                    ErrorCode::ContentModified,
                    "Translation authorization changed during translation",
                ));
            }
            if translation.kind == Kind::SameLanguage {
                return Ok(Completed::SameLanguage(translation.context));
            }
            self.settings
                .translation_limits
                .output_kib
                .check(translation.text.len(), SizeSetting::Output)
                .map_err(|m| failure(ErrorCode::RequestFailed, &m.to_string()))?;
            let mut preview = Preview::new(
                translation.context,
                work.target.range,
                translation.text,
                work.is_comment,
                P::capabilities().hover_note,
            );
            preview.form = if translation.kind == Kind::Entry {
                Form::Entry
            } else if self.documents[&work.target.uri].language == Some(Language::Markdown) {
                Form::Markdown(work.urls.clone())
            } else {
                Form::Text(work.urls.clone())
            };
            preview.hover = self.settings.hover;
            Ok(Completed::Preview(preview))
        })();
        match result {
            Ok(Completed::SameLanguage(context)) => {
                self.complete_same_language(connection, completion.ticket, &work, context)?;
            }
            Ok(Completed::Preview(preview)) => {
                self.complete(connection, completion.ticket, &work, Ok(preview), silent)?;
            }
            Err(error) => {
                self.complete(connection, completion.ticket, &work, Err(error), silent)?
            }
        }
        self.dispatch().map_err(|e| e.message)?;
        Ok(())
    }

    /// Ends a translation whose text the provider found already in the target
    /// language. Its Hover waiters get no Hover, and automatic requests reuse
    /// the result. A Code Action or a tab that joined after the automatic
    /// request was sent sends the text again as an explicit request, which
    /// gives the ready message or opens the tab when it completes.
    fn complete_same_language(
        &mut self,
        connection: &Connection,
        ticket: u64,
        work: &Translation<P::Context>,
        context: P::Context,
    ) -> ServerResult<()> {
        if work.owners.range_output() || self.scheduler.has_waiters(ticket) {
            self.cache.insert(
                work.target.uri.clone(),
                ticket,
                Stored::SameLanguage {
                    context,
                    range: work.target.range,
                },
            );
        }
        self.ui.refresh_lenses()?;
        for (id, _) in self.scheduler.take_waiters(ticket) {
            respond(connection, id, Ok(Value::Null))?;
        }
        // A tab first, so that a range request joins the same work: a range
        // request would not let a tab join once sent.
        let mut requests = Vec::new();
        if work.owners.tab() {
            requests.push(TranslationRequest::Document);
        }
        if work.owners.manual() {
            // A retry, so that a completed result, such as an older tab
            // translation, does not stand in for the text sent again.
            requests.push(TranslationRequest::Reply(
                RequestId::from(format!("translation-resend-{ticket}")),
                PendingReply::Retry {
                    notice: work.owners.notice(),
                },
            ));
        }
        for request in requests {
            if let Err(error) = self.queue_translation(work.target.clone(), request) {
                self.command_failure(&error)?;
            }
        }
        Ok(())
    }

    pub(super) fn complete(
        &mut self,
        connection: &Connection,
        ticket: u64,
        work: &Translation<P::Context>,
        result: RequestResult<Preview<P::Context>>,
        silent: bool,
    ) -> ServerResult<()> {
        match &result {
            Ok(preview) => {
                let has_hover = self.scheduler.has_waiters(ticket);
                if work.owners.range_output() || has_hover {
                    self.cache.insert(
                        work.target.uri.clone(),
                        ticket,
                        Stored::Translation(Output::Range, preview.clone()),
                    );
                }
                if work.owners.notice() {
                    self.ui
                        .inform(&format!("{}: {TRANSLATION_READY}", location(&work.target)))?;
                }
                if work.owners.tab() {
                    self.cache.insert(
                        work.target.uri.clone(),
                        ticket,
                        Stored::Translation(Output::Document, preview.clone()),
                    );
                    let markdown =
                        self.documents[&work.target.uri].language == Some(Language::Markdown);
                    if let Err(error) = self.preview_files.open(&self.ui, preview, markdown) {
                        self.command_failure(&failure(
                            ErrorCode::RequestFailed,
                            &format!("{}: {}", location(&work.target), error.message),
                        ))?;
                    }
                }
            }
            Err(error)
                if error.code != ErrorCode::ContentModified as i32
                    && error.code != ErrorCode::RequestCanceled as i32 =>
            {
                self.record_problem(work, Problem::Failed(error.message.clone()));
                if work.owners.range_output() {
                    self.cache.insert(
                        work.target.uri.clone(),
                        ticket,
                        Stored::failure(work.context.clone(), work.target.range, &error.message),
                    );
                }
                if (work.owners.manual() || work.owners.tab()) && !silent {
                    self.command_failure(&failure(
                        ErrorCode::RequestFailed,
                        &format!("{}: {}", location(&work.target), error.message),
                    ))?;
                }
            }
            _ => (),
        }
        self.ui.refresh_lenses()?;
        for (id, pending) in self.scheduler.take_waiters(ticket) {
            let response = match &result {
                Err(error) => Err(error.clone()),
                Ok(preview) => to_value(preview.hover(pending.position)),
            };
            respond(connection, id, response)?;
        }
        Ok(())
    }
}

/// What a completed translation gives the requests that own it.
enum Completed<C> {
    Preview(Preview<C>),
    /// The text is already in the target language, as found with the context.
    SameLanguage(C),
}

fn location(target: &TranslationTarget) -> String {
    format!(
        "{}:{}-{}",
        file_name(&target.uri),
        target.range.start.line + 1,
        target.range.end.line + 1
    )
}
