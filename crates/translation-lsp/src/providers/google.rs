//! Google Cloud Translation (Basic, v2) with an API key. It takes no prompt, so
//! a text is sent in the segments of [`Segmented`], as HTML with its code and
//! URLs marked to stay untranslated.
#[cfg(test)]
use std::path::Path;
use std::path::PathBuf;

use serde_json::{Value, json};

#[cfg(test)]
use super::api_key::test_support::Terminal as SyntheticTerminal;
use super::{
    AccountChange, AuthUiState, Capabilities, Intent, Interaction, Output, ProviderCommand,
    Translated, TranslationProvider,
    admission::{Admission, Attempt},
    api_key::{KeyFile, KeyId, KeySpec, Origin, Terminal},
    error::{Error, RemoteError, RemoteKind, Service, credentials::CredentialError},
    segments::{Markup, Segmented, batches},
    transport::{self, safe},
};
#[cfg(test)]
use crate::config::limits::TranslationLimits;
use crate::config::{language, limits::SizeLimit};

const API: &str = "https://translation.googleapis.com/language/translate/v2";
const SERVICE: Service = Service::GoogleTranslation;
/// The texts of one request. The API takes 100,000 bytes, which the rest of
/// the request, the escaping of HTML, and the marks around code count against,
/// and at most 128 texts.
const REQUEST_BYTES: usize = 48 * 1024;
const REQUEST_TEXTS: usize = 128;

macro_rules! menu {
    () => {
        "Google Cloud API key…"
    };
}
const MENU: &str = menu!();
const REJECTED: &str = concat!(
    "The Google Cloud API key was rejected. Run \"",
    menu!(),
    "\" from the code actions to replace it"
);
const PROJECT: &str = "Check that the Cloud Translation API and billing are enabled in the Google Cloud project of the key, and that the key may call the API";
const QUOTA: &str = "Check the quotas of the Cloud Translation API in the Google Cloud console";
const UNMARKED: &str =
    "Google Cloud Translation did not keep the code and URLs of the text; no translation was saved";

const SAVE: &str = "translation-lsp.save-google-key";
const REMOVE: &str = "translation-lsp.remove-google-key";
const COMMANDS: [ProviderCommand; 2] = [
    // Saving happens later, in the terminal. The server follows the file.
    ProviderCommand {
        command: SAVE,
        title: "Save Google Cloud API key",
        changes_account: AccountChange::Never,
    },
    ProviderCommand {
        command: REMOVE,
        title: "Remove Google Cloud API key",
        changes_account: AccountChange::IfConfirmed,
    },
];
const REMOVAL: &str = "Remove the saved Google Cloud API key from this computer? The key stays valid until you delete it in the Google Cloud console.";

fn key_commands(usable: bool) -> Vec<ProviderCommand> {
    let [mut save, remove] = COMMANDS;
    if usable {
        save.title = "Replace Google Cloud API key";
        vec![save, remove]
    } else {
        vec![save]
    }
}

/// The code the API takes for a language setting, such as `ja` or `zh-CN`.
/// `None` for `auto`, which the API detects.
fn code(value: &str) -> Option<String> {
    language::is_tag(value).then(|| value.replace('_', "-"))
}

/// The error that a failed response reports, in the error format of Google
/// APIs: a status name, and a reason in its details.
fn failure(status: u16, body: &Value, secrets: &[&str]) -> Error {
    let error = body
        .get("error")
        .filter(|error| error.is_object())
        .unwrap_or(body);
    let message = error
        .get("message")
        .and_then(Value::as_str)
        .or_else(|| body.as_str())
        .filter(|detail| !detail.trim().is_empty())
        .unwrap_or("Google Cloud Translation request failed");
    // Newer replies give the reason in `details`, older ones in `errors`.
    let reason = ["details", "errors"]
        .iter()
        .filter_map(|field| error.get(field)?.as_array())
        .flatten()
        .find_map(|detail| detail.get("reason")?.as_str());
    let name = error.get("status").and_then(Value::as_str);
    let (kind, advice) = match (status, reason) {
        (_, Some("API_KEY_INVALID" | "keyInvalid")) | (401, _) => {
            (RemoteKind::Configuration, Some(REJECTED))
        }
        (_, Some("RATE_LIMIT_EXCEEDED" | "rateLimitExceeded" | "userRateLimitExceeded")) => {
            (RemoteKind::Temporary, None)
        }
        (_, Some("dailyLimitExceeded" | "quotaExceeded")) => (RemoteKind::UsageLimit, Some(QUOTA)),
        (403, _) => (RemoteKind::Configuration, Some(PROJECT)),
        // The request names only the languages and the texts, and a text that
        // is too long never goes in one.
        (400, _) => (RemoteKind::Configuration, None),
        (429, _) => (RemoteKind::Temporary, None),
        (status, _) if status >= 500 => (RemoteKind::Temporary, None),
        _ => (RemoteKind::Failed, None),
    };
    RemoteError {
        service: SERVICE,
        kind,
        status,
        detail: safe(message, secrets, 512),
        code: reason.or(name).map(|value| safe(value, secrets, 128)),
        param: None,
        request_id: None,
        advice: advice.map(Into::into),
    }
    .into()
}

