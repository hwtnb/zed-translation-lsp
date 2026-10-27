use serde_json::{Value, json};

use super::*;
use crate::providers::{
    anthropic::test_support::KeyDirectory,
    credential_directory,
    error::ErrorKind,
    http_fixture::{HttpFixture, Reply},
};

const KEY: &str = "sk-or-synthetic-key";
const SOURCE: &str = "Private source sentence.";

fn chunk(value: Value) -> String {
    format!("data: {value}\n\n")
}

/// A reply in two pieces, a finish, and `[DONE]`.
pub(super) fn completed(text: &str) -> String {
    let middle = (0..=text.len() / 2)
        .rev()
        .find(|index| text.is_char_boundary(*index))
        .unwrap();
    let (first, second) = text.split_at(middle);
    format!(
        "{}{}{}data: [DONE]\n\n",
        chunk(
            json!({"choices": [{"index": 0, "delta": {"role": "assistant", "content": first}, "finish_reason": null}]})
        ),
        chunk(
            json!({"choices": [{"index": 0, "delta": {"content": second}, "finish_reason": null}]})
        ),
        chunk(json!({"choices": [{"index": 0, "delta": {}, "finish_reason": "stop"}]})),
    )
}

/// The file of the key for the fixture's origin.
fn file(fixture: &HttpFixture) -> String {
    let port = fixture.base.rsplit(':').next().unwrap();
    format!("api-key-http-127.0.0.1-{port}")
}

fn save(directory: &Path, file: &str, key: &str) {
    credential_directory::replace(directory, file, key.as_bytes()).unwrap();
}

async fn translate(provider: &CompatibleProvider, output: Output) -> Result<String, Error> {
    provider
        .translate(Intent::Explicit, output, SOURCE, "ja", provider.context()?)
        .await
        .map(|translated| translated.text)
}

#[tokio::test(flavor = "current_thread")]
async fn request_goes_to_the_base_url_with_the_documented_fields() {
    let fixture = HttpFixture::new().await;
    let keys = KeyDirectory::new();
    let provider = CompatibleProvider::synthetic(&format!("{}/api/v1/", fixture.base), &keys.path)
        .with_options(OutputOptions::new(
            CompatibleModel {
                model: "synthetic/small".into(),
                effort: Some("low".into()),
            },
            Some(CompatibleModel {
                model: "synthetic/large".into(),
                effort: None,
            }),
        ));
    for output in [Output::Range, Output::Document] {
        fixture.reply(Reply::stream(completed("訳文です")));
        assert_eq!(translate(&provider, output).await.unwrap(), "訳文です");
    }
    let records = fixture.records();
    let [range, document] = &records[..] else {
        panic!("Expected two requests")
    };
    for record in [range, document] {
        assert_eq!(record.path, "/api/v1/chat/completions");
        assert!(record.headers.get("authorization").is_none());
        assert_eq!(record.body["stream"], true);
        let messages = record.body["messages"].as_array().unwrap();
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0]["role"], "system");
        assert!(messages[0]["content"].as_str().unwrap().contains("ja"));
        assert_eq!(messages[1]["role"], "user");
        assert!(messages[1]["content"].as_str().unwrap().contains(SOURCE));
    }
    assert_eq!(range.body["model"], "synthetic/small");
    assert_eq!(range.body["reasoning_effort"], "low");
    assert_eq!(range.body["max_tokens"], 16_384 + 4 * SOURCE.len());
    assert_eq!(document.body["model"], "synthetic/large");
    // Neither an effort nor a limit is sent unless it applies.
    assert!(document.body.get("reasoning_effort").is_none());
    assert!(document.body.get("max_tokens").is_none());
}

