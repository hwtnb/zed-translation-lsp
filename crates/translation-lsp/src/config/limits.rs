use serde::Deserialize;

use super::error::{Budget, ConfigError, Relation, SizeExceeded, bounded_number};

#[derive(Clone, Copy, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct TranslationLimits {
    pub input_kib: InputLimits,
    pub output_kib: SizeLimit,
    pub work: WorkLimits,
    pub cache: CacheLimits,
}

#[derive(Clone, Copy, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct InputLimits {
    pub hover: SizeLimit,
    pub document: SizeLimit,
}

#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(try_from = "usize")]
pub(crate) struct SizeLimit(usize);

impl TryFrom<usize> for SizeLimit {
    type Error = SizeConversionError;

    fn try_from(kib: usize) -> Result<Self, Self::Error> {
        kib.checked_mul(1024)
            .filter(|bytes| *bytes > 0)
            .map(Self)
            .ok_or(SizeConversionError { kib })
    }
}

impl SizeLimit {
    pub fn bytes(self) -> usize {
        self.0
    }

    pub fn check(self, actual_bytes: usize, setting: SizeSetting) -> Result<(), SizeExceeded> {
        if actual_bytes > self.0 {
            Err(SizeExceeded {
                setting,
                actual_bytes,
                maximum_bytes: self.0,
            })
        } else {
            Ok(())
        }
    }
}

impl Default for TranslationLimits {
    fn default() -> Self {
        Self {
            input_kib: InputLimits::default(),
            output_kib: SizeLimit(4096 * 1024),
            work: WorkLimits::default(),
            cache: CacheLimits::default(),
        }
    }
}

impl Default for InputLimits {
    fn default() -> Self {
        Self {
            hover: SizeLimit(16 * 1024),
            document: SizeLimit(1024 * 1024),
        }
    }
}

#[derive(Clone, Copy, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct WorkLimits {
    pub admitted: usize,
    pub concurrent: usize,
    pub documents: usize,
    pub automatic: usize,
    pub hover_waiters: usize,
}

impl Default for WorkLimits {
    fn default() -> Self {
        Self {
            admitted: 8,
            concurrent: 2,
            documents: 1,
            automatic: 3,
            hover_waiters: 16,
        }
    }
}

#[derive(Clone, Copy, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct CacheLimits {
    pub entries: usize,
    pub capacity_kib: SizeLimit,
}

impl Default for CacheLimits {
    fn default() -> Self {
        Self {
            entries: 64,
            capacity_kib: SizeLimit(16 * 1024 * 1024),
        }
    }
}

#[derive(Debug, thiserror::Error)]
#[error(
    "Translation limits must be positive KiB integers that fit in memory-size arithmetic; received {kib}"
)]
pub(crate) struct SizeConversionError {
    pub kib: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SizeSetting {
    HoverInput,
    DocumentInput,
    Output,
    CacheCapacity,
}

impl SizeSetting {
    pub fn name(self) -> &'static str {
        match self {
            Self::HoverInput => "translation_limits.input_kib.hover",
            Self::DocumentInput => "translation_limits.input_kib.document",
            Self::Output => "translation_limits.output_kib",
            Self::CacheCapacity => "translation_limits.cache.capacity_kib",
        }
    }
}

impl std::fmt::Display for SizeSetting {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.name())
    }
}

impl TranslationLimits {
    pub fn validate(&self) -> Result<(), ConfigError> {
        // Local resource policy, not a provider context-window or token guarantee.
        for (setting, limit, maximum_kib) in [
            (SizeSetting::HoverInput, self.input_kib.hover, 1024),
            (
                SizeSetting::DocumentInput,
                self.input_kib.document,
                16 * 1024,
            ),
            (SizeSetting::Output, self.output_kib, 16 * 1024),
            (
                SizeSetting::CacheCapacity,
                self.cache.capacity_kib,
                256 * 1024,
            ),
        ] {
            bounded_number(
                setting.name(),
                (limit.bytes() / 1024) as u64,
                1,
                maximum_kib,
            )?;
        }
        let w = self.work;
        for (setting, value, maximum) in [
            ("translation_limits.work.admitted", w.admitted, 64),
            ("translation_limits.work.concurrent", w.concurrent, 8),
            ("translation_limits.work.documents", w.documents, 8),
            ("translation_limits.work.automatic", w.automatic, 63),
            (
                "translation_limits.work.hover_waiters",
                w.hover_waiters,
                256,
            ),
            ("translation_limits.cache.entries", self.cache.entries, 4096),
        ] {
            bounded_number(setting, value as u64, 1, maximum)?;
        }
        for (lower, lower_value, upper, upper_value, relation) in [
            (
                "translation_limits.work.documents",
                w.documents,
                "translation_limits.work.concurrent",
                w.concurrent,
                Relation::AtMost,
            ),
            (
                "translation_limits.work.concurrent",
                w.concurrent,
                "translation_limits.work.admitted",
                w.admitted,
                Relation::AtMost,
            ),
            (
                "translation_limits.work.automatic",
                w.automatic,
                "translation_limits.work.admitted",
                w.admitted,
                Relation::LessThan,
            ),
        ] {
            let valid = match relation {
                Relation::AtMost => lower_value <= upper_value,
                Relation::LessThan => lower_value < upper_value,
            };
            if !valid {
                return Err(ConfigError::Relation {
                    lower,
                    lower_value,
                    upper,
                    upper_value,
                    relation,
                });
            }
        }
        for (budget, required_bytes, maximum_bytes) in [
            (
                Budget::AcceptedInput,
                self.input_kib
                    .hover
                    .bytes()
                    .max(self.input_kib.document.bytes()) as u64
                    * w.admitted as u64,
                256 * 1024 * 1024,
            ),
            (
                Budget::ConcurrentOutput,
                self.output_kib.bytes() as u64 * w.concurrent as u64,
                64 * 1024 * 1024,
            ),
        ] {
            if required_bytes > maximum_bytes {
                return Err(ConfigError::Budget {
                    budget,
                    required_bytes,
                    maximum_bytes,
                });
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn nested_work_and_cache_limits_default_and_reject_inconsistent_values() {
        let limits: TranslationLimits =
            serde_json::from_value(json!({"work":{"admitted":12},"cache":{"capacity_kib":32768}}))
                .unwrap();
        limits.validate().unwrap();
        assert_eq!(limits.work.concurrent, 2);
        assert_eq!(limits.work.admitted, 12);
        assert_eq!(limits.cache.entries, 64);
        assert_eq!(limits.cache.capacity_kib.bytes(), 32 * 1024 * 1024);
        for value in [
            json!({"work":{"admitted":0}}),
            json!({"work":{"documents":3}}),
            json!({"work":{"automatic":8}}),
            json!({"cache":{"entries":0}}),
        ] {
            assert!(
                serde_json::from_value::<TranslationLimits>(value)
                    .unwrap()
                    .validate()
                    .is_err()
            );
        }
        for value in [
            json!({"work":{"admitted":-1}}),
            json!({"work":{"concurrent":1.5}}),
            json!({"cache":{"capacity_kib":0}}),
            json!({"cache":{"unknown":4}}),
            json!({"work":null}),
        ] {
            assert!(serde_json::from_value::<TranslationLimits>(value).is_err());
        }
    }
}
