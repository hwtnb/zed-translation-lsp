pub(crate) mod admission;
pub(crate) mod anthropic;
pub(crate) mod chatgpt;
pub(crate) mod error;
pub(crate) mod mock;
pub(crate) mod prompt;
pub(crate) mod sse;
pub(crate) mod transport;

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
    Anthropic,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Intent {
    Automatic,
    Explicit,
}

// What a translation is for: a range read in place, or a tab.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Output {
    Range,
    Document,
}

// A provider's request options for each output. `provider.document` gives tab
// translations their own; without it, both outputs send the same options.
#[derive(Clone)]
pub(crate) struct OutputOptions<T> {
    range: T,
    document: Option<T>,
}

impl<T> OutputOptions<T> {
    pub fn new(range: T, document: Option<T>) -> Self {
        Self { range, document }
    }

    pub fn get(&self, output: Output) -> &T {
        match (output, &self.document) {
            (Output::Document, Some(document)) => document,
            _ => &self.range,
        }
    }

    // The output whose settings `output` is sent with.
    pub fn configured(&self, output: Output) -> Output {
        if self.document.is_some() {
            output
        } else {
            Output::Range
        }
    }
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
        output: Output,
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
