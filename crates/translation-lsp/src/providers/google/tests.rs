use std::{fs, path::Path, sync::atomic::Ordering};

use serde_json::{Value, json};

use super::*;
use crate::providers::{
    anthropic::test_support::KeyDirectory,
    credential_directory,
    error::ErrorKind,
    http_fixture::{HttpFixture, Reply},
};

const KEY: &str = "AIza-synthetic-key";
const FILE: &str = "api-key-https-translation.googleapis.com-443";
const SOURCE: &str = "Retry only if the remote service\nreports a `Temporary` failure.";
const MARK: &str = "<span translate=\"no\" class=\"notranslate\" id=\"i0\">";

/// A private credential directory with the Google Cloud key saved.
fn keys() -> KeyDirectory {
    let directory = KeyDirectory::new();
    directory_with(&directory.path, KEY);
    directory
}

fn directory_with(directory: &Path, key: &str) {
    credential_directory::replace(directory, FILE, key.as_bytes()).unwrap();
}

fn translated(texts: &[&str]) -> Reply {
    let translations: Vec<Value> = texts
        .iter()
        .map(|text| json!({"translatedText": text, "detectedSourceLanguage": "en"}))
        .collect();
    Reply {
        status: 200,
        body: json!({"data": {"translations": translations}}).to_string(),
        headers: vec![("content-type", "application/json")],
    }
}

fn failure(status: u16, state: &str, reason: &str) -> Reply {
    let mut reply = Reply::failure(
        status,
        json!({"error": {
            "code": status,
            "message": "Synthetic failure",
            "status": state,
            "details": [{"@type": "type.googleapis.com/google.rpc.ErrorInfo", "reason": reason}],
        }})
        .to_string(),
    );
    reply.headers.push(("retry-after", "0"));
    reply
}

async fn translate(provider: &GoogleProvider, intent: Intent, text: &str) -> Result<String, Error> {
    provider
        .translate(intent, Output::Range, text, "ja", provider.context()?)
        .await
        .map(|translated| translated.text)
}

#[tokio::test(flavor = "current_thread")]
async fn request_carries_the_key_in_a_header_and_the_marked_segments_as_html() {
    let fixture = HttpFixture::new().await;
    let keys = keys();
    let provider = GoogleProvider::synthetic(format!("{}/v2", fixture.base), &keys.path);
    fixture.reply(translated(&[&format!(
        "リモートサービスが一時的な{MARK}`Temporary`</span>障害を報告した場合のみ再試行します。&#39;"
    )]));
    assert_eq!(
        translate(&provider, Intent::Explicit, SOURCE)
            .await
            .unwrap(),
        "リモートサービスが一時的な`Temporary`障害を報告した場合のみ再試行します。'"
    );
    let provider = provider.with_source_language("pt_BR");
    fixture.reply(translated(&["訳"]));
    translate(&provider, Intent::Explicit, "Text")
        .await
        .unwrap();

    let records = fixture.records();
    let [first, second] = &records[..] else {
        panic!("Expected two requests")
    };
    for record in [first, second] {
        assert_eq!(record.path, "/v2");
        assert_eq!(record.headers["x-goog-api-key"], KEY);
        assert!(record.headers.get("authorization").is_none());
        assert_eq!(record.body["target"], "ja");
        assert_eq!(record.body["format"], "html");
    }
    assert_eq!(
        first.body["q"],
        json!([format!(
            "Retry only if the remote service reports a {MARK}`Temporary`</span> failure."
        )])
    );
    assert!(first.body.get("source").is_none());
    assert_eq!(second.body["source"], "pt-BR");
}

#[tokio::test(flavor = "current_thread")]
async fn missing_key_is_a_settings_error_and_names_the_menu() {
    let fixture = HttpFixture::new().await;
    let keys = KeyDirectory::new();
    let provider = GoogleProvider::synthetic(format!("{}/v2", fixture.base), &keys.path);
    let error = provider.context().unwrap_err();
    assert_eq!(error.kind(), ErrorKind::Configuration);
    assert_eq!(
        error.to_string(),
        "Google Cloud API key is not saved. Run \"Google Cloud API key…\" from the code actions to save it"
    );
    // The key of the Gemini API is another origin's.
    credential_directory::replace(
        &keys.path,
        "api-key-https-generativelanguage.googleapis.com-443",
        KEY.as_bytes(),
    )
    .unwrap();
    assert!(provider.context().is_err());
    assert!(fixture.records().is_empty());
}

