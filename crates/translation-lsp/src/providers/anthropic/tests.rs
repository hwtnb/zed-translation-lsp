use std::time::{Duration, Instant};

use serde_json::{Value, json};

use super::test_support::{Fixture, KEY, Reply, event, started, text_block};
use super::*;
use crate::providers::error::{ErrorKind, Notification};

mod response;

const SOURCE: &str = "Private source sentence.";
const PAUSED: &str = "Automatic translation is paused";

async fn translate(provider: &AnthropicProvider, intent: Intent) -> Result<String, Error> {
    provider
        .translate(intent, SOURCE, "ja", Context::Anthropic)
        .await
        .map(|translated| translated.text)
}

fn automatic(provider: &AnthropicProvider) -> Result<u64, Error> {
    provider.admit(&Context::Anthropic, Intent::Automatic)
}

#[tokio::test(flavor = "current_thread")]
async fn request_sends_only_the_documented_headers_and_fields() {
    for (effort, thinking, kib, max_tokens) in [
        (AnthropicEffort::Low, false, 4096, 64_000),
        (AnthropicEffort::High, false, 1, 1024),
        (AnthropicEffort::Medium, true, 62, 63_488),
        (AnthropicEffort::Max, true, 63, 64_000),
    ] {
        let fixture = Fixture::new().await;
        let mut provider = fixture.provider();
        provider.effort = effort;
        provider.thinking = thinking;
        provider.output_limit = SizeLimit::try_from(kib).unwrap();
        provider.source_language = "en".into();
        let source = "Read `retry_count`.\r\n@docs/private.md\n/clear";
        let translated = provider
            .translate(Intent::Automatic, source, "ja", Context::Anthropic)
            .await
            .unwrap();
        assert_eq!(translated.text, format!("訳{source}"));
        assert_eq!(translated.context, Context::Anthropic);
        let state = fixture.state.lock().unwrap();
        assert_eq!(state.records.len(), 1);
        let record = &state.records[0];
        assert_eq!(record.path, "/v1/messages");
        assert_eq!(record.headers["x-api-key"], KEY);
        assert_eq!(record.headers["anthropic-version"], "2023-06-01");
        assert_eq!(record.headers["content-type"], "application/json");
        for header in ["anthropic-beta", "authorization"] {
            assert!(!record.headers.contains_key(header), "{header}");
        }
        let request: Value = serde_json::from_str(&record.body).unwrap();
        let mut fields: Vec<_> = request.as_object().unwrap().keys().collect();
        fields.sort();
        assert_eq!(
            fields,
            [
                "max_tokens",
                "messages",
                "model",
                "output_config",
                "stream",
                "system",
                "thinking"
            ]
        );
        assert_eq!(request["model"], "claude-haiku-5-5");
        assert_eq!(request["max_tokens"], max_tokens);
        assert_eq!(request["stream"], true);
        assert_eq!(request["system"], prompt::instructions("ja", "en"));
        assert_eq!(
            request["messages"],
            json!([{"role": "user", "content": source}])
        );
        assert_eq!(
            request["output_config"],
            json!({"effort": serde_json::to_value(effort).unwrap()})
        );
        assert_eq!(
            request["thinking"],
            json!({"type": if thinking { "adaptive" } else { "disabled" }})
        );
    }
}

