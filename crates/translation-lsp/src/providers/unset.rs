//! The provider in use while the settings choose none. It translates nothing
//! and shows how to choose a provider.

use std::path::PathBuf;

use super::{
    AccountChange, AuthUiState, Capabilities, Interaction, ProviderCommand, Translated,
    TranslationProvider, error::Error,
};

macro_rules! menu {
    () => {
        "Set up Translation LSP…"
    };
}
const MENU: &str = menu!();

const OPEN_SETTINGS: ProviderCommand = ProviderCommand {
    command: "translation-lsp.open-settings",
    title: "Open Zed settings file",
    changes_account: AccountChange::Never,
};
const OPEN_GUIDE: ProviderCommand = ProviderCommand {
    command: "translation-lsp.open-setup-guide",
    title: "Open setup guide",
    changes_account: AccountChange::Never,
};
const COMMANDS: [ProviderCommand; 2] = [OPEN_SETTINGS, OPEN_GUIDE];

/// The message of the prompt shown when the server starts.
const PROMPT: &str =
    "Translation LSP has no provider and translates nothing. Choose one in Zed's settings.";
const NEXT: &str = "Add lsp.translation-lsp.initialization_options.provider to Zed's settings, \
    save them, then run editor: restart language server.";
const NOT_SET_UP: &str = concat!(
    "Translation LSP has no provider. Run \"",
    menu!(),
    "\" from the code actions to choose one"
);
const NO_SETTINGS_FILE: &str =
    "Could not open Zed's settings file. Run zed: open settings file from the Command Palette";
/// The setup section of the README of this version.
const GUIDE: &str = concat!(
    env!("CARGO_PKG_REPOSITORY"),
    "/blob/v",
    env!("CARGO_PKG_VERSION"),
    "/README.md#set-up-a-provider"
);

/// The context of the provider that translates nothing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct UnsetContext;

#[derive(Clone, Default)]
pub(crate) struct UnsetProvider;

impl UnsetProvider {
    /// Asks the user to choose a provider, and runs the command they pick.
    /// Gives the message to show after it, or none if they picked nothing.
    pub async fn prompt(&self, ui: &impl Interaction) -> Result<Option<String>, Error> {
        let titles = COMMANDS.map(|command| command.title.to_owned());
        let Some(command) = ui
            .choose(PROMPT, &titles)
            .await?
            .and_then(|title| COMMANDS.iter().find(|command| command.title == title))
        else {
            return Ok(None);
        };
        self.administer(command.command, ui).await.map(Some)
    }
}

impl TranslationProvider for UnsetProvider {
    type Context = UnsetContext;

    fn context(&self) -> Result<UnsetContext, Error> {
        Err(Error::configuration(NOT_SET_UP))
    }

    fn auth_ui_state(&self) -> AuthUiState {
        AuthUiState::SignedOut
    }

    fn capabilities() -> Capabilities {
        Capabilities {
            server_name: "Translation LSP",
            automatic_selection: false,
            account_menu: Some(MENU),
            commands: &COMMANDS,
            hover_note: None,
            comment_notice: Some(concat!(
                "> **Translation LSP is not set up**\n\nTo choose a provider, run ",
                menu!(),
                " from Code Actions."
            )),
            range_output_bounded: false,
        }
    }

    async fn translate(
        &self,
        _intent: super::Intent,
        _output: super::Output,
        _text: &str,
        _target_language: &str,
        _context: UnsetContext,
    ) -> Result<Translated<UnsetContext>, Error> {
        Err(Error::configuration(NOT_SET_UP))
    }

    async fn administer(&self, command: &str, ui: &impl Interaction) -> Result<String, Error> {
        if command == OPEN_SETTINGS.command {
            let path = settings_file().ok_or(Error::failed(NO_SETTINGS_FILE))?;
            ui.edit(&path)
                .await
                .map_err(|_| Error::failed(NO_SETTINGS_FILE))?;
        } else {
            ui.open(GUIDE).await?;
        }
        Ok(NEXT.into())
    }
}

/// Where Zed keeps the user's settings, as Zed itself locates them.
fn settings_file() -> Option<PathBuf> {
    #[cfg(windows)]
    let directory = dirs::config_dir()?.join("Zed");
    #[cfg(any(target_os = "linux", target_os = "freebsd"))]
    let directory = std::env::var_os("FLATPAK_XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(dirs::config_dir)?
        .join("zed");
    #[cfg(not(any(windows, target_os = "linux", target_os = "freebsd")))]
    let directory = dirs::home_dir()?.join(".config").join("zed");
    Some(directory.join("settings.json"))
}
