mod response;
#[cfg(test)]
pub(crate) mod test_support;

#[cfg(test)]
use std::path::Path;
use std::{mem, path::PathBuf};

use serde_json::json;

#[cfg(test)]
use super::api_key::test_support::Terminal as SyntheticTerminal;

#[cfg(test)]
use crate::config::{AnthropicEffort, limits::TranslationLimits};
use crate::config::{AnthropicModel, anthropic_thinking, limits::SizeLimit};

use super::{
    AccountChange, AuthUiState, Capabilities, Intent, Interaction, Output, OutputOptions,
    ProviderCommand, Translated, TranslationProvider,
    admission::{Admission, Attempt},
    api_key::{KeyFile, KeyId, KeySpec, Terminal},
    dictionary,
    error::{Error, Service, credentials::CredentialError},
    prompt, range_output_tokens, transport,
};

const API: &str = "https://api.anthropic.com/v1";
const VERSION: &str = "2023-06-01";
/// `speed` is a beta field, so it is sent with this header whatever its value.
const SPEED_BETA: &str = "fast-mode-2026-02-01";
const MAX_OUTPUT_TOKENS: usize = 64_000;
const SERVICE: Service = Service::Anthropic;

macro_rules! menu {
    () => {
        "Anthropic API key…"
    };
}
const MENU: &str = menu!();
const SEVERAL_WORKSPACES: &str = concat!(
    "The API key works in more than one workspace, and the server does not send a workspace ",
    "ID. Create a key for a single workspace in the Claude Console, then run \"",
    menu!(),
    "\" from the code actions to replace the key"
);
const REJECTED: &str = concat!(
    "The API key was rejected. Run \"",
    menu!(),
    "\" from the code actions to replace it"
);

const SAVE: &str = "translation-lsp.save-anthropic-key";
const REMOVE: &str = "translation-lsp.remove-anthropic-key";
const COMMANDS: [ProviderCommand; 2] = [
    // Saving happens later, in the terminal. The server follows the file.
    ProviderCommand {
        command: SAVE,
        title: "Save Anthropic API key",
        changes_account: AccountChange::Never,
    },
    ProviderCommand {
        command: REMOVE,
        title: "Remove Anthropic API key",
        changes_account: AccountChange::IfConfirmed,
    },
];
const REMOVAL: &str = "Remove the saved Anthropic API key from this computer? The key stays valid until you delete it in the Claude Console.";

/// The `max_tokens` a request asks for. A range translation is also bounded by
/// the size of its source, so that text in a comment cannot make the model write
/// at length when the pointer only passes over it.
fn max_tokens(output: Output, source: &str, output_limit: SizeLimit) -> usize {
    let limit = MAX_OUTPUT_TOKENS.min(output_limit.bytes());
    match output {
        Output::Range => limit.min(range_output_tokens(source)),
        Output::Document => limit,
    }
}

fn key_commands(usable: bool) -> Vec<ProviderCommand> {
    let [mut save, remove] = COMMANDS;
    if usable {
        save.title = "Replace Anthropic API key";
        vec![save, remove]
    } else {
        vec![save]
    }
}

#[derive(Clone)]
pub(crate) struct AnthropicProvider {
    client: reqwest::Client,
    key: KeyFile,
    terminal: Terminal,
    options: OutputOptions<AnthropicModel>,
    source_language: String,
    output_limit: SizeLimit,
    api: String,
    admission: Admission<KeyId>,
}

impl AnthropicProvider {
    /// `key_directory` is the credential directory, where the key file is.
    pub fn new(
        options: OutputOptions<AnthropicModel>,
        source_language: String,
        output_limit: SizeLimit,
        key_directory: Result<PathBuf, CredentialError>,
    ) -> Result<Self, Error> {
        Ok(Self {
            // The worker owns the total deadline.
            client: transport::client_builder()
                .build()
                .map_err(|_| Error::failed("Could not prepare the Anthropic HTTP client"))?,
            key: KeyFile::new(KeySpec::anthropic(MENU), key_directory),
            terminal: Terminal::System,
            options,
            source_language,
            output_limit,
            api: API.into(),
            admission: Admission::default(),
        })
    }

