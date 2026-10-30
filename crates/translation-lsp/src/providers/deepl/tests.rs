use std::{fs, path::Path, sync::atomic::Ordering};

use serde_json::{Value, json};

use super::*;
use crate::providers::{
    anthropic::test_support::KeyDirectory,
    credential_directory,
    error::ErrorKind,
    http_fixture::{HttpFixture, Reply},
};

const KEY: &str = "synthetic-deepl-key";
const FILE: &str = "deepl-api-key";
const SOURCE: &str = "Retry only if the remote service\nreports a `Temporary` failure.";

/// A private credential directory with the DeepL key saved.
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
        .map(|text| json!({"detected_source_language": "EN", "text": text}))
        .collect();
    Reply {
        status: 200,
        body: json!({ "translations": translations }).to_string(),
        headers: vec![("content-type", "application/json")],
    }
}

fn failure(status: u16, message: &str) -> Reply {
    Reply::failure(status, json!({ "message": message }).to_string())
}

async fn translate(provider: &DeepLProvider, intent: Intent, text: &str) -> Result<String, Error> {
    provider
        .translate(intent, Output::Range, text, "ja", provider.context()?)
        .await
        .map(|translated| translated.text)
}

#[tokio::test(flavor = "current_thread")]
async fn request_carries_the_key_and_the_marked_segments() {
    let fixture = HttpFixture::new().await;
    let keys = keys();
    let provider = DeepLProvider::synthetic(fixture.base.clone(), &keys.path);
    fixture.reply(translated(&[
        "リモートサービスが一時的な<x i=\"0\">`Temporary`</x>障害を報告した場合のみ再試行します。",
    ]));
    assert_eq!(
        translate(&provider, Intent::Explicit, SOURCE)
            .await
            .unwrap(),
        "リモートサービスが一時的な`Temporary`障害を報告した場合のみ再試行します。"
    );
    let provider = provider.with_source_language("en_US");
    fixture.reply(translated(&["訳"]));
    translate(&provider, Intent::Explicit, "Text")
        .await
        .unwrap();

    let records = fixture.records();
    let [first, second] = &records[..] else {
        panic!("Expected two requests")
    };
    for record in [first, second] {
        assert_eq!(record.path, "/v2/translate");
        assert_eq!(
            record.headers["authorization"],
            format!("DeepL-Auth-Key {KEY}")
        );
        assert_eq!(record.body["target_lang"], "JA");
        assert_eq!(record.body["tag_handling"], "xml");
        assert_eq!(record.body["ignore_tags"], json!(["x"]));
    }
    assert_eq!(
        first.body["text"],
        json!(["Retry only if the remote service reports a <x i=\"0\">`Temporary`</x> failure."])
    );
    assert!(first.body.get("source_lang").is_none());
    assert_eq!(second.body["source_lang"], "EN");
}

#[test]
fn free_keys_go_to_the_free_api_and_language_settings_become_codes() {
    let keys = KeyDirectory::new();
    let provider = DeepLProvider::new(
        "auto",
        TranslationLimits::default().output_kib,
        Ok(keys.path.clone()),
    )
    .unwrap();
    let api = |key: &str| provider.api(&ApiKey::parse(key).unwrap());
    assert_eq!(api("synthetic:fx"), "https://api-free.deepl.com");
    assert_eq!(api("synthetic"), "https://api.deepl.com");
    assert_eq!(provider.source_language, None);

    assert_eq!(target_code("pt_BR").as_deref(), Some("PT-BR"));
    assert_eq!(target_code("zh-Hans").as_deref(), Some("ZH-HANS"));
    assert_eq!(source_code("pt-BR").as_deref(), Some("PT"));
    assert!(check_languages("auto", "ja").is_ok());
    assert!(check_languages("en", "en-US").is_ok());
    for (source, target, setting) in [
        ("auto", "Japanese", "target_language"),
        ("English", "ja", "source_language"),
    ] {
        let error = check_languages(source, target).unwrap_err();
        assert_eq!(
            error.to_string(),
            format!("{setting} must be a language code, such as ja or pt-BR, with provider deepl")
        );
    }
}

#[tokio::test(flavor = "current_thread")]
async fn missing_key_is_a_settings_error_and_text_without_letters_is_not_sent() {
    let fixture = HttpFixture::new().await;
    let keys = KeyDirectory::new();
    let provider = DeepLProvider::synthetic(fixture.base.clone(), &keys.path);
    let error = provider.context().unwrap_err();
    assert_eq!(error.kind(), ErrorKind::Configuration);
    assert_eq!(
        error.to_string(),
        "DeepL API key is not saved. Run \"DeepL API key…\" from the code actions to save it"
    );
    assert_eq!(provider.auth_ui_state(), AuthUiState::SignedOut);

    directory_with(&keys.path, KEY);
    assert_eq!(
        translate(&provider, Intent::Explicit, "`retain()`\n\n42")
            .await
            .unwrap(),
        "`retain()`\n\n42"
    );
    assert!(fixture.records().is_empty());
}