#[tokio::test(flavor = "current_thread")]
async fn key_is_sent_only_to_its_own_origin_and_followed_without_a_restart() {
    let fixture = HttpFixture::new().await;
    let keys = KeyDirectory::with_key("sk-ant-synthetic-key");
    // A key saved for another port of this computer is not this origin's.
    save(
        &keys.path,
        "api-key-http-127.0.0.1-9",
        "sk-synthetic-other-port",
    );
    save(
        &keys.path,
        "api-key-https-api.openai.com-443",
        "sk-proj-synthetic-key",
    );
    let provider = CompatibleProvider::synthetic(&format!("{}/v1", fixture.base), &keys.path);
    assert_eq!(provider.context().unwrap(), None);
    assert_eq!(provider.auth_ui_state(), AuthUiState::Ready);
    let titles = |provider: &CompatibleProvider| -> Vec<&str> {
        provider
            .account_commands()
            .iter()
            .map(|command| command.title)
            .collect()
    };
    assert_eq!(titles(&provider), ["Save OpenAI-compatible API key"]);
    fixture.reply(Reply::stream(completed("訳")));
    translate(&provider, Output::Range).await.unwrap();

    // A translation prepared without a key is not sent with the key saved since.
    let without = provider.context().unwrap();
    save(&keys.path, &file(&fixture), KEY);
    let error = provider
        .translate(Intent::Explicit, Output::Range, SOURCE, "ja", without)
        .await
        .err()
        .unwrap();
    assert_eq!(error.kind(), ErrorKind::Modified);
    assert_eq!(
        titles(&provider),
        [
            "Replace OpenAI-compatible API key",
            "Remove OpenAI-compatible API key"
        ]
    );
    fixture.reply(Reply::stream(completed("訳")));
    translate(&provider, Output::Range).await.unwrap();

    let records = fixture.records();
    assert_eq!(records.len(), 2);
    assert!(records[0].headers.get("authorization").is_none());
    assert_eq!(records[1].headers["authorization"], format!("Bearer {KEY}"));
}

#[tokio::test(flavor = "current_thread")]
async fn key_file_that_cannot_be_used_stops_translation_without_a_request() {
    let fixture = HttpFixture::new().await;
    let keys = KeyDirectory::new();
    let provider = CompatibleProvider::synthetic(&format!("{}/v1", fixture.base), &keys.path);
    for contents in ["", "two words"] {
        save(&keys.path, &file(&fixture), contents);
        let error = provider.context().unwrap_err();
        assert_eq!(error.kind(), ErrorKind::Configuration);
        assert!(
            error
                .to_string()
                .starts_with("OpenAI-compatible API key file "),
            "{error}"
        );
        assert!(!error.to_string().contains("two words"));
        assert_eq!(provider.auth_ui_state(), AuthUiState::SignedOut);
    }
    assert!(fixture.records().is_empty());
}

#[tokio::test(flavor = "current_thread")]
async fn failures_are_told_apart_by_status() {
    for (status, body, kind, text) in [
        (
            401,
            json!({"error": {"message": "No auth credentials found", "code": 401}}),
            ErrorKind::Configuration,
            "The service did not accept the key saved for http://127.0.0.1:",
        ),
        (
            402,
            json!({"error": {"message": "Insufficient credits", "code": 402}}),
            ErrorKind::UsageLimit,
            "OpenAI-compatible API HTTP 402: Insufficient credits (code=402)",
        ),
        (
            404,
            json!({"error": {"message": "model \"synthetic\" not found", "type": "not_found_error"}}),
            ErrorKind::Configuration,
            "Check provider.base_url and the model names",
        ),
        (
            403,
            json!({"error": "Forbidden"}),
            ErrorKind::NotEligible,
            "OpenAI-compatible API HTTP 403: Forbidden",
        ),
        (
            400,
            json!([{"error": {"message": "Invalid reasoning_effort", "status": "INVALID_ARGUMENT"}}]),
            ErrorKind::Failed,
            "OpenAI-compatible API HTTP 400: Invalid reasoning_effort",
        ),
    ] {
        let fixture = HttpFixture::new().await;
        let keys = KeyDirectory::new();
        save(&keys.path, &file(&fixture), KEY);
        let provider = CompatibleProvider::synthetic(&format!("{}/v1", fixture.base), &keys.path);
        fixture.reply(Reply::failure(status, body.to_string()));
        let error = translate(&provider, Output::Range).await.unwrap_err();
        assert_eq!(error.kind(), kind, "{error}");
        assert!(error.to_string().contains(text), "{error}");
        assert!(!error.to_string().contains(KEY));
    }
}

