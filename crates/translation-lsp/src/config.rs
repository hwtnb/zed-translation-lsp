pub(crate) mod error;
pub(crate) mod features;
pub(crate) mod limits;

use error::{ConfigError, bounded_number};

use features::Features;
use limits::TranslationLimits;
use serde::Deserialize;
use serde_json::Value;

#[derive(Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct Settings {
    pub translation_limits: TranslationLimits,
    pub target_language: String,
    pub source_language: String,
    pub provider: ProviderSettings,
    pub hover_timeout_ms: u64,
    pub selection_timeout_ms: u64,
    pub document_timeout_ms: u64,
    pub selection_debounce_ms: u64,
    pub comment_parse_timeout_ms: u64,
    pub features: Features,
}

#[derive(Clone, Deserialize)]
#[serde(tag = "name", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum ProviderSettings {
    Mock {},
    Chatgpt {
        #[serde(default = "default_chatgpt_model")]
        model: String,
        #[serde(default)]
        effort: ReasoningEffort,
        #[serde(default, deserialize_with = "explicit")]
        speed: Option<ChatgptSpeed>,
        #[serde(default, deserialize_with = "explicit")]
        document: Option<ChatgptDocument>,
    },
    Anthropic {
        #[serde(default = "default_anthropic_model")]
        model: String,
        #[serde(default)]
        effort: AnthropicEffort,
        // `None` when omitted. `anthropic_thinking` chooses the value to send.
        #[serde(default, deserialize_with = "explicit")]
        thinking: Option<bool>,
        #[serde(default, deserialize_with = "explicit")]
        speed: Option<AnthropicSpeed>,
        #[serde(default, deserialize_with = "explicit")]
        document: Option<AnthropicDocument>,
    },
}

// `provider.document`: the model and options for translations opened in a
// tab. An omitted field takes its own default here and inherits nothing from
// the surrounding provider settings.
#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ChatgptDocument {
    pub model: String,
    #[serde(default)]
    pub effort: ReasoningEffort,
    #[serde(default, deserialize_with = "explicit")]
    pub speed: Option<ChatgptSpeed>,
}

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct AnthropicDocument {
    pub model: String,
    #[serde(default)]
    pub effort: AnthropicEffort,
    #[serde(default, deserialize_with = "explicit")]
    pub thinking: Option<bool>,
    #[serde(default, deserialize_with = "explicit")]
    pub speed: Option<AnthropicSpeed>,
}

impl ProviderSettings {
    // Tab translations run apart from range translations exactly when
    // `provider.document` is present, whatever its values are.
    pub fn separates_documents(&self) -> bool {
        match self {
            Self::Mock {} => false,
            Self::Chatgpt { document, .. } => document.is_some(),
            Self::Anthropic { document, .. } => document.is_some(),
        }
    }

