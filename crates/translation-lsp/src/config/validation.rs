//! Checks of the settings beyond what decoding checks.
//!
//! Decoding reports every mismatch as one generic error, because the errors of
//! `serde` can repeat the rejected value, and a value can hold a secret. The
//! checks on the raw JSON run before decoding, to name the setting at fault
//! without its value. The checks on the decoded settings cover ranges, text,
//! and combinations.

use serde::{
    Deserialize,
    de::{self, DeserializeOwned},
};
use serde_json::Value;

use super::{
    AnthropicEffort, AnthropicSpeed, ChatgptSpeed, ProviderSettings, ReasoningEffort, Settings,
    error::{ConfigError, bounded_number},
    features::Feature,
};
use crate::language::Language;

pub(super) fn before_decoding(options: &Value) -> Result<(), ConfigError> {
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
    }
    Ok(())
}

/// Names the one field that `provider.document` requires.
fn document_model(options: &Value) -> Result<(), ConfigError> {
    let provider = &options["provider"];
    if (provider["name"] == "chatgpt" || provider["name"] == "anthropic")
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
        Some("chatgpt") => {
            EFFORT.check::<ReasoningEffort>(provider, ReasoningEffort::CHOICES)?;
            SPEED.check::<ChatgptSpeed>(provider, ChatgptSpeed::CHOICES)
        }
        Some("anthropic") => {
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
        return Err(ConfigError::ModelWhitespace { setting });
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
