//! A service that `provider.base_url` names and that takes the Chat
//! Completions requests of the OpenAI API, such as OpenRouter, Gemini, Ollama,
//! or LM Studio. A key is sent only when one is saved for its origin.

#[cfg(test)]
use std::path::Path;
use std::{mem, path::PathBuf};

use reqwest::{Url, header::AUTHORIZATION};
use serde_json::json;

#[cfg(test)]
use super::api_key::test_support::Terminal as SyntheticTerminal;
use super::{
    AccountChange, AuthUiState, Capabilities, Intent, Interaction, Output, OutputOptions,
    ProviderCommand, Translated, TranslationProvider,
    admission::{Admission, Attempt},
    api_key::{KeyFile, KeyId, KeySpec, Origin, Terminal},
    dictionary,
    error::{Error, Service, credentials::CredentialError},
    prompt, range_output_tokens, transport,
};
#[cfg(test)]
use crate::config::limits::TranslationLimits;
use crate::config::{CompatibleModel, limits::SizeLimit};

const SERVICE: Service = Service::Compatible;

macro_rules! menu {
    () => {
        "OpenAI-compatible API key…"
    };
}
const MENU: &str = menu!();

/// What to do when the service refuses a request (HTTP 401), which went to
/// `origin` with a saved key or without one.
fn rejected(origin: &Origin, keyed: bool) -> String {
    if keyed {
        format!(
            "The service did not accept the key saved for {}. Run \"{MENU}\" from the code actions to replace it",
            origin.as_str()
        )
    } else {
        format!(
            "No key is saved for {}, where provider.base_url sends requests. Run \"{MENU}\" from the code actions to save one",
            origin.as_str()
        )
    }
}

mod response;

const SAVE: &str = "translation-lsp.save-compatible-key";
const REMOVE: &str = "translation-lsp.remove-compatible-key";
const COMMANDS: [ProviderCommand; 2] = [
    // Saving happens later, in the terminal. The server follows the file.
    ProviderCommand {
        command: SAVE,
        title: "Save OpenAI-compatible API key",
        changes_account: AccountChange::Never,
    },
    ProviderCommand {
        command: REMOVE,
        title: "Remove OpenAI-compatible API key",
        changes_account: AccountChange::IfConfirmed,
    },
];
const REMOVAL: &str = "Remove the API key saved for this service from this computer? The key stays valid until you delete it at the service.";

fn key_commands(saved: bool) -> Vec<ProviderCommand> {
    let [mut save, remove] = COMMANDS;
    if saved {
        save.title = "Replace OpenAI-compatible API key";
        vec![save, remove]
    } else {
        vec![save]
    }
}

#[derive(Clone)]
pub(crate) struct CompatibleProvider {
    client: reqwest::Client,
    key: KeyFile,
    terminal: Terminal,
    options: OutputOptions<CompatibleModel>,
    source_language: String,
    output_limit: SizeLimit,
    /// Where requests go: `provider.base_url` with `/chat/completions`.
    endpoint: String,
    admission: Admission<Option<KeyId>>,
}

impl CompatibleProvider {
    /// `base_url` has passed the settings checks. `key_directory` is the
    /// credential directory, where the key file is.
    pub fn new(
        base_url: &str,
        options: OutputOptions<CompatibleModel>,
        source_language: String,
        output_limit: SizeLimit,
        key_directory: Result<PathBuf, CredentialError>,
    ) -> Result<Self, Error> {
        let origin = Url::parse(base_url)
            .ok()
            .as_ref()
            .and_then(Origin::of)
            .ok_or_else(|| Error::configuration("provider.base_url cannot be used"))?;
        Ok(Self {
            // The worker owns the total deadline.
            client: transport::client_builder().build().map_err(|_| {
                Error::failed("Could not prepare the OpenAI-compatible HTTP client")
            })?,
            key: KeyFile::new(
                KeySpec::origin("OpenAI-compatible API key", MENU, origin),
                key_directory,
            ),
            terminal: Terminal::System,
            options,
            source_language,
            output_limit,
            endpoint: format!("{}/chat/completions", base_url.trim_end_matches('/')),
            admission: Admission::default(),
        })
    }

    #[cfg(test)]
    pub fn synthetic(base_url: &str, directory: &Path) -> Self {
        Self {
            terminal: Terminal::Synthetic(SyntheticTerminal::default()),
            ..Self::new(
                base_url,
                OutputOptions::new(
                    CompatibleModel {
                        model: "synthetic/model".into(),
                        effort: None,
                    },
                    None,
                ),
                "auto".into(),
                TranslationLimits::default().output_kib,
                Ok(directory.into()),
            )
            .unwrap()
        }
    }

    #[cfg(test)]
    pub fn with_options(mut self, options: OutputOptions<CompatibleModel>) -> Self {
        self.options = options;
        self
    }

