pub(crate) mod credentials;

use crate::config::error::SizeExceeded;
use credentials::CredentialError;
use std::{fmt, sync::Arc};

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

/// The remote service named in user-visible diagnostics.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Service {
    OpenAi,
    Anthropic,
}

impl Service {
    fn network_advice(self) -> &'static str {
        match self {
            Self::OpenAi => "credentials are retained, retry later",
            Self::Anthropic => "retry later",
        }
    }

    fn code_label(self) -> &'static str {
        match self {
            Self::OpenAi => "code",
            Self::Anthropic => "type",
        }
    }
}

impl fmt::Display for Service {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::OpenAi => "OpenAI",
            Self::Anthropic => "Anthropic",
        })
    }
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

impl fmt::Display for Recovery {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
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
    #[error("{0}")]
    Configuration(&'static str),
    #[error("{service} {problem}")]
    Protocol {
        service: Service,
        problem: &'static str,
    },
    #[error(transparent)]
    Transport(#[from] TransportExceeded),
    #[error("Translation canceled")]
    Cancelled,
    /// What the translation was authorized with, which is no longer in use.
    #[error("{0} changed; retry the translation")]
    Modified(&'static str),
    #[error("No terminal window could be opened. To {purpose}, run this in a terminal: {command}")]
    Terminal {
        purpose: &'static str,
        command: String,
    },
    #[error("{0}")]
    Credentials(#[source] CredentialError),
    #[error(transparent)]
    Size(#[from] SizeExceeded),
    #[error("{service} network request failed or timed out; {}", .service.network_advice())]
    Network {
        service: Service,
        #[source]
        source: reqwest::Error,
    },
    #[error(transparent)]
    Remote(#[from] RemoteError),
    #[error(
        "{service} declined to translate this text{}; no translation was saved",
        .category.as_ref().map(|category| format!(" (category={category})")).unwrap_or_default()
    )]
    Refusal {
        service: Service,
        category: Option<String>,
    },
    #[error("{service} stream failed (event={event}); {source}{diagnostics}")]
    Stream {
        service: Service,
        event: String,
        #[source]
        source: Arc<Cause>,
        diagnostics: String,
    },
}

/// Only remote failures can carry these categories. Cancellation, local storage,
/// and size errors cannot be constructed as a remote usage-limit failure. A missing
/// local setting is `Cause::Configuration`.
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
    pub service: Service,
    pub kind: RemoteKind,
    pub status: u16,
    pub detail: String,
    pub code: Option<String>,
    pub param: Option<String>,
    pub request_id: Option<String>,
    pub advice: Option<&'static str>,
}

impl std::error::Error for RemoteError {}

impl fmt::Display for RemoteError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} HTTP {}: {}", self.service, self.status, self.detail)?;
        for (name, value) in [
            (self.service.code_label(), &self.code),
            ("param", &self.param),
            ("request_id", &self.request_id),
        ] {
            if let Some(value) = value {
                write!(f, " ({name}={value})")?;
            }
        }
        if let Some(advice) = self.advice {
            write!(f, ". {advice}")?;
        }
        Ok(())
    }
}

