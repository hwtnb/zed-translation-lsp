use super::*;

const ADMIN_TIMEOUT: Duration = Duration::from_secs(180);

impl<P: TranslationProvider> Server<P> {
    pub(super) fn cancel_actions(
        &self,
        params: &CodeActionParams,
    ) -> RequestResult<Vec<CodeAction>> {
        let mut actions = Vec::new();
        let mut ranges: Vec<_> = self
            .scheduler
            .translations()
            .iter()
            .filter(|(_, work)| {
                work.owners.retained()
                    && work.target.uri == params.text_document.uri
                    && overlaps(work.target.range, params.range)
            })
            .collect();
        ranges.sort_by_key(|(ticket, _)| *ticket);
        // A range can hold a tab operation next to a range operation only when
        // they do not always share, so only then does the title tell them apart.
        let separate = self.sharing() != Sharing::Both;
        for (&ticket, work) in ranges {
            actions.push(ServerCommand::Cancel(ticket).action(format!(
                "Cancel {} of lines {}-{}",
                if separate && work.owners.tab() {
                    "tab translation"
                } else {
                    "translation"
                },
                work.target.range.start.line + 1,
                work.target.range.end.line + 1
            ))?);
        }
        if !self.scheduler.translations().is_empty() {
            actions.push(ServerCommand::CancelAll.action("Cancel all translations".into())?);
        }
        Ok(actions)
    }

    pub(super) fn account_change(command: &str) -> AccountChange {
        P::capabilities()
            .commands
            .iter()
            .find(|entry| entry.command == command)
            .map_or(AccountChange::Never, |entry| entry.changes_account)
    }

    pub(super) fn finish_administration(
        &mut self,
        connection: &Connection,
        completion: Completion<Administered>,
    ) -> ServerResult<()> {
        let Some(id) = self
            .administrations
            .iter()
            .find(|(_, work)| work.ticket == completion.ticket)
            .map(|(id, _)| id.clone())
        else {
            return Ok(());
        };
        let operation = self.administrations.remove(&id).unwrap();
        match Self::account_change(&operation.command) {
            AccountChange::AtOnce => self.clear_previews(),
            // The user agreed, and the command changed what translations
            // are made with.
            AccountChange::IfConfirmed if self.provider.context().ok() != operation.context => {
                self.cancel_all_work(connection)?;
                self.clear_previews();
            }
            _ => (),
        }
        let result = match completion.result {
            _ if operation.deadline <= Instant::now() => Err(failure(
                ErrorCode::RequestFailed,
                "Account operation timed out",
            )),
            Ok(Administered::AccountChoice(choice)) => {
                let result = match choice {
                    None => Ok(Some(Value::Null)),
                    Some(command) => self.execute_command(
                        connection,
                        id.clone(),
                        ExecuteCommandParams {
                            command,
                            arguments: Vec::new(),
                            work_done_progress_params: Default::default(),
                        },
                    ),
                };
                // The chosen command answers the menu's request within its deadline.
                if let Some(work) = self.administrations.get_mut(&id) {
                    work.deadline = operation.deadline;
                    work.stop.send_replace(Some(operation.deadline));
                }
                match result {
                    Ok(None) => return Ok(()),
                    Ok(Some(value)) => Ok(value),
                    Err(error) => Err(error),
                }
            }
            Ok(Administered::Message(message)) => {
                self.ui.inform(&message)?;
                Ok(Value::Null)
            }
            Err(error) => Err(provider_failure(error)),
        };
        if let Err(error) = &result {
            self.command_failure(error)?;
        }
        respond(connection, id, result)
    }

    /// Starts the account menu or an account command of the provider. Its
    /// request is answered when the operation finishes.
    pub(super) fn start_administration(
        &mut self,
        connection: &Connection,
        id: RequestId,
        command: String,
    ) -> RequestResult<Option<Value>> {
        if !self.administrations.is_empty() {
            return Err(failure(
                ErrorCode::RequestFailed,
                "An account operation is in progress",
            ));
        }
        let change = Self::account_change(&command);
        if change == AccountChange::AtOnce {
            self.cancel_all_work(connection)
                .map_err(|_| failure(ErrorCode::InternalError, "Could not cancel pending work"))?;
            self.clear_previews();
        }
        let context = (change == AccountChange::IfConfirmed)
            .then(|| self.provider.context().ok())
            .flatten();
        self.next_ticket += 1;
        let ticket = self.next_ticket;
        let deadline = Instant::now() + ADMIN_TIMEOUT;
        let (stop, receiver) = watch::channel(Some(deadline));
        self.admin_jobs
            .as_ref()
            .unwrap()
            .try_send(Job {
                ticket,
                deadline: receiver,
                task: if command == SETTINGS_COMMAND {
                    Task::AccountMenu {
                        ui: self.ui.clone(),
                    }
                } else {
                    Task::Administration {
                        command: command.clone(),
                        ui: self.ui.clone(),
                    }
                },
            })
            .map_err(|_| {
                failure(
                    ErrorCode::RequestFailed,
                    "Translation worker is busy; retry the account command",
                )
            })?;
        self.administrations.insert(
            id,
            Administration {
                command,
                ticket,
                stop,
                deadline,
                context,
            },
        );
        Ok(None)
    }
}