#[tokio::test(flavor = "current_thread")]
async fn failures_are_told_apart_by_reason_and_status() {
    for (reply, kind, text) in [
        (
            failure(400, "INVALID_ARGUMENT", "API_KEY_INVALID"),
            ErrorKind::Configuration,
            "Run \"Google Cloud API key…\" from the code actions to replace it",
        ),
        (
            failure(400, "INVALID_ARGUMENT", "invalid"),
            ErrorKind::Configuration,
            "Google Cloud Translation HTTP 400: Synthetic failure (code=invalid)",
        ),
        (
            failure(403, "PERMISSION_DENIED", "SERVICE_DISABLED"),
            ErrorKind::Configuration,
            "Check that the Cloud Translation API and billing are enabled",
        ),
        (
            failure(403, "PERMISSION_DENIED", "dailyLimitExceeded"),
            ErrorKind::UsageLimit,
            "Check the quotas of the Cloud Translation API",
        ),
        (
            failure(429, "RESOURCE_EXHAUSTED", "RATE_LIMIT_EXCEEDED"),
            ErrorKind::Temporary,
            "(code=RATE_LIMIT_EXCEEDED)",
        ),
        (
            Reply::failure(503, "Service Unavailable"),
            ErrorKind::Temporary,
            "Google Cloud Translation HTTP 503: Service Unavailable",
        ),
    ] {
        let fixture = HttpFixture::new().await;
        let keys = keys();
        let provider = GoogleProvider::synthetic(format!("{}/v2", fixture.base), &keys.path);
        let mut reply = reply;
        reply.headers.push(("retry-after", "0"));
        fixture.reply(reply.clone());
        // A temporary failure is sent again once.
        fixture.reply(reply);
        let error = translate(&provider, Intent::Explicit, SOURCE)
            .await
            .unwrap_err();
        assert_eq!(error.kind(), kind, "{error}");
        assert!(error.to_string().contains(text), "{error}");
        assert!(!error.to_string().contains(KEY));
        assert!(!error.to_string().contains("remote service"));
    }
}

#[tokio::test(flavor = "current_thread")]
async fn many_segments_are_sent_in_groups_and_broken_marks_save_nothing() {
    let fixture = HttpFixture::new().await;
    let keys = keys();
    let provider = GoogleProvider::synthetic(format!("{}/v2", fixture.base), &keys.path);
    let text: Vec<String> = (0..130).map(|index| format!("- Item {index}")).collect();
    fixture.reply(translated(&["訳"; 128]));
    fixture.reply(translated(&["訳"; 2]));
    let translation = translate(&provider, Intent::Explicit, &text.join("\n"))
        .await
        .unwrap();
    assert_eq!(translation.lines().count(), 130);
    assert!(translation.lines().all(|line| line == "- 訳"));
    let records = fixture.records();
    assert_eq!(records.len(), 2);
    assert_eq!(records[0].body["q"].as_array().unwrap().len(), 128);

    for reply in [
        translated(&["マークのない訳"]),
        translated(&["二つの訳", "訳"]),
    ] {
        fixture.reply(reply);
        let error = translate(&provider, Intent::Explicit, SOURCE)
            .await
            .unwrap_err();
        assert_eq!(error.kind(), ErrorKind::Failed, "{error}");
    }
}

