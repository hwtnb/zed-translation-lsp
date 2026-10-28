//! Checks of the settings beyond what decoding checks.
//!
//! Decoding reports every mismatch as one generic error, because the errors of
//! `serde` can repeat the rejected value, and a value can hold a secret. The
//! checks on the raw JSON run before decoding, to name the setting at fault
//! without its value. The checks on the decoded settings cover ranges, text,
//! and combinations.

use reqwest::Url;
use serde::{
    Deserialize,
    de::{self, DeserializeOwned},
};
use serde_json::Value;

use super::{
    AnthropicEffort, AnthropicSpeed, OpenAiSpeed, ProviderSettings, ReasoningEffort, Settings,
    error::{ConfigError, bounded_number},
    features::Feature,
};
use crate::{language::Language, providers::api_key::Origin};

pub(super) fn before_decoding(options: &Value) -> Result<(), ConfigError> {
    compatible_service(options)?;
    document_model(options)?;
    choices(options)?;
    feature_languages(options)?;
    reject_arrays(options)
}

pub(super) fn after_decoding(settings: &Settings) -> Result<(), ConfigError> {
    settings.translation_limits.validate()?;
    for (name, value, minimum, maximum) in [
        (
            "hover_timeout_ms",
            settings.hover_timeout_ms,
            1_000,
            120_000,
        ),
        (
            "selection_timeout_ms",
            settings.selection_timeout_ms,
            1_000,
            120_000,
        ),
        (
            "document_timeout_ms",
            settings.document_timeout_ms,
            1_000,
            600_000,
        ),
        (
            "selection_debounce_ms",
            settings.selection_debounce_ms,
            100,
            2_000,
        ),
        (
            "comment_parse_timeout_ms",
            settings.comment_parse_timeout_ms,
            100,
            60_000,
        ),
    ] {
        bounded_number(name, value, minimum, maximum)?;
    }
    for (setting, value) in [
        ("source_language", &settings.source_language),
        ("target_language", &settings.target_language),
    ] {
        if !bounded(value, 128) {
            return Err(ConfigError::Text {
                setting,
                maximum_bytes: 128,
            });
        }
    }
    match &settings.provider {
        ProviderSettings::Unset | ProviderSettings::Mock {} => (),
        ProviderSettings::Chatgpt {
            model, document, ..
        }
        | ProviderSettings::OpenAiApi {
            model, document, ..
        } => {
            check_model("provider.model", model)?;
            if let Some(document) = document {
                check_model("provider.document.model", &document.model)?;
            }
        }
        ProviderSettings::Anthropic {
            model,
            effort,
            thinking,
            document,
            ..
        } => {
            check_model("provider.model", model)?;
            check_thinking("provider", *effort, *thinking)?;
            if let Some(document) = document {
                check_model("provider.document.model", &document.model)?;
                check_thinking("provider.document", document.effort, document.thinking)?;
            }
        }
        ProviderSettings::OpenAiCompatible {
            base_url,
            model,
            effort,
            document,
        } => {
            check_base_url(base_url)?;
            check_model("provider.model", model)?;
            check_effort("provider.effort", effort.as_deref())?;
            if let Some(document) = document {
                check_model("provider.document.model", &document.model)?;
                check_effort("provider.document.effort", document.effort.as_deref())?;
            }
        }
    }
    Ok(())
}

/// Names the fields that a service of `provider.base_url` requires: it has no
/// model of its own to fall back on.
fn compatible_service(options: &Value) -> Result<(), ConfigError> {
    let provider = &options["provider"];
    if provider["name"] != "openai_compatible" {
        return Ok(());
    }
    for (field, setting) in [
        ("base_url", "provider.base_url"),
        ("model", "provider.model"),
    ] {
        if provider.get(field).is_none() {
            return Err(ConfigError::Required { setting });
        }
    }
    Ok(())
}

/// Names the one field that `provider.document` requires.
fn document_model(options: &Value) -> Result<(), ConfigError> {
    let provider = &options["provider"];
    if [
        "chatgpt",
        "anthropic_api",
        "anthropic",
        "openai_api",
        "openai_compatible",
    ]
    .iter()
    .any(|name| provider["name"] == *name)
        && let Value::Object(document) = &provider["document"]
        && !document.contains_key("model")
    {
        return Err(ConfigError::Required {
            setting: "provider.document.model",
        });
    }
    Ok(())
}

/// Names the setting whose value is a string outside its list: the providers
/// take different values under the same names, such as `none` for ChatGPT
/// effort and `standard` for Anthropic speed.
fn choices(options: &Value) -> Result<(), ConfigError> {
    let provider = &options["provider"];
    match provider["name"].as_str() {
        Some("chatgpt" | "openai_api") => {
            EFFORT.check::<ReasoningEffort>(provider, ReasoningEffort::CHOICES)?;
            SPEED.check::<OpenAiSpeed>(provider, OpenAiSpeed::CHOICES)
        }
        Some("anthropic_api" | "anthropic") => {
            EFFORT.check::<AnthropicEffort>(provider, AnthropicEffort::CHOICES)?;
            SPEED.check::<AnthropicSpeed>(provider, AnthropicSpeed::CHOICES)
        }
        _ => Ok(()),
    }
}