async fn failure_of(response: reqwest::Response, secrets: &[&str]) -> Error {
    let status = response.status().as_u16();
    match transport::error_body(response, SERVICE).await {
        Ok(body) => failure(status, &body, secrets),
        Err(error) => error,
    }
}

/// The translations of a response to `count` texts.
async fn translations(
    response: reqwest::Response,
    count: usize,
    output_limit: SizeLimit,
) -> Result<Vec<String>, Error> {
    let body = transport::json_body(response, SERVICE, output_limit).await?;
    let translations: Option<Vec<String>> = body["data"]["translations"]
        .as_array()
        .map(|translations| {
            translations
                .iter()
                .map(|translation| translation["translatedText"].as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default();
    match translations {
        Some(translations) if translations.len() == count => Ok(translations),
        _ => Err(Error::protocol(
            SERVICE,
            "response does not hold a translation for each text",
        )),
    }
}

#[derive(Clone)]
pub(crate) struct GoogleProvider {
    client: reqwest::Client,
    key: KeyFile,
    terminal: Terminal,
    /// `None` for `auto`.
    source_language: Option<String>,
    output_limit: SizeLimit,
    api: String,
    admission: Admission<KeyId>,
}

impl GoogleProvider {
    /// `key_directory` is the credential directory, where the key file is.
    pub fn new(
        source_language: &str,
        output_limit: SizeLimit,
        key_directory: Result<PathBuf, CredentialError>,
    ) -> Result<Self, Error> {
        Ok(Self {
            // The worker owns the total deadline.
            client: transport::client_builder().build().map_err(|_| {
                Error::failed("Could not prepare the Google Cloud Translation HTTP client")
            })?,
            key: KeyFile::new(
                KeySpec::origin("Google Cloud API key", MENU, Origin::google_translation()),
                key_directory,
            ),
            terminal: Terminal::System,
            source_language: code(source_language),
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
                "auto",
                TranslationLimits::default().output_kib,
                Ok(directory.into()),
            )
            .unwrap()
        }
    }

    #[cfg(test)]
    pub fn with_source_language(mut self, source_language: &str) -> Self {
        self.source_language = code(source_language);
        self
    }

    /// Sends `texts` and returns their translations. `source` is the text they
    /// were taken from, which errors never show.
    async fn send(
        &self,
        intent: Intent,
        texts: &[String],
        target_language: &str,
        source: &str,
        expected: KeyId,
    ) -> Result<Vec<String>, Error> {
        let key = self.key.in_use(expected)?;
        let secrets = [&*key.text, source];
        // The settings were checked when the server started.
        let target = code(target_language).unwrap_or_else(|| target_language.to_owned());
        let mut request = json!({"q": texts, "target": target, "format": "html"});
        if let Some(source_language) = &self.source_language {
            request["source"] = json!(source_language);
        }
        let in_use = || self.key.in_use(expected).map(drop);
        // Dropping this future on cancellation or expiry drops the request or
        // response it is awaiting, which closes the connection.
        self.admission
            .send(&expected, intent, in_use, async || {
                // In a header rather than the URL, where addresses get logged.
                let response = self
                    .client
                    .post(&self.api)
                    .header("x-goog-api-key", key.header.clone())
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
                    translations(response, texts.len(), self.output_limit).await
                } else {
                    Err(failure_of(response, &secrets).await)
                };
                Attempt {
                    result,
                    retry_after,
                }
            })
            .await
    }
}

impl TranslationProvider for GoogleProvider {
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
            server_name: "Translation LSP (Google Cloud Translation)",
            automatic_selection: true,
            account_menu: Some(MENU),
            commands: &COMMANDS,
            hover_note: None,
            comment_notice: None,
            range_output_bounded: false,
        }
    }

    fn account_commands(&self) -> Vec<ProviderCommand> {
        key_commands(self.key.read().is_ok())
    }

    /// A term is translated like any other text: the API writes no entries.
    async fn translate(
        &self,
        intent: Intent,
        _output: Output,
        text: &str,
        target_language: &str,
        expected: KeyId,
    ) -> Result<Translated<KeyId>, Error> {
        let segmented = Segmented::new(text);
        let marked = segmented.marked(Markup::Html);
        let mut translations = Vec::with_capacity(marked.len());
        // The requests of one text go one after another, so that a failure
        // stops the rest.
        for batch in batches(&marked, REQUEST_BYTES, REQUEST_TEXTS) {
            translations.extend(
                self.send(intent, batch, target_language, text, expected)
                    .await?,
            );
        }
        let text = segmented
            .assemble(Markup::Html, &translations)
            .ok_or_else(|| Error::failed(UNMARKED))?;
        Ok(Translated {
            text,
            context: expected,
            entry: false,
        })
    }

    async fn administer(&self, command: &str, ui: &impl Interaction) -> Result<String, Error> {
        match command {
            SAVE => self.key.ask_in_terminal(&self.terminal).await,
            REMOVE => self.key.remove(ui, REMOVAL).await,
            _ => Err(Error::failed("Unknown Google Cloud Translation command")),
        }
    }
}

#[cfg(test)]
mod tests;
