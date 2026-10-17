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
        for (ticket, work) in self.scheduler.take_expired(now) {
            self.complete(
                connection,
                ticket,
                &work,
                Err(failure(ErrorCode::RequestFailed, "Translation timed out")),
                false,
            )?;
            self.record_problem(&work, Problem::TimedOut);
        }
        for id in self.scheduler.take_expired_waiters(now) {
            let error = failure(ErrorCode::RequestFailed, "Translation timed out");
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
            for (id, _) in self.scheduler.take_waiters(ticket) {
                refuse_translation(connection, id, code)?;
            }
        }
        Ok(())
    }

    /// Cancels every translation and its Hover waiters, for `Cancel all
    /// translations`. Account operations and tabs being opened go on, and the
    /// current selection waits for its next change.
    pub(super) fn cancel_translations(&mut self, connection: &Connection) -> RequestResult<()> {
        self.suppress_selection();
        self.clear_problem();
        for id in self.scheduler.stop_all() {
            refuse_translation(connection, id, ErrorCode::RequestCanceled)?;
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
        for id in self.scheduler.stop_document(uri) {
            refuse_translation(connection, id, ErrorCode::ContentModified)?;
        }
        Ok(())
    }

    /// Cancels all work, for shutdown and account changes: translations,
    /// account operations, tabs being opened, and the current selection.
    pub(super) fn cancel_all_work(&mut self, connection: &Connection) -> ServerResult<()> {
        self.preview_files.cancel_all(&self.ui);
        self.clear_problem();
        self.retire_selection();
        for id in self.scheduler.stop_all() {
            refuse_translation(connection, id, ErrorCode::RequestCanceled)
                .map_err(|error| error.message)?;
        }
        let ids: Vec<_> = self.administrations.keys().cloned().collect();
        for id in ids {
            self.cancel(connection, &id, ErrorCode::RequestCanceled)
                .map_err(|error| error.message)?;
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
        if self.scheduler.remove_waiter(id) {
            refuse_translation(connection, id.clone(), code)?;
        }
        Ok(())
    }
}

/// Answers a Hover waiter whose translation will not come.
fn refuse_translation(
    connection: &Connection,
    id: RequestId,
    code: ErrorCode,
) -> RequestResult<()> {
    let response = Response {
        id,
        response_result: Err(failure(code, "Translation canceled or document changed")),
    };
    connection
        .sender
        .send(response.into())
        .map_err(|_| failure(ErrorCode::InternalError, "Client disconnected"))
}
