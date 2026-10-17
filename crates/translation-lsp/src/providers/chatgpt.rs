mod response;
use response::response_text;
pub(crate) mod auth;
pub(crate) mod auth_store;
pub(crate) mod http;
#[cfg(test)]
pub(crate) mod test_support;

use std::time::Duration;
#[cfg(test)]
use std::time::Instant;

#[cfg(test)]
use serde_json::Value;
use serde_json::json;

use crate::config::{ChatgptSpeed, ReasoningEffort, limits::SizeLimit};

use super::{
    AccountChange, AuthUiState, Capabilities, Context, Intent, Interaction, Output, OutputOptions,
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

/// The model and options one output sends.
#[derive(Clone)]
pub(crate) struct ModelOptions {
    pub model: String,
    pub effort: ReasoningEffort,
    pub speed: Option<ChatgptSpeed>,
}

#[derive(Clone)]
pub(crate) struct ChatGptProvider {
    auth: Auth,
    responses: reqwest::Client,
    options: OutputOptions<ModelOptions>,
    source_language: String,
    output_limit: SizeLimit,
    api: String,
    admission: Admission<AccountKey>,
}

impl ChatGptProvider {
    pub fn new(
        options: OutputOptions<ModelOptions>,
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
                ModelOptions {
                    model,
                    effort: ReasoningEffort::Low,
                    speed: None,
                },
                None,
            ),
            source_language: "auto".into(),
            output_limit: crate::config::limits::TranslationLimits::default().output_kib,
            admission: Admission::default(),
        }
    }

    #[cfg(test)]
    pub fn with_options(mut self, options: OutputOptions<ModelOptions>) -> Self {
        self.options = options;
        self
    }

    async fn unchanged<T>(
        &self,
        key: &AccountKey,
        future: impl std::future::Future<Output = Result<T, Error>>,
    ) -> Result<T, Error> {
        tokio::pin!(future);
        let mut completed = None;
        let mut interval = tokio::time::interval(Duration::from_millis(100));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tokio::select! {
                result = &mut future, if completed.is_none() => completed = Some(result),
                _ = interval.tick() => (),
            }
            match self.auth.key() {
                Ok(current) if &current == key => {
                    if let Some(result) = completed.take() {
                        return result;
                    }
                }
                Err(error) if error.kind() == ErrorKind::Unavailable => (),
                Err(error) if error.kind() == ErrorKind::Storage => return Err(error),
                _ => return Err(account_changed()),
            }
        }
    }

    async fn request(
        &self,
        body: &str,
        target: &str,
        expected: &AccountKey,
        intent: Intent,
        output: Output,
    ) -> Result<Translated, Error> {
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
        let mut result = self
            .unchanged(key, async {
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
                    "input": [{"role": "user", "content": body}],
                    "reasoning": {"effort": options.effort},
                });
                if let Some(speed) = options.speed {
                    request["service_tier"] = json!(speed);
                }
                self.admission
                    .retrying(key, intent, async || {
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
                    .await
            })
            .await;
        self.admission.settle(key, intent, generation, &mut result);
        Ok(Translated {
            text: result?,
            context: Context::ChatGpt(access.key),
        })
    }
}

impl TranslationProvider for ChatGptProvider {
    fn check_admission(&self, context: &Context, generation: u64) -> Result<(), Error> {
        let Context::ChatGpt(key) = context else {
            return Ok(());
        };
        self.admission.check(key, generation)
    }

    fn context(&self) -> Result<Context, Error> {
        let key = self.auth.key()?;
        Ok(Context::ChatGpt(key))
    }

