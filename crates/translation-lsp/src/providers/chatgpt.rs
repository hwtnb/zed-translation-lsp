mod response;
use response::response_text;
pub(crate) mod auth;
pub(crate) mod auth_store;
pub(crate) mod http;
#[cfg(test)]
pub(crate) mod test_support;

use std::mem;
#[cfg(test)]
use std::time::Instant;

#[cfg(test)]
use serde_json::Value;
use serde_json::json;

use crate::config::{ChatgptModel, limits::SizeLimit};
#[cfg(test)]
use crate::config::{ReasoningEffort, limits::TranslationLimits};

use super::{
    AccountChange, AuthUiState, Capabilities, Intent, Interaction, Output, OutputOptions,
    ProviderCommand, Translated, TranslationProvider,
    admission::{Admission, Attempt},
    error::{Error, ErrorKind},
    prompt, transport,
};
use auth::{AccountKey, Auth, account_changed};
use http::{Http, response_error, temporary_network};

const CONNECT: &str = "translation-lsp.connect";
const ENABLE: &str = "translation-lsp.enable";
const SIGN_OUT: &str = "translation-lsp.sign-out";
const USAGE: &str = "translation-lsp.usage";
const COMMANDS: [ProviderCommand; 4] = [
    ProviderCommand {
        command: CONNECT,
        title: "Continue with ChatGPT / Choose account",
        changes_account: AccountChange::AtOnce,
    },
    ProviderCommand {
        command: ENABLE,
        title: "Enable ChatGPT plan use",
        changes_account: AccountChange::AtOnce,
    },
    ProviderCommand {
        command: SIGN_OUT,
        title: "Sign out of ChatGPT",
        changes_account: AccountChange::AtOnce,
    },
    ProviderCommand {
        command: USAGE,
        title: "ChatGPT Usage",
        changes_account: AccountChange::Never,
    },
];

pub(crate) fn management_commands(state: AuthUiState) -> Vec<ProviderCommand> {
    COMMANDS
        .into_iter()
        .filter_map(|mut command| {
            let visible = match command.command {
                CONNECT | USAGE => true,
                ENABLE => state == AuthUiState::PlanDisabled,
                SIGN_OUT => matches!(state, AuthUiState::Ready | AuthUiState::PlanDisabled),
                _ => false,
            };
            command.title = match (command.command, state) {
                (CONNECT, AuthUiState::Ready) => "Change ChatGPT account",
                (CONNECT, AuthUiState::Unavailable) => "Check ChatGPT connection…",
                _ => command.title,
            };
            visible.then_some(command)
        })
        .collect()
}

#[derive(Clone)]
pub(crate) struct ChatGptProvider {
    auth: Auth,
    responses: reqwest::Client,
    options: OutputOptions<ChatgptModel>,
    source_language: String,
    output_limit: SizeLimit,
    api: String,
    admission: Admission<AccountKey>,
}

impl ChatGptProvider {
    pub fn new(
        options: OutputOptions<ChatgptModel>,
        source_language: String,
        output_limit: SizeLimit,
    ) -> Result<Self, Error> {
        let http = Http::new()?;
        Ok(Self {
            auth: Auth::production(http)?,
            responses: Http::responses_client()?,
            options,
            source_language,
            output_limit,
            api: "https://api.openai.com/v1".into(),
            admission: Admission::default(),
        })
    }

    #[cfg(test)]
    pub fn synthetic(auth: Auth, api: String, model: String) -> Self {
        Self {
            auth,
            responses: Http::responses_client().unwrap(),
            api,
            options: OutputOptions::new(
                ChatgptModel {
                    model,
                    effort: ReasoningEffort::Low,
                    speed: None,
                },
                None,
            ),
            source_language: "auto".into(),
            output_limit: TranslationLimits::default().output_kib,
            admission: Admission::default(),
        }
    }

    #[cfg(test)]
    pub fn with_options(mut self, options: OutputOptions<ChatgptModel>) -> Self {
        self.options = options;
        self
    }

    /// Fails unless `key` is still the selected account once no update holds
    /// the credential store. A store that cannot be read fails as itself.
    async fn check_account(&self, key: &AccountKey) -> Result<(), Error> {
        match self.auth.settled_key().await {
            Ok(current) if &current == key => Ok(()),
            Err(error) if error.kind() == ErrorKind::Storage => Err(error),
            _ => Err(account_changed()),
        }
    }

