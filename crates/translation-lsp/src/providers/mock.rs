use super::{Context, TextKind, Translated, TranslationProvider, error::Error};

#[derive(Clone)]
pub(crate) struct MockProvider;

const COMMENT: &str = "// Retry only if the remote service\n    // reports a temporary failure.";
const COMMENT_BODY: &str = "Retry only if the remote service\nreports a temporary failure.";
const XML_COMMENT_BODY: &str =
    "<summary>\nRetry only if the remote service\nreports a temporary failure.\n</summary>";
const PARAGRAPH: &str =
    "The `retry_count` value is preserved.\nRetry only after a temporary failure. 🚀";

impl TranslationProvider for MockProvider {
    async fn translate(
        &self,
        _: TextKind,
        text: &str,
        target_language: &str,
        context: Context,
    ) -> Result<Translated, Error> {
        if target_language != "ja" {
            return Err(Error::failed("The mock preview only targets Japanese"));
        }
        let translation = match text.replace("\r\n", "\n").replace('\r', "\n").trim() {
            COMMENT | COMMENT_BODY => {
                "リモートサービスが一時的な障害を報告した場合に限り、再試行する。"
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
            MockProvider
                .translate(TextKind::Selection, "Any text", "ja", Context::Mock)
                .await
                .is_err()
        );
        for text in [COMMENT, COMMENT_BODY, PARAGRAPH] {
            assert!(
                MockProvider
                    .translate(
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
            MockProvider
                .translate(TextKind::Selection, COMMENT_BODY, "en", Context::Mock)
                .await
                .is_err()
        );
    }
}
