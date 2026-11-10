//! The providers that send an API key, other than Anthropic, in the server.
use std::time::Duration;

use serde_json::json;
use tokio::task::spawn_blocking;

use super::*;
use crate::providers::{
    anthropic::test_support::KeyDirectory,
    credential_directory,
    deepl::DeepLProvider,
    google::GoogleProvider,
    http_fixture::{HttpFixture, Reply},
    openai_api::OpenAiApiProvider,
    openai_compatible::CompatibleProvider,
};

fn server<P: TranslationProvider>(
    provider: P,
) -> (
    Server<P>,
    Connection,
    Connection,
    crossbeam_channel::Receiver<WorkerEvent<P::Context>>,
) {
    let (connection, client) = Connection::memory();
    let (finished, completions) = bounded(WORKER_EVENT_CHANNEL_CAPACITY);
    let server = Server::configured(
        finished,
        provider,
        Settings::default(),
        Ui::new(connection.sender.clone(), UiCapabilities::default()),
    )
    .unwrap();
    (server, connection, client, completions)
}

fn titles<P: TranslationProvider>(server: &mut Server<P>, uri: &Uri, range: Range) -> Vec<String> {
    server
        .code_actions(
            parse(json!({
                "textDocument": {"uri": uri}, "range": range, "context": {"diagnostics": []}
            }))
            .unwrap(),
        )
        .unwrap()
        .into_iter()
        .map(|action| action.title)
        .collect()
}

fn markdown<P: TranslationProvider>(server: &mut Server<P>) -> (Uri, Range) {
    let uri: Uri = "file:///selection.md".parse().unwrap();
    let source = "Alpha sentence.";
    server.documents.insert(
        uri.clone(),
        Document::new(1, 1, "markdown".into(), source.into()),
    );
    let range = Range::new(Position::new(0, 0), Position::new(0, source.len() as u32));
    (uri, range)
}

fn commands<P: TranslationProvider>() -> (String, Vec<String>) {
    let initialized = capabilities::<P>();
    (
        initialized.server_info.unwrap().name,
        initialized
            .capabilities
            .execute_command_provider
            .unwrap()
            .commands,
    )
}

#[test]
fn each_provider_declares_its_name_and_key_commands() {
    let shared = [
        COMMAND,
        RETRY_COMMAND,
        LENS_COMMAND,
        SHOW_COMMAND,
        MENU_COMMAND,
        AUTOMATIC_OFF_COMMAND,
        AUTOMATIC_ON_COMMAND,
        OPEN_SETTINGS_COMMAND,
        DOCUMENT_COMMAND,
        CANCEL_COMMAND,
        CANCEL_ALL_COMMAND,
    ];
    for ((name, commands), expected_name, keys) in [
        (
            commands::<OpenAiApiProvider>(),
            "Translation LSP (OpenAI API)",
            [
                "translation-lsp.save-openai-key",
                "translation-lsp.remove-openai-key",
            ],
        ),
        (
            commands::<CompatibleProvider>(),
            "Translation LSP (OpenAI-compatible)",
            [
                "translation-lsp.save-compatible-key",
                "translation-lsp.remove-compatible-key",
            ],
        ),
        (
            commands::<DeepLProvider>(),
            "Translation LSP (DeepL API)",
            [
                "translation-lsp.save-deepl-key",
                "translation-lsp.remove-deepl-key",
            ],
        ),
        (
            commands::<GoogleProvider>(),
            "Translation LSP (Google Cloud Translation)",
            [
                "translation-lsp.save-google-key",
                "translation-lsp.remove-google-key",
            ],
        ),
    ] {
        assert_eq!(name, expected_name);
        let expected: Vec<&str> = shared
            .into_iter()
            .chain(keys)
            .chain([SETTINGS_COMMAND])
            .collect();
        assert_eq!(commands, expected);
    }
}

#[tokio::test(flavor = "current_thread")]
async fn openai_api_offers_only_its_key_menu_until_a_key_is_saved() {
    let keys = KeyDirectory::new();
    let (mut server, _connection, _client, _completions) = server(OpenAiApiProvider::synthetic(
        "http://127.0.0.1:9/v1".into(),
        &keys.path,
    ));
    let (uri, range) = markdown(&mut server);
    assert_eq!(titles(&mut server, &uri, range), ["OpenAI API key…"]);
}