#[tokio::test(flavor = "current_thread")]
async fn failures_are_told_apart_by_status() {
    for (reply, kind, text) in [
        (
            failure(403, "Wrong key"),
            ErrorKind::Configuration,
            "Run \"DeepL API key…\" from the code actions to replace it",
        ),
        (
            failure(456, "Quota exceeded"),
            ErrorKind::UsageLimit,
            "https://www.deepl.com/your-account/usage",
        ),
        (
            failure(400, "Value for 'target_lang' not supported."),
            ErrorKind::Configuration,
            "DeepL API HTTP 400: Value for 'target_lang' not supported.",
        ),
        (
            Reply::failure(
                400,
                json!({"message": "Bad request", "code": 1234}).to_string(),
            ),
            ErrorKind::Failed,
            "DeepL API HTTP 400: Bad request (code=1234)",
        ),
        (
            Reply::failure(
                502,
                json!({"error": {"message": "Bad Gateway."}}).to_string(),
            ),
            ErrorKind::Temporary,
            "DeepL API HTTP 502: Bad Gateway.",
        ),
        (
            failure(413, "Request Entity Too Large"),
            ErrorKind::Failed,
            "DeepL API HTTP 413",
        ),
    ] {
        let fixture = HttpFixture::new().await;
        let keys = keys();
        let provider = DeepLProvider::synthetic(fixture.base.clone(), &keys.path);
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
async fn rate_limit_is_sent_again_once_and_broken_marks_save_nothing() {
    let fixture = HttpFixture::new().await;
    let keys = keys();
    let provider = DeepLProvider::synthetic(fixture.base.clone(), &keys.path);
    let mut limited = failure(429, "Too many requests");
    limited.headers.push(("retry-after", "0"));
    fixture.reply(limited);
    fixture.reply(translated(&["<x i=\"0\">`Temporary`</x> の障害"]));
    assert_eq!(
        translate(&provider, Intent::Explicit, SOURCE)
            .await
            .unwrap(),
        "`Temporary` の障害"
    );
    assert_eq!(fixture.records().len(), 2);

    for reply in [
        translated(&["マークのない訳"]),
        translated(&["二つの訳", "<x i=\"0\">`Temporary`</x>"]),
        Reply {
            status: 200,
            body: "{\"translations\": [{}]}".into(),
            headers: vec![("content-type", "application/json")],
        },
    ] {
        fixture.reply(reply);
        let error = translate(&provider, Intent::Explicit, SOURCE)
            .await
            .unwrap_err();
        assert_eq!(error.kind(), ErrorKind::Failed, "{error}");
    }
}

#[test]
fn long_texts_are_sent_in_requests_that_fit() {
    let texts: Vec<String> = ["a".repeat(60 * 1024), "b".repeat(40 * 1024), "c".into()]
        .into_iter()
        .chain(["d".repeat(REQUEST_BYTES + 1), "e".into()])
        .collect();
    let lengths: Vec<usize> = batches(&texts).iter().map(|batch| batch.len()).collect();
    assert_eq!(lengths, [1, 2, 1, 1]);
    assert!(batches(&[]).is_empty());
}

#[tokio::test(flavor = "current_thread")]
async fn rejected_key_pauses_automatic_translation_until_another_key_is_saved() {
    let fixture = HttpFixture::new().await;
    let keys = keys();
    let provider = DeepLProvider::synthetic(fixture.base.clone(), &keys.path);
    fixture.reply(failure(403, "Wrong key"));
    let error = translate(&provider, Intent::Automatic, SOURCE)
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

    directory_with(&keys.path, "synthetic-replacement:fx");
    fixture.reply(translated(&["<x i=\"0\">`Temporary`</x> の障害"]));
    translate(&provider, Intent::Automatic, SOURCE)
        .await
        .unwrap();
    let records = fixture.records();
    assert_eq!(
        records[1].headers["authorization"],
        "DeepL-Auth-Key synthetic-replacement:fx"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn menu_saves_in_a_terminal_and_removes_only_the_deepl_key() {
    let keys = KeyDirectory::with_key("sk-ant-synthetic-key");
    let provider = DeepLProvider::synthetic("http://127.0.0.1:9".into(), &keys.path);
    let titles = |provider: &DeepLProvider| -> Vec<&str> {
        provider
            .account_commands()
            .iter()
            .map(|command| command.title)
            .collect()
    };
    assert_eq!(titles(&provider), ["Save DeepL API key"]);
    let Terminal::Synthetic(terminal) = &provider.terminal else {
        panic!("Expected the synthetic terminal")
    };
    let notice = provider.administer(SAVE, &Confirm(true)).await.unwrap();
    assert!(
        notice.starts_with("A terminal window was opened. Enter the DeepL API key there."),
        "{notice}"
    );
    assert!(notice.ends_with(" deepl-key"), "{notice}");
    assert_eq!(terminal.opened.lock().unwrap().len(), 1);
    terminal.fails.store(true, Ordering::SeqCst);
    let error = provider.administer(SAVE, &Confirm(true)).await.unwrap_err();
    assert!(error.to_string().starts_with(
        "No terminal window could be opened. To save the DeepL API key, run this in a terminal: "
    ));

    directory_with(&keys.path, KEY);
    assert_eq!(
        titles(&provider),
        ["Replace DeepL API key", "Remove DeepL API key"]
    );
    assert_eq!(
        provider.administer(REMOVE, &Confirm(false)).await.unwrap(),
        "The DeepL API key was not removed."
    );
    assert_eq!(
        provider.administer(REMOVE, &Confirm(true)).await.unwrap(),
        "The DeepL API key was removed."
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
