pub(crate) mod credentials;

use crate::config::error::SizeExceeded;
use credentials::CredentialError;
use std::sync::Arc;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ErrorKind {
    Failed,
    Cancelled,
    Modified,
    Unavailable,
    Storage,
    UsageLimit,
    NotEligible,
    Temporary,
    Configuration,
    RefreshUnusable,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub(crate) enum Notification {
    #[default]
    Show,
    Suppress,
}

#[derive(Clone, Copy, Debug, Default)]
enum Recovery {
    #[default]
    None,
    ExplicitTranslation,
}

impl std::fmt::Display for Recovery {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::None => "",
            Self::ExplicitTranslation => ". Automatic translation is paused. After resolving the cause, run a selection or document translation to retry.",
        })
    }
}

#[derive(Clone, thiserror::Error)]
#[error("{cause}{recovery}")]
pub(crate) struct Error {
    #[source]
    cause: Arc<Cause>,
    notification: Notification,
    recovery: Recovery,
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum Cause {
    #[error("{0}")]
    Failed(&'static str),
    #[error(transparent)]
    Transport(#[from] TransportExceeded),
    #[error("Translation canceled")]
    Cancelled,
    #[error("ChatGPT account or authorization changed; retry the preview")]
    Modified,
    #[error("{0}")]
    Credentials(#[source] CredentialError),
    #[error(transparent)]
    Size(#[from] SizeExceeded),
    #[error("OpenAI network request failed or timed out; credentials are retained, retry later")]
    Network(#[source] reqwest::Error),
    #[error(transparent)]
    Remote(#[from] RemoteError),
    #[error("OpenAI stream failed (event={event}); {source}{diagnostics}")]
    Stream {
        event: String,
        #[source]
        source: Arc<Cause>,
        diagnostics: String,
    },
}

// Only remote failures can carry these categories. Cancellation, local storage,
// and size errors cannot be constructed as a remote usage-limit failure.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RemoteKind {
    Failed,
    UsageLimit,
    NotEligible,
    Temporary,
    Configuration,
    RefreshUnusable,
}

#[derive(Clone, Debug)]
pub(crate) struct RemoteError {
    pub kind: RemoteKind,
    pub status: u16,
    pub detail: String,
    pub code: Option<String>,
    pub param: Option<String>,
    pub request_id: Option<String>,
}

impl std::error::Error for RemoteError {}

impl std::fmt::Display for RemoteError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "OpenAI HTTP {}: {}", self.status, self.detail)?;
        for (name, value) in [
            ("code", &self.code),
            ("param", &self.param),
            ("request_id", &self.request_id),
        ] {
            if let Some(value) = value {
                write!(f, " ({name}={value})")?;
            }
        }
        match self.kind {
            RemoteKind::UsageLimit => {
                f.write_str(". Check ChatGPT usage at https://chatgpt.com/settings/usage")
            }
            RemoteKind::RefreshUnusable => f.write_str(
                ". Choose Continue with ChatGPT / Choose account and reconnect the saved account.",
            ),
            _ => Ok(()),
        }
    }
}

impl Cause {
    pub fn kind(&self) -> ErrorKind {
        match self {
            Self::Failed(_) | Self::Size(_) | Self::Transport(_) => ErrorKind::Failed,
            Self::Cancelled => ErrorKind::Cancelled,
            Self::Modified => ErrorKind::Modified,
            Self::Credentials(CredentialError::Busy) => ErrorKind::Unavailable,
            Self::Credentials(_) => ErrorKind::Storage,
            Self::Network(_) => ErrorKind::Temporary,
            Self::Stream { source, .. } => source.kind(),
            Self::Remote(error) => match error.kind {
                RemoteKind::Failed => ErrorKind::Failed,
                RemoteKind::UsageLimit => ErrorKind::UsageLimit,
                RemoteKind::NotEligible => ErrorKind::NotEligible,
                RemoteKind::Temporary => ErrorKind::Temporary,
                RemoteKind::Configuration => ErrorKind::Configuration,
                RemoteKind::RefreshUnusable => ErrorKind::RefreshUnusable,
            },
        }
    }
}

impl From<Cause> for Error {
    fn from(cause: Cause) -> Self {
        Self {
            cause: Arc::new(cause),
            notification: Notification::Show,
            recovery: Recovery::None,
        }
    }
}

impl From<SizeExceeded> for Error {
    fn from(error: SizeExceeded) -> Self {
        Cause::Size(error).into()
    }
}

impl From<CredentialError> for Error {
    fn from(error: CredentialError) -> Self {
        Cause::Credentials(error).into()
    }
}

impl From<RemoteError> for Error {
    fn from(error: RemoteError) -> Self {
        Cause::Remote(error).into()
    }
}

impl Error {
    pub fn kind(&self) -> ErrorKind {
        self.cause().kind()
    }
    pub fn cause(&self) -> &Cause {
        &self.cause
    }
    pub fn notification(&self) -> Notification {
        self.notification
    }
    pub fn suppress_notification(&mut self) {
        self.notification = Notification::Suppress;
    }
    pub fn require_explicit_retry(&mut self) {
        self.recovery = Recovery::ExplicitTranslation;
    }
    pub fn in_stream(self, event: String, diagnostics: String) -> Self {
        Self {
            cause: Arc::new(Cause::Stream {
                event,
                source: self.cause,
                diagnostics,
            }),
            ..self
        }
    }
    pub fn cancelled() -> Self {
        Cause::Cancelled.into()
    }
    pub fn failed(message: &'static str) -> Self {
        Cause::Failed(message).into()
    }
}

// Ordinary diagnostics must not dump credential/JSON/network source internals.
impl std::fmt::Debug for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Error")
            .field("kind", &self.kind())
            .field("message", &self.to_string())
            .field("notification", &self.notification)
            .finish()
    }
}

#[derive(Clone, Copy, Debug)]
pub(crate) enum TransportScope {
    Stream,
    Event,
}

impl std::fmt::Display for TransportScope {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Stream => "stream",
            Self::Event => "SSE event",
        })
    }
}

