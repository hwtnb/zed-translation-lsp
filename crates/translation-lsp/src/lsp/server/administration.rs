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
                work.retained()
                    && work.target.uri == params.text_document.uri
                    && overlaps(work.target.range, params.range)
            })
            .collect();
        ranges.sort_by_key(|(ticket, _)| *ticket);
        for (&ticket, work) in ranges {
            add(
                format!(
                    "Cancel translation of lines {}-{}",
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

    pub(super) fn changes_account(command: &str) -> bool {
        P::capabilities()
            .commands
            .iter()
            .any(|entry| entry.command == command && entry.changes_account)
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
            if Self::changes_account(&params.command) {
                self.cancel_all(connection).map_err(|_| {
                    failure(ErrorCode::InternalError, "Could not cancel pending work")
                })?;
                self.clear_previews();
            }
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
                },
            );
            return Ok(None);
        }
        if params.command != COMMAND && params.command != DOCUMENT_COMMAND {
            return Err(failure(ErrorCode::InvalidParams, "Unknown preview command"));
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
                    "Document previews support Markdown and Plain Text",
                ));
            }
            self.queue_translation(target, TranslationRequest::Document)?;
            Ok(Some(Value::Null))
        } else {
            self.queue(id, target, PendingReply::Command)
        }
    }
}