#[tokio::test(flavor = "current_thread")]
async fn request_carries_the_thinking_resolved_from_the_settings() {
    use crate::config::anthropic_thinking;
    let fixture = Fixture::new().await;
    for (model, effort, setting, expected) in [
        ("claude-haiku-5-5", AnthropicEffort::Low, None, "disabled"),
        ("claude-haiku-5-5", AnthropicEffort::High, None, "disabled"),
        ("claude-haiku-5-5", AnthropicEffort::Xhigh, None, "adaptive"),
        ("claude-haiku-5-5", AnthropicEffort::Max, None, "adaptive"),
        ("claude-opus-5-5", AnthropicEffort::Low, None, "adaptive"),
        ("claude-opus-5-5", AnthropicEffort::Max, None, "adaptive"),
        (
            "claude-haiku-5-5",
            AnthropicEffort::Low,
            Some(true),
            "adaptive",
        ),
        (
            "claude-opus-5-5",
            AnthropicEffort::Low,
            Some(false),
            "disabled",
        ),
    ] {
        let mut provider = fixture.provider();
        provider.model = model.into();
        provider.effort = effort;
        provider.thinking = anthropic_thinking(model, effort, setting);
        translate(&provider, Intent::Explicit).await.unwrap();
        let record = fixture.state.lock().unwrap().records.pop().unwrap();
        let request: Value = serde_json::from_str(&record.body).unwrap();
        assert_eq!(request["model"], model);
        assert_eq!(
            request["output_config"]["effort"],
            serde_json::to_value(effort).unwrap()
        );
        assert_eq!(
            request["thinking"],
            json!({"type": expected}),
            "{model} {effort:?} {setting:?}"
        );
    }
}

#[tokio::test(flavor = "current_thread")]
async fn http_failures_follow_the_documented_status_and_type_table() {
    let detail = format!("rejected {KEY} while reading {SOURCE}");
    let documented = |status, error_type| Reply::failure(status, error_type, &detail);
    for (reply, error_type, kind, advice) in [
        (
            documented(401, "authentication_error"),
            Some("authentication_error"),
            ErrorKind::Configuration,
            API_KEY_VARIABLE,
        ),
        (
            documented(402, "billing_error"),
            Some("billing_error"),
            ErrorKind::UsageLimit,
            "Billing page in the Claude Console",
        ),
        (
            documented(403, "permission_error"),
            Some("permission_error"),
            ErrorKind::NotEligible,
            "",
        ),
        (
            documented(404, "not_found_error"),
            Some("not_found_error"),
            ErrorKind::Configuration,
            "Check provider.model",
        ),
        (
            documented(400, "invalid_request_error"),
            Some("invalid_request_error"),
            ErrorKind::Failed,
            "",
        ),
        (
            documented(413, "request_too_large"),
            Some("request_too_large"),
            ErrorKind::Failed,
            "",
        ),
        (
            documented(429, "rate_limit_error"),
            Some("rate_limit_error"),
            ErrorKind::Temporary,
            "",
        ),
        (
            documented(500, "api_error"),
            Some("api_error"),
            ErrorKind::Temporary,
            "",
        ),
        (
            documented(529, "overloaded_error"),
            Some("overloaded_error"),
            ErrorKind::Temporary,
            "",
        ),
        // Bodies without a documented type are classified by status alone.
        (
            Reply::new(503, "plain unavailable detail"),
            None,
            ErrorKind::Temporary,
            "plain unavailable detail",
        ),
        (
            Reply::new(401, String::new()),
            None,
            ErrorKind::Configuration,
            API_KEY_VARIABLE,
        ),
        (
            documented(418, "future_error"),
            Some("future_error"),
            ErrorKind::Failed,
            "",
        ),
        (
            Reply::new(
                307,
                json!({"detail":"redirect must not be followed"}).to_string(),
            ),
            None,
            ErrorKind::Failed,
            "",
        ),
    ] {
        let fixture = Fixture::new().await;
        let status = reply.status;
        fixture.always(Some(reply));
        fixture.state.lock().unwrap().retry_after = Some("0".into());
        let provider = fixture.provider();
        let error = translate(&provider, Intent::Automatic).await.unwrap_err();
        let message = error.to_string();
        assert_eq!(error.kind(), kind, "{message}");
        assert!(message.starts_with(&format!("Anthropic HTTP {status}: ")));
        assert!(message.contains("(request_id=req_synthetic)"), "{message}");
        assert!(message.contains(advice), "{message}");
        if let Some(error_type) = error_type {
            assert!(message.contains(&format!("(type={error_type})")));
            assert!(message.contains("rejected [redacted] while reading [redacted]"));
        }
        for secret in [KEY, SOURCE] {
            assert!(!message.contains(secret));
            assert!(!format!("{error:?}").contains(secret));
        }
        let stops = matches!(
            kind,
            ErrorKind::Configuration | ErrorKind::UsageLimit | ErrorKind::NotEligible
        );
        assert_eq!(message.contains(PAUSED), stops);
        assert_eq!(
            fixture.calls(),
            if kind == ErrorKind::Temporary { 2 } else { 1 }
        );
        match kind {
            ErrorKind::Failed => {
                automatic(&provider).unwrap();
            }
            _ => assert_eq!(automatic(&provider).unwrap_err().kind(), kind),
        }
    }
}

