mod response;
#[cfg(test)]
pub(crate) mod test_support;

use std::sync::Arc;

use reqwest::header::HeaderValue;
use serde_json::json;

use crate::config::{AnthropicEffort, limits::SizeLimit};

use super::{
    Capabilities, Context, Intent, Translated, TranslationProvider,
    admission::{Admission, Attempt},
    error::{Error, Service},
    prompt, transport,
};

// `ANTHROPIC_API_KEY` is deliberately not read: Claude Code prefers that name
// over a subscription login, so exporting it for this server would change
// where the user's other Claude usage is billed.
pub(crate) const API_KEY_VARIABLE: &str = "TRANSLATION_LSP_ANTHROPIC_API_KEY";
const API: &str = "https://api.anthropic.com/v1";
const VERSION: &str = "2023-06-01";
const MAX_OUTPUT_TOKENS: usize = 64_000;
const SERVICE: Service = Service::Anthropic;

#[derive(Clone)]
struct ApiKey {
    header: HeaderValue,
    text: Arc<str>,
}

impl ApiKey {
    fn parse(value: Option<String>) -> Result<Self, Error> {
        let value = value.unwrap_or_default();
        let text = value.trim();
        if text.is_empty() {
            return Err(Error::configuration(concat!(
                "Anthropic API key is not set. Add TRANSLATION_LSP_ANTHROPIC_API_KEY to ",
                "lsp.translation-lsp.binary.env in Zed settings and restart the language server"
            )));
        }
        let header = HeaderValue::from_str(text)
            .ok()
            .filter(|_| text.bytes().all(|byte| byte.is_ascii_graphic()));
        let Some(mut header) = header else {
            return Err(Error::configuration(concat!(
                "TRANSLATION_LSP_ANTHROPIC_API_KEY contains whitespace or non-ASCII characters. ",
                "Correct it in lsp.translation-lsp.binary.env and restart the language server"
            )));
        };
        header.set_sensitive(true);
        Ok(Self {
            header,
            text: text.into(),
        })
    }
}

impl std::fmt::Debug for ApiKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ApiKey([redacted])")
    }
}

#[derive(Clone)]
pub(crate) struct AnthropicProvider {
    client: reqwest::Client,
    // Read once at startup. A missing key is reported at admission, not here.
    key: Result<ApiKey, Error>,
    model: String,
    effort: AnthropicEffort,
    thinking: bool,
    source_language: String,
    output_limit: SizeLimit,
    api: String,
    admission: Admission<()>,
}

impl AnthropicProvider {
    pub fn new(
        model: String,
        effort: AnthropicEffort,
        thinking: bool,
        source_language: String,
        output_limit: SizeLimit,
    ) -> Result<Self, Error> {
        Ok(Self {
            // The worker owns the total deadline.
            client: transport::client_builder()
                .build()
                .map_err(|_| Error::failed("Could not prepare the Anthropic HTTP client"))?,
            key: ApiKey::parse(std::env::var(API_KEY_VARIABLE).ok()),
            model,
            effort,
            thinking,
            source_language,
            output_limit,
            api: API.into(),
            admission: Admission::default(),
        })
    }

    #[cfg(test)]
    pub fn synthetic(api: String, key: Option<&str>) -> Self {
        Self {
            client: transport::client_builder().build().unwrap(),
            key: ApiKey::parse(key.map(str::to_owned)),
            model: "claude-haiku-5-5".into(),
            effort: AnthropicEffort::Low,
            thinking: false,
            source_language: "auto".into(),
            output_limit: crate::config::limits::TranslationLimits::default().output_kib,
            api,
            admission: Admission::default(),
        }
    }

    fn key(&self) -> Result<&ApiKey, Error> {
        self.key.as_ref().map_err(Error::clone)
    }
}

fn expect_context(context: &Context) -> Result<(), Error> {
    if context == &Context::Anthropic {
        Ok(())
    } else {
        Err(Error::failed(
            "Translation was prepared for a different provider",
        ))
    }
}

impl TranslationProvider for AnthropicProvider {
    fn context(&self) -> Result<Context, Error> {
        Ok(Context::Anthropic)
    }

    fn admit(&self, context: &Context, intent: Intent) -> Result<u64, Error> {
        expect_context(context)?;
        self.key()?;
        self.admission.admit(&(), intent)
    }

    fn check_admission(&self, context: &Context, generation: u64) -> Result<(), Error> {
        if context != &Context::Anthropic {
            return Ok(());
        }
        self.admission.check(&(), generation)
    }

    fn capabilities() -> Capabilities {
        Capabilities {
            server_name: "Translation LSP (Anthropic)",
            automatic_selection: true,
            account_menu: None,
            commands: &[],
        }
    }

    async fn translate(
        &self,
        intent: Intent,
        text: &str,
        target_language: &str,
        context: Context,
    ) -> Result<Translated, Error> {
        expect_context(&context)?;
        let key = self.key()?;
        let generation = self.admission.admit(&(), intent)?;
        let secrets = [&*key.text, text];
        let request = json!({
            "model": self.model,
            "max_tokens": MAX_OUTPUT_TOKENS.min(self.output_limit.bytes()),
            "stream": true,
            "system": prompt::instructions(target_language, &self.source_language),
            "messages": [{"role": "user", "content": text}],
            "output_config": {"effort": self.effort},
            "thinking": {"type": if self.thinking { "adaptive" } else { "disabled" }},
        });
        // Dropping this future on cancellation or expiry drops the request or
        // response it is awaiting, which closes the connection.
        let mut result = self
            .admission
            .retrying(&(), intent, async || {
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
                    response::text(response, &secrets, self.output_limit).await
                } else {
                    Err(response::failure(response, &secrets).await)
                };
                Attempt {
                    result,
                    retry_after,
                }
            })
            .await;
        self.admission.settle(&(), intent, generation, &mut result);
        Ok(Translated {
            text: result?,
            context: Context::Anthropic,
        })
    }
}

#[cfg(test)]
mod tests;