    // Decoding reports a missing field as the same generic error as any other
    // mismatch. Name the one field that `provider.document` requires.
    fn check_document_model(options: &Value) -> Result<(), ConfigError> {
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

    // Decoding reports every mismatch as the same generic error. Name the
    // setting when its value is a string outside its list: the providers take
    // different values under the same names, such as `none` for ChatGPT effort
    // and `standard` for Anthropic speed.
    fn check_choices(options: &Value) -> Result<(), ConfigError> {
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
}

// A field of both `provider` and `provider.document`.
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

// A `null` speed may be meant as no speed, which is written by omitting it.
const SPEED: Choice = Choice {
    field: "speed",
    settings: ["provider.speed", "provider.document.speed"],
    named_null: true,
};

impl Choice {
    fn check<T: serde::de::DeserializeOwned>(
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

const DEFAULT_CHATGPT_MODEL: &str = "gpt-6-luna";
const DEFAULT_ANTHROPIC_MODEL: &str = "claude-haiku-5-5";

fn default_chatgpt_model() -> String {
    DEFAULT_CHATGPT_MODEL.into()
}

fn default_anthropic_model() -> String {
    DEFAULT_ANTHROPIC_MODEL.into()
}

// Unlike a plain `Option`, this rejects an explicit `null`.
fn explicit<'de, T: Deserialize<'de>, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<T>, D::Error> {
    T::deserialize(deserializer).map(Some)
}

// An omitted setting turns thinking off only where that is known to be
// accepted: the default model up to `high`. Other models may reject disabled
// thinking, and no list of them is kept here, so the omitted value enables it.
pub(crate) fn anthropic_thinking(
    model: &str,
    effort: AnthropicEffort,
    thinking: Option<bool>,
) -> bool {
    thinking.unwrap_or(model != DEFAULT_ANTHROPIC_MODEL || effort.requires_thinking().is_some())
}

impl Default for ProviderSettings {
    fn default() -> Self {
        Self::Mock {}
    }
}

#[derive(Clone, Copy, Debug, Default, Deserialize, serde::Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub(crate) enum ReasoningEffort {
    None,
    Minimal,
    #[default]
    Low,
    Medium,
    High,
    Xhigh,
    Max,
}

impl ReasoningEffort {
    const CHOICES: &str = "none, minimal, low, medium, high, xhigh, max";
}

#[derive(Clone, Copy, Debug, Deserialize, serde::Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub(crate) enum ChatgptSpeed {
    Auto,
    Default,
    Flex,
    Scale,
    Priority,
    Fast,
    Ultrafast,
}

impl ChatgptSpeed {
    const CHOICES: &str = "auto, default, flex, scale, priority, fast, ultrafast";
}

#[derive(Clone, Copy, Debug, Deserialize, serde::Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub(crate) enum AnthropicSpeed {
    Standard,
    Fast,
}

impl AnthropicSpeed {
    const CHOICES: &str = "standard, fast";
}

#[derive(Clone, Copy, Debug, Default, Deserialize, serde::Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub(crate) enum AnthropicEffort {
    #[default]
    Low,
    Medium,
    High,
    Xhigh,
    Max,
}

impl AnthropicEffort {
    const CHOICES: &str = "low, medium, high, xhigh, max";

    // The API rejects disabled thinking at these levels.
    fn requires_thinking(self) -> Option<&'static str> {
        match self {
            Self::Xhigh => Some("xhigh"),
            Self::Max => Some("max"),
            Self::Low | Self::Medium | Self::High => None,
        }
    }
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            translation_limits: TranslationLimits::default(),
            target_language: "ja".into(),
            source_language: "auto".into(),
            provider: ProviderSettings::Mock {},
            hover_timeout_ms: 20_000,
            selection_timeout_ms: 30_000,
            document_timeout_ms: 180_000,
            selection_debounce_ms: 450,
            comment_parse_timeout_ms: 1_000,
            features: Features::default(),
        }
    }
}

impl Settings {
    pub fn from_options(options: Option<Value>) -> Result<Self, ConfigError> {
        let settings = match options {
            None | Some(Value::Null) => Self::default(),
            Some(options) => {
                ProviderSettings::check_document_model(&options)?;
                ProviderSettings::check_choices(&options)?;
                Features::check_languages(&options)?;
                reject_arrays(&options)?;
                serde_json::from_value(options)?
            }
        };
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
            ProviderSettings::Mock {} => (),
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
        Ok(settings)
    }
}

