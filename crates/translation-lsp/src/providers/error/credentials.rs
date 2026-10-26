use std::{fmt, io};

#[derive(thiserror::Error)]
pub(crate) enum CredentialError {
    #[error("ChatGPT credentials are being updated by another process; retry later")]
    Busy,
    #[error("ChatGPT credential storage is invalid")]
    Invalid,
    #[error(
        "ChatGPT credential storage must be a private directory (0700) with private files (0600)"
    )]
    Permissions,
    #[error("Could not locate the local credential directory")]
    MissingDirectory,
    /// Retain sources for diagnosis, but never interpolate file paths or JSON values.
    #[error("Could not read or save the dedicated ChatGPT credentials ({operation})")]
    Io {
        operation: &'static str,
        #[source]
        source: io::Error,
    },
    #[error("ChatGPT credential storage is invalid")]
    Json(#[from] serde_json::Error),
}

impl CredentialError {
    pub fn io(operation: &'static str, source: io::Error) -> Self {
        Self::Io { operation, source }
    }
}

impl fmt::Debug for CredentialError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}