#[tokio::test(flavor = "current_thread")]
async fn menu_saves_for_the_api_origin_and_removes_only_its_key() {
    let keys = KeyDirectory::with_key("sk-ant-synthetic-key");
    let provider = GoogleProvider::synthetic("http://127.0.0.1:9/v2".into(), &keys.path);
    let titles = |provider: &GoogleProvider| -> Vec<&str> {
        provider
            .account_commands()
            .iter()
            .map(|command| command.title)
            .collect()
    };
    assert_eq!(titles(&provider), ["Save Google Cloud API key"]);
    let Terminal::Synthetic(terminal) = &provider.terminal else {
        panic!("Expected the synthetic terminal")
    };
    let notice = provider.administer(SAVE, &Confirm(true)).await.unwrap();
    assert!(
        notice.starts_with("A terminal window was opened. Enter the Google Cloud API key there."),
        "{notice}"
    );
    assert!(
        notice.ends_with(" api-key https://translation.googleapis.com"),
        "{notice}"
    );
    assert_eq!(terminal.opened.lock().unwrap().len(), 1);
    terminal.fails.store(true, Ordering::SeqCst);
    assert!(provider.administer(SAVE, &Confirm(true)).await.is_err());

    directory_with(&keys.path, KEY);
    assert_eq!(
        titles(&provider),
        [
            "Replace Google Cloud API key",
            "Remove Google Cloud API key"
        ]
    );
    assert_eq!(
        provider.administer(REMOVE, &Confirm(true)).await.unwrap(),
        "The Google Cloud API key was removed."
    );
    assert_eq!(keys.names(), ["anthropic-api-key"]);
    assert_eq!(fs::read(keys.file()).unwrap(), b"sk-ant-synthetic-key");
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

/// A reply with a translation of each segment and the language detected in
/// it, if any.
fn detected_in(segments: &[(Option<&str>, &str)]) -> Reply {
    let translations: Vec<Value> = segments
        .iter()
        .map(|(language, text)| match language {
            Some(language) => json!({"translatedText": text, "detectedSourceLanguage": language}),
            None => json!({"translatedText": text}),
        })
        .collect();
    Reply {
        status: 200,
        body: json!({"data": {"translations": translations}}).to_string(),
        headers: vec![("content-type", "application/json")],
    }
}

#[tokio::test(flavor = "current_thread")]
async fn automatic_text_detected_in_the_target_language_in_every_segment_is_not_shown() {
    const TWO: &str = "First paragraph.\n\nSecond paragraph.";
    for (intent, source_language, languages, kind) in [
        (
            Intent::Automatic,
            None,
            [Some("ja"), Some("ja")],
            Kind::SameLanguage,
        ),
        (
            Intent::Automatic,
            None,
            [Some("ja"), Some("en")],
            Kind::Translation,
        ),
        (
            Intent::Automatic,
            None,
            [Some("ja"), None],
            Kind::Translation,
        ),
        (
            Intent::Explicit,
            None,
            [Some("ja"), Some("ja")],
            Kind::Translation,
        ),
        // The detected language reflects the source language that was sent.
        (
            Intent::Automatic,
            Some("ja"),
            [Some("ja"), Some("ja")],
            Kind::Translation,
        ),
    ] {
        let fixture = HttpFixture::new().await;
        let keys = keys();
        let mut provider = GoogleProvider::synthetic(format!("{}/v2", fixture.base), &keys.path);
        if let Some(source_language) = source_language {
            provider = provider.with_source_language(source_language);
        }
        fixture.reply(detected_in(&[
            (languages[0], "第一段。"),
            (languages[1], "第二段。"),
        ]));
        let translated = provider
            .translate(
                intent,
                Output::Range,
                TWO,
                "ja",
                provider.context().unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            translated.kind, kind,
            "{intent:?} {source_language:?} {languages:?}"
        );
        let shown = if kind == Kind::SameLanguage {
            ""
        } else {
            "第一段。\n\n第二段。"
        };
        assert_eq!(translated.text, shown);
        assert_eq!(fixture.records().len(), 1);
    }
    // `zh` is the code of Chinese (Simplified), as `zh-CN` is.
    for (detected, target, kind) in [
        ("zh-CN", "zh", Kind::SameLanguage),
        ("zh-CN", "zh-CN", Kind::SameLanguage),
        ("zh-TW", "zh", Kind::Translation),
        ("zh-CN", "zh-TW", Kind::Translation),
    ] {
        let fixture = HttpFixture::new().await;
        let keys = keys();
        let provider = GoogleProvider::synthetic(format!("{}/v2", fixture.base), &keys.path);
        fixture.reply(detected_in(&[(Some(detected), "第一段。")]));
        let translated = provider
            .translate(
                Intent::Automatic,
                Output::Range,
                "First paragraph.",
                target,
                provider.context().unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(translated.kind, kind, "{detected} {target}");
    }
}

#[tokio::test(flavor = "current_thread")]
async fn text_in_the_target_language_shows_nothing_even_when_its_translation_lost_a_mark() {
    let fixture = HttpFixture::new().await;
    let keys = keys();
    let provider = GoogleProvider::synthetic(format!("{}/v2", fixture.base), &keys.path);
    fixture.reply(detected_in(&[(Some("ja"), "失敗なら再試行します。")]));
    let translated = provider
        .translate(
            Intent::Automatic,
            Output::Range,
            "`Temporary` の失敗なら再試行します。",
            "ja",
            provider.context().unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(translated.kind, Kind::SameLanguage);
}
