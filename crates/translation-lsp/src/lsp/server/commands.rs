//! The commands that Code Actions carry and `workspace/executeCommand` runs.

use super::*;

pub(super) enum ServerCommand {
    TranslateSelection(TranslationTarget),
    Retry(TranslationTarget),
    OpenTab(TranslationTarget),
    Cancel(u64),
    CancelAll,
    /// The account menu, or an account command of the provider.
    Account(String),
}

impl ServerCommand {
    /// Decodes a command, checking its arguments.
    pub fn parse<P: TranslationProvider>(params: ExecuteCommandParams) -> RequestResult<Self> {
        let ExecuteCommandParams {
            command, arguments, ..
        } = params;
        let one = |arguments: Vec<Value>, message: &str| {
            <[Value; 1]>::try_from(arguments)
                .map(|[argument]| argument)
                .map_err(|_| failure(ErrorCode::InvalidParams, message))
        };
        let target = |arguments| parse(one(arguments, "Expected one selection argument")?);
        let none = |arguments: &[Value], message: &str| {
            if arguments.is_empty() {
                Ok(())
            } else {
                Err(failure(ErrorCode::InvalidParams, message))
            }
        };
        match command.as_str() {
            COMMAND => Ok(Self::TranslateSelection(target(arguments)?)),
            RETRY_COMMAND => Ok(Self::Retry(target(arguments)?)),
            DOCUMENT_COMMAND => Ok(Self::OpenTab(target(arguments)?)),
            CANCEL_COMMAND => Ok(Self::Cancel(parse(one(
                arguments,
                "Expected one translation ticket",
            )?)?)),
            CANCEL_ALL_COMMAND => {
                none(&arguments, "Cancel all takes no arguments")?;
                Ok(Self::CancelAll)
            }
            _ if P::capabilities().account_menu.is_some()
                && (command == SETTINGS_COMMAND
                    || P::capabilities()
                        .commands
                        .iter()
                        .any(|entry| entry.command == command)) =>
            {
                none(&arguments, "Account commands take no arguments")?;
                Ok(Self::Account(command))
            }
            _ => Err(failure(
                ErrorCode::InvalidParams,
                "Unknown translation command",
            )),
        }
    }

    /// The Code Action that carries this command.
    pub fn action(&self, title: String) -> RequestResult<CodeAction> {
        let (command, arguments) = match self {
            Self::TranslateSelection(target) => (COMMAND, Some(vec![to_value(target)?])),
            Self::Retry(target) => (RETRY_COMMAND, Some(vec![to_value(target)?])),
            Self::OpenTab(target) => (DOCUMENT_COMMAND, Some(vec![to_value(target)?])),
            Self::Cancel(ticket) => (CANCEL_COMMAND, Some(vec![to_value(ticket)?])),
            Self::CancelAll => (CANCEL_ALL_COMMAND, Some(Vec::new())),
            Self::Account(command) => (command.as_str(), None),
        };
        Ok(CodeAction {
            title: title.clone(),
            kind: Some(CodeActionKind::new(ACTION_KIND)),
            command: Some(Command {
                title,
                command: command.into(),
                arguments,
            }),
            ..Default::default()
        })
    }
}

impl<P: TranslationProvider> Server<P> {
    pub(super) fn execute_command(
        &mut self,
        connection: &Connection,
        id: RequestId,
        params: ExecuteCommandParams,
    ) -> RequestResult<Option<Value>> {
        let command = ServerCommand::parse::<P>(params)?;
        if let ServerCommand::TranslateSelection(target)
        | ServerCommand::Retry(target)
        | ServerCommand::OpenTab(target) = &command
            && target.target_language != self.settings.target_language
        {
            return Err(failure(
                ErrorCode::InvalidParams,
                "The target language must match initialization_options",
            ));
        }
        match command {
            ServerCommand::CancelAll => {
                self.cancel_activity(connection)?;
                Ok(Some(Value::Null))
            }
            ServerCommand::Cancel(ticket) => {
                self.cancel_range(connection, ticket, ErrorCode::RequestCanceled)?;
                Ok(Some(Value::Null))
            }
            ServerCommand::Account(command) => self.start_administration(connection, id, command),
            ServerCommand::OpenTab(target) => {
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
            }
            ServerCommand::TranslateSelection(target) => {
                self.check_feature(Feature::SelectionAction, &target)?;
                self.queue(id, target, PendingReply::Command)
            }
            ServerCommand::Retry(target) => self.queue(id, target, PendingReply::Retry),
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