    #[cfg(test)]
    pub fn synthetic(api: String, directory: &Path) -> Self {
        Self {
            api,
            terminal: Terminal::Synthetic(test_support::Terminal::default()),
            ..Self::new(
                OutputOptions::new(
                    AnthropicModel {
                        model: "claude-haiku-5-5".into(),
                        effort: AnthropicEffort::Low,
                        thinking: None,
                        speed: None,
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
    pub fn with_terminal(mut self, terminal: SyntheticTerminal) -> Self {
        self.terminal = Terminal::Synthetic(terminal);
        self
    }

    #[cfg(test)]
    pub fn with_options(mut self, options: OutputOptions<AnthropicModel>) -> Self {
        self.options = options;
        self
    }

    /// Sends `message` with the `system` prompt and returns the reply. `texts`
    /// are the document's texts in the message, the first sizing the reply;
    /// errors never show them.
    async fn send(
        &self,
        intent: Intent,
        output: Output,
        system: String,
        message: String,
        texts: &[&str],
        expected: KeyId,
    ) -> Result<String, Error> {
        let key = self.key.in_use(expected)?;
        let generation = self.admission.admit(&expected, intent)?;
        let secrets: Vec<&str> = [&*key.text]
            .into_iter()
            .chain(texts.iter().copied())
            .collect();
        let options = self.options.get(output);
        let configured = self.options.configured(output);
        let mut request = json!({
            "model": options.model,
            "max_tokens": max_tokens(output, texts[0], self.output_limit),
            "stream": true,
            "system": system,
            "messages": [{"role": "user", "content": message}],
            "output_config": {"effort": options.effort},
            "thinking": {"type": if anthropic_thinking(&options.model, options.effort, options.thinking) {
                "adaptive"
            } else {
                "disabled"
            }},
        });
        if let Some(speed) = options.speed {
            request["speed"] = json!(speed);
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
                // The key may have been replaced or removed during the wait.
                if mem::replace(&mut sent, true)
                    && let Err(error) = self.key.in_use(expected)
                {
                    unsent = Some(error.clone());
                    return Attempt::without_response(error);
                }
                let mut sending = self
                    .client
                    .post(format!("{}/messages", self.api))
                    .header("x-api-key", key.header.clone())
                    .header("anthropic-version", VERSION);
                if options.speed.is_some() {
                    sending = sending.header("anthropic-beta", SPEED_BETA);
                }
                let response = sending.json(&request).send().await;
                let response = match response {
                    Ok(response) => response,
                    Err(error) => {
                        return Attempt::without_response(transport::network_error(SERVICE)(error));
                    }
                };
                let retry_after = transport::retry_delay(response.headers());
                let result = if response.status().is_success() {
                    response::text(response, &secrets, self.output_limit, configured).await
                } else {
                    Err(response::failure(response, &secrets, configured).await)
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
        let completed = result?;
        if let Some(speed) = options.speed {
            response::log_speed(speed, &completed);
        }
        Ok(completed.text)
    }
}

impl TranslationProvider for AnthropicProvider {
    /// The key in use: what was translated or refused with one key is not kept
    /// for another.
    type Context = KeyId;

    fn context(&self) -> Result<KeyId, Error> {
        let (_, key) = self.key.read()?;
        Ok(key)
    }

    fn admit(&self, key: &KeyId, intent: Intent) -> Result<u64, Error> {
        self.admission.admit(key, intent)
    }

    fn check_admission(&self, key: &KeyId, generation: u64) -> Result<(), Error> {
        self.admission.check(key, generation)
    }

    fn auth_ui_state(&self) -> AuthUiState {
        if self.key.read().is_ok() {
            AuthUiState::Ready
        } else {
            AuthUiState::SignedOut
        }
    }

    fn capabilities() -> Capabilities {
        Capabilities {
            server_name: "Translation LSP (Anthropic)",
            automatic_selection: true,
            account_menu: Some(MENU),
            commands: &COMMANDS,
            hover_note: None,
            comment_notice: None,
            range_output_bounded: true,
        }
    }

    fn account_commands(&self) -> Vec<ProviderCommand> {
        key_commands(self.key.read().is_ok())
    }

    async fn translate(
        &self,
        intent: Intent,
        output: Output,
        text: &str,
        target_language: &str,
        expected: KeyId,
    ) -> Result<Translated<KeyId>, Error> {
        let system = prompt::instructions(target_language, &self.source_language);
        let reply = self
            .send(
                intent,
                output,
                system,
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
        expected: KeyId,
    ) -> Result<Translated<KeyId>, Error> {
        let system = dictionary::instructions(target_language, &self.source_language);
        let message = dictionary::message(term, line);
        let reply = self
            .send(intent, output, system, message, &[line, term], expected)
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
            _ => Err(Error::failed("Unknown Anthropic command")),
        }
    }
}

#[cfg(test)]
mod tests;
