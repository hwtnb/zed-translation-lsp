use super::*;

impl<P: TranslationProvider> Server<P> {
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
        let timeout = match &request {
            TranslationRequest::Reply(_, PendingReply::Hover(_)) => self.settings.hover_timeout_ms,
            TranslationRequest::Document => self.settings.document_timeout_ms,
            _ => self.settings.selection_timeout_ms,
        };
        let deadline = Instant::now() + Duration::from_millis(timeout);
        let context = self.check_cache()?;
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
            .map_err(|message| failure(ErrorCode::InvalidParams, message))?;
        let (input_limit, setting) = match request {
            TranslationRequest::Document => (
                self.settings.translation_limits.input_kib.document,
                "translation_limits.input_kib.document",
            ),
            _ => (
                self.settings.translation_limits.input_kib.hover,
                "translation_limits.input_kib.hover",
            ),
        };
        input_limit
            .check(original.len(), setting)
            .map_err(|message| failure(ErrorCode::InvalidParams, &message))?;
        if let Some(preview) = document
            .preview
            .as_ref()
            .filter(|preview| preview.range == target.range)
        {
            return match request {
                TranslationRequest::Reply(_, PendingReply::Hover(position)) => {
                    to_value(preview.hover(position)).map(Some)
                }
                TranslationRequest::Document => {
                    self.preview_files.open(
                        &self.ui,
                        &preview.translation,
                        document.language_id == "markdown",
                    )?;
                    Ok(None)
                }
                _ => Ok(Some(Value::Null)),
            };
        }
        if self.scheduler.owner_count() >= MAX_PENDING {
            return Err(failure(
                ErrorCode::RequestFailed,
                "Too many pending translation requests",
            ));
        }
        if let Some(ticket) = self.scheduler.ticket(&target) {
            self.scheduler.join(ticket, request, deadline)?;
            return Ok(None);
        }
        if self.scheduler.translations.len() >= MAX_JOBS
            || self
                .scheduler
                .translations
                .values()
                .any(|work| work.target.uri == target.uri)
        {
            return Err(failure(
                ErrorCode::RequestFailed,
                "Translation is busy; retry after completion",
            ));
        }
        let intent = if matches!(
            request,
            TranslationRequest::Document | TranslationRequest::Reply(_, PendingReply::Command)
        ) {
            crate::providers::Intent::Explicit
        } else {
            crate::providers::Intent::Automatic
        };
        self.provider
            .admit(&context, intent)
            .map_err(provider_failure)?;
        let input = document
            .translation_input(&target.uri, target.range)
            .map_err(|message| failure(ErrorCode::InvalidParams, message))?;
        let (deadline_sender, deadline_receiver) = watch::channel(Some(deadline));
        self.next_ticket += 1;
        let ticket = self.next_ticket;
        let job = Job {
            ticket,
            deadline: deadline_receiver,
            task: Task::Translation {
                intent,
                text: input.text,
                target_language: target.target_language.clone(),
                context: context.clone(),
            },
        };
        self.jobs.as_ref().unwrap().try_send(job).map_err(|_| {
            failure(
                ErrorCode::RequestFailed,
                "Translation queue is full; retry later",
            )
        })?;
        document.preview = None;
        self.activity.clear_problem();
        self.problem_target = None;
        self.scheduler.translations.insert(
            ticket,
            Translation {
                context,
                phase: Phase::Queued,
                target,
                is_comment: input.is_comment,
                deadline: deadline_sender,
                failure_notified: false,
                background_deadline: matches!(request, TranslationRequest::Selection)
                    .then_some(deadline),
                document_deadline: matches!(request, TranslationRequest::Document)
                    .then_some(deadline),
            },
        );
        if let TranslationRequest::Reply(id, reply) = request {
            self.scheduler.pending.insert(
                id,
                Pending {
                    ticket,
                    deadline,
                    reply,
                },
            );
        }
        Ok(None)
    }

    pub(super) fn finish(
        &mut self,
        connection: &Connection,
        completion: Finished,
    ) -> ServerResult<()> {
        self.expire(connection)?;
        if let Some(id) = self
            .administrations
            .iter()
            .find(|(_, work)| work.ticket == completion.ticket)
            .map(|(id, _)| id.clone())
        {
            let operation = self.administrations.remove(&id).unwrap();
            if Self::changes_account(&operation.command) {
                self.clear_previews();
            }
            if operation.command == SETTINGS_COMMAND
                && operation.deadline > Instant::now()
                && let Ok(Outcome::AccountChoice(choice)) = &completion.result
            {
                let result = match choice {
                    None => Ok(Some(Value::Null)),
                    Some(command) => self.execute_command(
                        connection,
                        id.clone(),
                        ExecuteCommandParams {
                            command: command.clone(),
                            arguments: Vec::new(),
                            work_done_progress_params: Default::default(),
                        },
                    ),
                };
                if let Some(work) = self.administrations.get_mut(&id) {
                    work.deadline = operation.deadline;
                    work.stop.send_replace(Some(operation.deadline));
                }
                return match result {
                    Ok(None) => Ok(()),
                    Ok(Some(value)) => respond(connection, id, Ok(value)),
                    Err(error) => {
                        self.command_failure(&error)?;
                        respond(connection, id, Err(error))
                    }
                };
            }
            let result = match completion.result {
                _ if operation.deadline <= Instant::now() => Err(failure(
                    ErrorCode::RequestFailed,
                    "Account operation timed out",
                )),
                Ok(Outcome::Administration(message)) => {
                    self.ui.inform(&message)?;
                    Ok(Value::Null)
                }
                Err(error) => Err(provider_failure(error)),
                _ => Err(failure(
                    ErrorCode::InternalError,
                    "Invalid account completion",
                )),
            };
            if let Err(error) = &result {
                self.command_failure(error)?;
            }
            return respond(connection, id, result);
        }
        let Some(work) = self.scheduler.translations.remove(&completion.ticket) else {
            return Ok(());
        };
        let target = &work.target;
        let result = (|| {
            let document = self
                .documents
                .get_mut(&target.uri)
                .ok_or_else(|| failure(ErrorCode::ContentModified, "Document was closed"))?;
            if document.version != target.version
                || document.session != target.session
                || document.text().is_none()
            {
                return Err(failure(
                    ErrorCode::ContentModified,
                    "Document changed during translation",
                ));
            }
            let translation = match completion.result.map_err(provider_failure)? {
                Outcome::Translation(translation) => translation,
                _ => {
                    return Err(failure(
                        ErrorCode::InternalError,
                        "Invalid translation completion",
                    ));
                }
            };
            if self.provider.context().map_err(provider_failure)? != translation.context {
                return Err(failure(
                    ErrorCode::ContentModified,
                    "Translation authorization changed during translation",
                ));
            }
            self.settings
                .translation_limits
                .output_kib
                .check(translation.text.len(), "translation_limits.output_kib")
                .map_err(|message| failure(ErrorCode::RequestFailed, &message))?;
            document.preview = Some(Preview {
                range: target.range,
                translation: translation.text,
                context: translation.context,
                is_comment: work.is_comment,
            });
            Ok(())
        })();
        if let Err(error) = &result
            && error.code != ErrorCode::ContentModified as i32
            && error.code != ErrorCode::RequestCanceled as i32
        {
            self.record_problem(&work, Problem::Failed(error.message.clone()));
        } else if self
            .problem_target
            .as_ref()
            .is_some_and(|(target, _)| target == &work.target)
        {
            self.clear_problem();
        }
        self.complete(connection, completion.ticket, &work, result)
    }

    pub(super) fn complete(
        &mut self,
        connection: &Connection,
        ticket: u64,
        work: &Translation,
        result: RequestResult<()>,
    ) -> ServerResult<()> {
        match &result {
            Ok(())
                if self.scheduler.pending.values().any(|pending| {
                    pending.ticket == ticket && matches!(pending.reply, PendingReply::Command)
                }) =>
            {
                self.ui.inform(TRANSLATION_READY)?
            }
            Err(error)
                if !work.failure_notified
                    && (work.document_deadline.is_some()
                        || self.scheduler.pending.values().any(|pending| {
                            pending.ticket == ticket
                                && !matches!(pending.reply, PendingReply::Hover(_))
                        })) =>
            {
                self.command_failure(error)?
            }
            _ => (),
        }
        if result.is_ok() && work.document_deadline.is_some() {
            let document = &self.documents[&work.target.uri];
            if let Err(error) = self.preview_files.open(
                &self.ui,
                &document.preview.as_ref().unwrap().translation,
                document.language_id == "markdown",
            ) {
                self.command_failure(&error)?;
            }
        }
        let ids: Vec<_> = self
            .scheduler
            .pending
            .iter()
            .filter(|(_, pending)| pending.ticket == ticket)
            .map(|(id, _)| id.clone())
            .collect();
        for id in ids {
            let pending = self.scheduler.pending.remove(&id).unwrap();
            let response = match &result {
                Err(error) => Err(error.clone()),
                Ok(()) => match pending.reply {
                    PendingReply::Command => Ok(Value::Null),
                    PendingReply::Hover(position) => to_value(
                        self.documents
                            .get(&work.target.uri)
                            .and_then(|document| document.preview.as_ref())
                            .and_then(|preview| preview.hover(position)),
                    ),
                },
            };
            respond(connection, id, response)?;
        }
        Ok(())
    }
}
