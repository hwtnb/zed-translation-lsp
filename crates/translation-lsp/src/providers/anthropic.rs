pub(crate) mod key;
mod response;
#[cfg(test)]
pub(crate) mod test_support;

use std::path::PathBuf;

use key::{ApiKey, KeyFile, KeyId, terminal};
use serde_json::json;

#[cfg(test)]
use crate::config::AnthropicEffort;
use crate::config::{AnthropicModel, anthropic_thinking, limits::SizeLimit};

use super::{
    AccountChange, AuthUiState, Capabilities, Intent, Interaction, Output, OutputOptions,
    ProviderCommand, Translated, TranslationProvider,
    admission::{Admission, Attempt},
    error::{Cause, Error, Service, credentials::CredentialError},
    prompt, transport,
};

const API: &str = "https://api.anthropic.com/v1";
const VERSION: &str = "2023-06-01";
/// `speed` is a beta field, so it is sent with this header whatever its value.
const SPEED_BETA: &str = "fast-mode-2026-02-01";
const MAX_OUTPUT_TOKENS: usize = 64_000;
/// Output tokens a range translation may use besides those that grow with its
/// source. Thinking counts toward `max_tokens`, so this leaves room for it.
const RANGE_OUTPUT_ALLOWANCE: usize = 16_384;
/// Output tokens a range translation may use for each byte of its source.
const RANGE_OUTPUT_PER_SOURCE_BYTE: usize = 4;
const SERVICE: Service = Service::Anthropic;

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
const CONFIRM: &str = "Remove";

/// The `max_tokens` a request asks for. A range translation is also bounded by
/// the size of its source, so that text in a comment cannot make the model write
/// at length when the pointer only passes over it.
fn max_tokens(output: Output, source: &str, output_limit: SizeLimit) -> usize {
    let limit = MAX_OUTPUT_TOKENS.min(output_limit.bytes());
    match output {
        Output::Range => limit.min(
            RANGE_OUTPUT_ALLOWANCE
                .saturating_add(source.len().saturating_mul(RANGE_OUTPUT_PER_SOURCE_BYTE)),
        ),
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
enum Terminal {
    System,
    /// Records the executable instead, and answers whether a window opened.
    #[cfg(test)]
    Synthetic(test_support::Terminal),
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
            key: KeyFile::new(key_directory),
            terminal: Terminal::System,
            options,
            source_language,
            output_limit,
            api: API.into(),
            admission: Admission::default(),
        })
    }

    #[cfg(test)]
    pub fn synthetic(api: String, directory: &std::path::Path) -> Self {
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
                crate::config::limits::TranslationLimits::default().output_kib,
                Ok(directory.into()),
            )
            .unwrap()
        }
    }

    #[cfg(test)]
    pub fn with_terminal(mut self, terminal: test_support::Terminal) -> Self {
        self.terminal = Terminal::Synthetic(terminal);
        self
    }

    #[cfg(test)]
    pub fn with_options(mut self, options: OutputOptions<AnthropicModel>) -> Self {
        self.options = options;
        self
    }

    /// The notice names the command either way: the window may be on a machine
    /// the user is not sitting at.
    async fn open_terminal(&self) -> Result<String, Error> {
        let executable = std::env::current_exe()
            .map_err(|_| Error::failed("Could not locate the language server executable"))?;
        let command = terminal::manual_command(&executable);
        let opened = match &self.terminal {
            Terminal::System => terminal::open(&executable).await,
            #[cfg(test)]
            Terminal::Synthetic(terminal) => terminal.open(&executable),
        };
        if opened {
            Ok(format!(
                "A terminal window was opened. Enter the Anthropic API key there. If no window appeared, run this in a terminal: {command}"
            ))
        } else {
            Err(Cause::Terminal {
                purpose: "save the Anthropic API key",
                command,
            }
            .into())
        }
    }

    fn key_in_use(&self, expected: KeyId) -> Result<ApiKey, Error> {
        let (key, current) = self.key.read()?;
        if current != expected {
            return Err(Cause::Modified("Anthropic API key").into());
        }
        Ok(key)
    }

    async fn remove_key(&self, ui: &impl Interaction) -> Result<String, Error> {
        let directory = self.key.directory()?;
        if ui.choose(REMOVAL, &[CONFIRM.into()]).await?.is_none() {
            return Ok("The Anthropic API key was not removed.".into());
        }
        Ok(if key::remove(directory).map_err(key::unusable)? {
            "The Anthropic API key was removed."
        } else {
            "No Anthropic API key is saved."
        }
        .into())
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
            account_menu: Some(key::MENU),
            commands: &COMMANDS,
            hover_note: None,
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
        let key = self.key_in_use(expected)?;
        let generation = self.admission.admit(&expected, intent)?;
        let secrets = [&*key.text, text];
        let options = self.options.get(output);
        let configured = self.options.configured(output);
        let mut request = json!({
            "model": options.model,
            "max_tokens": max_tokens(output, text, self.output_limit),
            "stream": true,
            "system": prompt::instructions(target_language, &self.source_language),
            "messages": [{"role": "user", "content": text}],
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
                if std::mem::replace(&mut sent, true)
                    && let Err(error) = self.key_in_use(expected)
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
        Ok(Translated {
            text: completed.text,
            context: expected,
        })
    }

    async fn administer(&self, command: &str, ui: &impl Interaction) -> Result<String, Error> {
        match command {
            SAVE => self.open_terminal().await,
            REMOVE => self.remove_key(ui).await,
            _ => Err(Error::failed("Unknown Anthropic command")),
        }
    }
}

#[cfg(test)]
mod tests;
