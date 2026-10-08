use super::{Capabilities, Context, TextKind, Translated, TranslationProvider, error::Error};

use std::time::Duration;

#[derive(Clone, Default)]
pub(crate) struct MockOptions {
    pub delay: Duration,
    pub timeout_ms: Option<u64>,
    pub fail: bool,
}

impl MockOptions {
    pub fn apply_timeouts(&self, settings: &mut crate::config::Settings) {
        if let Some(milliseconds) = self.timeout_ms {
            settings.hover_timeout_ms = milliseconds;
            settings.selection_timeout_ms = milliseconds;
            settings.document_timeout_ms = milliseconds;
        }
    }

    pub fn from_args() -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        let mut options = Self::default();
        let mut args = std::env::args().skip(1);
        while let Some(argument) = args.next() {
            match argument.as_str() {
                "--stdio" => (),
                "--mock-delay-ms" => {
                    let milliseconds: u64 = args.next().ok_or("Missing mock delay")?.parse()?;
                    if milliseconds > 30_000 {
                        return Err("Mock delay must be between 0 and 30000 milliseconds".into());
                    }
                    options.delay = Duration::from_millis(milliseconds);
                }
                "--mock-fail" => options.fail = true,
                "--mock-timeout-ms" => {
                    let milliseconds: u64 = args.next().ok_or("Missing mock timeout")?.parse()?;
                    if !(1..=30_000).contains(&milliseconds) {
                        return Err("Mock timeout must be between 1 and 30000 milliseconds".into());
                    }
                    // This fixture-only override caps both kinds of mock request.
                    options.timeout_ms = Some(milliseconds);
                }
                _ => return Err(format!("Unknown argument: {argument}").into()),
            }
        }
        Ok(options)
    }
}

#[derive(Clone, Default)]
pub(crate) struct MockProvider {
    options: MockOptions,
}

impl MockProvider {
    pub fn new(options: MockOptions) -> Self {
        Self { options }
    }
}

const COMMENT: &str = "// Retry only if the remote service\n    // reports a temporary failure.";
const COMMENT_BODY: &str = "Retry only if the remote service\nreports a temporary failure.";
const DOCSTRING_BODY: &str = "Retry only if the remote service\n    reports a temporary failure.";
const DOXYGEN_BODY: &str = "@brief Retry only if the remote service\nreports a temporary failure.";
const XML_COMMENT_BODY: &str =
    "<summary>\nRetry only if the remote service\nreports a temporary failure.\n</summary>";
const PARAGRAPH: &str =
    "The `retry_count` value is preserved.\nRetry only after a temporary failure. 🚀";

impl TranslationProvider for MockProvider {
    fn capabilities() -> Capabilities {
        Capabilities {
            server_name: "Translation LSP (Mock)",
            automatic_selection: false,
            account_menu: None,
            commands: &[],
        }
    }

    async fn translate(
        &self,
        _intent: crate::providers::Intent,
        _: TextKind,
        text: &str,
        target_language: &str,
        context: Context,
    ) -> Result<Translated, Error> {
        tokio::time::sleep(self.options.delay).await;
        if self.options.fail {
            return Err(Error::failed("Simulated mock translation failure"));
        }
        if target_language != "ja" {
            return Err(Error::failed("The mock preview only targets Japanese"));
        }
        let translation = match text.replace("\r\n", "\n").replace('\r', "\n").trim() {
            COMMENT | COMMENT_BODY | DOCSTRING_BODY => {
                "リモートサービスが一時的な障害を報告した場合に限り、再試行する。"
            }
            DOXYGEN_BODY => {
                "@brief リモートサービスが一時的な障害を報告した場合に限り、再試行する。"
            }
            XML_COMMENT_BODY => {
                "<summary>\nリモートサービスが一時的な障害を報告した場合に限り、再試行する。\n</summary>"
            }
            PARAGRAPH => {
                "`retry_count` の値を維持する。\n一時的な障害が発生した場合に限り再試行する。🚀"
            }
            _ => {
                return Err(Error::failed(
                    "Mock translation only supports the bundled sample text",
                ));
            }
        };
        Ok(Translated {
            text: translation.into(),
            context,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test(flavor = "current_thread")]
    async fn arbitrary_text_is_not_presented_as_a_translation() {
        assert!(
            MockProvider::default()
                .translate(
                    crate::providers::Intent::Automatic,
                    TextKind::Selection,
                    "Any text",
                    "ja",
                    Context::Mock
                )
                .await
                .is_err()
        );
        for text in [COMMENT, COMMENT_BODY, PARAGRAPH] {
            assert!(
                MockProvider::default()
                    .translate(
                        crate::providers::Intent::Automatic,
                        TextKind::Selection,
                        &text.replace('\n', "\r\n"),
                        "ja",
                        Context::Mock
                    )
                    .await
                    .is_ok()
            );
        }
        assert!(
            MockProvider::default()
                .translate(
                    crate::providers::Intent::Automatic,
                    TextKind::Selection,
                    COMMENT_BODY,
                    "en",
                    Context::Mock
                )
                .await
                .is_err()
        );
    }
}