#[tokio::test(flavor = "current_thread")]
async fn stream_errors_are_classified_like_http_failures_after_status_200() {
    let failed = |error_type: &str| {
        format!(
            "{}{}{}",
            started(),
            text_block(0, "途中まで"),
            event(
                "error",
                json!({"type":"error","error":{"type":error_type,
                    "message":format!("stopped {KEY} while reading {SOURCE}")}}),
            )
        )
    };
    for (error_type, kind) in [
        ("overloaded_error", ErrorKind::Temporary),
        ("api_error", ErrorKind::Temporary),
        ("rate_limit_error", ErrorKind::Temporary),
        ("billing_error", ErrorKind::UsageLimit),
        ("authentication_error", ErrorKind::Configuration),
        ("invalid_request_error", ErrorKind::Failed),
        ("future_error", ErrorKind::Failed),
    ] {
        for recovers in [false, true] {
            let fixture = Fixture::new().await;
            {
                let mut state = fixture.state.lock().unwrap();
                state.retry_after = Some("0".into());
                state.omit_request_id = true;
                state.scripted.push_back(Reply::stream(failed(error_type)));
                if !recovers {
                    state.scripted.push_back(Reply::stream(failed(error_type)));
                }
            }
            let provider = fixture.provider();
            let result = translate(&provider, Intent::Automatic).await;
            let temporary = kind == ErrorKind::Temporary;
            assert_eq!(fixture.calls(), if temporary { 2 } else { 1 });
            if temporary && recovers {
                assert_eq!(result.unwrap(), format!("訳{SOURCE}"));
                continue;
            }
            let error = result.unwrap_err();
            let message = error.to_string();
            assert_eq!(error.kind(), kind, "{message}");
            assert!(
                message.starts_with("Anthropic stream failed (event=error); Anthropic HTTP 200: "),
                "{message}"
            );
            assert!(message.contains(&format!("(type={error_type})")));
            assert!(message.contains("stopped [redacted] while reading [redacted]"));
            assert!(!message.contains("途中まで"));
            assert!(!message.contains("request_id"));
        }
    }
    let fixture = Fixture::new().await;
    fixture.state.lock().unwrap().omit_request_id = true;
    fixture.always(Some(Reply::failure(
        400,
        "invalid_request_error",
        "invalid",
    )));
    let error = translate(&fixture.provider(), Intent::Automatic)
        .await
        .unwrap_err();
    assert!(error.to_string().contains("(request_id=req_body)"));
}

#[tokio::test(flavor = "current_thread")]
async fn rate_limits_retry_once_after_a_bounded_numeric_delay() {
    for delay in [
        "1",
        "121",
        "18446744073709551615",
        "Wed, 21 Oct 2037 07:28:00 GMT",
    ] {
        let fixture = Fixture::new().await;
        fixture.state.lock().unwrap().retry_after = Some(delay.into());
        fixture.reply(Reply::failure(429, "rate_limit_error", "Slow down"));
        let provider = fixture.provider();
        let started = Instant::now();
        let result = translate(&provider, Intent::Automatic).await;
        if delay == "1" {
            assert_eq!(result.unwrap(), format!("訳{SOURCE}"));
            assert!(started.elapsed() >= Duration::from_secs(1));
            assert_eq!(fixture.calls(), 2);
        } else {
            assert_eq!(result.unwrap_err().kind(), ErrorKind::Temporary);
            assert_eq!(fixture.calls(), 1);
        }
    }
    // Without the header, the default two-second wait applies and can be dropped.
    let fixture = Fixture::new().await;
    fixture.reply(Reply::failure(529, "overloaded_error", "Overloaded"));
    let provider = fixture.provider();
    let request = translate(&provider, Intent::Automatic);
    tokio::pin!(request);
    assert!(
        tokio::time::timeout(Duration::from_millis(300), &mut request)
            .await
            .is_err()
    );
    assert_eq!(fixture.calls(), 1);
}

