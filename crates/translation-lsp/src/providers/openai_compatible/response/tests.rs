use super::*;
use crate::providers::error::ErrorKind;

fn limit() -> SizeLimit {
    SizeLimit::try_from(1).unwrap()
}

fn read(chunks: &[&str]) -> Result<String, Error> {
    let mut reply = Reply::default();
    for chunk in chunks {
        reply.accept(chunk, serde_json::from_str(chunk).ok(), limit())?;
    }
    reply.finish(&["synthetic-secret"])
}

fn delta(text: &str) -> String {
    json!({"choices": [{"index": 0, "delta": {"content": text}, "finish_reason": null}]})
        .to_string()
}

fn finish(reason: &str) -> String {
    json!({"choices": [{"index": 0, "delta": {}, "finish_reason": reason}]}).to_string()
}

#[test]
fn text_of_the_first_choice_is_kept_once_it_stops() {
    let reasoning = json!({"choices": [{"index": 0, "delta": {"reasoning": "think", "reasoning_content": "think"}}]}).to_string();
    let other =
        json!({"choices": [{"index": 1, "delta": {"content": "other"}, "finish_reason": "stop"}]})
            .to_string();
    let usage = json!({"choices": [], "usage": {"completion_tokens": 2}}).to_string();
    // OpenRouter repeats the finished choice in the chunk with the usage.
    let repeated = json!({"choices": [{"index": 0, "delta": {"role": "assistant", "content": ""}, "finish_reason": "stop"}], "usage": {"completion_tokens": 2}}).to_string();
    let ended = format!(
        "{}{}",
        delta("訳文").trim_end_matches("}]}"),
        r#","finish_reason":"stop"}]}"#
    );
    for chunks in [
        vec![delta("訳"), delta("文"), finish("stop"), "[DONE]".into()],
        // Some services leave `[DONE]` out, or finish in the chunk with the text.
        vec![delta("訳"), delta("文"), finish("stop")],
        vec![ended],
        vec![
            reasoning,
            delta("訳"),
            other,
            delta("文"),
            finish("stop"),
            usage,
            " [DONE] ".into(),
        ],
        vec![delta("訳文"), finish("stop"), repeated, "[DONE]".into()],
    ] {
        let chunks: Vec<&str> = chunks.iter().map(String::as_str).collect();
        assert_eq!(read(&chunks).unwrap(), "訳文", "{chunks:?}");
    }
}

#[test]
fn reply_that_does_not_stop_saves_nothing() {
    for (chunks, kind, text) in [
        (
            vec![delta("訳"), finish("length")],
            ErrorKind::Failed,
            "stopped at the output token limit",
        ),
        (
            vec![delta("訳"), finish("content_filter")],
            ErrorKind::Failed,
            "declined to translate this text (category=content_filter)",
        ),
        (
            vec![delta("訳"), finish("tool_calls")],
            ErrorKind::Failed,
            "(finish_reason=tool_calls)",
        ),
        (
            vec![delta("訳"), finish("synthetic-secret")],
            ErrorKind::Failed,
            "(finish_reason=[redacted])",
        ),
        (
            vec![delta("訳"), "[DONE]".into()],
            ErrorKind::Failed,
            "stream ended without finish_reason",
        ),
        (
            vec![finish("stop")],
            ErrorKind::Failed,
            "completed without translation text",
        ),
    ] {
        let chunks: Vec<&str> = chunks.iter().map(String::as_str).collect();
        let error = read(&chunks).unwrap_err();
        assert_eq!(error.kind(), kind, "{error}");
        assert!(error.to_string().contains(text), "{error}");
    }
}

#[test]
fn malformed_streams_are_protocol_errors() {
    for chunks in [
        vec!["not json".to_owned()],
        vec![delta("訳"), finish("stop"), delta("more")],
        vec![delta("訳"), finish("stop"), finish("content_filter")],
        vec![
            delta("訳"),
            finish("stop"),
            finish("length"),
            "[DONE]".into(),
        ],
        vec![delta("訳"), "[DONE]".into(), delta("more")],
    ] {
        let chunks: Vec<&str> = chunks.iter().map(String::as_str).collect();
        let error = read(&chunks).unwrap_err();
        assert!(
            error.to_string().starts_with("OpenAI-compatible API "),
            "{error}"
        );
    }
}

#[test]
fn text_beyond_the_output_limit_is_refused() {
    let long = "a".repeat(1024);
    let error = read(&[&delta(&long), &delta("b")]).unwrap_err();
    assert!(
        error.to_string().contains("translation_limits.output_kib"),
        "{error}"
    );
}
