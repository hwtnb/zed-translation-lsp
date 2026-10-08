use super::*;

impl<P: TranslationProvider> Server<P> {
    pub(super) fn expire(&mut self, connection: &Connection) -> ServerResult<()> {
        self.preview_files.expire(&self.ui);
        let now = Instant::now();
        let ids: Vec<_> = self
            .administrations
            .iter()
            .filter(|(_, work)| work.deadline <= now)
            .map(|(id, _)| id.clone())
            .collect();
        for id in ids {
            let work = self.administrations.remove(&id).unwrap();
            let _ = work.stop.send(None);
            let error = failure(ErrorCode::RequestFailed, "Account operation timed out");
            self.command_failure(&error)?;
            respond(connection, id, Err(error))?;
        }
        let tickets: Vec<_> = self
            .scheduler
            .translations
            .iter()
            .filter(|(_, work)| work.detached_deadlines().any(|deadline| deadline <= now))
            .map(|(&ticket, _)| ticket)
            .collect();
        for ticket in tickets {
            let work = self.scheduler.translations.get_mut(&ticket).unwrap();
            if work
                .background_deadline
                .is_some_and(|deadline| deadline <= now)
            {
                work.background_deadline = None;
            }
            if work
                .document_deadline
                .is_some_and(|deadline| deadline <= now)
            {
                work.document_deadline = None;
                if !work.failure_notified {
                    work.failure_notified = true;
                    self.command_failure(&failure(
                        ErrorCode::RequestFailed,
                        "Translation timed out",
                    ))?;
                }
            }
            self.expire_owner(ticket);
        }
        let ids: Vec<_> = self
            .scheduler
            .pending
            .iter()
            .filter(|(_, pending)| pending.deadline <= now)
            .map(|(id, _)| id.clone())
            .collect();
        for id in ids {
            let pending = self.scheduler.pending.remove(&id).unwrap();
            let error = failure(ErrorCode::RequestFailed, "Translation timed out");
            if matches!(pending.reply, PendingReply::Command) {
                let work = self
                    .scheduler
                    .translations
                    .get_mut(&pending.ticket)
                    .unwrap();
                if !work.failure_notified {
                    work.failure_notified = true;
                    self.command_failure(&error)?;
                }
            }
            self.expire_owner(pending.ticket);
            respond(connection, id, Err(error))?;
        }
        if self.selection.as_ref().is_some_and(|selection| {
            self.can_start_selection(selection) && selection.start.is_some_and(|start| start <= now)
        }) {
            let selection = self.selection.as_mut().unwrap();
            selection.start = None;
            let target = selection.target.clone();
            let context = selection.context.clone();
            if context.is_some() && self.provider.context().ok() == context {
                let _ = self.queue_translation(target, TranslationRequest::Selection);
            }
        }
        Ok(())
    }

    pub(super) fn expire_owner(&mut self, ticket: u64) {
        if let Some(work) = self.scheduler.translations.get(&ticket) {
            let has_owner = work.detached_deadlines().next().is_some()
                || self
                    .scheduler
                    .pending
                    .values()
                    .any(|pending| pending.ticket == ticket);
            if !has_owner {
                let work = self.scheduler.translations.remove(&ticket).unwrap();
                work.deadline.send_replace(None);
                self.record_problem(&work, Problem::TimedOut);
                return;
            }
        }
        self.scheduler.update_deadline(ticket);
    }

    pub(super) fn cancel_activity(&mut self, connection: &Connection) -> RequestResult<()> {
        self.retire_selection();
        self.clear_problem();
        let ids: Vec<_> = self.scheduler.pending.keys().cloned().collect();
        for id in ids {
            self.cancel(connection, &id, ErrorCode::RequestCanceled)?;
        }
        for (_, work) in self.scheduler.translations.drain() {
            work.deadline.send_replace(None);
        }
        Ok(())
    }

    pub(super) fn invalidate(&mut self, connection: &Connection, uri: &Uri) -> RequestResult<()> {
        if self
            .problem_target
            .as_ref()
            .is_some_and(|(target, _)| &target.uri == uri)
        {
            self.clear_problem();
        }
        if self
            .selection
            .as_ref()
            .is_some_and(|selection| &selection.target.uri == uri)
        {
            self.retire_selection();
        }
        let ids: Vec<_> = self
            .scheduler
            .pending
            .iter()
            .filter(|(_, pending)| {
                self.scheduler
                    .translations
                    .get(&pending.ticket)
                    .is_some_and(|work| &work.target.uri == uri)
            })
            .map(|(id, _)| id.clone())
            .collect();
        for id in ids {
            self.cancel(connection, &id, ErrorCode::ContentModified)?;
        }
        self.scheduler.translations.retain(|_, work| {
            if &work.target.uri == uri {
                work.deadline.send_replace(None);
                false
            } else {
                true
            }
        });
        Ok(())
    }

    pub(super) fn cancel_all(&mut self, connection: &Connection) -> ServerResult<()> {
        self.preview_files.cancel_all(&self.ui);
        self.clear_problem();
        self.retire_selection();
        let ids: Vec<_> = self
            .scheduler
            .pending
            .keys()
            .chain(self.administrations.keys())
            .cloned()
            .collect();
        for id in ids {
            self.cancel(connection, &id, ErrorCode::RequestCanceled)
                .map_err(|error| error.message)?;
        }
        for (_, work) in self.scheduler.translations.drain() {
            work.deadline.send_replace(None);
        }
        Ok(())
    }

    pub(super) fn cancel(
        &mut self,
        connection: &Connection,
        id: &RequestId,
        code: ErrorCode,
    ) -> RequestResult<()> {
        if let Some(work) = self.administrations.remove(id) {
            let _ = work.stop.send(None);
            connection
                .sender
                .send(
                    Response {
                        id: id.clone(),
                        response_result: Err(failure(code, "Account operation cancelled")),
                    }
                    .into(),
                )
                .map_err(|_| failure(ErrorCode::InternalError, "Client disconnected"))?;
        }
        if let Some(pending) = self.scheduler.pending.remove(id) {
            self.scheduler.update_deadline(pending.ticket);
            let response = Response {
                id: id.clone(),
                response_result: Err(failure(
                    code,
                    "Translation preview canceled or document changed",
                )),
            };
            connection
                .sender
                .send(response.into())
                .map_err(|_| failure(ErrorCode::InternalError, "Client disconnected"))?;
        }
        Ok(())
    }
}