// No setting takes an array. Derived decoding would read one as the fields of
// a table in declaration order, which would turn that order into a settings
// format. Reject arrays before decoding, wherever they appear.
fn reject_arrays(options: &Value) -> Result<(), ConfigError> {
    match options {
        Value::Array(_) => Err(ConfigError::Decode(serde::de::Error::custom(
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

#[cfg(test)]
mod document_tests;

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn decode_error_retains_source_without_echoing_rejected_values() {
        use std::error::Error as _;
        let error = Settings::from_options(Some(json!({"provider": {"name": "synthetic-secret"}})))
            .err()
            .unwrap();
        assert!(matches!(error, ConfigError::Decode(_)));
        assert!(
            error
                .source()
                .unwrap()
                .downcast_ref::<serde_json::Error>()
                .is_some()
        );
        assert!(!error.to_string().contains("synthetic-secret"));
        assert!(!format!("{error:?}").contains("synthetic-secret"));
    }

    #[test]
    fn no_setting_accepts_an_array_in_place_of_its_table_or_value() {
        let tab = "synthetic-tab";
        for options in [
            json!([]),
            json!([{}]),
            json!([[16, 1024], 4096]),
            json!({"provider": ["anthropic"]}),
            json!({"provider": ["chatgpt", "m"]}),
            json!({"provider": ["chatgpt", "m", "low"]}),
            json!({"provider": ["mock"]}),
            json!({"provider": [{"name": "mock"}]}),
            json!({"provider": {"name": "chatgpt", "model": "m", "document": [tab, "low"]}}),
            json!({"provider": {"name": "chatgpt", "model": "m", "document": [tab]}}),
            json!({"provider": {"name": "chatgpt", "model": "m", "document": [{"model": tab}]}}),
            json!({"provider": {"name": "anthropic", "document": [tab, "low", true]}}),
            json!({"provider": {"name": "anthropic", "document": [tab]}}),
            json!({"provider": {"name": "anthropic", "document": []}}),
            json!({"translation_limits": []}),
            json!({"translation_limits": [[16, 1024], 4096]}),
            json!({"translation_limits": {"input_kib": [16, 1024]}}),
            json!({"translation_limits": {"input_kib": []}}),
            json!({"translation_limits": {"work": []}}),
            json!({"translation_limits": {"work": [8, 2, 1, 3, 16]}}),
            json!({"translation_limits": {"cache": [64]}}),
            json!({"translation_limits": {"cache": [64, 16384]}}),
            // Nor in place of a single value.
            json!({"target_language": ["ja"]}),
            json!({"hover_timeout_ms": [20000]}),
            json!({"translation_limits": {"output_kib": [4096]}}),
            json!({"provider": {"name": "chatgpt", "model": ["m"]}}),
            json!({"provider": {"name": "anthropic", "effort": ["low"]}}),
            json!({"provider": {"name": "anthropic", "document": {"model": [tab]}}}),
        ] {
            let error = Settings::from_options(Some(options.clone()))
                .err()
                .unwrap_or_else(|| panic!("Accepted {options}"));
            assert!(matches!(error, ConfigError::Decode(_)), "{options}");
            assert_eq!(
                error.to_string(),
                "Invalid translation settings: check field names and value types"
            );
            assert!(!format!("{error:?}").contains(tab));
        }
        // The same values written as tables are settings.
        for options in [
            json!({}),
            json!({"provider": {"name": "anthropic", "document": {"model": tab, "effort": "low", "thinking": true}}}),
            json!({"provider": {"name": "chatgpt", "model": "m", "document": {"model": tab, "effort": "low", "speed": "fast"}}}),
            json!({"translation_limits": {"input_kib": {"hover": 16, "document": 1024}, "work": {}, "cache": {"entries": 64}}}),
        ] {
            assert!(Settings::from_options(Some(options)).is_ok());
        }
        // The named checks still report before the generic error.
        let error = Settings::from_options(Some(json!({
            "provider": {"name": "anthropic", "document": {"effort": "high"}},
            "translation_limits": []
        })))
        .err()
        .unwrap();
        assert_eq!(error.to_string(), "provider.document.model is required");
    }

    #[test]
    fn resource_limits_accept_boundaries_and_reject_excess_with_typed_details() {
        for (path, maximum) in [
            (vec!["input_kib", "hover"], 1024),
            (vec!["input_kib", "document"], 16384),
            (vec!["output_kib"], 16384),
            (vec!["cache", "capacity_kib"], 262144),
            (vec!["cache", "entries"], 4096),
            (vec!["work", "admitted"], 64),
            (vec!["work", "concurrent"], 8),
            (vec!["work", "documents"], 8),
            (vec!["work", "automatic"], 63),
            (vec!["work", "hover_waiters"], 256),
        ] {
            let settings = |value| {
                let mut options = json!({"translation_limits": {"work": {"admitted": 64, "concurrent": 8}, "input_kib": {"document": 1024}}});
                // Use fewer accepted operations for the maximum document size.
                if path == ["input_kib", "document"] {
                    options["translation_limits"]["work"]["admitted"] = json!(8);
                }
                // Use fewer workers for the maximum output size.
                if path == ["output_kib"] {
                    options["translation_limits"]["work"]["concurrent"] = json!(2);
                }
                let mut slot = &mut options["translation_limits"];
                for name in &path {
                    slot = &mut slot[*name];
                }
                *slot = json!(value);
                Settings::from_options(Some(options))
            };
            assert!(settings(maximum).is_ok(), "{path:?}");
            let error = settings(maximum + 1).err().unwrap();
            let ConfigError::OutOfRange {
                setting,
                value,
                minimum,
                maximum: bound,
            } = error
            else {
                panic!("Wrong error: {error}")
            };
            assert_eq!(setting, format!("translation_limits.{}", path.join(".")));
            assert_eq!((value, minimum, bound), (maximum + 1, 1, maximum));
        }
    }

    #[test]
    fn combined_budgets_reject_individually_valid_settings() {
        use error::Budget;
        let options = json!({"translation_limits": {"input_kib":{"document":4096},"output_kib":8192,"work":{"admitted":64,"concurrent":8}}});
        assert!(Settings::from_options(Some(options.clone())).is_ok());
        for (field, value, input) in [("document", 4097, true), ("output_kib", 8193, false)] {
            let mut options = options.clone();
            if input {
                options["translation_limits"]["input_kib"][field] = json!(value);
            } else {
                options["translation_limits"][field] = json!(value);
            }
            let error = Settings::from_options(Some(options)).err().unwrap();
            let ConfigError::Budget {
                budget,
                required_bytes,
                maximum_bytes,
            } = error
            else {
                panic!("Wrong error: {error}")
            };
            assert!(required_bytes > maximum_bytes);
            assert_eq!(matches!(budget, Budget::AcceptedInput), input);
        }
    }

    #[test]
    fn translation_limits_default_at_every_level_and_validate_kib_values() {
        for options in [
            json!({}),
            json!({"translation_limits":{}}),
            json!({"translation_limits":{"input_kib":{}}}),
        ] {
            let limits = Settings::from_options(Some(options))
                .unwrap()
                .translation_limits;
            assert_eq!(limits.input_kib.hover.bytes(), 16 * 1024);
            assert_eq!(limits.input_kib.document.bytes(), 1024 * 1024);
            assert_eq!(limits.output_kib.bytes(), 4 * 1024 * 1024);
        }
        let limits = Settings::from_options(Some(
            json!({"translation_limits":{"input_kib":{"document":2048}}}),
        ))
        .unwrap()
        .translation_limits;
        assert_eq!(limits.input_kib.document.bytes(), 2 * 1024 * 1024);
        assert_eq!(limits.input_kib.hover.bytes(), 16 * 1024);
        assert_eq!(limits.output_kib.bytes(), 4 * 1024 * 1024);
        for value in [
            json!(0),
            json!(-1),
            json!(1.5),
            json!("16"),
            json!(true),
            Value::Null,
            json!(u64::MAX),
        ] {
            for limits in [
                json!({"input_kib":{"hover":value}}),
                json!({"input_kib":{"document":value}}),
                json!({"output_kib":value}),
            ] {
                assert!(
                    Settings::from_options(Some(json!({"translation_limits":limits}))).is_err()
                );
            }
        }
        for limits in [
            Value::Null,
            json!({"input_kib":null}),
            json!({"input_kib":{"unknown":16}}),
            json!({"unknown":16}),
        ] {
            assert!(Settings::from_options(Some(json!({"translation_limits":limits}))).is_err());
        }
        let limits = Settings::from_options(Some(
            json!({"translation_limits":{"input_kib":{"hover":1},"output_kib":2}}),
        ))
        .unwrap()
        .translation_limits;
        assert!(
            limits
                .input_kib
                .hover
                .check("é".repeat(512).len(), limits::SizeSetting::HoverInput)
                .is_ok()
        );
        assert!(
            limits
                .input_kib
                .hover
                .check("é".repeat(513).len(), limits::SizeSetting::HoverInput)
                .is_err()
        );
        assert_eq!(limits.output_kib.bytes(), 2048);
    }

    #[test]
    fn defaults_keep_mock_and_explicit_settings_are_validated() {
        let settings = Settings::from_options(None).unwrap();
        assert!(matches!(
            Settings::from_options(Some(Value::Null)).unwrap().provider,
            ProviderSettings::Mock {}
        ));
        for effort in ["none", "minimal", "low", "medium", "high", "xhigh", "max"] {
            assert!(
                Settings::from_options(Some(
                    json!({"provider":{"name":"chatgpt","model":"synthetic-model","effort":effort}})
                ))
                .is_ok()
            );
        }
        assert_eq!(settings.target_language, "ja");
        assert_eq!(settings.source_language, "auto");
        assert!(matches!(settings.provider, ProviderSettings::Mock {}));
        for options in [None, Some(Value::Null), Some(json!({}))] {
            let settings = Settings::from_options(options).unwrap();
            assert_eq!(settings.hover_timeout_ms, 20_000);
            assert_eq!(settings.selection_timeout_ms, 30_000);
            assert_eq!(settings.document_timeout_ms, 180_000);
            assert_eq!(settings.selection_debounce_ms, 450);
            assert_eq!(settings.comment_parse_timeout_ms, 1_000);
        }
        let settings = Settings::from_options(Some(json!({
            "target_language":"en", "provider":{"name":"chatgpt", "model":"gpt-6.1-sol", "effort":"medium"}
        }))).unwrap();
        assert!(matches!(
            settings.provider,
            ProviderSettings::Chatgpt {
                effort: ReasoningEffort::Medium,
                ..
            }
        ));
        for invalid in [
            json!({"provider":{"name":"chatgpt","model":" "}}),
            json!({"provider":{"name":"chatgpt","model":"gpt-6.1-sol","effort":"ultra"}}),
            json!({"provider":{"name":"chatgpt","model":"gpt-6.1-sol","reasoning_effort":"low"}}),
            json!({"provider":{"name":"mock","model":"unexpected"}}),
            json!({"target_language":""}),
            json!({"api_key":"not-a-supported-setting"}),
        ] {
            assert!(Settings::from_options(Some(invalid)).is_err());
        }
    }

    #[test]
    fn anthropic_settings_default_to_light_haiku_and_name_rejected_values() {
        let anthropic = |fields: Value| {
            let mut provider = json!({"name": "anthropic"});
            provider
                .as_object_mut()
                .unwrap()
                .extend(fields.as_object().unwrap().clone());
            Settings::from_options(Some(json!({"provider": provider})))
        };
        let ProviderSettings::Anthropic {
            model,
            effort,
            thinking,
            ..
        } = anthropic(json!({})).unwrap().provider
        else {
            panic!("Expected Anthropic settings")
        };
        assert_eq!(model, "claude-haiku-5-5");
        assert_eq!(effort, AnthropicEffort::Low);
        assert_eq!(thinking, None);
        let ProviderSettings::Anthropic {
            model,
            effort,
            thinking,
            ..
        } = anthropic(json!({"model": "claude-opus-5", "effort": "max", "thinking": true}))
            .unwrap()
            .provider
        else {
            panic!("Expected Anthropic settings")
        };
        assert_eq!(model, "claude-opus-5");
        assert_eq!(effort, AnthropicEffort::Max);
        assert_eq!(thinking, Some(true));
        for (effort, thinking) in [
            ("low", false),
            ("medium", false),
            ("high", false),
            ("low", true),
            ("xhigh", true),
            ("max", true),
        ] {
            assert!(anthropic(json!({"effort": effort, "thinking": thinking})).is_ok());
        }

        for effort in ["none", "minimal", "ultra", "", "LOW"] {
            let error = anthropic(json!({"effort": effort, "model": "synthetic-secret"}))
                .err()
                .unwrap();
            assert!(matches!(
                error,
                ConfigError::Choice {
                    setting: "provider.effort",
                    ..
                }
            ));
            assert_eq!(
                error.to_string(),
                "provider.effort must be one of low, medium, high, xhigh, max"
            );
        }
        for effort in ["xhigh", "max"] {
            for model in ["claude-haiku-5-5", "claude-opus-5-5"] {
                let error = anthropic(json!({"model": model, "effort": effort, "thinking": false}))
                    .err()
                    .unwrap();
                assert!(matches!(error, ConfigError::ThinkingRequired { .. }));
                assert_eq!(
                    error.to_string(),
                    format!("provider.thinking must not be false when provider.effort is {effort}")
                );
            }
        }
        for model in ["", " ", "\t"] {
            assert!(matches!(
                anthropic(json!({"model": model})).err().unwrap(),
                ConfigError::Text {
                    setting: "provider.model",
                    ..
                }
            ));
        }
        assert!(matches!(
            anthropic(json!({"model": "claude haiku"})).err().unwrap(),
            ConfigError::ModelWhitespace { .. }
        ));
        for fields in [
            json!({"api_key": "synthetic-secret"}),
            json!({"reasoning_effort": "low"}),
            json!({"base_url": "http://127.0.0.1"}),
            json!({"effort": 1}),
            json!({"effort": null}),
            json!({"thinking": "false"}),
            json!({"thinking": null}),
            json!({"model": null}),
        ] {
            let error = anthropic(fields).err().unwrap();
            assert!(matches!(error, ConfigError::Decode(_)));
            assert!(!error.to_string().contains("synthetic-secret"));
        }

        // The Anthropic fields and checks do not leak into the other providers.
        for options in [
            json!({"provider": {"name": "chatgpt", "model": "synthetic-model", "thinking": true}}),
            json!({"provider": {"name": "mock", "effort": "low"}}),
            json!({"provider": {"name": "mock", "speed": "fast"}}),
        ] {
            assert!(matches!(
                Settings::from_options(Some(options)).err().unwrap(),
                ConfigError::Decode(_)
            ));
        }
        assert!(matches!(
            Settings::from_options(Some(json!({
                "provider": {"name": "chatgpt", "effort": "none"}
            })))
            .unwrap()
            .provider,
            ProviderSettings::Chatgpt {
                effort: ReasoningEffort::None,
                ..
            }
        ));
        assert!(matches!(
            Settings::from_options(Some(json!({"provider": {"name": "mock"}})))
                .unwrap()
                .provider,
            ProviderSettings::Mock {}
        ));
    }

    #[test]
    fn chatgpt_settings_default_to_light_luna_and_take_no_reasoning_effort() {
        let ProviderSettings::Chatgpt {
            model,
            effort,
            speed,
            document,
        } = Settings::from_options(Some(json!({"provider": {"name": "chatgpt"}})))
            .unwrap()
            .provider
        else {
            panic!("Expected ChatGPT settings")
        };
        assert_eq!(model, "gpt-6-luna");
        assert_eq!(effort, ReasoningEffort::Low);
        assert_eq!(speed, None);
        assert!(document.is_none());
        // The former name is an unknown field, inside `provider.document` too.
        for provider in [
            json!({"name": "chatgpt", "reasoning_effort": "low"}),
            json!({"name": "chatgpt", "document": {"model": "m", "reasoning_effort": "low"}}),
        ] {
            assert!(matches!(
                Settings::from_options(Some(json!({"provider": provider})))
                    .err()
                    .unwrap(),
                ConfigError::Decode(_)
            ));
        }
    }

    #[test]
    fn effort_and_speed_take_the_values_of_their_provider_and_name_them() {
        const CHATGPT_EFFORT: &str = "none, minimal, low, medium, high, xhigh, max";
        const CHATGPT_SPEED: &str = "auto, default, flex, scale, priority, fast, ultrafast";
        const ANTHROPIC_SPEED: &str = "standard, fast";
        let settings = |name: &str, field: &str, value: Value, inside: bool| {
            let mut provider = json!({"name": name});
            if inside {
                provider["document"] = json!({"model": "m", (field): value});
            } else {
                provider[field] = value;
            }
            Settings::from_options(Some(json!({"provider": provider})))
        };
        for (name, field, accepted) in [
            ("chatgpt", "speed", CHATGPT_SPEED),
            ("anthropic", "speed", ANTHROPIC_SPEED),
            ("chatgpt", "effort", CHATGPT_EFFORT),
        ] {
            for value in accepted.split(", ") {
                for inside in [false, true] {
                    assert!(
                        settings(name, field, json!(value), inside).is_ok(),
                        "{name} {field} {value}"
                    );
                }
            }
        }
        for (name, field, value, choices) in [
            // The values of the other provider.
            ("chatgpt", "speed", json!("standard"), CHATGPT_SPEED),
            ("anthropic", "speed", json!("priority"), ANTHROPIC_SPEED),
            ("anthropic", "speed", json!("ultrafast"), ANTHROPIC_SPEED),
            (
                "anthropic",
                "effort",
                json!("minimal"),
                AnthropicEffort::CHOICES,
            ),
            ("chatgpt", "effort", json!("ultra"), CHATGPT_EFFORT),
            ("chatgpt", "speed", json!("Fast"), CHATGPT_SPEED),
            (
                "anthropic",
                "speed",
                json!("synthetic-secret"),
                ANTHROPIC_SPEED,
            ),
            // `null` is no way to write that no speed is requested.
            ("chatgpt", "speed", Value::Null, CHATGPT_SPEED),
            ("anthropic", "speed", Value::Null, ANTHROPIC_SPEED),
        ] {
            for (inside, scope) in [(false, "provider"), (true, "provider.document")] {
                let error = settings(name, field, value.clone(), inside).err().unwrap();
                assert_eq!(
                    error.to_string(),
                    format!("{scope}.{field} must be one of {choices}"),
                    "{name} {value}"
                );
            }
        }
        for name in ["chatgpt", "anthropic"] {
            for value in [json!(1), json!(true), json!({"tier": "fast"})] {
                for inside in [false, true] {
                    assert!(matches!(
                        settings(name, "speed", value.clone(), inside)
                            .err()
                            .unwrap(),
                        ConfigError::Decode(_)
                    ));
                }
            }
        }
    }

    #[test]
    fn omitted_anthropic_thinking_follows_the_model_and_effort() {
        let resolved = |fields: Value| {
            let mut provider = json!({"name": "anthropic"});
            provider
                .as_object_mut()
                .unwrap()
                .extend(fields.as_object().unwrap().clone());
            let ProviderSettings::Anthropic {
                model,
                effort,
                thinking,
                ..
            } = Settings::from_options(Some(json!({"provider": provider})))
                .unwrap()
                .provider
            else {
                panic!("Expected Anthropic settings")
            };
            anthropic_thinking(&model, effort, thinking)
        };
        let efforts = [
            ("low", false),
            ("medium", false),
            ("high", false),
            ("xhigh", true),
            ("max", true),
        ];
        // The default model keeps thinking off up to `high`, named or not.
        for (effort, expected) in efforts {
            assert_eq!(resolved(json!({"effort": effort})), expected, "{effort}");
            assert_eq!(
                resolved(json!({"model": "claude-haiku-5-5", "effort": effort})),
                expected,
                "{effort}"
            );
        }
        assert!(!resolved(json!({})));
        assert!(!resolved(json!({"model": "claude-haiku-5-5"})));
        // Any other model name enables it, including near matches of the default.
        for model in [
            "claude-opus-5-5",
            "claude-sonnet-5-5",
            "claude-haiku-4-5",
            "claude-haiku-5-5-20260101",
            "anthropic.claude-haiku-5-5",
            "claude-haiku-5",
            "Claude-Haiku-5-5",
        ] {
            assert!(resolved(json!({"model": model})), "{model}");
            for (effort, _) in efforts {
                assert!(
                    resolved(json!({"model": model, "effort": effort})),
                    "{model} {effort}"
                );
            }
        }
        // An explicit value is sent as written wherever the settings accept it.
        for model in ["claude-haiku-5-5", "claude-opus-5-5"] {
            for (effort, required) in efforts {
                assert!(resolved(
                    json!({"model": model, "effort": effort, "thinking": true})
                ));
                if !required {
                    assert!(!resolved(
                        json!({"model": model, "effort": effort, "thinking": false})
                    ));
                }
            }
        }
    }

    #[test]
    fn timing_options_accept_inclusive_bounds_and_reject_wrong_types_and_names() {
        for (field, minimum, maximum) in [
            ("hover_timeout_ms", 1_000, 120_000),
            ("selection_timeout_ms", 1_000, 120_000),
            ("document_timeout_ms", 1_000, 600_000),
            ("selection_debounce_ms", 100, 2_000),
            ("comment_parse_timeout_ms", 100, 60_000),
        ] {
            for value in [minimum, maximum] {
                assert!(Settings::from_options(Some(json!({(field):value}))).is_ok());
            }
            for value in [minimum - 1, maximum + 1] {
                let error = Settings::from_options(Some(json!({(field):value})))
                    .err()
                    .unwrap();
                assert!(error.to_string().starts_with(field), "{field}: {error}");
            }
            for value in [
                json!(-1),
                json!(1.5),
                json!("1000"),
                json!(true),
                Value::Null,
                json!(u64::MAX),
            ] {
                assert!(Settings::from_options(Some(json!({(field):value}))).is_err());
            }
        }
        assert!(Settings::from_options(Some(json!({"unknown_setting":true}))).is_err());
    }
}
