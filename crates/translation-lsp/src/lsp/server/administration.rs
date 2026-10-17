use super::*;

const ADMIN_TIMEOUT: Duration = Duration::from_secs(180);

impl<P: TranslationProvider> Server<P> {
    pub(super) fn cancel_actions(
        &self,
        params: &CodeActionParams,
    ) -> RequestResult<Vec<CodeAction>> {
        let mut actions = Vec::new();
        let mut add = |title: String, command: &str, arguments: Vec<Value>| {
            actions.push(CodeAction {
                title: title.clone(),
                kind: Some(CodeActionKind::new(ACTION_KIND)),
                command: Some(Command {
                    title,
                    command: command.into(),
                    arguments: Some(arguments),
                }),
                ..Default::default()
            });
        };
        let mut ranges: Vec<_> = self
            .scheduler
            .translations
            .iter()
            .filter(|(_, work)| {
                work.owners.retained()
                    && work.target.uri == params.text_document.uri
                    && overlaps(work.target.range, params.range)
            })
            .collect();
        ranges.sort_by_key(|(ticket, _)| *ticket);
        // A range can hold a tab operation next to a range operation only when
        // they are kept apart, so only then does the title tell them apart.
        let separate = self.settings.provider.separates_documents();
        for (&ticket, work) in ranges {
            add(
                format!(
                    "Cancel {} of lines {}-{}",
                    if separate && work.owners.tab() {
                        "tab translation"
                    } else {
                        "translation"
                    },
                    work.target.range.start.line + 1,
                    work.target.range.end.line + 1
                ),
                CANCEL_COMMAND,
                vec![to_value(ticket)?],
            );
        }
        if !self.scheduler.translations.is_empty() {
            add(
                "Cancel all translations".into(),
                CANCEL_ALL_COMMAND,
                Vec::new(),
            );
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
                self.cancel_all(connection)?;
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

    pub(super) fn execute_command(
        &mut self,
        connection: &Connection,
        id: RequestId,
        params: ExecuteCommandParams,
    ) -> RequestResult<Option<Value>> {
        if params.command == CANCEL_ALL_COMMAND {
            if !params.arguments.is_empty() {
                return Err(failure(
                    ErrorCode::InvalidParams,
                    "Cancel all takes no arguments",
                ));
            }
            self.cancel_activity(connection)?;
            return Ok(Some(Value::Null));
        }
        if params.command == CANCEL_COMMAND {
            if params.arguments.len() != 1 {
                return Err(failure(
                    ErrorCode::InvalidParams,
                    "Expected one translation ticket",
                ));
            }
            let ticket: u64 = parse(params.arguments.into_iter().next().unwrap())?;
            self.cancel_range(connection, ticket, ErrorCode::RequestCanceled)?;
            return Ok(Some(Value::Null));
        }
        if P::capabilities().account_menu.is_some()
            && (params.command == SETTINGS_COMMAND
                || P::capabilities()
                    .commands
                    .iter()
                    .any(|command| command.command == params.command))
        {
            if !params.arguments.is_empty() {
                return Err(failure(
                    ErrorCode::InvalidParams,
                    "Account commands take no arguments",
                ));
            }
            if !self.administrations.is_empty() {
                return Err(failure(
                    ErrorCode::RequestFailed,
                    "An account operation is in progress",
                ));
            }
            let change = Self::account_change(&params.command);
            if change == AccountChange::AtOnce {
                self.cancel_all(connection).map_err(|_| {
                    failure(ErrorCode::InternalError, "Could not cancel pending work")
                })?;
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
                    task: if params.command == SETTINGS_COMMAND {
                        Task::AccountMenu {
                            ui: self.ui.clone(),
                        }
                    } else {
                        Task::Administration {
                            command: params.command.clone(),
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
                    command: params.command,
                    ticket,
                    stop,
                    deadline,
                    context,
                },
            );
            return Ok(None);
        }
        if !matches!(
            params.command.as_str(),
            COMMAND | RETRY_COMMAND | DOCUMENT_COMMAND
        ) {
            return Err(failure(
                ErrorCode::InvalidParams,
                "Unknown translation command",
            ));
        }
        if params.arguments.len() != 1 {
            return Err(failure(
                ErrorCode::InvalidParams,
                "Expected one selection argument",
            ));
        }
        let target: TranslationTarget = parse(params.arguments.into_iter().next().unwrap())?;
        if target.target_language != self.settings.target_language {
            return Err(failure(
                ErrorCode::InvalidParams,
                "The target language must match initialization_options",
            ));
        }
        if params.command == DOCUMENT_COMMAND {
            if !self.ui.capabilities.show_document {
                return Err(failure(
                    ErrorCode::RequestFailed,
                    "The editor does not support opening translation documents",
                ));
            }
            if !self
                .documents
                .get(&target.uri)
                .is_some_and(Document::supports_document_preview)
            {
                return Err(failure(
                    ErrorCode::InvalidParams,
                    "Tab translations support Markdown and Plain Text",
                ));
            }
            self.check_feature(Feature::DocumentAction, &target)?;
            self.queue_translation(target, TranslationRequest::Document)?;
            Ok(Some(Value::Null))
        } else {
            let reply = if params.command == RETRY_COMMAND {
                PendingReply::Retry
            } else {
                self.check_feature(Feature::SelectionAction, &target)?;
                PendingReply::Command
            };
            self.queue(id, target, reply)
        }
    }

    /// The actions of a feature turned off are not listed, so its command can
    /// only come from a client that did not take it from the list.
    fn check_feature(&self, feature: Feature, target: &TranslationTarget) -> RequestResult<()> {
        if self
            .documents
            .get(&target.uri)
            .is_some_and(|document| !self.settings.features.enables(feature, document.language))
        {
            return Err(failure(
                ErrorCode::InvalidParams,
                &format!("The features setting turns off {feature} for this language"),
            ));
        }
        Ok(())
    }
}