/// Names a language ID outside the registered ones, and a feature written for
/// a language it does not run in.
fn feature_languages(options: &Value) -> Result<(), ConfigError> {
    let Value::Object(languages) = &options["features"]["languages"] else {
        return Ok(());
    };
    for (language_id, switches) in languages {
        let Some(language) = Language::from_id(language_id) else {
            return Err(ConfigError::FeatureLanguage);
        };
        let Value::Object(switches) = switches else {
            continue;
        };
        for name in switches.keys() {
            if let Ok(feature) = Feature::deserialize(Value::String(name.clone()))
                && !feature.applies_to(language)
            {
                return Err(ConfigError::FeatureUnavailable {
                    language: language.id(),
                    feature,
                });
            }
        }
    }
    Ok(())
}

/// A field of both `provider` and `provider.document`.
struct Choice {
    field: &'static str,
    settings: [&'static str; 2],
    named_null: bool,
}

const EFFORT: Choice = Choice {
    field: "effort",
    settings: ["provider.effort", "provider.document.effort"],
    named_null: false,
};

/// A `null` speed may be meant as no speed, which is written by omitting it.
const SPEED: Choice = Choice {
    field: "speed",
    settings: ["provider.speed", "provider.document.speed"],
    named_null: true,
};

impl Choice {
    fn check<T: DeserializeOwned>(
        &self,
        provider: &Value,
        choices: &'static str,
    ) -> Result<(), ConfigError> {
        let document = provider
            .get("document")
            .and_then(|document| document.get(self.field));
        for (value, setting) in [provider.get(self.field), document]
            .into_iter()
            .zip(self.settings)
        {
            let rejected = match value {
                Some(value @ Value::String(_)) => T::deserialize(value).is_err(),
                Some(Value::Null) => self.named_null,
                _ => false,
            };
            if rejected {
                return Err(ConfigError::Choice { setting, choices });
            }
        }
        Ok(())
    }
}

/// No setting takes an array. Derived decoding would read one as the fields of
/// a table in declaration order, which would turn that order into a settings
/// format. Reject arrays before decoding, wherever they appear.
fn reject_arrays(options: &Value) -> Result<(), ConfigError> {
    match options {
        Value::Array(_) => Err(ConfigError::Decode(de::Error::custom(
            "settings do not take arrays",
        ))),
        Value::Object(fields) => fields.values().try_for_each(reject_arrays),
        _ => Ok(()),
    }
}

fn check_model(setting: &'static str, model: &str) -> Result<(), ConfigError> {
    if !bounded(model, 256) {
        return Err(ConfigError::Text {
            setting,
            maximum_bytes: 256,
        });
    }
    if model.chars().any(char::is_whitespace) {
        return Err(ConfigError::Whitespace { setting });
    }
    Ok(())
}

/// The URL that requests go to with `/chat/completions` after it. Texts and
/// keys cross a network only encrypted. A query or fragment would be lost when
/// the path is extended, and a user name or password would be sent with every
/// request.
fn check_base_url(base_url: &str) -> Result<(), ConfigError> {
    if !bounded(base_url, 2048) {
        return Err(ConfigError::Text {
            setting: "provider.base_url",
            maximum_bytes: 2048,
        });
    }
    let accepted = Url::parse(base_url).is_ok_and(|url| {
        Origin::of(&url).is_some()
            && url.username().is_empty()
            && url.password().is_none()
            && url.query().is_none()
            && url.fragment().is_none()
    });
    if !accepted {
        return Err(ConfigError::BaseUrl);
    }
    Ok(())
}

/// Each service takes reasoning efforts of its own, so only the form is checked.
fn check_effort(setting: &'static str, effort: Option<&str>) -> Result<(), ConfigError> {
    let Some(effort) = effort else {
        return Ok(());
    };
    if !bounded(effort, 64) {
        return Err(ConfigError::Text {
            setting,
            maximum_bytes: 64,
        });
    }
    if effort.chars().any(char::is_whitespace) {
        return Err(ConfigError::Whitespace { setting });
    }
    Ok(())
}

fn check_thinking(
    scope: &'static str,
    effort: AnthropicEffort,
    thinking: Option<bool>,
) -> Result<(), ConfigError> {
    if thinking == Some(false)
        && let Some(effort) = effort.requires_thinking()
    {
        return Err(ConfigError::ThinkingRequired { scope, effort });
    }
    Ok(())
}

fn bounded(value: &str, maximum: usize) -> bool {
    !value.trim().is_empty() && value.len() <= maximum && !value.chars().any(char::is_control)
}
