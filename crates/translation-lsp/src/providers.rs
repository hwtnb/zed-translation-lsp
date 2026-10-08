pub(crate) mod chatgpt;
pub(crate) mod error;
pub(crate) mod mock;

use std::future::Future;

use chatgpt::auth::AccountKey;
use error::Error;

pub(crate) trait Interaction {
    fn open(&self, uri: &str) -> impl Future<Output = Result<(), Error>>;
    fn choose(
        &self,
        message: &str,
        choices: &[String],
    ) -> impl Future<Output = Result<Option<String>, Error>>;
}

#[derive(Clone, Copy)]
pub(crate) struct ProviderCommand {
    pub command: &'static str,
    pub title: &'static str,
    pub changes_account: bool,
}

pub(crate) struct Capabilities {
    pub server_name: &'static str,
    pub automatic_selection: bool,
    pub account_menu: Option<&'static str>,
    pub commands: &'static [ProviderCommand],
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum AuthUiState {
    SignedOut,
    PlanDisabled,
    Ready,
    Unavailable,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Context {
    Mock,
    ChatGpt(AccountKey),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Intent {
    Automatic,
    Explicit,
}

pub(crate) struct Translated {
    pub text: String,
    pub context: Context,
}

pub(crate) trait TranslationProvider: Clone + Send + 'static {
    fn context(&self) -> Result<Context, Error> {
        Ok(Context::Mock)
    }
    fn admit(&self, _context: &Context, _intent: Intent) -> Result<u64, Error> {
        Ok(0)
    }
    fn check_admission(&self, _context: &Context, _generation: u64) -> Result<(), Error> {
        Ok(())
    }
    fn auth_ui_state(&self) -> AuthUiState {
        AuthUiState::Ready
    }
    fn capabilities() -> Capabilities {
        Capabilities {
            server_name: "Translation LSP",
            automatic_selection: false,
            account_menu: None,
            commands: &[],
        }
    }
    fn account_commands(&self) -> Vec<ProviderCommand> {
        Self::capabilities().commands.to_vec()
    }
    fn translate(
        &self,
        intent: Intent,
        text: &str,
        target_language: &str,
        context: Context,
    ) -> impl Future<Output = Result<Translated, Error>>;
    fn administer(
        &self,
        _command: &str,
        _ui: &impl Interaction,
    ) -> impl Future<Output = Result<String, Error>> {
        async {
            Err(Error::failed(
                "This provider does not support account commands",
            ))
        }
    }
}
