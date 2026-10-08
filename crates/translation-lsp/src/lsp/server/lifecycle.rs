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
            .filter(|(_, work)| {
                work.execution_deadline()
                    .is_some_and(|deadline| deadline <= now)
            })
            .map(|(&ticket, _)| ticket)
            .collect();
        for ticket in tickets {
            let work = self.scheduler.remove(ticket).unwrap();
            self.complete(
                connection,
                ticket,
                &work,
                Err(failure(ErrorCode::RequestFailed, "Translation timed out")),
                false,
            )?;
            self.record_problem(&work, Problem::TimedOut);
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
            self.scheduler.update_deadline(pending.ticket);
            respond(connection, id, Err(error))?;
        }
        self.start_selection();
        self.dispatch().map_err(|e| e.message)?;
        Ok(())
    }

    pub(super) fn cancel_range(
        &mut self,
        connection: &Connection,
        ticket: u64,
        code: ErrorCode,
    ) -> RequestResult<()> {
        if let Some(work) = self.scheduler.remove(ticket) {
            if let Some(selection) = &mut self.selection
                && selection.target.uri == work.target.uri
                && overlaps(selection.target.range, work.target.range)
            {
                selection.start = None;
            }
            let ids: Vec<_> = self
                .scheduler
                .pending
                .iter()
                .filter(|(_, p)| p.ticket == ticket)
                .map(|(id, _)| id.clone())
                .collect();
            for id in ids {
                self.cancel(connection, &id, code)?;
            }
        }
        Ok(())
    }

    pub(super) fn cancel_activity(&mut self, connection: &Connection) -> RequestResult<()> {
        self.suppress_selection();
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
        self.cache.remove_document(uri);
        if self
            .previous_selection
            .as_ref()
            .is_some_and(|target| &target.uri == uri)
        {
            self.previous_selection = None;
        }
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
