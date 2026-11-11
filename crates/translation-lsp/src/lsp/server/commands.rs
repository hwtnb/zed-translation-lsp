//! The commands that Code Actions carry and `workspace/executeCommand` runs.

use super::{
    menu::{RETRY, card_message},
    *,
};

pub(super) enum ServerCommand {
    TranslateSelection(TranslationTarget),
    Retry(TranslationTarget),
    /// The translation of a range from its CodeLens, which shows it when it
    /// is ready.
    LensTranslate(TranslationTarget),
    /// A retry from the CodeLens of a range, which shows the translation when
    /// it is ready.
    LensRetry(TranslationTarget),
    /// Shows the translation or failure of a range in a card.
    Show(TranslationTarget),
    OpenTab(TranslationTarget),
    Cancel(u64),
    CancelAll,
    /// The account menu, or an account command of the provider.
    Account(String),
    /// The server menu.
    Menu,
    /// Turns the automatic features back to the settings (`true`) or off.
    Automatic(bool),
    /// Opens Zed's settings file.
    OpenSettings,
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
            LENS_TRANSLATE_COMMAND => Ok(Self::LensTranslate(target(arguments)?)),
            LENS_RETRY_COMMAND => Ok(Self::LensRetry(target(arguments)?)),
            SHOW_COMMAND => Ok(Self::Show(target(arguments)?)),
            DOCUMENT_COMMAND => Ok(Self::OpenTab(target(arguments)?)),
            CANCEL_COMMAND => Ok(Self::Cancel(parse(one(
                arguments,
                "Expected one translation ticket",
            )?)?)),
            CANCEL_ALL_COMMAND => {
                none(&arguments, "Cancel all takes no arguments")?;
                Ok(Self::CancelAll)
            }
            MENU_COMMAND | AUTOMATIC_OFF_COMMAND | AUTOMATIC_ON_COMMAND | OPEN_SETTINGS_COMMAND => {
                none(&arguments, "Menu commands take no arguments")?;
                Ok(match command.as_str() {
                    MENU_COMMAND => Self::Menu,
                    AUTOMATIC_OFF_COMMAND => Self::Automatic(false),
                    AUTOMATIC_ON_COMMAND => Self::Automatic(true),
                    _ => Self::OpenSettings,
                })
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
            Self::LensTranslate(target) => (LENS_TRANSLATE_COMMAND, Some(vec![to_value(target)?])),
            Self::LensRetry(target) => (LENS_RETRY_COMMAND, Some(vec![to_value(target)?])),
            Self::Show(target) => (SHOW_COMMAND, Some(vec![to_value(target)?])),
            Self::OpenTab(target) => (DOCUMENT_COMMAND, Some(vec![to_value(target)?])),
            Self::Cancel(ticket) => (CANCEL_COMMAND, Some(vec![to_value(ticket)?])),
            Self::CancelAll => (CANCEL_ALL_COMMAND, Some(Vec::new())),
            Self::Account(command) => (command.as_str(), None),
            Self::Menu => (MENU_COMMAND, None),
            Self::Automatic(true) => (AUTOMATIC_ON_COMMAND, None),
            Self::Automatic(false) => (AUTOMATIC_OFF_COMMAND, None),
            Self::OpenSettings => (OPEN_SETTINGS_COMMAND, None),
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
        if params.command == LENS_COMMAND {
            return Ok(Some(Value::Null));
        }
        let command = ServerCommand::parse::<P>(params)?;
        if let ServerCommand::TranslateSelection(target)
        | ServerCommand::Retry(target)
        | ServerCommand::LensTranslate(target)
        | ServerCommand::LensRetry(target)
        | ServerCommand::Show(target)
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
                self.cancel_translations(connection)?;
                Ok(Some(Value::Null))
            }
            ServerCommand::Cancel(ticket) => {
                self.cancel_range(connection, ticket, ErrorCode::RequestCanceled)?;
                Ok(Some(Value::Null))
            }
            ServerCommand::Account(command) => self.start_administration(connection, id, command),
            ServerCommand::Menu => self.start_administration(connection, id, MENU_COMMAND.into()),
            ServerCommand::Automatic(on) => {
                self.set_automatic(connection, on)?;
                Ok(Some(Value::Null))
            }
            ServerCommand::OpenSettings => {
                if !self.ui.capabilities.show_document {
                    return Err(failure(
                        ErrorCode::RequestFailed,
                        "The editor does not support opening files",
                    ));
                }
                self.start_administration(connection, id, OPEN_SETTINGS_COMMAND.into())
            }
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
                self.queue(id, target, PendingReply::Command { notice: true })
            }
            ServerCommand::LensTranslate(target) => {
                self.check_feature(Feature::SelectionAction, &target)?;
                self.queue(id, target, PendingReply::Command { notice: false })
            }
            ServerCommand::Retry(target) => {
                self.queue(id, target, PendingReply::Retry { notice: true })
            }
            ServerCommand::LensRetry(target) => {
                self.queue(id, target, PendingReply::Retry { notice: false })
            }
            ServerCommand::Show(target) => {
                let gone = || {
                    failure(
                        ErrorCode::ContentModified,
                        "The translation is no longer available",
                    )
                };
                if !self.target_is_current(&target) {
                    return Err(gone());
                }
                self.check_cache()?;
                let (kind, message) = match self
                    .cache
                    .message(&target.uri, target.range)
                    .ok_or_else(gone)?
                {
                    Ok(translation) => (MessageType::INFO, translation),
                    Err(reason) => (MessageType::ERROR, reason),
                };
                let card = self
                    .ui
                    .show_card(kind, &card_message(&message), &[RETRY])
                    .map_err(provider_failure)?;
                self.cards.insert(card, target);
                Ok(Some(Value::Null))
            }
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
