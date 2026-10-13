pub(crate) mod key;
mod response;
#[cfg(test)]
pub(crate) mod test_support;

use std::path::PathBuf;

use key::{KeyFile, KeyId, terminal};
use serde_json::json;

use crate::config::{AnthropicEffort, limits::SizeLimit};

use super::{
    AccountChange, AuthUiState, Capabilities, Context, Intent, Interaction, Output, OutputOptions,
    ProviderCommand, Translated, TranslationProvider,
    admission::{Admission, Attempt},
    error::{Cause, Error, Service, credentials::CredentialError},
    prompt, transport,
};

const API: &str = "https://api.anthropic.com/v1";
const VERSION: &str = "2023-06-01";
const MAX_OUTPUT_TOKENS: usize = 64_000;
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
    // Records the executable instead, and answers whether a window opened.
    #[cfg(test)]
    Synthetic(test_support::Terminal),
}

// The model and options one output sends.
#[derive(Clone)]
pub(crate) struct ModelOptions {
    pub model: String,
    pub effort: AnthropicEffort,
    pub thinking: bool,
}

#[derive(Clone)]
pub(crate) struct AnthropicProvider {
    client: reqwest::Client,
    key: KeyFile,
    terminal: Terminal,
    options: OutputOptions<ModelOptions>,
    source_language: String,
    output_limit: SizeLimit,
    api: String,
    admission: Admission<KeyId>,
}

impl AnthropicProvider {
    // `key_directory` is the credential directory, where the key file is.
    pub fn new(
        options: OutputOptions<ModelOptions>,
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
                    ModelOptions {
                        model: "claude-haiku-5-5".into(),
                        effort: AnthropicEffort::Low,
                        thinking: false,
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
    pub fn with_options(mut self, options: OutputOptions<ModelOptions>) -> Self {
        self.options = options;
        self
    }

    // The notice names the command either way: the window may be on a machine
    // the user is not sitting at.
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

fn key_of(context: &Context) -> Result<KeyId, Error> {
    if let Context::Anthropic(key) = context {
        Ok(*key)
    } else {
        Err(Error::failed(
            "Translation was prepared for a different provider",
        ))
    }
}

impl TranslationProvider for AnthropicProvider {
    // The key in use is part of the context: what was translated or refused
    // with one key is not kept for another.
    fn context(&self) -> Result<Context, Error> {
        let (_, key) = self.key.read()?;
        Ok(Context::Anthropic(key))
    }

    fn admit(&self, context: &Context, intent: Intent) -> Result<u64, Error> {
        self.admission.admit(&key_of(context)?, intent)
    }

    fn check_admission(&self, context: &Context, generation: u64) -> Result<(), Error> {
        let Context::Anthropic(key) = context else {
            return Ok(());
        };
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
        context: Context,
    ) -> Result<Translated, Error> {
        let expected = key_of(&context)?;
        let (key, current) = self.key.read()?;
        if current != expected {
            return Err(Cause::Modified("Anthropic API key").into());
        }
        let generation = self.admission.admit(&expected, intent)?;
        let secrets = [&*key.text, text];
        let options = self.options.get(output);
        let configured = self.options.configured(output);
        let request = json!({
            "model": options.model,
            "max_tokens": MAX_OUTPUT_TOKENS.min(self.output_limit.bytes()),
            "stream": true,
            "system": prompt::instructions(target_language, &self.source_language),
            "messages": [{"role": "user", "content": text}],
            "output_config": {"effort": options.effort},
            "thinking": {"type": if options.thinking { "adaptive" } else { "disabled" }},
        });
        // Dropping this future on cancellation or expiry drops the request or
        // response it is awaiting, which closes the connection.
        let mut result = self
            .admission
            .retrying(&expected, intent, async || {
                let response = self
                    .client
                    .post(format!("{}/messages", self.api))
                    .header("x-api-key", key.header.clone())
                    .header("anthropic-version", VERSION)
                    .json(&request)
                    .send()
                    .await;
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
        self.admission
            .settle(&expected, intent, generation, &mut result);
        Ok(Translated {
            text: result?,
            context,
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
