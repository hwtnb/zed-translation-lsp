pub(crate) mod error;
pub(crate) mod limits;

use error::{ConfigError, bounded_number};

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
    pub markdown_auto_translate: bool,
    pub selection_debounce_ms: u64,
}

#[derive(Clone, Deserialize)]
#[serde(tag = "name", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum ProviderSettings {
    Mock {},
    Chatgpt {
        model: String,
        reasoning_effort: Option<ReasoningEffort>,
    },
    Anthropic {
        #[serde(default = "default_anthropic_model")]
        model: String,
        #[serde(default)]
        effort: AnthropicEffort,
        // `None` when omitted. `anthropic_thinking` chooses the value to send.
        #[serde(default, deserialize_with = "explicit_bool")]
        thinking: Option<bool>,
    },
}

const DEFAULT_ANTHROPIC_MODEL: &str = "claude-haiku-5-5";

fn default_anthropic_model() -> String {
    DEFAULT_ANTHROPIC_MODEL.into()
}

// Unlike `Option<bool>`, this rejects an explicit `null`.
fn explicit_bool<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<bool>, D::Error> {
    bool::deserialize(deserializer).map(Some)
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

#[derive(Clone, Copy, Deserialize, serde::Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub(crate) enum ReasoningEffort {
    None,
    Minimal,
    Low,
    Medium,
    High,
    Xhigh,
    Max,
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

    // Decoding reports every mismatch as the same generic error. Name this
    // setting when its value is a string outside the list, such as the
    // ChatGPT-only `none` and `minimal`.
    fn check_choice(options: &Value) -> Result<(), ConfigError> {
        let provider = &options["provider"];
        if provider["name"] == "anthropic"
            && let effort @ Value::String(_) = &provider["effort"]
            && Self::deserialize(effort).is_err()
        {
            return Err(ConfigError::Choice {
                setting: "provider.effort",
                choices: Self::CHOICES,
            });
        }
        Ok(())
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
            markdown_auto_translate: true,
            selection_debounce_ms: 450,
        }
    }
}

impl Settings {
    pub fn from_options(options: Option<Value>) -> Result<Self, ConfigError> {
        let settings = match options {
            None | Some(Value::Null) => Self::default(),
            Some(options) => {
                AnthropicEffort::check_choice(&options)?;
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
        if let ProviderSettings::Chatgpt { model, .. } | ProviderSettings::Anthropic { model, .. } =
            &settings.provider
        {
            if !bounded(model, 256) {
                return Err(ConfigError::Text {
                    setting: "provider.model",
                    maximum_bytes: 256,
                });
            }
            if model.chars().any(char::is_whitespace) {
                return Err(ConfigError::ModelWhitespace);
            }
        }
        if let ProviderSettings::Anthropic {
            effort,
            thinking: Some(false),
            ..
        } = &settings.provider
            && let Some(effort) = effort.requires_thinking()
        {
            return Err(ConfigError::ThinkingRequired { effort });
        }
        Ok(settings)
    }
}

fn bounded(value: &str, maximum: usize) -> bool {
    !value.trim().is_empty() && value.len() <= maximum && !value.chars().any(char::is_control)
}

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
            assert!(Settings::from_options(Some(json!({"provider":{"name":"chatgpt","model":"synthetic-model","reasoning_effort":effort}}))).is_ok());
        }
        assert_eq!(settings.target_language, "ja");
        assert_eq!(settings.source_language, "auto");
        assert!(matches!(settings.provider, ProviderSettings::Mock {}));
        for options in [None, Some(Value::Null), Some(json!({}))] {
            let settings = Settings::from_options(options).unwrap();
            assert_eq!(settings.hover_timeout_ms, 20_000);
            assert_eq!(settings.selection_timeout_ms, 30_000);
            assert_eq!(settings.document_timeout_ms, 180_000);
            assert!(settings.markdown_auto_translate);
            assert_eq!(settings.selection_debounce_ms, 450);
        }
        let settings = Settings::from_options(Some(json!({
            "target_language":"en", "provider":{"name":"chatgpt", "model":"gpt-6.1-sol", "reasoning_effort":"low"}
        }))).unwrap();
        assert!(matches!(
            settings.provider,
            ProviderSettings::Chatgpt {
                reasoning_effort: Some(ReasoningEffort::Low),
                ..
            }
        ));
        for invalid in [
            json!({"provider":{"name":"chatgpt"}}),
            json!({"provider":{"name":"chatgpt","model":" "}}),
            json!({"provider":{"name":"chatgpt","model":"gpt-6.1-sol","reasoning_effort":"ultra"}}),
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
            ConfigError::ModelWhitespace
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
            json!({"provider": {"name": "chatgpt", "model": "synthetic-model", "effort": "low"}}),
            json!({"provider": {"name": "chatgpt", "model": "synthetic-model", "thinking": true}}),
            json!({"provider": {"name": "mock", "effort": "low"}}),
            json!({"provider": {"name": "chatgpt", "effort": "none"}}),
        ] {
            assert!(matches!(
                Settings::from_options(Some(options)).err().unwrap(),
                ConfigError::Decode(_)
            ));
        }
        assert!(matches!(
            Settings::from_options(Some(json!({
                "provider": {"name": "chatgpt", "model": "synthetic-model", "reasoning_effort": "none"}
            })))
            .unwrap()
            .provider,
            ProviderSettings::Chatgpt {
                reasoning_effort: Some(ReasoningEffort::None),
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
        assert!(
            !Settings::from_options(Some(json!({"markdown_auto_translate":false})))
                .unwrap()
                .markdown_auto_translate
        );
        assert!(
            Settings::from_options(Some(json!({"markdown_auto_translate":true})))
                .unwrap()
                .markdown_auto_translate
        );
        for value in [Value::Null, json!(0), json!("false")] {
            assert!(
                Settings::from_options(Some(json!({"markdown_auto_translate":value}))).is_err()
            );
        }
        assert!(Settings::from_options(Some(json!({"unknown_setting":true}))).is_err());
    }
}
