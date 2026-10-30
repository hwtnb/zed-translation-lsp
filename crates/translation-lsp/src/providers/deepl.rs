//! The DeepL API with an API key. DeepL takes no prompt, so a text is sent in
//! the segments of [`Segmented`], with its code and URLs marked as XML that
//! DeepL leaves untranslated.
#[cfg(test)]
use std::path::Path;
use std::path::PathBuf;

use reqwest::header::AUTHORIZATION;
use serde_json::{Value, json};

#[cfg(test)]
use super::api_key::test_support::Terminal as SyntheticTerminal;
use super::{
    AccountChange, AuthUiState, Capabilities, Intent, Interaction, Output, ProviderCommand,
    Translated, TranslationProvider,
    admission::{Admission, Attempt},
    api_key::{ApiKey, KeyFile, KeyId, KeySpec, Terminal},
    error::{
        Error, RemoteError, RemoteKind, Service, TransportExceeded, TransportScope,
        credentials::CredentialError,
    },
    segments::{Markup, Segmented},
    transport::{self, safe},
};
#[cfg(test)]
use crate::config::limits::TranslationLimits;
use crate::config::{error::ConfigError, language, limits::SizeLimit};

const PRO: &str = "https://api.deepl.com";
/// Where a key of DeepL API Free, which ends in `:fx`, is sent.
const FREE: &str = "https://api-free.deepl.com";
const SERVICE: Service = Service::DeepL;
/// The texts of one request. DeepL takes 128 KiB, which the rest of the
/// request and the marks around code count against.
const REQUEST_BYTES: usize = 96 * 1024;

macro_rules! menu {
    () => {
        "DeepL API key…"
    };
}
const MENU: &str = menu!();
const REJECTED: &str = concat!(
    "The DeepL API key was rejected. Run \"",
    menu!(),
    "\" from the code actions to replace it"
);
const QUOTA: &str =
    "Check the usage of the DeepL account at https://www.deepl.com/your-account/usage";
const UNMARKED: &str = "DeepL did not keep the code and URLs of the text; no translation was saved";

const SAVE: &str = "translation-lsp.save-deepl-key";
const REMOVE: &str = "translation-lsp.remove-deepl-key";
const COMMANDS: [ProviderCommand; 2] = [
    // Saving happens later, in the terminal. The server follows the file.
    ProviderCommand {
        command: SAVE,
        title: "Save DeepL API key",
        changes_account: AccountChange::Never,
    },
    ProviderCommand {
        command: REMOVE,
        title: "Remove DeepL API key",
        changes_account: AccountChange::IfConfirmed,
    },
];
const REMOVAL: &str = "Remove the saved DeepL API key from this computer? The key stays valid until you delete it in your DeepL account.";

fn key_commands(usable: bool) -> Vec<ProviderCommand> {
    let [mut save, remove] = COMMANDS;
    if usable {
        save.title = "Replace DeepL API key";
        vec![save, remove]
    } else {
        vec![save]
    }
}

/// The code DeepL takes for a language to translate into: a language tag in
/// capitals, such as `JA` or `PT-BR`.
fn target_code(value: &str) -> Option<String> {
    language::is_tag(value).then(|| value.replace('_', "-").to_ascii_uppercase())
}

/// The code DeepL takes for the language of the source: the language alone,
/// such as `PT` for `pt-BR`. `None` for `auto`, which DeepL detects.
fn source_code(value: &str) -> Option<String> {
    let language = value.split(['-', '_']).next()?;
    language::is_tag(value).then(|| language.to_ascii_uppercase())
}

/// Checks that the language settings are codes that DeepL takes.
pub(crate) fn check_languages(source: &str, target: &str) -> Result<(), ConfigError> {
    if target_code(target).is_none() {
        return Err(ConfigError::LanguageCode {
            setting: "target_language",
            provider: "deepl",
        });
    }
    if source != "auto" && source_code(source).is_none() {
        return Err(ConfigError::LanguageCode {
            setting: "source_language",
            provider: "deepl",
        });
    }
    Ok(())
}