    fn admit(&self, context: &Context, intent: Intent) -> Result<u64, Error> {
        let Context::ChatGpt(key) = context else {
            return Err(account_changed());
        };
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
        context: Context,
    ) -> Result<Translated, Error> {
        let Context::ChatGpt(key) = context else {
            return Err(account_changed());
        };
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
mod tests {
    use super::test_support::Fixture;
    use super::*;

    #[tokio::test(flavor = "current_thread")]
    async fn invalid_store_aborts_inflight_work_with_its_original_diagnostic() {
        use super::{auth_store::AuthStore, http::Http};
        for permissions in [false, true] {
            if permissions && !cfg!(unix) {
                continue;
            }
            let fixture = Fixture::new().await;
            let directory = std::env::temp_dir().join(format!(
                "translation-store-failure-{}",
                uuid::Uuid::new_v4()
            ));
            let auth = Auth::synthetic(
                AuthStore::synthetic(directory.clone()),
                Http::synthetic_revocation(&fixture.base),
                fixture.base.clone(),
            );
            fixture.connected(&auth, 3600).await;
            let key = auth.key().unwrap();
            let provider = ChatGptProvider::synthetic(
                auth,
                format!("{}/v1", fixture.base),
                "synthetic-model".into(),
            );
            let operation = async {
                let path = directory.join("auth.json");
                if permissions {
                    #[cfg(unix)]
                    {
                        use std::os::unix::fs::PermissionsExt;
                        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o644))
                            .unwrap();
                    }
                } else {
                    std::fs::write(path, b"{}").unwrap();
                }
                std::future::pending::<Result<(), Error>>().await
            };
            let error =
                tokio::time::timeout(Duration::from_secs(1), provider.unchanged(&key, operation))
                    .await
                    .expect("Storage errors must release work without waiting for its deadline")
                    .unwrap_err();
            assert_eq!(error.kind(), ErrorKind::Storage);
            assert!(error.to_string().contains(if permissions {
                "private directory"
            } else {
                "storage is invalid"
            }));
            std::fs::remove_dir_all(directory).unwrap();
        }
    }

    fn response_calls(fixture: &Fixture) -> usize {
        fixture
            .state
            .lock()
            .unwrap()
            .records
            .iter()
            .filter(|record| record.path == "/v1/responses")
            .count()
    }

