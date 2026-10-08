use serde::Deserialize;

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
    type Error = &'static str;

    fn try_from(kib: usize) -> Result<Self, Self::Error> {
        kib.checked_mul(1024)
            .filter(|bytes| *bytes > 0)
            .map(Self)
            .ok_or("Translation limits must be positive KiB integers that fit in memory-size arithmetic")
    }
}

impl SizeLimit {
    pub fn bytes(self) -> usize {
        self.0
    }

    pub fn check(self, actual: usize, setting: &str) -> Result<(), String> {
        if actual > self.0 {
            Err(format!(
                "Translation size {actual} bytes exceeds {} KiB; adjust {setting}",
                self.0 / 1024,
            ))
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

impl TranslationLimits {
    pub fn validate(&self) -> Result<(), &'static str> {
        let w = self.work;
        if [
            w.admitted,
            w.concurrent,
            w.documents,
            w.automatic,
            w.hover_waiters,
            self.cache.entries,
        ]
        .contains(&0)
        {
            return Err("Translation work and cache counts must be positive integers");
        }
        if w.concurrent > tokio::sync::Semaphore::MAX_PERMITS {
            return Err(
                "translation_limits.work.concurrent exceeds the platform concurrency capacity",
            );
        }
        if w.documents > w.concurrent || w.concurrent > w.admitted || w.automatic >= w.admitted {
            return Err(
                "translation_limits.work requires documents <= concurrent <= admitted and automatic < admitted",
            );
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