    /// Sends `message` after the `instructions` and returns the reply. `texts`
    /// are the document's texts in the message, the first sizing the reply;
    /// errors never show them.
    async fn send(
        &self,
        intent: Intent,
        output: Output,
        instructions: String,
        message: String,
        texts: &[&str],
        expected: Option<KeyId>,
    ) -> Result<String, Error> {
        let key = self.key.optional_in_use(expected)?;
        let generation = self.admission.admit(&expected, intent)?;
        let secrets: Vec<&str> = key
            .as_ref()
            .map(|key| &*key.text)
            .into_iter()
            .chain(texts.iter().copied())
            .collect();
        let rejected = rejected(
            self.key
                .destination()
                .expect("an OpenAI-compatible key belongs to an origin"),
            key.is_some(),
        );
        let options = self.options.get(output);
        let mut request = json!({
            "model": options.model,
            "stream": true,
            "messages": [
                {"role": "system", "content": instructions},
                {"role": "user", "content": message},
            ],
        });
        // A tab is bounded by `translation_limits.output_kib` alone, and a
        // limit above a model's own may be refused.
        if output == Output::Range {
            request["max_tokens"] =
                json!(range_output_tokens(texts[0]).min(self.output_limit.bytes()));
        }
        if let Some(effort) = &options.effort {
            request["reasoning_effort"] = json!(effort);
        }
        let mut sent = false;
        // A request that was not sent again is no result of its key: it neither
        // pauses automatic translation nor resumes it.
        let mut unsent = None;
        // Dropping this future on cancellation or expiry drops the request or
        // response it is awaiting, which closes the connection.
        let mut result = self
            .admission
            .retrying(&expected, intent, async || {
                // The key may have been saved, replaced, or removed during the wait.
                if mem::replace(&mut sent, true)
                    && let Err(error) = self.key.optional_in_use(expected)
                {
                    unsent = Some(error.clone());
                    return Attempt::without_response(error);
                }
                let mut sending = self.client.post(&self.endpoint).json(&request);
                if let Some(key) = &key {
                    sending = sending.header(AUTHORIZATION, key.bearer());
                }
                let response = match sending.send().await {
                    Ok(response) => response,
                    Err(error) => {
                        return Attempt::without_response(transport::network_error(SERVICE)(error));
                    }
                };
                let retry_after = transport::retry_delay(response.headers());
                let result = if response.status().is_success() {
                    response::text(response, &secrets, &rejected, self.output_limit).await
                } else {
                    Err(response::failure_of(response, &secrets, &rejected).await)
                };
                Attempt {
                    result,
                    retry_after,
                }
            })
            .await;
        if let Some(error) = unsent {
            return Err(error);
        }
        self.admission
            .settle(&expected, intent, generation, &mut result);
        result
    }
}

impl TranslationProvider for CompatibleProvider {
    /// The key in use, or none: what was translated or refused with one key
    /// is not kept for another, or for none.
    type Context = Option<KeyId>;

    fn context(&self) -> Result<Option<KeyId>, Error> {
        Ok(self.key.read_optional()?.map(|(_, key)| key))
    }

    fn admit(&self, key: &Option<KeyId>, intent: Intent) -> Result<u64, Error> {
        self.admission.admit(key, intent)
    }

    fn check_admission(&self, key: &Option<KeyId>, generation: u64) -> Result<(), Error> {
        self.admission.check(key, generation)
    }

    /// A service may take requests without a key, so only a key file that
    /// cannot be read keeps translations from starting.
    fn auth_ui_state(&self) -> AuthUiState {
        if self.key.read_optional().is_ok() {
            AuthUiState::Ready
        } else {
            AuthUiState::SignedOut
        }
    }

    fn capabilities() -> Capabilities {
        Capabilities {
            server_name: "Translation LSP (OpenAI-compatible)",
            automatic_selection: true,
            account_menu: Some(MENU),
            commands: &COMMANDS,
            hover_note: None,
            comment_notice: None,
            range_output_bounded: true,
        }
    }

    fn account_commands(&self) -> Vec<ProviderCommand> {
        key_commands(self.key.read_optional().is_ok_and(|key| key.is_some()))
    }

    async fn translate(
        &self,
        intent: Intent,
        output: Output,
        text: &str,
        target_language: &str,
        expected: Option<KeyId>,
    ) -> Result<Translated<Option<KeyId>>, Error> {
        let instructions = prompt::instructions(target_language, &self.source_language);
        let reply = self
            .send(
                intent,
                output,
                instructions,
                prompt::message(text),
                &[text],
                expected,
            )
            .await?;
        Ok(Translated {
            text: prompt::translation(reply),
            context: expected,
            entry: false,
        })
    }

    async fn look_up(
        &self,
        intent: Intent,
        output: Output,
        term: &str,
        line: &str,
        target_language: &str,
        expected: Option<KeyId>,
    ) -> Result<Translated<Option<KeyId>>, Error> {
        let instructions = dictionary::instructions(target_language, &self.source_language);
        let message = dictionary::message(term, line);
        let reply = self
            .send(
                intent,
                output,
                instructions,
                message,
                &[line, term],
                expected,
            )
            .await?;
        Ok(Translated {
            text: dictionary::entry(term, &reply, SERVICE)?,
            context: expected,
            entry: true,
        })
    }

    async fn administer(&self, command: &str, ui: &impl Interaction) -> Result<String, Error> {
        match command {
            SAVE => self.key.ask_in_terminal(&self.terminal).await,
            REMOVE => self.key.remove(ui, REMOVAL).await,
            _ => Err(Error::failed("Unknown OpenAI-compatible command")),
        }
    }
}

#[cfg(test)]
mod tests;
