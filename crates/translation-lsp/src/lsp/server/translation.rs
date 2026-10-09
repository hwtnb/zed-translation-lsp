use super::*;
use crate::{config::limits::SizeSetting, providers::error::Notification as ErrorNotification};

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
        let output = request.output();
        let tab = output == Output::Document;
        let (limit, setting) = if tab {
            (
                self.settings.translation_limits.input_kib.document,
                SizeSetting::DocumentInput,
            )
        } else {
            (
                self.settings.translation_limits.input_kib.hover,
                SizeSetting::HoverInput,
            )
        };
        limit
            .check(original.len(), setting)
            .map_err(|m| failure(ErrorCode::InvalidParams, &m.to_string()))?;
        if request.reuses_completed()
            && let Some(preview) = self
                .cache
                .exact(&target.uri, target.range, tab)
                .or_else(|| {
                    // Each output keeps its own results when tabs have their own settings.
                    if request.explicit() && !self.settings.provider.separates_documents() {
                        self.cache.exact(&target.uri, target.range, !tab)
                    } else {
                        None
                    }
                })
        {
            self.next_ticket += 1;
            self.cache.insert(
                target.uri.clone(),
                self.next_ticket,
                preview.clone(),
                tab,
                false,
            );
            return match request {
                TranslationRequest::Reply(_, PendingReply::Hover(position)) => {
                    to_value(preview.hover(position)).map(Some)
                }
                TranslationRequest::Document => {
                    self.preview_files.open(
                        &self.ui,
                        &preview,
                        document.language_id == "markdown",
                    )?;
                    Ok(None)
                }
                TranslationRequest::Reply(_, PendingReply::Command | PendingReply::Retry) => {
                    self.ui
                        .inform(&format!("{}: {TRANSLATION_READY}", location(&target)))
                        .map_err(|_| failure(ErrorCode::InternalError, "Client disconnected"))?;
                    Ok(Some(Value::Null))
                }
                _ => Ok(None),
            };
        }
        let immediate = matches!(
            request,
            TranslationRequest::Reply(_, PendingReply::Command | PendingReply::Retry)
        );
        if let Some(ticket) = self.scheduler.ticket(&target, output, &self.settings) {
            let generation = if request.explicit() {
                Some(
                    self.provider
                        .admit(&context, crate::providers::Intent::Explicit)
                        .map_err(provider_failure)?,
                )
            } else {
                None
            };
            self.scheduler
                .join(ticket, request, &self.settings, Instant::now())?;
            if let Some(fresh) = generation
                && let Some(Task::Translation { generation, .. }) = self
                    .scheduler
                    .translations
                    .get_mut(&ticket)
                    .and_then(|work| work.task.as_mut())
            {
                *generation = fresh;
            }
            return Ok(immediate.then_some(Value::Null));
        }
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
            crate::providers::Intent::Explicit
        } else {
            crate::providers::Intent::Automatic
        };
        let generation = self
            .provider
            .admit(&context, intent)
            .map_err(provider_failure)?;
        let input = document
            .translation_input(&target.uri, target.range)
            .map_err(|m| failure(ErrorCode::InvalidParams, m))?;
        let task = Task::Translation {
            intent,
            output,
            text: input.text,
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
                task,
            },
            request,
            &self.settings,
            Instant::now(),
        )?;
        self.clear_problem();
        self.dispatch()?;
        Ok(immediate.then_some(Value::Null))
    }

    pub(super) fn finish(
        &mut self,
        connection: &Connection,
        completion: Finished,
    ) -> ServerResult<()> {
        self.scheduler.terminated(completion.ticket);
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
                .check(translation.text.len(), SizeSetting::Output)
                .map_err(|m| failure(ErrorCode::RequestFailed, &m.to_string()))?;
            Ok(Preview::new(
                translation.context,
                work.target.range,
                translation.text,
                work.is_comment,
            ))
        })();
        self.complete(connection, completion.ticket, &work, result, silent)?;
        self.dispatch().map_err(|e| e.message)?;
        Ok(())
    }

    pub(super) fn complete(
        &mut self,
        connection: &Connection,
        ticket: u64,
        work: &Translation,
        result: RequestResult<Preview>,
        silent: bool,
    ) -> ServerResult<()> {
        match &result {
            Ok(preview) => {
                let has_hover = self.scheduler.pending.values().any(|p| p.ticket == ticket);
                if work.range_output() || has_hover {
                    self.cache.insert(
                        work.target.uri.clone(),
                        ticket,
                        preview.clone(),
                        false,
                        false,
                    );
                }
                if work.manual {
                    self.ui
                        .inform(&format!("{}: {TRANSLATION_READY}", location(&work.target)))?;
                }
                if work.document {
                    self.cache.insert(
                        work.target.uri.clone(),
                        ticket,
                        preview.clone(),
                        true,
                        false,
                    );
                    let markdown = self.documents[&work.target.uri].language_id == "markdown";
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
                if work.range_output() {
                    let preview = Preview::new(
                        work.context.clone(),
                        work.target.range,
                        error.message.chars().take(2048).collect(),
                        false,
                    );
                    self.cache.insert(
                        work.target.uri.clone(),
                        ticket,
                        preview.clone(),
                        false,
                        true,
                    );
                }
                if (work.manual || work.document) && !silent {
                    self.command_failure(&failure(
                        ErrorCode::RequestFailed,
                        &format!("{}: {}", location(&work.target), error.message),
                    ))?;
                }
            }
            _ => (),
        }
        let ids: Vec<_> = self
            .scheduler
            .pending
            .iter()
            .filter(|(_, p)| p.ticket == ticket)
            .map(|(id, _)| id.clone())
            .collect();
        for id in ids {
            let pending = self.scheduler.pending.remove(&id).unwrap();
            let response = match &result {
                Err(error) => Err(error.clone()),
                Ok(preview) => to_value(preview.hover(pending.position)),
            };
            respond(connection, id, response)?;
        }
        Ok(())
    }
}

fn location(target: &TranslationTarget) -> String {
    let path = target.uri.as_str().split(['?', '#']).next().unwrap_or("");
    let filename = path.rsplit('/').next().unwrap_or(path);
    format!(
        "{}:{}-{}",
        filename,
        target.range.start.line + 1,
        target.range.end.line + 1
    )
}
