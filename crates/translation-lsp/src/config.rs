pub(crate) mod error;
pub(crate) mod features;
pub(crate) mod language;
pub(crate) mod limits;
mod validation;

use error::ConfigError;

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

#[derive(Clone, Default, Deserialize)]
#[serde(tag = "name", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum ProviderSettings {
    /// No provider in the settings: the server translates nothing and shows
    /// how to choose one. Settings cannot name it.
    #[default]
    #[serde(skip_deserializing)]
    Unset,
    Mock {},
    Chatgpt {
        #[serde(default = "default_openai_model")]
        model: String,
        #[serde(default)]
        effort: ReasoningEffort,
        #[serde(default, deserialize_with = "explicit")]
        speed: Option<OpenAiSpeed>,
        #[serde(default, deserialize_with = "explicit")]
        document: Option<OpenAiModel>,
    },
    Anthropic {
        #[serde(default = "default_anthropic_model")]
        model: String,
        #[serde(default)]
        effort: AnthropicEffort,
        /// `None` when omitted. `anthropic_thinking` chooses the value to send.
        #[serde(default, deserialize_with = "explicit")]
        thinking: Option<bool>,
        #[serde(default, deserialize_with = "explicit")]
        speed: Option<AnthropicSpeed>,
        #[serde(default, deserialize_with = "explicit")]
        document: Option<AnthropicModel>,
    },
    #[serde(rename = "openai_api")]
    OpenAiApi {
        #[serde(default = "default_openai_model")]
        model: String,
        #[serde(default)]
        effort: ReasoningEffort,
        #[serde(default, deserialize_with = "explicit")]
        speed: Option<OpenAiSpeed>,
        #[serde(default, deserialize_with = "explicit")]
        document: Option<OpenAiModel>,
    },
    #[serde(rename = "openai_compatible")]
    OpenAiCompatible {
        base_url: String,
        model: String,
        /// Sent as written, and only when written: each service has values of
        /// its own.
        #[serde(default, deserialize_with = "explicit")]
        effort: Option<String>,
        #[serde(default, deserialize_with = "explicit")]
        document: Option<CompatibleModel>,
    },
}

/// The model and options that one output sends. `provider.document` is one of
/// these for translations opened in a tab: an omitted field takes its own
/// default there and inherits nothing from the surrounding provider settings.
#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct OpenAiModel {
    pub model: String,
    #[serde(default)]
    pub effort: ReasoningEffort,
    #[serde(default, deserialize_with = "explicit")]
    pub speed: Option<OpenAiSpeed>,
}

/// The model and options of a service that `provider.base_url` names, for one
/// output. Both outputs go to the same service.
#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct CompatibleModel {
    pub model: String,
    #[serde(default, deserialize_with = "explicit")]
    pub effort: Option<String>,
}

/// The Anthropic counterpart of [`OpenAiModel`].
#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct AnthropicModel {
    pub model: String,
    #[serde(default)]
    pub effort: AnthropicEffort,
    /// `None` when omitted. `anthropic_thinking` chooses the value to send.
    #[serde(default, deserialize_with = "explicit")]
    pub thinking: Option<bool>,
    #[serde(default, deserialize_with = "explicit")]
    pub speed: Option<AnthropicSpeed>,
}

impl ProviderSettings {
    /// Tab translations run apart from range translations exactly when
    /// `provider.document` is present, whatever its values are.
    pub fn separates_documents(&self) -> bool {
        match self {
            Self::Unset | Self::Mock {} => false,
            Self::Chatgpt { document, .. } => document.is_some(),
            Self::Anthropic { document, .. } => document.is_some(),
            Self::OpenAiApi { document, .. } => document.is_some(),
            Self::OpenAiCompatible { document, .. } => document.is_some(),
        }
    }
}

const DEFAULT_OPENAI_MODEL: &str = "gpt-6-luna";
const DEFAULT_ANTHROPIC_MODEL: &str = "claude-haiku-5-5";

fn default_openai_model() -> String {
    DEFAULT_OPENAI_MODEL.into()
}

fn default_anthropic_model() -> String {
    DEFAULT_ANTHROPIC_MODEL.into()
}

/// Unlike a plain `Option`, this rejects an explicit `null`.
fn explicit<'de, T: Deserialize<'de>, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<T>, D::Error> {
    T::deserialize(deserializer).map(Some)
}

/// An omitted setting turns thinking off only where that is known to be
/// accepted: the default model up to `high`. Other models may reject disabled
/// thinking, and no list of them is kept here, so the omitted value enables it.
pub(crate) fn anthropic_thinking(
    model: &str,
    effort: AnthropicEffort,
    thinking: Option<bool>,
) -> bool {
    thinking.unwrap_or(model != DEFAULT_ANTHROPIC_MODEL || effort.requires_thinking().is_some())
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
pub(crate) enum OpenAiSpeed {
    Auto,
    Default,
    Flex,
    Scale,
    Priority,
    Fast,
    Ultrafast,
}

impl OpenAiSpeed {
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

    /// The API rejects disabled thinking at these levels.
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
            provider: ProviderSettings::Unset,
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
                validation::before_decoding(&options)?;
                serde_json::from_value(options)?
            }
        };
        validation::after_decoding(&settings)?;
        Ok(settings)
    }
}

#[cfg(test)]
mod document_tests;

#[cfg(test)]
mod tests;
