use std::{fs, path::Path, sync::atomic::Ordering};

use serde_json::{Value, json};

use super::*;
use crate::{
    config::OpenAiSpeed,
    providers::{
        anthropic::test_support::KeyDirectory,
        credential_directory,
        error::ErrorKind,
        http_fixture::{HttpFixture, Reply},
    },
};

const KEY: &str = "sk-proj-synthetic-key";
const FILE: &str = "api-key-https-api.openai.com-443";
const SOURCE: &str = "Private source sentence.";

/// A private credential directory with the OpenAI key saved.
fn keys() -> KeyDirectory {
    let directory = KeyDirectory::new();
    directory_with(&directory.path, KEY);
    directory
}

fn directory_with(directory: &Path, key: &str) {
    credential_directory::replace(directory, FILE, key.as_bytes()).unwrap();
}

fn event(value: Value) -> String {
    format!(
        "event: {}\ndata: {value}\n\n",
        value["type"].as_str().unwrap()
    )
}

/// A reply streamed as one delta, then completed with the same text.
fn completed(text: &str) -> String {
    let message = json!({
        "type": "message", "id": "msg_synthetic", "status": "completed", "role": "assistant",
        "content": [{"type": "output_text", "text": text}],
    });
    format!(
        "{}{}",
        event(json!({
            "type": "response.output_text.delta", "output_index": 0, "content_index": 0,
            "item_id": "msg_synthetic", "delta": text,
        })),
        event(json!({
            "type": "response.completed",
            "response": {"status": "completed", "error": null, "output": [message]},
        })),
    )
}

fn failure(status: u16, code: &str) -> Reply {
    Reply::failure(
        status,
        json!({"error": {"message": "Synthetic failure", "type": "invalid_request_error", "code": code}})
            .to_string(),
    )
}

async fn translate(
    provider: &OpenAiApiProvider,
    intent: Intent,
    output: Output,
) -> Result<String, Error> {
    provider
        .translate(intent, output, SOURCE, "ja", provider.context()?)
        .await
        .map(|translated| translated.text)
}

#[tokio::test(flavor = "current_thread")]
async fn request_carries_the_key_and_the_documented_fields() {
    let fixture = HttpFixture::new().await;
    let keys = keys();
    let provider = OpenAiApiProvider::synthetic(format!("{}/v1", fixture.base), &keys.path);
    fixture.reply(Reply::stream(completed("訳")));
    fixture.reply(Reply::stream(completed("訳")));
    assert_eq!(
        translate(&provider, Intent::Explicit, Output::Range)
            .await
            .unwrap(),
        "訳"
    );
    let provider = provider.with_options(OutputOptions::new(
        OpenAiModel {
            model: "gpt-6-luna".into(),
            effort: ReasoningEffort::Low,
            speed: None,
        },
        Some(OpenAiModel {
            model: "gpt-6.1-sol".into(),
            effort: ReasoningEffort::High,
            speed: Some(OpenAiSpeed::Priority),
        }),
    ));
    translate(&provider, Intent::Explicit, Output::Document)
        .await
        .unwrap();

    let records = fixture.records();
    let [range, document] = &records[..] else {
        panic!("Expected two requests")
    };
    for record in [range, document] {
        assert_eq!(record.path, "/v1/responses");
        assert_eq!(record.headers["authorization"], format!("Bearer {KEY}"));
        assert_eq!(record.body["store"], false);
        assert_eq!(record.body["stream"], true);
        assert!(record.body["instructions"].as_str().unwrap().contains("ja"));
        let message = record.body["input"][0]["content"].as_str().unwrap();
        assert!(message.contains(SOURCE));
    }
    assert_eq!(range.body["model"], "gpt-6-luna");
    assert_eq!(range.body["reasoning"], json!({"effort": "low"}));
    // A range asks for 16,384 tokens and 4 for each byte of its source.
    assert_eq!(range.body["max_output_tokens"], 16_384 + 4 * SOURCE.len());
    assert!(range.body.get("service_tier").is_none());
    assert_eq!(document.body["model"], "gpt-6.1-sol");
    assert_eq!(document.body["reasoning"], json!({"effort": "high"}));
    assert_eq!(document.body["service_tier"], "priority");
    assert!(document.body.get("max_output_tokens").is_none());
}

