use std::fmt;

use super::{features::Feature, limits::SizeSetting};
use crate::language::Language;

#[derive(thiserror::Error)]
pub(crate) enum ConfigError {
    /// The source can contain rejected user input. Do not include it in UI/log output.
    #[error("Invalid translation settings: check field names and value types")]
    Decode(#[from] serde_json::Error),
    #[error("{setting} must be between {minimum} and {maximum}; received {value}")]
    OutOfRange {
        setting: &'static str,
        value: u64,
        minimum: u64,
        maximum: u64,
    },
    #[error("{lower} ({lower_value}) must be {relation} {upper} ({upper_value})")]
    Relation {
        lower: &'static str,
        lower_value: usize,
        upper: &'static str,
        upper_value: usize,
        relation: Relation,
    },
    #[error(
        "{budget} requires {required_bytes} bytes; maximum is {maximum_bytes} bytes; reduce the size or work limit"
    )]
    Budget {
        budget: Budget,
        required_bytes: u64,
        maximum_bytes: u64,
    },
    #[error(
        "{setting} must be nonempty, without control characters, and at most {maximum_bytes} bytes"
    )]
    Text {
        setting: &'static str,
        maximum_bytes: usize,
    },
    #[error("{setting} must not contain whitespace")]
    Whitespace { setting: &'static str },
    #[error(
        "provider.base_url must be an https URL, or an http URL of this computer (localhost, 127.0.0.1, or ::1), without a user name, password, query, or fragment"
    )]
    BaseUrl,
    #[error("{setting} is required")]
    Required { setting: &'static str },
    #[error("{setting} must be one of {choices}")]
    Choice {
        setting: &'static str,
        choices: &'static str,
    },
    /// `scope` is the table holding both settings: `provider` or `provider.document`.
    #[error("{scope}.thinking must not be false when {scope}.effort is {effort}")]
    ThinkingRequired {
        scope: &'static str,
        effort: &'static str,
    },
    #[error("features.languages keys must be one of {}", Language::ALL.map(Language::id).join(", "))]
    FeatureLanguage,
    #[error(
        "features.languages.{language}.{feature} is not available; {feature} applies to {}",
        .feature.language_ids().join(", ")
    )]
    FeatureUnavailable {
        language: &'static str,
        feature: Feature,
    },
}

#[derive(Debug)]
pub(crate) enum Relation {
    AtMost,
    LessThan,
}

impl fmt::Display for Relation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::AtMost => "at most",
            Self::LessThan => "less than",
        })
    }
}

#[derive(Debug)]
pub(crate) enum Budget {
    AcceptedInput,
    ConcurrentOutput,
}

impl fmt::Display for Budget {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::AcceptedInput => "translation_limits input size times work.admitted",
            Self::ConcurrentOutput => "translation_limits.output_kib times work.concurrent",
        })
    }
}

#[derive(Clone, Debug, thiserror::Error)]
#[error("Translation size {actual_bytes} bytes exceeds {maximum_kib} KiB; adjust {setting}", maximum_kib = .maximum_bytes / 1024)]
pub(crate) struct SizeExceeded {
    pub setting: SizeSetting,
    pub actual_bytes: usize,
    pub maximum_bytes: usize,
}

pub(super) fn bounded_number(
    setting: &'static str,
    value: u64,
    minimum: u64,
    maximum: u64,
) -> Result<(), ConfigError> {
    if !(minimum..=maximum).contains(&value) {
        return Err(ConfigError::OutOfRange {
            setting,
            value,
            minimum,
            maximum,
        });
    }
    Ok(())
}

impl fmt::Debug for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}