/// The error that a failed response reports.
fn failure(status: u16, body: &Value, secrets: &[&str]) -> Error {
    let message = body
        .get("message")
        .or_else(|| body.get("error").and_then(|error| error.get("message")))
        .and_then(Value::as_str)
        .or_else(|| body.as_str())
        .filter(|detail| !detail.trim().is_empty())
        .unwrap_or("DeepL request failed");
    let code = body.get("code").and_then(|code| match code {
        Value::String(code) => Some(code.clone()),
        Value::Number(code) => Some(code.to_string()),
        _ => None,
    });
    let (kind, advice) = match status {
        401 | 403 => (RemoteKind::Configuration, Some(REJECTED)),
        456 => (RemoteKind::UsageLimit, Some(QUOTA)),
        429 | 529 => (RemoteKind::Temporary, None),
        // DeepL names the setting it refuses, such as a language it does not
        // translate.
        400 if message.contains("target_lang") || message.contains("source_lang") => {
            (RemoteKind::Configuration, None)
        }
        status if status >= 500 => (RemoteKind::Temporary, None),
        _ => (RemoteKind::Failed, None),
    };
    RemoteError {
        service: SERVICE,
        kind,
        status,
        detail: safe(message, secrets, 512),
        code: code.map(|value| safe(&value, secrets, 128)),
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

/// The translations of a response to `count` texts. The body is bounded by
/// the output limit, with room for JSON escaping.
async fn translations(
    mut response: reqwest::Response,
    count: usize,
    output_limit: SizeLimit,
) -> Result<Vec<String>, Error> {
    let limit = output_limit
        .bytes()
        .saturating_mul(6)
        .saturating_add(1024 * 1024);
    let mut bytes = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(transport::network_error(SERVICE))?
    {
        if bytes.len().saturating_add(chunk.len()) > limit {
            return Err(TransportExceeded {
                service: SERVICE,
                scope: TransportScope::Response,
                actual_bytes: bytes.len().saturating_add(chunk.len()),
                maximum_bytes: limit,
            }
            .into());
        }
        bytes.extend_from_slice(&chunk);
    }
    let body: Value = serde_json::from_slice(&bytes)
        .map_err(|_| Error::protocol(SERVICE, "response is not JSON"))?;
    let translations: Option<Vec<String>> = body["translations"]
        .as_array()
        .map(|translations| {
            translations
                .iter()
                .map(|translation| translation["text"].as_str().map(str::to_owned))
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

/// The texts in groups that each fit in one request.
fn batches(texts: &[String]) -> Vec<&[String]> {
    let mut batches = Vec::new();
    let mut start = 0;
    let mut bytes = 0;
    for (index, text) in texts.iter().enumerate() {
        if index > start && bytes + text.len() > REQUEST_BYTES {
            batches.push(&texts[start..index]);
            (start, bytes) = (index, 0);
        }
        bytes += text.len();
    }
    if start < texts.len() {
        batches.push(&texts[start..]);
    }
    batches
}

#[derive(Clone)]
pub(crate) struct DeepLProvider {
    client: reqwest::Client,
    key: KeyFile,
    terminal: Terminal,
    /// `None` for `auto`.
    source_language: Option<String>,
    output_limit: SizeLimit,
    /// Where requests go in place of the address the key belongs to.
    #[cfg(test)]
    api: Option<String>,
    admission: Admission<KeyId>,
}

impl DeepLProvider {
    /// `key_directory` is the credential directory, where the key file is.
    pub fn new(
        source_language: &str,
        output_limit: SizeLimit,
        key_directory: Result<PathBuf, CredentialError>,
    ) -> Result<Self, Error> {
        Ok(Self {
            // The worker owns the total deadline.
            client: transport::client_builder()
                .build()
                .map_err(|_| Error::failed("Could not prepare the DeepL HTTP client"))?,
            key: KeyFile::new(KeySpec::deepl(MENU), key_directory),
            terminal: Terminal::System,
            source_language: source_code(source_language),
            output_limit,
            #[cfg(test)]
            api: None,
            admission: Admission::default(),
        })
    }

    #[cfg(test)]
    pub fn synthetic(api: String, directory: &Path) -> Self {
        Self {
            api: Some(api),
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
        self.source_language = source_code(source_language);
        self
    }

    /// Where the requests made with `key` go.
    fn api(&self, key: &ApiKey) -> String {
        #[cfg(test)]
        if let Some(api) = &self.api {
            return api.clone();
        }
        if key.text.ends_with(":fx") { FREE } else { PRO }.to_owned()
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
        let target = target_code(target_language).unwrap_or_else(|| target_language.to_owned());
        let mut request = json!({
            "text": texts,
            "target_lang": target,
            "tag_handling": "xml",
            "ignore_tags": ["x"],
        });
        if let Some(source_language) = &self.source_language {
            request["source_lang"] = json!(source_language);
        }
        let endpoint = format!("{}/v2/translate", self.api(&key));
        let in_use = || self.key.in_use(expected).map(drop);
        // Dropping this future on cancellation or expiry drops the request or
        // response it is awaiting, which closes the connection.
        self.admission
            .send(&expected, intent, in_use, async || {
                let response = self
                    .client
                    .post(&endpoint)
                    .header(AUTHORIZATION, key.authorization("DeepL-Auth-Key"))
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

impl TranslationProvider for DeepLProvider {
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
            server_name: "Translation LSP (DeepL API)",
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

    /// A term is translated like any other text: DeepL writes no entries.
    async fn translate(
        &self,
        intent: Intent,
        _output: Output,
        text: &str,
        target_language: &str,
        expected: KeyId,
    ) -> Result<Translated<KeyId>, Error> {
        let segmented = Segmented::new(text);
        let marked = segmented.marked(Markup::Xml);
        let mut translations = Vec::with_capacity(marked.len());
        // The requests of one text go one after another, so that a failure
        // stops the rest.
        for batch in batches(&marked) {
            translations.extend(
                self.send(intent, batch, target_language, text, expected)
                    .await?,
            );
        }
        let text = segmented
            .assemble(Markup::Xml, &translations)
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
            _ => Err(Error::failed("Unknown DeepL API command")),
        }
    }
}

#[cfg(test)]
mod tests;