#[tokio::test(flavor = "current_thread")]
async fn missing_key_is_a_settings_error_and_nothing_is_sent() {
    let fixture = HttpFixture::new().await;
    let keys = KeyDirectory::new();
    let provider = OpenAiApiProvider::synthetic(format!("{}/v1", fixture.base), &keys.path);
    let error = provider.context().unwrap_err();
    assert_eq!(error.kind(), ErrorKind::Configuration);
    assert_eq!(
        error.to_string(),
        "OpenAI API key is not saved. Run \"OpenAI API key…\" from the code actions to save it"
    );
    assert_eq!(provider.auth_ui_state(), AuthUiState::SignedOut);
    // The Anthropic key is no key for OpenAI.
    let anthropic = KeyDirectory::with_key("sk-ant-synthetic-key");
    let provider = OpenAiApiProvider::synthetic(format!("{}/v1", fixture.base), &anthropic.path);
    assert!(provider.context().is_err());
    assert!(fixture.records().is_empty());
}

#[tokio::test(flavor = "current_thread")]
async fn failures_are_told_apart_by_code_and_status() {
    for (reply, kind, text) in [
        (
            failure(401, "invalid_api_key"),
            ErrorKind::Configuration,
            "Run \"OpenAI API key…\" from the code actions to replace it",
        ),
        (
            failure(429, "insufficient_quota"),
            ErrorKind::UsageLimit,
            "https://platform.openai.com/settings/organization/billing",
        ),
        (
            failure(404, "model_not_found"),
            ErrorKind::Configuration,
            "(code=model_not_found)",
        ),
        (
            failure(403, "unsupported_country_region_territory"),
            ErrorKind::NotEligible,
            "OpenAI HTTP 403: Synthetic failure",
        ),
        (
            Reply::failure(
                400,
                json!({"error": {"message": "Unsupported value", "param": "reasoning.effort", "code": "unsupported_value"}})
                    .to_string(),
            ),
            ErrorKind::Configuration,
            "(param=reasoning.effort)",
        ),
        (
            failure(400, "invalid_value"),
            ErrorKind::Failed,
            "(request_id=req_synthetic)",
        ),
    ] {
        let fixture = HttpFixture::new().await;
        let keys = keys();
        let provider = OpenAiApiProvider::synthetic(format!("{}/v1", fixture.base), &keys.path);
        fixture.reply(reply);
        let error = translate(&provider, Intent::Explicit, Output::Range)
            .await
            .unwrap_err();
        assert_eq!(error.kind(), kind, "{error}");
        assert!(error.to_string().contains(text), "{error}");
        assert!(!error.to_string().contains(KEY));
        assert!(!error.to_string().contains(SOURCE));
    }
}

#[tokio::test(flavor = "current_thread")]
async fn rate_limit_is_sent_again_once() {
    let fixture = HttpFixture::new().await;
    let keys = keys();
    let provider = OpenAiApiProvider::synthetic(format!("{}/v1", fixture.base), &keys.path);
    let mut limited = failure(429, "rate_limit_exceeded");
    limited.headers.push(("retry-after", "0"));
    fixture.reply(limited);
    fixture.reply(Reply::stream(completed("訳")));
    assert_eq!(
        translate(&provider, Intent::Explicit, Output::Range)
            .await
            .unwrap(),
        "訳"
    );
    assert_eq!(fixture.records().len(), 2);
}

#[tokio::test(flavor = "current_thread")]
async fn incomplete_and_failed_streams_save_nothing() {
    for (stream, kind, text) in [
        (
            event(json!({
                "type": "response.incomplete",
                "response": {"status": "incomplete", "incomplete_details": {"reason": "max_output_tokens"}},
            })),
            ErrorKind::Failed,
            "OpenAI response stopped at the output token limit",
        ),
        (
            event(json!({
                "type": "response.incomplete",
                "response": {"status": "incomplete", "incomplete_details": {"reason": "content_filter"}},
            })),
            ErrorKind::Failed,
            "OpenAI declined to translate this text (category=content_filter)",
        ),
        (
            event(json!({
                "type": "response.failed",
                "response": {"status": "failed", "error": {"code": "insufficient_quota", "message": "Quota"}},
            })),
            ErrorKind::UsageLimit,
            "OpenAI stream failed (event=response.failed)",
        ),
    ] {
        let fixture = HttpFixture::new().await;
        let keys = keys();
        let provider = OpenAiApiProvider::synthetic(format!("{}/v1", fixture.base), &keys.path);
        fixture.reply(Reply::stream(stream));
        let error = translate(&provider, Intent::Explicit, Output::Range)
            .await
            .unwrap_err();
        assert_eq!(error.kind(), kind, "{error}");
        assert!(error.to_string().contains(text), "{error}");
    }
}