#[tokio::test(flavor = "current_thread")]
async fn compatible_service_translates_without_a_key_and_hover_shows_it() {
    let fixture = HttpFixture::new().await;
    let keys = KeyDirectory::new();
    let (mut server, connection, client, completions) = server(CompatibleProvider::synthetic(
        &format!("{}/v1", fixture.base),
        &keys.path,
    ));
    let (uri, range) = markdown(&mut server);
    assert_eq!(
        titles(&mut server, &uri, range),
        [
            "Translate selection to Japanese (ja)",
            "OpenAI-compatible API key…"
        ]
    );

    let uri: Uri = "file:///comment.rs".parse().unwrap();
    server.documents.insert(
        uri.clone(),
        Document::new(1, 1, "rust".into(), "// Read this.".into()),
    );
    fixture.reply(Reply::stream(format!(
        "data: {}\n\ndata: [DONE]\n\n",
        json!({"choices": [{"index": 0, "delta": {"content": "これを読む。"}, "finish_reason": "stop"}]})
    )));
    let hover =
        || parse(json!({"textDocument":{"uri":uri},"position":{"line":0,"character":4}})).unwrap();
    assert!(server.hover(1.into(), hover()).unwrap().is_none());
    let finished = {
        let completions = completions.clone();
        spawn_blocking(move || {
            loop {
                if let WorkerEvent::Finished(completion) =
                    completions.recv_timeout(Duration::from_secs(3)).unwrap()
                {
                    break completion;
                }
            }
        })
        .await
        .unwrap()
    };
    server.finish(&connection, finished).unwrap();
    let Message::Response(response) = client.receiver.try_recv().unwrap() else {
        panic!("Expected Hover response");
    };
    let contents = response.response_result.unwrap()["contents"]["value"].clone();
    assert!(
        contents.as_str().unwrap().contains("これを読む。"),
        "{contents}"
    );
    assert_eq!(server.cache.latest(&uri).unwrap().context, None);
    let records = fixture.records();
    assert_eq!(records.len(), 1);
    assert!(records[0].headers.get("authorization").is_none());
}

#[tokio::test(flavor = "current_thread")]
async fn deepl_translates_a_comment_of_several_lines_as_one_sentence() {
    let fixture = HttpFixture::new().await;
    let keys = KeyDirectory::new();
    let (mut server, connection, client, completions) =
        server(DeepLProvider::synthetic(fixture.base.clone(), &keys.path));
    let (uri, range) = markdown(&mut server);
    assert_eq!(titles(&mut server, &uri, range), ["DeepL API key…"]);
    credential_directory::replace(&keys.path, "deepl-api-key", b"synthetic-deepl-key").unwrap();

    let uri: Uri = "file:///comment.rs".parse().unwrap();
    server.documents.insert(
        uri.clone(),
        Document::new(
            1,
            1,
            "rust".into(),
            "// Call `retry()` only if the service\n// reports a temporary failure.".into(),
        ),
    );
    fixture.reply(Reply {
        status: 200,
        body: json!({"translations": [{"text": "サービスが一時的な障害を報告した場合のみ <x i=\"0\">`retry()`</x> を呼びます。"}]})
            .to_string(),
        headers: vec![("content-type", "application/json")],
    });
    let hover =
        || parse(json!({"textDocument":{"uri":uri},"position":{"line":0,"character":4}})).unwrap();
    assert!(server.hover(1.into(), hover()).unwrap().is_none());
    let finished = {
        let completions = completions.clone();
        spawn_blocking(move || {
            loop {
                if let WorkerEvent::Finished(completion) =
                    completions.recv_timeout(Duration::from_secs(3)).unwrap()
                {
                    break completion;
                }
            }
        })
        .await
        .unwrap()
    };
    server.finish(&connection, finished).unwrap();
    let Message::Response(response) = client.receiver.try_recv().unwrap() else {
        panic!("Expected Hover response");
    };
    let contents = response.response_result.unwrap()["contents"]["value"].clone();
    assert!(
        contents
            .as_str()
            .unwrap()
            .contains("サービスが一時的な障害を報告した場合のみ `retry()` を呼びます。"),
        "{contents}"
    );
    let records = fixture.records();
    assert_eq!(
        records[0].body["text"],
        json!(["Call <x i=\"0\">`retry()`</x> only if the service reports a temporary failure."])
    );
}