    async fn request(
        &self,
        body: &str,
        target: &str,
        expected: &AccountKey,
        intent: Intent,
        output: Output,
    ) -> Result<Translated<AccountKey>, Error> {
        let generation = self.admission.admit(expected, intent)?;
        let access = match self.auth.access(expected).await {
            Ok(access) => access,
            Err(mut error) => {
                self.admission
                    .record_error(expected, &mut error, generation);
                return Err(error);
            }
        };
        let key = &access.key;
        let secrets = [
            access.credentials.access_token.as_str(),
            access.credentials.refresh_token.as_str(),
            access.credentials.id_token.as_str(),
            body,
        ];
        let options = self.options.get(output);
        let mut request = json!({
            "model": options.model,
            "store": false,
            "stream": true,
            "instructions": prompt::instructions(target, &self.source_language),
            "input": [{"role": "user", "content": prompt::message(body)}],
            "reasoning": {"effort": options.effort},
        });
        if let Some(speed) = options.speed {
            request["service_tier"] = json!(speed);
        }
        let mut sent = false;
        let mut result = self
            .admission
            .retrying(key, intent, async || {
                // The account may have changed during the wait.
                if mem::replace(&mut sent, true)
                    && let Err(error) = self.check_account(key).await
                {
                    return Attempt::without_response(error);
                }
                let response = self
                    .responses
                    .post(format!("{}/responses", self.api))
                    .bearer_auth(&access.credentials.access_token)
                    .json(&request)
                    .send()
                    .await;
                let response = match response {
                    Ok(response) => response,
                    Err(error) => {
                        return Attempt::without_response(temporary_network(error));
                    }
                };
                let retry_after = transport::retry_delay(response.headers());
                let result = if response.status().is_success() {
                    response_text(response, &secrets, self.output_limit).await
                } else {
                    Err(self.auth.http.failure(response, &secrets).await)
                };
                Attempt {
                    result,
                    retry_after,
                }
            })
            .await;
        // The server compares the account again without waiting, so return
        // only after an update of the credentials has finished.
        if result.is_ok()
            && let Err(error) = self.check_account(key).await
        {
            result = Err(error);
        }
        self.admission.settle(key, intent, generation, &mut result);
        Ok(Translated {
            text: prompt::translation(result?),
            context: access.key,
        })
    }
}

impl TranslationProvider for ChatGptProvider {
    /// The selected account and its authorization.
    type Context = AccountKey;

    fn check_admission(&self, key: &AccountKey, generation: u64) -> Result<(), Error> {
        self.admission.check(key, generation)
    }

    fn context(&self) -> Result<AccountKey, Error> {
        self.auth.key()
    }

    fn admit(&self, key: &AccountKey, intent: Intent) -> Result<u64, Error> {
        self.admission.admit(key, intent)
    }

    fn auth_ui_state(&self) -> AuthUiState {
        self.auth.ui_state()
    }
    fn capabilities() -> Capabilities {
        Capabilities {
            server_name: "Translation LSP (ChatGPT)",
            automatic_selection: true,
            account_menu: Some("ChatGPT account…"),
            commands: &COMMANDS,
            hover_note: None,
            comment_notice: None,
            range_output_bounded: false,
        }
    }

    fn account_commands(&self) -> Vec<ProviderCommand> {
        management_commands(self.auth.ui_state())
    }

    async fn translate(
        &self,
        intent: Intent,
        output: Output,
        text: &str,
        target: &str,
        key: AccountKey,
    ) -> Result<Translated<AccountKey>, Error> {
        self.request(text, target, &key, intent, output).await
    }

    async fn administer(&self, command: &str, ui: &impl Interaction) -> Result<String, Error> {
        match command {
            CONNECT => self.auth.choose_account(ui, false).await,
            ENABLE => self.auth.choose_account(ui, true).await,
            SIGN_OUT => self.auth.sign_out().await,
            USAGE => {
                ui.open("https://chatgpt.com/settings/usage").await?;
                Ok("ChatGPT Usage opened.".into())
            }
            _ => Err(Error::failed("Unknown ChatGPT command")),
        }
    }
}

#[cfg(test)]
mod document_tests;

#[cfg(test)]
mod tests;