#[tokio::test(flavor = "current_thread")]
async fn rejected_key_pauses_automatic_translation_until_another_key_is_saved() {
    let fixture = HttpFixture::new().await;
    let keys = keys();
    let provider = OpenAiApiProvider::synthetic(format!("{}/v1", fixture.base), &keys.path);
    fixture.reply(failure(401, "invalid_api_key"));
    let error = translate(&provider, Intent::Automatic, Output::Range)
        .await
        .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("Automatic translation is paused")
    );
    let refused = provider
        .admit(&provider.context().unwrap(), Intent::Automatic)
        .unwrap_err();
    assert_eq!(refused.kind(), ErrorKind::Configuration);

    directory_with(&keys.path, "sk-proj-synthetic-replacement");
    fixture.reply(Reply::stream(completed("訳")));
    assert_eq!(
        translate(&provider, Intent::Automatic, Output::Range)
            .await
            .unwrap(),
        "訳"
    );
    let records = fixture.records();
    assert_eq!(
        records[1].headers["authorization"],
        "Bearer sk-proj-synthetic-replacement"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn menu_saves_in_a_terminal_and_removes_only_the_openai_key() {
    let keys = KeyDirectory::with_key("sk-ant-synthetic-key");
    let provider = OpenAiApiProvider::synthetic("http://127.0.0.1:9/v1".into(), &keys.path);
    let titles = |provider: &OpenAiApiProvider| -> Vec<&str> {
        provider
            .account_commands()
            .iter()
            .map(|command| command.title)
            .collect()
    };
    assert_eq!(titles(&provider), ["Save OpenAI API key"]);
    let Terminal::Synthetic(terminal) = &provider.terminal else {
        panic!("Expected the synthetic terminal")
    };
    let notice = provider.administer(SAVE, &Confirm(true)).await.unwrap();
    assert!(
        notice.starts_with("A terminal window was opened. Enter the OpenAI API key there."),
        "{notice}"
    );
    assert!(
        notice.ends_with(" api-key https://api.openai.com"),
        "{notice}"
    );
    assert_eq!(terminal.opened.lock().unwrap().len(), 1);
    terminal.fails.store(true, Ordering::SeqCst);
    let error = provider.administer(SAVE, &Confirm(true)).await.unwrap_err();
    assert!(error.to_string().starts_with(
        "No terminal window could be opened. To save the OpenAI API key, run this in a terminal: "
    ));

    directory_with(&keys.path, KEY);
    assert_eq!(
        titles(&provider),
        ["Replace OpenAI API key", "Remove OpenAI API key"]
    );
    assert_eq!(
        provider.administer(REMOVE, &Confirm(false)).await.unwrap(),
        "The OpenAI API key was not removed."
    );
    assert_eq!(
        provider.administer(REMOVE, &Confirm(true)).await.unwrap(),
        "The OpenAI API key was removed."
    );
    assert_eq!(keys.names(), ["anthropic-api-key"]);
    assert_eq!(fs::read(keys.file()).unwrap(), b"sk-ant-synthetic-key");
    assert_eq!(
        provider.administer(REMOVE, &Confirm(true)).await.unwrap(),
        "No OpenAI API key is saved."
    );
}

/// Answers the removal question.
struct Confirm(bool);

impl Interaction for Confirm {
    async fn open(&self, _uri: &str) -> Result<(), Error> {
        unreachable!()
    }

    async fn edit(&self, _path: &Path) -> Result<(), Error> {
        unreachable!()
    }

    async fn choose(&self, message: &str, choices: &[String]) -> Result<Option<String>, Error> {
        assert_eq!(message, REMOVAL);
        Ok(self.0.then(|| choices[0].clone()))
    }
}
