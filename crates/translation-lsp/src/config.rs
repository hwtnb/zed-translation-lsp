use serde::Deserialize;
use serde_json::Value;

#[derive(Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct Settings {
    pub target_language: String,
    pub source_language: String,
    pub provider: ProviderSettings,
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
        }
    }
}

impl Settings {
    pub fn from_options(options: Option<Value>) -> Result<Self, &'static str> {
        let settings = match options {
            None | Some(Value::Null) => Self::default(),
            Some(options) => serde_json::from_value(options).map_err(
                |_| "Invalid translation settings: check provider, model and reasoning_effort",
            )?,
        };
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
}
