//! The OpenAI API with an API key, over the Responses API that the ChatGPT
//! provider also reads.
#[cfg(test)]
use std::path::Path;
use std::{borrow::Cow, mem, path::PathBuf};

use reqwest::header::AUTHORIZATION;
use serde_json::{Value, json};

#[cfg(test)]
use super::api_key::test_support::Terminal as SyntheticTerminal;
use super::{
    AccountChange, AuthUiState, Capabilities, Intent, Interaction, Output, OutputOptions,
    ProviderCommand, Translated, TranslationProvider,
    admission::{Admission, Attempt},
    api_key::{KeyFile, KeyId, KeySpec, Origin, Terminal},
    dictionary,
    error::{Cause, Error, RemoteError, RemoteKind, Service, credentials::CredentialError},
    prompt, range_output_tokens,
    responses::{self, Reading, request_id},
    transport::{self, safe},
};
use crate::config::{OpenAiModel, limits::SizeLimit};
#[cfg(test)]
use crate::config::{ReasoningEffort, limits::TranslationLimits};

const API: &str = "https://api.openai.com/v1";
const SERVICE: Service = Service::OpenAiApi;

macro_rules! menu {
    () => {
        "OpenAI API key…"
    };
}
const MENU: &str = menu!();
const REJECTED: &str = concat!(
    "The API key was rejected. Run \"",
    menu!(),
    "\" from the code actions to replace it"
);
const BILLING: &str = "Check the billing of the organization at https://platform.openai.com/settings/organization/billing";

const SAVE: &str = "translation-lsp.save-openai-key";
const REMOVE: &str = "translation-lsp.remove-openai-key";
const COMMANDS: [ProviderCommand; 2] = [
    // Saving happens later, in the terminal. The server follows the file.
    ProviderCommand {
        command: SAVE,
        title: "Save OpenAI API key",
        changes_account: AccountChange::Never,
    },
    ProviderCommand {
        command: REMOVE,
        title: "Remove OpenAI API key",
        changes_account: AccountChange::IfConfirmed,
    },
];
const REMOVAL: &str = "Remove the saved OpenAI API key from this computer? The key stays valid until you delete it in the OpenAI Platform.";

const READING: Reading = Reading {
    service: SERVICE,
    not_text: "OpenAI did not return translation text",
    failure,
    incomplete,
};

fn key_commands(usable: bool) -> Vec<ProviderCommand> {
    let [mut save, remove] = COMMANDS;
    if usable {
        save.title = "Replace OpenAI API key";
        vec![save, remove]
    } else {
        vec![save]
    }
}

/// The error that a failed response or a stream error reports. The codes come
/// first: a stream error arrives after HTTP 200.
fn failure(status: u16, body: &Value, request_id: Option<&str>, secrets: &[&str]) -> Error {
    let error = body
        .get("error")
        .filter(|error| error.is_object())
        .unwrap_or(body);
    let code = error.get("code").and_then(Value::as_str);
    let param = error.get("param").and_then(Value::as_str);
    let detail = error
        .get("message")
        .and_then(Value::as_str)
        .or_else(|| body.as_str())
        .filter(|detail| !detail.trim().is_empty())
        .unwrap_or("OpenAI request failed");
    let (kind, advice) = match (status, code) {
        (_, Some("insufficient_quota")) => (RemoteKind::UsageLimit, Some(BILLING)),
        (_, Some("invalid_api_key")) | (401, _) => (RemoteKind::Configuration, Some(REJECTED)),
        (_, Some("model_not_found")) | (404, _) => (RemoteKind::Configuration, None),
        (_, Some("rate_limit_exceeded" | "server_error" | "server_is_overloaded")) => {
            (RemoteKind::Temporary, None)
        }
        (403, _) => (RemoteKind::NotEligible, None),
        (400, _) if matches!(param, Some("model" | "reasoning.effort" | "service_tier")) => {
            (RemoteKind::Configuration, None)
        }
        (429, _) => (RemoteKind::Temporary, None),
        (status, _) if status >= 500 => (RemoteKind::Temporary, None),
        _ => (RemoteKind::Failed, None),
    };
    RemoteError {
        service: SERVICE,
        kind,
        status,
        detail: safe(detail, secrets, 512),
        code: code.map(|value| safe(value, secrets, 128)),
        param: param.map(|value| safe(value, secrets, 128)),
        request_id: request_id.map(|value| safe(value, secrets, 128)),
        advice: advice.map(Cow::Borrowed),
    }
    .into()
}

