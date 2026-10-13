mod key;
mod response;
#[cfg(test)]
pub(crate) mod test_support;

use std::path::PathBuf;

use key::ApiKey;
use serde_json::json;

use crate::config::{AnthropicEffort, limits::SizeLimit};

use super::{
    Capabilities, Context, Intent, Output, OutputOptions, Translated, TranslationProvider,
    admission::{Admission, Attempt},
    error::{Error, Service, credentials::CredentialError},
    prompt, transport,
};

const API: &str = "https://api.anthropic.com/v1";
const VERSION: &str = "2023-06-01";
const MAX_OUTPUT_TOKENS: usize = 64_000;
const SERVICE: Service = Service::Anthropic;

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
    // Read once at startup. A key that could not be read is reported at admission, not here.
    key: Result<ApiKey, Error>,
    options: OutputOptions<ModelOptions>,
    source_language: String,
    output_limit: SizeLimit,
    api: String,
    admission: Admission<()>,
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
            key: ApiKey::read(key_directory),
            options,
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
            key: ApiKey::synthetic(key),
            options: OutputOptions::new(
                ModelOptions {
                    model: "claude-haiku-5-5".into(),
                    effort: AnthropicEffort::Low,
                    thinking: false,
                },
                None,
            ),
            source_language: "auto".into(),
            output_limit: crate::config::limits::TranslationLimits::default().output_kib,
            api,
            admission: Admission::default(),
        }
    }

    #[cfg(test)]
    pub fn with_key_directory(mut self, directory: &std::path::Path) -> Self {
        self.key = ApiKey::read(Ok(directory.into()));
        self
    }

    #[cfg(test)]
    pub fn with_options(mut self, options: OutputOptions<ModelOptions>) -> Self {
        self.options = options;
        self
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
        output: Output,
        text: &str,
        target_language: &str,
        context: Context,
    ) -> Result<Translated, Error> {
        expect_context(&context)?;
        let key = self.key()?;
        let generation = self.admission.admit(&(), intent)?;
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
        self.admission.settle(&(), intent, generation, &mut result);
        Ok(Translated {
            text: result?,
            context: Context::Anthropic,
        })
    }
}

#[cfg(test)]
mod tests;
