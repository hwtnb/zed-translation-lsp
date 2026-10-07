use serde::Deserialize;

#[derive(Clone, Copy, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct TranslationLimits {
    pub input_kib: InputLimits,
    pub output_kib: SizeLimit,
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
