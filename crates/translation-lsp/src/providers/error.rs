#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum ErrorKind {
    Failed,
    Cancelled,
    Paused,
    Modified,
    Unavailable,
    Storage,
    UsageLimit,
    NotEligible,
    Temporary,
    Configuration,
    RefreshUnusable,
}

#[derive(Clone, Debug)]
pub(crate) struct Error {
    pub kind: ErrorKind,
    pub message: String,
}

impl std::fmt::Display for Error {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for Error {}

impl Error {
    pub fn cancelled() -> Self {
        Self {
            kind: ErrorKind::Cancelled,
            message: "Translation canceled".into(),
        }
    }
    pub fn failed(message: &str) -> Self {
        Self {
            kind: ErrorKind::Failed,
            message: message.into(),
        }
    }
}