#[tokio::test(flavor = "current_thread")]
async fn interrupted_responses_and_refused_connections_are_not_sent_again() {
    let partial = format!("{}{}", started(), text_block(0, "途中まで"));
    for (first, status) in [
        (Reply::interrupted(partial), 200),
        (
            Reply {
                interrupted: true,
                ..Reply::failure(529, "overloaded_error", "Overloaded")
            },
            529,
        ),
    ] {
        for intent in [Intent::Automatic, Intent::Explicit] {
            let fixture = Fixture::new().await;
            // A wrongly repeated request would be answered at once with a translation.
            fixture.state.lock().unwrap().retry_after = Some("0".into());
            fixture.reply(first.clone());
            let provider = fixture.provider();
            let error = translate(&provider, intent).await.unwrap_err();
            assert_eq!(error.kind(), ErrorKind::Temporary, "{status}: {error}");
            assert!(!error.retryable());
            assert!(
                error
                    .to_string()
                    .starts_with("Anthropic network request failed or timed out")
            );
            assert_eq!(fixture.calls(), 1, "HTTP {status}");
        }
    }

    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
        .await
        .unwrap();
    let api = format!("http://{}/v1", listener.local_addr().unwrap());
    let connections = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let accepted = connections.clone();
    let refusing = tokio::spawn(async move {
        loop {
            let (socket, _) = listener.accept().await.unwrap();
            accepted.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            drop(socket);
        }
    });
    let provider = AnthropicProvider::synthetic(api, Some(KEY));
    let error = translate(&provider, Intent::Automatic).await.unwrap_err();
    assert_eq!(error.kind(), ErrorKind::Temporary);
    assert!(!error.retryable());
    assert_eq!(connections.load(std::sync::atomic::Ordering::SeqCst), 1);
    // The short pause for automatic work still follows a network failure.
    assert_eq!(
        automatic(&provider).unwrap_err().kind(),
        ErrorKind::Temporary
    );
    refusing.abort();
}

#[tokio::test(flavor = "current_thread")]
async fn billing_stop_pauses_automatic_work_until_an_explicit_translation_succeeds() {
    let fixture = Fixture::new().await;
    fixture.always(Some(Reply::failure(402, "billing_error", "No credit")));
    let provider = fixture.provider();
    let admitted = automatic(&provider).unwrap();
    let error = translate(&provider, Intent::Automatic).await.unwrap_err();
    assert_eq!(error.kind(), ErrorKind::UsageLimit);
    assert_eq!(error.notification(), Notification::Show);
    assert!(
        error
            .to_string()
            .contains("run a selection or document translation to retry")
    );

    // Work admitted before the stop fails without a request or a second notice.
    let drained = provider
        .check_admission(&Context::Anthropic, admitted)
        .unwrap_err();
    assert_eq!(drained.notification(), Notification::Suppress);
    assert_eq!(drained.to_string(), error.to_string());
    assert_eq!(
        automatic(&provider).unwrap_err().kind(),
        ErrorKind::UsageLimit
    );
    assert_eq!(
        translate(&provider, Intent::Automatic)
            .await
            .unwrap_err()
            .kind(),
        ErrorKind::UsageLimit
    );
    assert_eq!(fixture.calls(), 1);

    // An explicit request is admitted, and its own failure is reported.
    let fresh = provider
        .admit(&Context::Anthropic, Intent::Explicit)
        .unwrap();
    provider
        .check_admission(&Context::Anthropic, fresh)
        .unwrap();
    let retried = translate(&provider, Intent::Explicit).await.unwrap_err();
    assert_eq!(retried.notification(), Notification::Show);
    assert_eq!(fixture.calls(), 2);
    assert!(automatic(&provider).is_err());

    fixture.always(None);
    assert_eq!(
        translate(&provider, Intent::Explicit).await.unwrap(),
        format!("訳{SOURCE}")
    );
    assert_eq!(fixture.calls(), 3);
    automatic(&provider).unwrap();
}