    #[tokio::test(flavor = "current_thread")]
    async fn configurable_output_accepts_large_escaped_text_and_checks_exact_byte_boundary() {
        let fixture = Fixture::new().await;
        let auth = fixture.auth();
        fixture.connected(&auth, 3600).await;
        let mut provider = ChatGptProvider::synthetic(
            auth,
            format!("{}/v1", fixture.base),
            "synthetic-model".into(),
        );
        for (kib, mode, extra) in [
            (1, "", 0),
            (1, "delta_compact", 0),
            (1, "", 1),
            (4096, "", 0),
            (4096, "delta_compact", 0),
            (4096, "", 1),
        ] {
            provider.output_limit = SizeLimit::try_from(kib).unwrap();
            let expected = format!("{}é", "\u{1}".repeat(kib * 1024 - 2 + extra));
            {
                let mut state = fixture.state.lock().unwrap();
                state.translation = Some(expected.clone());
                state.response_mode = mode.into();
            }
            let result = provider
                .translate(
                    crate::providers::Intent::Automatic,
                    crate::providers::Output::Range,
                    "Translate this.",
                    "ja",
                    provider.context().unwrap(),
                )
                .await;
            if extra == 0 {
                assert_eq!(result.unwrap().text, expected);
            } else {
                let error = result.err().unwrap();
                assert!(
                    error.to_string().contains("translation_limits.output_kib"),
                    "{}",
                    error.to_string()
                );
                assert!(error.to_string().contains(&(kib * 1024 + 1).to_string()));
            }
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn temporary_failures_retry_once_and_return_only_the_successful_response() {
        for (status, body) in [
            (
                200,
                json!({"type":"error", "code":"server_is_overloaded", "message":"Overloaded"}),
            ),
            (
                503,
                json!({"error":{"code":"server_is_overloaded", "message":"Overloaded"}}),
            ),
        ] {
            for permanent in [false, true] {
                let fixture = Fixture::new().await;
                {
                    let mut state = fixture.state.lock().unwrap();
                    state.retry_after = Some("0".into());
                    state.response_failures.push_back((status, body.clone()));
                    if permanent {
                        state.response_failures.push_back((status, body.clone()));
                    }
                    state.translation = Some("一時的な障害の後に再試行する。".into());
                }
                let auth = fixture.auth();
                let id = fixture.connected(&auth, 3600).await;
                let provider = ChatGptProvider::synthetic(
                    auth.clone(),
                    format!("{}/v1", fixture.base),
                    "synthetic-model".into(),
                );
                let result = provider
                    .translate(
                        crate::providers::Intent::Automatic,
                        crate::providers::Output::Range,
                        "Retry after a temporary failure.",
                        "ja",
                        provider.context().unwrap(),
                    )
                    .await;
                assert_eq!(response_calls(&fixture), 2);
                if permanent {
                    assert_eq!(result.err().unwrap().kind(), ErrorKind::Temporary);
                    assert_eq!(
                        provider
                            .admit(
                                &provider.context().unwrap(),
                                crate::providers::Intent::Automatic
                            )
                            .unwrap_err()
                            .kind(),
                        ErrorKind::Temporary
                    );
                } else {
                    assert_eq!(result.unwrap().text, "一時的な障害の後に再試行する。");
                    assert!(provider.context().is_ok());
                }
                assert!(
                    auth.store
                        .lock(false)
                        .await
                        .unwrap()
                        .unwrap()
                        .state
                        .registrations[&id]
                        .credentials
                        .is_some()
                );
            }
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn interrupted_stream_is_reported_without_sending_the_request_again() {
        for intent in [Intent::Automatic, Intent::Explicit] {
            let fixture = Fixture::new().await;
            {
                let mut state = fixture.state.lock().unwrap();
                state.response_mode = "delta_compact".into();
                // A wrongly repeated request would be answered at once with a translation.
                state.retry_after = Some("0".into());
                state.interrupted_streams = 1;
            }
            let auth = fixture.auth();
            fixture.connected(&auth, 3600).await;
            let provider = ChatGptProvider::synthetic(
                auth,
                format!("{}/v1", fixture.base),
                "synthetic-model".into(),
            );
            let error = provider
                .translate(
                    intent,
                    crate::providers::Output::Range,
                    "Read this.",
                    "ja",
                    provider.context().unwrap(),
                )
                .await
                .err()
                .unwrap();
            assert_eq!(error.kind(), ErrorKind::Temporary, "{error}");
            assert!(!error.retryable());
            assert!(
                error
                    .to_string()
                    .starts_with("OpenAI network request failed or timed out")
            );
            assert_eq!(response_calls(&fixture), 1);
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn retry_wait_is_cancellable_and_account_changes_stop_it() {
        for account_change in [false, true] {
            let fixture = Fixture::new().await;
            fixture.state.lock().unwrap().response_failures.push_back((
                200,
                json!({"type":"error", "code":"server_is_overloaded", "message":"Overloaded"}),
            ));
            let auth = fixture.auth();
            fixture.connected(&auth, 3600).await;
            let provider = ChatGptProvider::synthetic(
                auth.clone(),
                format!("{}/v1", fixture.base),
                "synthetic-model".into(),
            );
            let request = provider.translate(
                crate::providers::Intent::Automatic,
                crate::providers::Output::Range,
                "Read this.",
                "ja",
                provider.context().unwrap(),
            );
            tokio::pin!(request);
            assert!(
                tokio::time::timeout(Duration::from_millis(100), &mut request)
                    .await
                    .is_err()
            );
            assert_eq!(response_calls(&fixture), 1);
            if account_change {
                fixture.connected(&auth, 3600).await;
                let result = tokio::time::timeout(Duration::from_secs(1), &mut request)
                    .await
                    .unwrap();
                assert_eq!(result.err().unwrap().kind(), ErrorKind::Modified);
                assert_eq!(response_calls(&fixture), 1);
            }
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn server_retry_delay_is_honored_or_left_for_manual_retry() {
        for delay in [
            "1",
            "121",
            "18446744073709551615",
            "Wed, 21 Oct 2037 07:28:00 GMT",
        ] {
            let fixture = Fixture::new().await;
            {
                let mut state = fixture.state.lock().unwrap();
                state.retry_after = Some(delay.into());
                state
                    .response_failures
                    .push_back((503, json!({"detail":"Overloaded"})));
            }
            let auth = fixture.auth();
            fixture.connected(&auth, 3600).await;
            let provider = ChatGptProvider::synthetic(
                auth,
                format!("{}/v1", fixture.base),
                "synthetic-model".into(),
            );
            let started = Instant::now();
            let result = provider
                .translate(
                    crate::providers::Intent::Automatic,
                    crate::providers::Output::Range,
                    "Read this.",
                    "ja",
                    provider.context().unwrap(),
                )
                .await;
            if delay == "1" {
                assert!(result.is_ok());
                assert!(started.elapsed() >= Duration::from_secs(1));
                assert_eq!(response_calls(&fixture), 2);
            } else {
                assert_eq!(result.err().unwrap().kind(), ErrorKind::Temporary);
                assert_eq!(response_calls(&fixture), 1);
            }
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn complete_translation_is_returned_as_text_without_reassembly() {
        let source = "- Use named module files, such as `providers.rs` and `providers/chatgpt.rs`. Do not introduce `mod.rs` files.\r\n- Keep providers replaceable through `TranslationProvider`.";
        let translation = "- `providers.rs` や `providers/chatgpt.rs` のような名前付きモジュールファイルを使用してください。\n  `mod.rs` ファイルを導入しないでください。\n- `TranslationProvider` を通じてプロバイダーを交換可能にしてください。";
        for (source, translation) in [
            (source, translation),
            (
                "@brief Retry safely.\n@param count Retry count.",
                "@brief 安全に再試行します。\n@param count 再試行回数。",
            ),
            (
                "<summary>Retry safely.</summary>\n<param name=\"count\">Retry count.</param>",
                "<summary>安全に再試行します。</summary>\n<param name=\"count\">再試行回数。</param>",
            ),
        ] {
            let fixture = Fixture::new().await;
            fixture.state.lock().unwrap().translation = Some(translation.into());
            let auth = fixture.auth();
            fixture.connected(&auth, 3600).await;
            let provider = ChatGptProvider::synthetic(
                auth,
                format!("{}/v1", fixture.base),
                "synthetic-model".into(),
            );
            let result = provider
                .translate(
                    crate::providers::Intent::Automatic,
                    crate::providers::Output::Range,
                    source,
                    "ja",
                    provider.context().unwrap(),
                )
                .await;
            assert_eq!(
                result
                    .map(|translated| translated.text)
                    .map_err(|error| error.to_string()),
                Ok(translation.into())
            );
            let state = fixture.state.lock().unwrap();
            let record = state
                .records
                .iter()
                .find(|record| record.path == "/v1/responses")
                .unwrap();
            let request: Value = serde_json::from_str(&record.body).unwrap();
            assert_eq!(request["input"][0]["content"], source);
            assert!(request.get("text").is_none());
            assert!(
                request["instructions"]
                    .as_str()
                    .unwrap()
                    .contains("Return only the translation")
            );
            assert!(
                request["instructions"]
                    .as_str()
                    .unwrap()
                    .contains("Keep XML documentation tags and attribute values unchanged")
            );
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn stream_failures_keep_typed_diagnostics_without_dumping_payloads() {
        let source = "Private source sentence.";
        for (event, kind, unknown) in [
            (
                json!({"type":"error","code":"unsupported_value","param":"reasoning.effort",
                "message":format!("unsupported: {source} synthetic-access-0 synthetic-refresh-0")}),
                ErrorKind::Configuration,
                false,
            ),
            (
                json!({"type":"error","sequence_number":1,"error":{
                    "code":"unsupported_value","param":"reasoning.effort",
                    "message":format!("unsupported: {source} synthetic-access-0 synthetic-refresh-0")}}),
                ErrorKind::Configuration,
                false,
            ),
            (
                json!({"type":"error","sequence_number":1,"error":{
                    "code":"subscription_sharing_usage_limit_exceeded","message":"synthetic limit"}}),
                ErrorKind::UsageLimit,
                false,
            ),
            (
                json!({"type":"response.failed","response":{"error":{
                "code":"subscription_sharing_usage_limit_exceeded","message":"synthetic limit"}}}),
                ErrorKind::UsageLimit,
                false,
            ),
            (
                json!({"type":"response.failed","response":{"error":null,"private_payload":source}}),
                ErrorKind::Failed,
                true,
            ),
            (
                json!({"type":"error","sequence_number":1,"error":null}),
                ErrorKind::Failed,
                true,
            ),
            (
                json!({"type":"error","sequence_number":1,"error":{
                    "unexpected":source,"authorization_url":"https://auth.openai.com/authorize?id_token_hint=synthetic-secret"}}),
                ErrorKind::Failed,
                true,
            ),
            (
                json!({"type":"error","sequence_number":1,"error":
                    format!("{source} synthetic-secret https://auth.openai.com/authorize")}),
                ErrorKind::Failed,
                true,
            ),
            (
                json!({"type":"error","unexpected":{"message":source},
                "authorization_url":"https://auth.openai.com/authorize?id_token_hint=synthetic-secret",
                "https://auth.openai.com/private":"synthetic-secret"}),
                ErrorKind::Failed,
                true,
            ),
        ] {
            let fixture = Fixture::new().await;
            fixture.state.lock().unwrap().stream_error = Some(event.clone());
            let auth = fixture.auth();
            fixture.connected(&auth, 3600).await;
            let provider = ChatGptProvider::synthetic(
                auth,
                format!("{}/v1", fixture.base),
                "synthetic-model".into(),
            );
            let error = provider
                .translate(
                    crate::providers::Intent::Automatic,
                    crate::providers::Output::Range,
                    source,
                    "ja",
                    provider.context().unwrap(),
                )
                .await
                .err()
                .unwrap();
            assert_eq!(error.kind(), kind);
            if matches!(kind, ErrorKind::Configuration | ErrorKind::UsageLimit) {
                assert_eq!(
                    provider
                        .admit(
                            &provider.context().unwrap(),
                            crate::providers::Intent::Automatic
                        )
                        .unwrap_err()
                        .kind(),
                    kind
                );
            }
            assert!(
                error
                    .to_string()
                    .contains(&format!("event={}", event["type"].as_str().unwrap()))
            );
            assert!(error.to_string().contains("HTTP 200"));
            assert!(error.to_string().contains("request_id=req-synthetic"));
            assert_eq!(error.to_string().contains("event_keys="), unknown);
            for secret in [
                source,
                "synthetic-access-0",
                "synthetic-refresh-0",
                "synthetic-secret",
                "https://auth.openai.com/",
            ] {
                assert!(!error.to_string().contains(secret));
            }
            if !unknown {
                let fields = if event["type"] == "error" {
                    event.get("error").unwrap_or(&event)
                } else {
                    &event["response"]["error"]
                };
                assert!(error.to_string().contains(fields["code"].as_str().unwrap()));
                if fields.get("param").is_some() {
                    assert!(error.to_string().contains("param=reasoning.effort"));
                    assert!(error.to_string().contains("[redacted]"));
                }
            } else {
                assert!(error.to_string().contains("type"));
            }
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn real_http_requires_complete_consistent_streams() {
        for mode in [
            "",
            "crlf",
            "delta_compact",
            "done_compact",
            "text_done_compact",
            "delta_snapshot",
            "delta_without_item",
            "delta_mismatch",
            "empty_completed",
            "delta_after_completed",
            "compact_late_failed",
            "empty_text",
            "compact_output_limit",
            "compact_incomplete",
            "compact_truncated",
            "item_role",
            "item_status",
            "item_type",
            "item_content",
            "item_missing_id",
            "item_id_mismatch",
            "item_duplicate",
            "item_duplicate_id",
            "item_after_completed",
            "item_without_completed",
            "item_snapshot_mismatch",
            "item_snapshot_index_mismatch",
            "item_snapshot_id_mismatch",
            "item_text_order",
            "item_index_order",
            "item_output_limit",
            "item_total_limit",
            "text_done_mismatch",
            "text_done_duplicate",
            "delta_after_text_done",
            "delta_after_item_done",
            "output_limit",
            "late_failed",
            "after_complete_failed",
            "incomplete",
            "explicit_error",
            "disconnect",
            "invalid_json",
            "raw_limit",
            "stream_limit",
            "delta_limit",
            "trailing_partial",
        ] {
            let fixture = Fixture::new().await;
            fixture.state.lock().unwrap().response_mode = mode.into();
            let auth = fixture.auth();
            fixture.connected(&auth, 3600).await;
            let mut provider = ChatGptProvider::synthetic(
                auth,
                format!("{}/v1", fixture.base),
                "synthetic-model".into(),
            );
            provider.output_limit = SizeLimit::try_from(1).unwrap();
            let body = "Read `retry_count` and [the guide](https://example.invalid). 🚀\r\n\r\n```rust\r\nlet n = 1;\r\n```\r\n";
            let result = provider
                .translate(
                    crate::providers::Intent::Automatic,
                    crate::providers::Output::Range,
                    body,
                    "ja",
                    provider.context().unwrap(),
                )
                .await;
            assert_eq!(
                result.is_ok(),
                matches!(
                    mode,
                    "" | "crlf"
                        | "delta_compact"
                        | "delta_snapshot"
                        | "done_compact"
                        | "text_done_compact"
                        | "item_index_order"
                ),
                "SSE mode {mode}"
            );
            if matches!(
                mode,
                "late_failed" | "after_complete_failed" | "explicit_error" | "compact_late_failed"
            ) {
                assert!(
                    result
                        .as_ref()
                        .err()
                        .unwrap()
                        .to_string()
                        .contains("HTTP 200")
                );
            }
            if let Ok(translated) = result {
                for code in [
                    "`retry_count`",
                    "https://example.invalid",
                    "```rust\r\nlet n = 1;\r\n```\r\n",
                ] {
                    assert!(translated.text.contains(code));
                }
                assert!(translated.text.contains("訳Read"));
            }
            let state = fixture.state.lock().unwrap();
            assert_eq!(
                state
                    .records
                    .iter()
                    .filter(|record| record.path == "/v1/responses")
                    .count(),
                1
            );
            assert!(
                state
                    .records
                    .iter()
                    .filter(|record| record.path.starts_with("/v1/"))
                    .all(|record| record.authorization == "Bearer synthetic-access-0")
            );
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn terminal_plan_errors_do_not_retry_or_clear_credentials_and_pause_new_requests() {
        for (status, body, kind) in [
            (
                429,
                json!({"error":{"code":"subscription_sharing_usage_limit_exceeded","message":"limit"}}),
                ErrorKind::UsageLimit,
            ),
            (
                403,
                json!({"error":{"code":"subscription_sharing_user_not_eligible","message":"policy"}}),
                ErrorKind::NotEligible,
            ),
            (
                400,
                json!({"error":{"code":"unsupported_value","param":"reasoning.effort","message":"unsupported effort"}}),
                ErrorKind::Configuration,
            ),
        ] {
            let fixture = Fixture::new().await;
            fixture.state.lock().unwrap().failure = Some((status, body));
            let auth = fixture.auth();
            let id = fixture.connected(&auth, 3600).await;
            let provider = ChatGptProvider::synthetic(
                auth.clone(),
                format!("{}/v1", fixture.base),
                "synthetic-model".into(),
            );
            let error = provider
                .translate(
                    crate::providers::Intent::Automatic,
                    crate::providers::Output::Range,
                    "Translate this.",
                    "ja",
                    provider.context().unwrap(),
                )
                .await
                .err()
                .unwrap();
            assert_eq!(error.kind(), kind);
            let suppressed = provider
                .check_admission(&provider.context().unwrap(), 0)
                .unwrap_err();
            assert_eq!(suppressed.kind(), kind);
            assert_eq!(
                suppressed.notification(),
                crate::providers::error::Notification::Suppress
            );
            assert_eq!(suppressed.to_string(), error.to_string());
            assert!(error.to_string().contains("req-synthetic"));
            assert!(
                error
                    .to_string()
                    .contains("run a selection or document translation to retry")
            );
            assert!(
                provider
                    .admit(
                        &provider.context().unwrap(),
                        crate::providers::Intent::Automatic
                    )
                    .is_err()
            );
            assert_eq!(
                fixture
                    .state
                    .lock()
                    .unwrap()
                    .records
                    .iter()
                    .filter(|record| record.path == "/v1/responses")
                    .count(),
                1
            );
            assert!(
                auth.store.snapshot().unwrap().unwrap().state.registrations[&id]
                    .credentials
                    .is_some()
            );
            let context = provider.context().unwrap();
            assert_eq!(
                provider
                    .admit(&context, crate::providers::Intent::Automatic)
                    .unwrap_err()
                    .kind(),
                kind
            );
            fixture.state.lock().unwrap().failure = None;
            provider
                .admit(&context, crate::providers::Intent::Explicit)
                .unwrap();
            assert_eq!(
                provider
                    .admit(&context, crate::providers::Intent::Automatic)
                    .unwrap_err()
                    .kind(),
                kind
            );
            let translated = provider
                .translate(
                    Intent::Explicit,
                    Output::Range,
                    "Try again.",
                    "ja",
                    context.clone(),
                )
                .await
                .unwrap();
            assert_eq!(translated.context, context);
            assert_eq!(response_calls(&fixture), 2);
            provider
                .admit(&context, crate::providers::Intent::Automatic)
                .unwrap();
        }
    }

    /// Signing in gives an account a new key every time, and a request is given
    /// up once its account is no longer the active one. One account's failure
    /// can follow another's stop only when the store names a saved account
    /// again, which this server never writes.
    #[tokio::test(flavor = "current_thread")]
    async fn failure_of_one_account_keeps_the_stop_of_another() {
        let limit = (
            429,
            json!({"error":{"code":"subscription_sharing_usage_limit_exceeded","message":"limit"}}),
        );
        for (failure, kind) in [
            ((503, json!({"detail":"Overloaded"})), ErrorKind::Temporary),
            (limit.clone(), ErrorKind::UsageLimit),
        ] {
            let fixture = Fixture::new().await;
            // A temporary failure is not sent again, so it ends the request.
            fixture.state.lock().unwrap().retry_after = Some("121".into());
            let auth = fixture.auth();
            let activate = async |id| {
                let mut locked = auth.store.lock(true).await.unwrap().unwrap();
                locked.state.active_registration = Some(id);
                locked.save().unwrap();
            };
            let other = fixture.connected(&auth, 3600).await;
            let stopped = fixture.connected(&auth, 3600).await;
            let provider = ChatGptProvider::synthetic(
                auth.clone(),
                format!("{}/v1", fixture.base),
                "synthetic-model".into(),
            );
            let automatic = async || {
                provider
                    .translate(
                        Intent::Automatic,
                        Output::Range,
                        "Read this.",
                        "ja",
                        provider.context().unwrap(),
                    )
                    .await
            };
            let context = provider.context().unwrap();
            let admitted = provider.admit(&context, Intent::Automatic).unwrap();
            fixture
                .state
                .lock()
                .unwrap()
                .response_failures
                .push_back(limit.clone());
            let stop = automatic().await.err().unwrap();
            assert_eq!(stop.kind(), ErrorKind::UsageLimit);

            activate(other).await;
            assert_ne!(provider.context().unwrap(), context);
            fixture
                .state
                .lock()
                .unwrap()
                .response_failures
                .push_back(failure);
            let error = automatic().await.err().unwrap();
            assert_eq!(error.kind(), kind);
            assert_eq!(
                error.notification(),
                crate::providers::error::Notification::Show
            );
            assert_eq!(response_calls(&fixture), 2);

            activate(stopped).await;
            assert_eq!(provider.context().unwrap(), context);
            for refused in [
                provider.admit(&context, Intent::Automatic).unwrap_err(),
                automatic().await.err().unwrap(),
                provider.check_admission(&context, admitted).unwrap_err(),
            ] {
                assert_eq!(refused.to_string(), stop.to_string());
            }
            assert_eq!(response_calls(&fixture), 2);
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn configured_model_is_sent_without_fallback() {
        for (model, supported) in [
            ("gpt-6.1-sol", true),
            ("gpt-6-luna", true),
            ("unsupported-model", false),
        ] {
            let fixture = Fixture::new().await;
            if supported {
                fixture.state.lock().unwrap().response_mode = "public_compact".into();
            } else {
                fixture.state.lock().unwrap().failure = Some((
                    400,
                    json!({"error":{
                        "code":"model_not_found","param":"model","message":"The configured model is unsupported"
                    }}),
                ));
            }
            let auth = fixture.auth();
            fixture.connected(&auth, 3600).await;
            let provider =
                ChatGptProvider::synthetic(auth, format!("{}/v1", fixture.base), model.into());
            let context = provider.context().unwrap();
            let result = provider
                .translate(
                    crate::providers::Intent::Automatic,
                    crate::providers::Output::Range,
                    "Retry only when the remote service reports a transient failure.",
                    "ja",
                    context.clone(),
                )
                .await;
            assert_eq!(result.is_ok(), supported);
            if supported {
                assert_eq!(
                    result.unwrap().text,
                    if model == "gpt-6.1-sol" {
                        "リモートサービスが一時的な障害を報告した場合にのみ再試行してください。"
                    } else {
                        "リモートサービスが一時的な障害を報告した場合に限り、再試行してください。"
                    }
                );
            } else {
                assert_eq!(result.err().unwrap().kind(), ErrorKind::Configuration);
                assert!(
                    provider
                        .admit(
                            &provider.context().unwrap(),
                            crate::providers::Intent::Automatic
                        )
                        .is_err()
                );
                assert_eq!(
                    provider
                        .translate(
                            crate::providers::Intent::Automatic,
                            crate::providers::Output::Range,
                            "Translate this.",
                            "ja",
                            context
                        )
                        .await
                        .err()
                        .unwrap()
                        .kind(),
                    ErrorKind::Configuration
                );
            }
            let state = fixture.state.lock().unwrap();
            assert!(
                state
                    .records
                    .iter()
                    .all(|record| record.path != "/v1/models")
            );
            let requests: Vec<_> = state
                .records
                .iter()
                .filter(|record| record.path == "/v1/responses")
                .collect();
            assert_eq!(requests.len(), 1);
            let body: Value = serde_json::from_str(&requests[0].body).unwrap();
            assert_eq!(body["model"], model);
            assert_eq!(body["reasoning"]["effort"], "low");
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn inference_redirects_are_not_followed_and_plain_errors_keep_detail() {
        for (status, body) in [
            (307, json!({"detail":"redirect must not be followed"})),
            (503, json!("plain unavailable detail")),
        ] {
            let fixture = Fixture::new().await;
            fixture.state.lock().unwrap().failure = Some((status, body));
            let auth = fixture.auth();
            fixture.connected(&auth, 3600).await;
            let provider = ChatGptProvider::synthetic(
                auth,
                format!("{}/v1", fixture.base),
                "synthetic-model".into(),
            );
            let result = provider
                .translate(
                    crate::providers::Intent::Automatic,
                    crate::providers::Output::Range,
                    "Translate this.",
                    "ja",
                    provider.context().unwrap(),
                )
                .await;
            let error = result.err().unwrap();
            assert!(error.to_string().contains(&status.to_string()));
            assert!(error.to_string().contains("req-synthetic"));
            assert!(error.to_string().contains(if status == 307 {
                "redirect"
            } else {
                "plain unavailable detail"
            }));
            assert_eq!(
                fixture
                    .state
                    .lock()
                    .unwrap()
                    .records
                    .iter()
                    .filter(|record| record.path == "/v1/responses")
                    .count(),
                if status == 503 { 2 } else { 1 }
            );
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn responses_can_outlast_the_common_http_timeout_without_changing_oauth() {
        let fixture = Fixture::new().await;
        let mut auth = fixture.auth();
        fixture.connected(&auth, 3600).await;
        // Scale only the common client's limit down for this local fixture.
        auth.http.client = reqwest::Client::builder()
            .timeout(Duration::from_millis(80))
            .build()
            .unwrap();
        let provider = ChatGptProvider::synthetic(
            auth.clone(),
            format!("{}/v1", fixture.base),
            "synthetic-model".into(),
        );
        fixture.state.lock().unwrap().delay = Duration::from_millis(180);
        // The same slow endpoint times out through the common client.
        let error = auth
            .http
            .client
            .post(format!("{}/token", fixture.base))
            .body(concat!(
                "grant_type=refresh_token&resource=https%3A%2F%2Fapi.openai.com%2Fv1",
                "&client_id=oaiapp_synthetic&refresh_token=synthetic-refresh-0",
            ))
            .send()
            .await
            .unwrap_err();
        assert!(error.is_timeout());
        // Responses uses its own client; the worker will bound the total operation.
        let translated = provider
            .translate(
                crate::providers::Intent::Automatic,
                crate::providers::Output::Range,
                "Translate this.",
                "ja",
                provider.context().unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(translated.text, "訳Translate this.");
        assert_eq!(
            fixture
                .state
                .lock()
                .unwrap()
                .records
                .iter()
                .filter(|r| r.path == "/v1/responses")
                .count(),
            1
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn manual_selections_reach_http_and_account_changes_drop_http_future() {
        let fixture = Fixture::new().await;
        let auth = fixture.auth();
        fixture.connected(&auth, 3600).await;
        let provider = ChatGptProvider::synthetic(
            auth.clone(),
            format!("{}/v1", fixture.base),
            "synthetic-model".into(),
        );
        for body in [
            "`code`",
            "    Example prose.",
            "    // Translate this comment.",
        ] {
            let translated = provider
                .translate(
                    crate::providers::Intent::Automatic,
                    crate::providers::Output::Range,
                    body,
                    "ja",
                    provider.context().unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(translated.text, format!("訳{body}"));
            let mut state = fixture.state.lock().unwrap();
            let requests: Vec<_> = state
                .records
                .iter()
                .filter(|record| record.path == "/v1/responses")
                .collect();
            assert_eq!(requests.len(), 1);
            let request: Value = serde_json::from_str(&requests[0].body).unwrap();
            assert_eq!(request["input"][0]["content"], body);
            state.records.clear();
        }
        fixture.state.lock().unwrap().delay = Duration::from_secs(5);
        let key = provider.context().unwrap();
        let translation = provider.translate(
            crate::providers::Intent::Automatic,
            crate::providers::Output::Range,
            "Translate this.",
            "ja",
            key,
        );
        let switch = async {
            tokio::time::sleep(Duration::from_millis(150)).await;
            fixture.connected(&auth, 3600).await;
        };
        let (result, ()) = tokio::join!(translation, switch);
        assert_eq!(result.err().unwrap().kind(), ErrorKind::Modified);
        assert_eq!(
            fixture
                .state
                .lock()
                .unwrap()
                .records
                .iter()
                .filter(|record| record.path == "/v1/responses")
                .count(),
            1
        );
    }
}