#[derive(Clone, Debug, thiserror::Error)]
#[error(
    "OpenAI {scope} exceeds {maximum_bytes} bytes derived from translation_limits.output_kib; received {actual_bytes} bytes"
)]
pub(crate) struct TransportExceeded {
    pub scope: TransportScope,
    pub actual_bytes: usize,
    pub maximum_bytes: usize,
}

impl From<TransportExceeded> for Error {
    fn from(error: TransportExceeded) -> Self {
        Cause::Transport(error).into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::error::Error as _;

    #[test]
    fn notification_policy_does_not_change_remote_cause_or_recovery() {
        let mut error: Error = RemoteError {
            kind: RemoteKind::UsageLimit,
            status: 429,
            detail: "Quota reached".into(),
            code: Some("usage_limit".into()),
            param: None,
            request_id: Some("request-1".into()),
        }
        .into();
        error.require_explicit_retry();
        let before = error.to_string();
        error.suppress_notification();
        assert_eq!(error.kind(), ErrorKind::UsageLimit);
        assert_eq!(error.notification(), Notification::Suppress);
        assert_eq!(error.to_string(), before);
        let stream = error.in_stream("response.failed".into(), String::new());
        assert_eq!(stream.kind(), ErrorKind::UsageLimit);
        assert_eq!(stream.notification(), Notification::Suppress);
        assert!(
            stream
                .to_string()
                .contains("run a selection or document translation")
        );
        let Cause::Stream { source, .. } = stream.cause() else {
            panic!("Missing stream cause")
        };
        let Cause::Remote(remote) = source.as_ref() else {
            panic!("Missing remote cause")
        };
        assert_eq!(remote.status, 429);
        assert_eq!(remote.request_id.as_deref(), Some("request-1"));
    }

    #[test]
    fn credential_io_retains_native_cause_without_displaying_it() {
        let error: Error = CredentialError::io(
            "read credentials",
            std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "synthetic-private-path-token",
            ),
        )
        .into();
        assert_eq!(error.kind(), ErrorKind::Storage);
        let Cause::Credentials(storage) = error.cause() else {
            panic!("Missing storage cause")
        };
        let native = storage
            .source()
            .unwrap()
            .downcast_ref::<std::io::Error>()
            .unwrap();
        assert_eq!(native.kind(), std::io::ErrorKind::PermissionDenied);
        assert!(format!("{error}").contains("read credentials"));
        assert!(!format!("{error:?}").contains("synthetic-private-path-token"));
        assert!(!error.to_string().contains("synthetic-private-path-token"));
    }

    #[test]
    fn output_limit_keeps_structured_sizes_across_provider_boundary() {
        let error: Error = SizeExceeded {
            setting: crate::config::limits::SizeSetting::Output,
            actual_bytes: 1025,
            maximum_bytes: 1024,
        }
        .into();
        let Cause::Size(size) = error.cause() else {
            panic!("Missing size cause")
        };
        assert_eq!(size.actual_bytes, 1025);
        assert_eq!(size.maximum_bytes, 1024);
        assert!(error.to_string().contains("translation_limits.output_kib"));
    }
}