#[tokio::test(flavor = "current_thread")]
async fn missing_or_unusable_key_is_a_configuration_error_without_any_request() {
    let fixture = Fixture::new().await;
    for key in [
        None,
        Some(""),
        Some(" \n"),
        Some("sk-ant synthetic"),
        Some("sk-ant-合成"),
    ] {
        let provider = AnthropicProvider::synthetic(format!("{}/v1", fixture.base), key);
        assert_eq!(provider.context().unwrap(), Context::Anthropic);
        let mut errors = Vec::new();
        for intent in [Intent::Automatic, Intent::Explicit] {
            errors.push(provider.admit(&Context::Anthropic, intent).unwrap_err());
            errors.push(translate(&provider, intent).await.unwrap_err());
        }
        for error in errors {
            assert_eq!(error.kind(), ErrorKind::Configuration);
            let message = error.to_string();
            assert!(message.contains(API_KEY_VARIABLE), "{message}");
            assert!(message.contains("lsp.translation-lsp.binary.env"));
            assert!(message.contains("restart the language server"));
            // Only a restart can supply the key, so no retry is suggested.
            assert!(!message.contains(PAUSED));
            if let Some(key) = key.map(str::trim).filter(|key| !key.is_empty()) {
                assert!(!message.contains(key));
                assert!(!format!("{error:?}").contains(key));
            }
        }
    }
    assert_eq!(fixture.calls(), 0);
    // Surrounding whitespace from a settings file is not part of the key.
    let provider =
        AnthropicProvider::synthetic(format!("{}/v1", fixture.base), Some(&format!(" {KEY}\n")));
    translate(&provider, Intent::Explicit).await.unwrap();
    assert_eq!(
        fixture.state.lock().unwrap().records[0].headers["x-api-key"],
        KEY
    );
}

#[tokio::test(flavor = "current_thread")]
async fn key_stays_out_of_debug_output_and_network_errors() {
    let key = ApiKey::parse(Some(KEY.into())).unwrap();
    assert_eq!(format!("{key:?}"), "ApiKey([redacted])");
    assert!(key.header.is_sensitive());
    assert!(!format!("{:?}", key.header).contains(KEY));

    let unused = {
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
            .await
            .unwrap();
        format!("http://{}/v1", listener.local_addr().unwrap())
    };
    let provider = AnthropicProvider::synthetic(unused.clone(), Some(KEY));
    let error = translate(&provider, Intent::Automatic).await.unwrap_err();
    assert_eq!(error.kind(), ErrorKind::Temporary);
    assert!(
        error
            .to_string()
            .starts_with("Anthropic network request failed or timed out")
    );
    for hidden in [KEY, SOURCE, unused.as_str()] {
        assert!(!error.to_string().contains(hidden));
        assert!(!format!("{error:?}").contains(hidden));
    }
}

#[tokio::test(flavor = "current_thread")]
async fn dropping_the_translation_closes_its_request() {
    let fixture = Fixture::new().await;
    {
        let mut state = fixture.state.lock().unwrap();
        state.hold = true;
        state.scripted.push_back(Reply::stream(format!(
            "{}{}",
            started(),
            text_block(0, "途中まで")
        )));
    }
    let provider = fixture.provider();
    {
        let request = translate(&provider, Intent::Automatic);
        tokio::pin!(request);
        tokio::select! {
            result = &mut request => panic!("An open stream must not complete: {result:?}"),
            () = fixture.open.wait(1) => (),
        }
    }
    fixture.open.wait(0).await;
    assert_eq!(fixture.calls(), 1);
    // Abandoned work is neither an error nor a reason to pause automatic translation.
    automatic(&provider).unwrap();
}

#[test]
fn foreign_contexts_are_rejected_before_any_work() {
    let provider = AnthropicProvider::synthetic("http://127.0.0.1:9/v1".into(), Some(KEY));
    assert!(provider.admit(&Context::Mock, Intent::Explicit).is_err());
    provider.check_admission(&Context::Mock, 0).unwrap();
    assert_eq!(
        AnthropicProvider::capabilities().server_name,
        "Translation LSP (Anthropic)"
    );
}
