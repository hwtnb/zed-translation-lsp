pub(crate) mod chatgpt;
pub(crate) mod error;
pub(crate) mod mock;

use std::future::Future;

use crate::lsp::ui::Ui;
use chatgpt::auth::AccountKey;
use error::Error;

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
pub(crate) enum TextKind {
    Comment,
    Selection,
}

pub(crate) struct Translated {
    pub text: String,
    pub context: Context,
}

pub(crate) trait TranslationProvider: Clone + Send + 'static {
    fn context(&self) -> Result<Context, Error> {
        Ok(Context::Mock)
    }
    fn admit(&self, _context: &Context, _explicit: bool) -> Result<(), Error> {
        Ok(())
    }
    fn auth_ui_state(&self) -> AuthUiState {
        AuthUiState::Unavailable
    }
    fn is_chatgpt(&self) -> bool {
        false
    }
    fn translate(
        &self,
        kind: TextKind,
        text: &str,
        target_language: &str,
        context: Context,
    ) -> impl Future<Output = Result<Translated, Error>>;
    fn administer(&self, _command: &str, _ui: &Ui) -> impl Future<Output = Result<String, Error>> {
        async {
            Err(Error::failed(
                "ChatGPT commands require provider.name=chatgpt",
            ))
        }
    }
}