impl Cause {
    pub fn kind(&self) -> ErrorKind {
        match self {
            Self::Failed(_)
            | Self::Terminal { .. }
            | Self::Protocol { .. }
            | Self::Size(_)
            | Self::Transport(_)
            | Self::Refusal { .. } => ErrorKind::Failed,
            Self::Configuration(_) => ErrorKind::Configuration,
            Self::Cancelled => ErrorKind::Cancelled,
            Self::Modified(_) => ErrorKind::Modified,
            Self::Credentials(CredentialError::Busy) => ErrorKind::Unavailable,
            Self::Credentials(_) => ErrorKind::Storage,
            Self::Network { .. } => ErrorKind::Temporary,
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

impl Cause {
    /// Only a temporary failure the service itself reported may be sent again.
    /// A network failure is also temporary, but the request may be in progress
    /// or partly answered, so it is left for a manual retry.
    fn service_reported_temporary(&self) -> bool {
        match self {
            Self::Remote(error) => error.kind == RemoteKind::Temporary,
            Self::Stream { source, .. } => source.service_reported_temporary(),
            _ => false,
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
    pub fn retryable(&self) -> bool {
        self.cause.service_reported_temporary()
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
    pub fn in_stream(self, service: Service, event: String, diagnostics: String) -> Self {
        Self {
            cause: Arc::new(Cause::Stream {
                service,
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
    pub fn configuration(message: &'static str) -> Self {
        Cause::Configuration(message).into()
    }
    pub fn protocol(service: Service, problem: &'static str) -> Self {
        Cause::Protocol { service, problem }.into()
    }
}

/// Ordinary diagnostics must not dump credential/JSON/network source internals.
impl fmt::Debug for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
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

impl fmt::Display for TransportScope {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Stream => "stream",
            Self::Event => "SSE event",
        })
    }
}

#[derive(Clone, Debug, thiserror::Error)]
#[error(
    "{service} {scope} exceeds {maximum_bytes} bytes derived from translation_limits.output_kib; received {actual_bytes} bytes"
)]
pub(crate) struct TransportExceeded {
    pub service: Service,
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
    use crate::config::limits::SizeSetting;

    use super::*;
    use std::{error::Error as _, io};

    #[test]
    fn notification_policy_does_not_change_remote_cause_or_recovery() {
        let mut error: Error = RemoteError {
            service: Service::OpenAi,
            kind: RemoteKind::UsageLimit,
            status: 429,
            detail: "Quota reached".into(),
            code: Some("usage_limit".into()),
            param: None,
            request_id: Some("request-1".into()),
            advice: None,
        }
        .into();
        error.require_explicit_retry();
        let before = error.to_string();
        error.suppress_notification();
        assert_eq!(error.kind(), ErrorKind::UsageLimit);
        assert_eq!(error.notification(), Notification::Suppress);
        assert_eq!(error.to_string(), before);
        let stream = error.in_stream(Service::OpenAi, "response.failed".into(), String::new());
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
    fn only_temporary_failures_reported_by_the_service_are_retryable() {
        let remote = |kind| -> Error {
            RemoteError {
                service: Service::Anthropic,
                kind,
                status: 529,
                detail: "Overloaded".into(),
                code: None,
                param: None,
                request_id: None,
                advice: None,
            }
            .into()
        };
        let temporary = remote(RemoteKind::Temporary);
        assert_eq!(temporary.kind(), ErrorKind::Temporary);
        assert!(temporary.retryable());
        assert!(
            temporary
                .in_stream(Service::Anthropic, "error".into(), String::new())
                .retryable()
        );
        for kind in [
            RemoteKind::Failed,
            RemoteKind::UsageLimit,
            RemoteKind::NotEligible,
            RemoteKind::Configuration,
            RemoteKind::RefreshUnusable,
        ] {
            assert!(!remote(kind).retryable());
            assert!(
                !remote(kind)
                    .in_stream(Service::Anthropic, "error".into(), String::new())
                    .retryable()
            );
        }
        for error in [
            Error::cancelled(),
            Error::failed("Translation timed out"),
            Error::configuration("Missing setting"),
            Error::protocol(Service::OpenAi, "SSE stream is invalid or truncated"),
            Cause::Refusal {
                service: Service::Anthropic,
                category: None,
            }
            .into(),
            Cause::Modified("ChatGPT account or authorization").into(),
        ] {
            assert!(!error.retryable(), "{error}");
        }
    }

    #[test]
    fn credential_io_retains_native_cause_without_displaying_it() {
        let error: Error = CredentialError::io(
            "read credentials",
            io::Error::new(
                io::ErrorKind::PermissionDenied,
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
            .downcast_ref::<io::Error>()
            .unwrap();
        assert_eq!(native.kind(), io::ErrorKind::PermissionDenied);
        assert!(format!("{error}").contains("read credentials"));
        assert!(!format!("{error:?}").contains("synthetic-private-path-token"));
        assert!(!error.to_string().contains("synthetic-private-path-token"));
    }

    #[test]
    fn output_limit_keeps_structured_sizes_across_provider_boundary() {
        let error: Error = SizeExceeded {
            setting: SizeSetting::Output,
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