#[tokio::test(flavor = "current_thread")]
async fn refusal_without_a_key_names_the_origin_that_has_none() {
    let fixture = HttpFixture::new().await;
    let keys = KeyDirectory::new();
    // A key saved for another service is not sent here.
    save(
        &keys.path,
        "api-key-https-generativelanguage.googleapis.com-443",
        KEY,
    );
    let provider = CompatibleProvider::synthetic(&format!("{}/v1", fixture.base), &keys.path);
    fixture.reply(Reply::failure(
        401,
        json!({"error": {"message": "Missing Authentication header", "code": 401}}).to_string(),
    ));
    let error = translate(&provider, Output::Range).await.unwrap_err();
    assert_eq!(error.kind(), ErrorKind::Configuration, "{error}");
    assert!(
        error.to_string().contains(&format!(
            "No key is saved for {}, where provider.base_url sends requests. Run \"OpenAI-compatible API key…\" from the code actions to save one",
            fixture.base
        )),
        "{error}"
    );
    assert!(fixture.records()[0].headers.get("authorization").is_none());
}

#[tokio::test(flavor = "current_thread")]
async fn overload_is_sent_again_once_and_an_error_chunk_ends_the_stream() {
    let fixture = HttpFixture::new().await;
    let keys = KeyDirectory::new();
    let provider = CompatibleProvider::synthetic(&format!("{}/v1", fixture.base), &keys.path);
    let mut busy = Reply::failure(503, r#"{"error":{"message":"Overloaded"}}"#);
    busy.headers.push(("retry-after", "0"));
    fixture.reply(busy);
    fixture.reply(Reply::stream(completed("訳")));
    assert_eq!(translate(&provider, Output::Range).await.unwrap(), "訳");
    assert_eq!(fixture.records().len(), 2);

    let failed = |code: Value| {
        Reply::stream(format!(
            "{}{}",
            chunk(
                json!({"choices": [{"index": 0, "delta": {"content": "部分"}, "finish_reason": null}]})
            ),
            chunk(
                json!({"error": {"code": code, "message": "Upstream failed"}, "choices": [{"index": 0, "delta": {"content": ""}, "finish_reason": "error"}]})
            ),
        ))
    };
    // A temporary error in the stream, by its number or its name, is sent
    // again once as well.
    for code in [json!(502), json!("server_error")] {
        let sent = fixture.records().len();
        fixture.reply(failed(code.clone()));
        fixture.reply(Reply::stream(completed("訳")));
        assert_eq!(
            translate(&provider, Output::Range).await.unwrap(),
            "訳",
            "{code}"
        );
        assert_eq!(fixture.records().len(), sent + 2, "{code}");
    }

    fixture.reply(failed(json!(400)));
    let error = translate(&provider, Output::Range).await.unwrap_err();
    assert_eq!(error.kind(), ErrorKind::Failed);
    assert!(
        error
            .to_string()
            .starts_with("OpenAI-compatible API stream failed (event=error); OpenAI-compatible API HTTP 200: Upstream failed (code=400)"),
        "{error}"
    );
}

#[test]
fn base_url_without_an_origin_is_refused() {
    let keys = KeyDirectory::new();
    for base_url in ["http://example.com/v1", "not a url"] {
        let error = CompatibleProvider::new(
            base_url,
            OutputOptions::new(
                CompatibleModel {
                    model: "m".into(),
                    effort: None,
                },
                None,
            ),
            "auto".into(),
            TranslationLimits::default().output_kib,
            Ok(keys.path.clone()),
        )
        .err()
        .unwrap();
        assert_eq!(error.kind(), ErrorKind::Configuration);
    }
}
