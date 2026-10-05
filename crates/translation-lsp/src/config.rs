use serde::Deserialize;
use serde_json::Value;

#[derive(Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct Settings {
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

impl Default for Settings {
    fn default() -> Self {
        Self {
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
    pub fn from_options(options: Option<Value>) -> Result<Self, &'static str> {
        let settings = match options {
            None | Some(Value::Null) => Self::default(),
            Some(options) => serde_json::from_value(options)
                .map_err(|_| "Invalid translation settings: check field names and value types")?,
        };
        if !(1_000..=120_000).contains(&settings.hover_timeout_ms) {
            return Err("hover_timeout_ms must be between 1000 and 120000 milliseconds");
        }
        if !(1_000..=120_000).contains(&settings.selection_timeout_ms) {
            return Err("selection_timeout_ms must be between 1000 and 120000 milliseconds");
        }
        if !(1_000..=600_000).contains(&settings.document_timeout_ms) {
            return Err("document_timeout_ms must be between 1000 and 600000 milliseconds");
        }
        if !(100..=2_000).contains(&settings.selection_debounce_ms) {
            return Err("selection_debounce_ms must be between 100 and 2000 milliseconds");
        }
        if !bounded(&settings.target_language, 128) || !bounded(&settings.source_language, 128) {
            return Err(
                "source_language and target_language must be nonempty, without control characters, and at most 128 bytes",
            );
        }
        if let ProviderSettings::Chatgpt { model, .. } = &settings.provider
            && (!bounded(model, 256) || model.chars().any(char::is_whitespace))
        {
            return Err(
                "ChatGPT model must be a nonempty slug without whitespace and at most 256 bytes",
            );
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
                assert!(error.starts_with(field), "{field}: {error}");
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
