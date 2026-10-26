pub(crate) mod admission;
pub(crate) mod anthropic;
pub(crate) mod chatgpt;
pub(crate) mod credential_directory;
pub(crate) mod dictionary;
pub(crate) mod error;
pub(crate) mod mock;
pub(crate) mod prompt;
pub(crate) mod sse;
pub(crate) mod transport;
pub(crate) mod unset;

use std::{fmt::Debug, future::Future, path::Path};

use error::Error;

pub(crate) trait Interaction {
    /// Opens a page outside the editor, such as in the browser.
    fn open(&self, uri: &str) -> impl Future<Output = Result<(), Error>>;
    /// Opens a file in the editor.
    fn edit(&self, path: &Path) -> impl Future<Output = Result<(), Error>>;
    fn choose(
        &self,
        message: &str,
        choices: &[String],
    ) -> impl Future<Output = Result<Option<String>, Error>>;
}

/// When a command changes the account or key that translations are made with.
/// The change cancels the work in progress and discards the previews.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum AccountChange {
    Never,
    /// As soon as the command starts, so both are done before it runs.
    AtOnce,
    /// Only if the user agrees while the command runs, so both are done after
    /// it, and only when it has changed the context.
    IfConfirmed,
}

#[derive(Clone, Copy)]
pub(crate) struct ProviderCommand {
    pub command: &'static str,
    pub title: &'static str,
    pub changes_account: AccountChange,
}

pub(crate) struct Capabilities {
    pub server_name: &'static str,
    pub automatic_selection: bool,
    /// The title of the account menu. A provider without one has no account,
    /// and its authentication state is always `Ready`.
    pub account_menu: Option<&'static str>,
    pub commands: &'static [ProviderCommand],
    /// A line that the Hover header adds under the title.
    pub hover_note: Option<&'static str>,
    /// The Markdown that Hover over a comment shows in place of a translation,
    /// for a provider that translates nothing.
    pub comment_notice: Option<&'static str>,
    /// Whether a range request asks for less output than a tab request for
    /// the same text.
    pub range_output_bounded: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum AuthUiState {
    SignedOut,
    PlanDisabled,
    Ready,
    Unavailable,
}

impl AuthUiState {
    /// Whether the user has to connect an account or save a key before
    /// translating.
    pub fn needs_account(self) -> bool {
        matches!(self, Self::SignedOut | Self::PlanDisabled)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Intent {
    Automatic,
    Explicit,
}

/// What a request sends.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Source {
    /// Text to translate.
    Passage(String),
    /// A word, short phrase or identifier to look up, with the line it was
    /// selected from.
    Term { term: String, line: String },
}

/// What a translation is for: a range read in place, or a tab.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Output {
    Range,
    Document,
}

/// A provider's request options for each output. `provider.document` gives tab
/// translations their own; without it, both outputs send the same options.
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

    /// The output whose settings `output` is sent with.
    pub fn configured(&self, output: Output) -> Output {
        if self.document.is_some() {
            output
        } else {
            Output::Range
        }
    }
}

pub(crate) struct Translated<C> {
    pub text: String,
    pub context: C,
    /// The text is a dictionary entry in Markdown, not a translation.
    pub entry: bool,
}

pub(crate) trait TranslationProvider: Clone + Send + 'static {
    /// What a translation is made with, such as an account or a key. Results
    /// made with one context are not shown for another.
    type Context: Clone + Debug + PartialEq + Send + Sync + 'static;

    fn context(&self) -> Result<Self::Context, Error>;
    fn admit(&self, _context: &Self::Context, _intent: Intent) -> Result<u64, Error> {
        Ok(0)
    }
    fn check_admission(&self, _context: &Self::Context, _generation: u64) -> Result<(), Error> {
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
            hover_note: None,
            comment_notice: None,
            range_output_bounded: false,
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
        context: Self::Context,
    ) -> impl Future<Output = Result<Translated<Self::Context>, Error>>;
    /// Describes `term` as a dictionary entry for a reader of
    /// `target_language`. `line` is the line the term was selected from. A
    /// provider without entries translates the term.
    fn look_up(
        &self,
        intent: Intent,
        output: Output,
        term: &str,
        _line: &str,
        target_language: &str,
        context: Self::Context,
    ) -> impl Future<Output = Result<Translated<Self::Context>, Error>> {
        self.translate(intent, output, term, target_language, context)
    }
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