fn incomplete(event: &Value) -> Error {
    match event["response"]["incomplete_details"]["reason"].as_str() {
        Some("max_output_tokens") => Error::failed(
            "OpenAI response stopped at the output token limit; no partial translation was saved",
        ),
        Some("content_filter") => Cause::Refusal {
            service: SERVICE,
            category: Some("content_filter".into()),
        }
        .into(),
        _ => Error::failed("OpenAI response is incomplete; no partial translation was saved"),
    }
}

/// An error body that is not JSON is shown as text.
async fn failure_of(response: reqwest::Response, secrets: &[&str]) -> Error {
    let status = response.status().as_u16();
    let request_id = request_id(response.headers()).map(str::to_owned);
    match transport::error_body(response, SERVICE).await {
        Ok(body) => failure(status, &body, request_id.as_deref(), secrets),
        Err(error) => error,
    }
}

#[derive(Clone)]
pub(crate) struct OpenAiApiProvider {
    client: reqwest::Client,
    key: KeyFile,
    terminal: Terminal,
    options: OutputOptions<OpenAiModel>,
    source_language: String,
    output_limit: SizeLimit,
    api: String,
    admission: Admission<KeyId>,
}

impl OpenAiApiProvider {
    /// `key_directory` is the credential directory, where the key file is.
    pub fn new(
        options: OutputOptions<OpenAiModel>,
        source_language: String,
        output_limit: SizeLimit,
        key_directory: Result<PathBuf, CredentialError>,
    ) -> Result<Self, Error> {
        Ok(Self {
            // The worker owns the total deadline.
            client: transport::client_builder()
                .build()
                .map_err(|_| Error::failed("Could not prepare the OpenAI HTTP client"))?,
            key: KeyFile::new(
                KeySpec::origin("OpenAI API key", MENU, Origin::openai()),
                key_directory,
            ),
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
            terminal: Terminal::Synthetic(SyntheticTerminal::default()),
            ..Self::new(
                OutputOptions::new(
                    OpenAiModel {
                        model: "gpt-6-luna".into(),
                        effort: ReasoningEffort::Low,
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
    pub fn with_options(mut self, options: OutputOptions<OpenAiModel>) -> Self {
        self.options = options;
        self
    }

    /// Sends `message` with the `instructions` and returns the reply. `texts`
    /// are the document's texts in the message, the first sizing the reply;
    /// errors never show them.
    async fn send(
        &self,
        intent: Intent,
        output: Output,
        instructions: String,
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
        let mut request = json!({
            "model": options.model,
            "store": false,
            "stream": true,
            "instructions": instructions,
            "input": [{"role": "user", "content": message}],
            "reasoning": {"effort": options.effort},
        });
        // A tab is bounded by `translation_limits.output_kib` alone.
        if output == Output::Range {
            request["max_output_tokens"] =
                json!(range_output_tokens(texts[0]).min(self.output_limit.bytes()));
        }
        if let Some(speed) = options.speed {
            request["service_tier"] = json!(speed);
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
                let response = self
                    .client
                    .post(format!("{}/responses", self.api))
                    .header(AUTHORIZATION, key.bearer())
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
                    responses::response_text(response, &secrets, self.output_limit, &READING).await
                } else {
                    Err(failure_of(response, &secrets).await)
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

impl TranslationProvider for OpenAiApiProvider {
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
            server_name: "Translation LSP (OpenAI API)",
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
        expected: KeyId,
    ) -> Result<Translated<KeyId>, Error> {
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
            _ => Err(Error::failed("Unknown OpenAI API command")),
        }
    }
}

#[cfg(test)]
mod tests;
