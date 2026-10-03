use super::{Context, Translated, TranslationProvider, error::Error};

#[derive(Clone)]
pub(crate) struct MockProvider;

const COMMENT: &str = "// Retry only if the remote service\n    // reports a temporary failure.";
const COMMENT_BODY: &str = "Retry only if the remote service\nreports a temporary failure.";
const PARAGRAPH: &str =
    "The `retry_count` value is preserved.\nRetry only after a temporary failure. 🚀";

impl TranslationProvider for MockProvider {
    async fn translate(
        &self,
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
            PARAGRAPH => {
                "`retry_count` の値を維持する。\n一時的な障害が発生した場合に限り再試行する。🚀"
            }
            _ => {
                return Err(Error::failed(
                    "Mock translation only supports the sample text in samples/comment.rs and samples/paragraph.md",
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
                .translate("Any text", "ja", Context::Mock)
                .await
                .is_err()
        );
        for text in [COMMENT, COMMENT_BODY, PARAGRAPH] {
            assert!(
                MockProvider
                    .translate(&text.replace('\n', "\r\n"), "ja", Context::Mock)
                    .await
                    .is_ok()
            );
        }
        assert!(
            MockProvider
                .translate(COMMENT_BODY, "en", Context::Mock)
                .await
                .is_err()
        );
    }
}
