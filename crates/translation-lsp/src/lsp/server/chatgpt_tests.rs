use super::*;
use crate::providers::chatgpt::{
    auth::AccountKey,
    test_support::{Fixture, StreamTailGate},
};
use serde_json::json;

mod account;
mod activity;
mod document;
mod parallel;
mod ranges;
mod selection;
mod usage;
mod worker;

#[tokio::test(flavor = "current_thread")]
async fn manual_indented_selections_translate_in_code_plain_text_and_markdown() {
    for language in ["rust", "plaintext", "markdown"] {
        let fixture = Fixture::new().await;
        let auth = fixture.auth();
        fixture.connected(&auth, 3600).await;
        let provider = ChatGptProvider::synthetic(
            auth,
            format!("{}/v1", fixture.base),
            "synthetic-model".into(),
        );
        let (connection, client) = Connection::memory();
        let (finished, completions) = bounded(WORKER_EVENT_CHANNEL_CAPACITY);
        let mut server = Server::configured(
            finished,
            provider,
            Settings::default(),
            Ui::new(connection.sender.clone(), UiCapabilities::default()),
        )
        .unwrap();
        let uri: Uri = "file:///selection.rs".parse().unwrap();
        let source = "    // Translate this indented comment.";
        let range = Range::new(Position::new(0, 0), Position::new(0, source.len() as u32));
        server.documents.insert(
            uri.clone(),
            Document::new(1, 1, language.into(), source.into()),
        );
        let actions = markdown_actions(&mut server, &uri, range);
        assert!(
            server.selection.is_none(),
            "Indented Markdown must not auto-translate"
        );
        let command = actions
            .into_iter()
            .filter_map(|action| action.command)
            .find(|command| command.command == COMMAND)
            .unwrap();
        server
            .execute_command(
                &connection,
                1.into(),
                ExecuteCommandParams {
                    command: command.command,
                    arguments: command.arguments.unwrap(),
                    work_done_progress_params: Default::default(),
                },
            )
            .unwrap();
        server
            .finish(&connection, next(&completions).await)
            .unwrap();
        let document = &server.documents[&uri];
        assert_eq!(document.text(), Some(source));
        assert_eq!(
            server.cache.latest(&uri).unwrap().translation,
            format!("訳{source}")
        );
        let state = fixture.state.lock().unwrap();
        let requests: Vec<_> = state
            .records
            .iter()
            .filter(|record| record.path == "/v1/responses")
            .collect();
        assert_eq!(requests.len(), 1);
        let request: Value = serde_json::from_str(&requests[0].body).unwrap();
        assert_eq!(request["input"][0]["content"], source);
        assert!(client.receiver.try_iter().any(|message| matches!(message,
            Message::Notification(n) if n.params["message"].as_str().is_some_and(|m| m.contains("Translation ready")))));
    }
}

#[tokio::test(flavor = "current_thread")]
async fn hover_limits_control_actions_background_and_manual_admission() {
    let fixture = Fixture::new().await;
    let auth = fixture.auth();
    fixture.connected(&auth, 3600).await;
    for (kib, bytes) in [(1, 1024), (1, 1025), (64, 32 * 1024)] {
        let provider = ChatGptProvider::synthetic(
            auth.clone(),
            format!("{}/v1", fixture.base),
            "synthetic-model".into(),
        );
        let settings = Settings::from_options(Some(
            json!({"translation_limits":{"input_kib":{"hover":kib}}}),
        ))
        .unwrap();
        let (connection, _client) = Connection::memory();
        let (finished, _completions) = bounded(WORKER_EVENT_CHANNEL_CAPACITY);
        let mut server = Server::configured(
            finished,
            provider,
            settings,
            Ui::new(connection.sender.clone(), UiCapabilities::default()),
        )
        .unwrap();
        let uri: Uri = "file:///sized.md".parse().unwrap();
        let source = format!("{}é", "a".repeat(bytes - 2));
        let range = Range::new(Position::new(0, 0), Position::new(0, (bytes - 1) as u32));
        server
            .documents
            .insert(uri.clone(), Document::new(1, 1, "markdown".into(), source));
        let actions = markdown_actions(&mut server, &uri, range);
        let accepted = bytes <= kib * 1024;
        assert_eq!(
            actions.iter().any(|action| action
                .command
                .as_ref()
                .is_some_and(|command| command.command == COMMAND)),
            accepted
        );
        assert_eq!(server.selection.is_some(), accepted);
        let target = TranslationTarget {
            uri,
            version: 1,
            session: 1,
            range,
            target_language: "ja".into(),
        };
        let result = server.queue_translation(
            target,
            TranslationRequest::Reply(1.into(), PendingReply::Command),
        );
        assert_eq!(result.is_ok(), accepted);
        if !accepted {
            let error = result.unwrap_err();
            assert!(error.message.contains("1025 bytes"));
            assert!(error.message.contains("translation_limits.input_kib.hover"));
        }
    }
}

#[tokio::test(flavor = "current_thread")]
async fn configured_auto_toggle_keeps_non_ja_manual_and_comment_translation() {
    for (features, automatic) in [
        (json!({"automatic_selection": false}), false),
        (json!({"automatic_selection": true}), true),
        (
            json!({"languages": {"markdown": {"automatic_selection": false}}}),
            false,
        ),
        (
            json!({
                "automatic_selection": false,
                "languages": {"markdown": {"automatic_selection": true}}
            }),
            true,
        ),
    ] {
        let fixture = Fixture::new().await;
        fixture.state.lock().unwrap().translation = Some("Translated sentence.".into());
        let auth = fixture.auth();
        fixture.connected(&auth, 3600).await;
        let provider = ChatGptProvider::synthetic(
            auth,
            format!("{}/v1", fixture.base),
            "synthetic-model".into(),
        );
        let settings = Settings::from_options(Some(json!({
            "target_language":"en", "features":features,
            "hover_timeout_ms":1000, "selection_timeout_ms":1500, "selection_debounce_ms":100,
            "provider":{"name":"chatgpt","model":"synthetic-model"}
        })))
        .unwrap();
        let (connection, client) = Connection::memory();
        let (finished, completions) = bounded(WORKER_EVENT_CHANNEL_CAPACITY);
        let mut server = Server::configured(
            finished,
            provider,
            settings,
            Ui::new(connection.sender.clone(), UiCapabilities::default()),
        )
        .unwrap();
        let uri: Uri = "file:///configured-en.md".parse().unwrap();
        let range = Range::new(Position::new(0, 0), Position::new(0, 15));
        server.documents.insert(
            uri.clone(),
            Document::new(1, 1, "markdown".into(), "First sentence.".into()),
        );
        let actions = markdown_actions(&mut server, &uri, range);
        assert_eq!(actions[0].title, "Translate selection to English (en)");
        assert_eq!(actions[1].title, "ChatGPT account…");
        let command = actions[0].command.clone().unwrap();
        assert_eq!(
            command.arguments.as_ref().unwrap()[0]["targetLanguage"],
            "en"
        );
        if automatic {
            let due = server.selection.as_ref().unwrap().start.unwrap();
            assert!(due <= Instant::now() + Duration::from_millis(100));
            markdown_actions(&mut server, &uri, range);
            assert_eq!(server.selection.as_ref().unwrap().start, Some(due));
            server.expire(&connection).unwrap();
            assert!(server.scheduler.translations().is_empty());
            assert!(fixture.state.lock().unwrap().records.is_empty());
            tokio::time::sleep(Duration::from_millis(110)).await;
            server.expire(&connection).unwrap();
            server
                .finish(&connection, next(&completions).await)
                .unwrap();
            assert!(
                client.receiver.is_empty(),
                "Background completion remains silent"
            );
            markdown_actions(&mut server, &uri, range);
            server.expire(&connection).unwrap();
            assert!(
                server.scheduler.translations().is_empty(),
                "Same selection cannot duplicate completed work"
            );
        } else {
            assert!(server.selection.is_none());
            tokio::time::sleep(Duration::from_millis(110)).await;
            markdown_actions(&mut server, &uri, range);
            server.expire(&connection).unwrap();
            assert!(
                fixture.state.lock().unwrap().records.is_empty(),
                "Disabled automatic translation sends nothing"
            );
        }
        server
            .request(
                &connection,
                Request::new(
                    1.into(),
                    ExecuteCommand::METHOD.into(),
                    json!({
                        "command":command.command,"arguments":command.arguments.unwrap()
                    }),
                ),
            )
            .unwrap();
        if !automatic {
            server
                .finish(&connection, next(&completions).await)
                .unwrap();
        }
        assert!(command_notice(&client, 1, 3).unwrap().is_null());
        let markdown = server
            .hover(
                2.into(),
                parse(json!({"textDocument":{"uri":uri},"position":{"line":0,"character":4}}))
                    .unwrap(),
            )
            .unwrap()
            .unwrap();
        assert!(
            markdown["contents"]["value"]
                .as_str()
                .unwrap()
                .ends_with("```text\nTranslated sentence.\n```")
        );
        assert_eq!(server.documents[&uri].text(), Some("First sentence."));
        let comment_uri: Uri = "file:///configured-en.rs".parse().unwrap();
        server.documents.insert(
            comment_uri.clone(),
            Document::new(1, 2, "rust".into(), "// First sentence.".into()),
        );
        server
            .request(
                &connection,
                Request::new(
                    3.into(),
                    HoverRequest::METHOD.into(),
                    json!({
                        "textDocument":{"uri":comment_uri},"position":{"line":0,"character":4}
                    }),
                ),
            )
            .unwrap();
        server
            .finish(&connection, next(&completions).await)
            .unwrap();
        let Message::Response(comment) = client.receiver.try_recv().unwrap() else {
            panic!("Expected comment Hover");
        };
        assert!(
            comment.response_result.unwrap()["contents"]["value"]
                .as_str()
                .unwrap()
                .contains("Translated sentence.")
        );
        assert_eq!(
            server.documents[&comment_uri].text(),
            Some("// First sentence.")
        );
        let records = fixture.state.lock().unwrap().records.clone();
        let responses: Vec<_> = records
            .iter()
            .filter(|record| record.path == "/v1/responses")
            .collect();
        assert_eq!(responses.len(), 2);
        for response in responses {
            let body: Value = serde_json::from_str(&response.body).unwrap();
            assert!(
                body["instructions"]
                    .as_str()
                    .unwrap()
                    .starts_with("Translate the text into English (en). Source language: auto.")
            );
        }
        assert!(client.receiver.is_empty());
    }
}

async fn next(
    receiver: &crossbeam_channel::Receiver<WorkerEvent<AccountKey>>,
) -> Finished<AccountKey> {
    let receiver = receiver.clone();
    tokio::task::spawn_blocking(move || {
        loop {
            if let WorkerEvent::Finished(completion) =
                receiver.recv_timeout(Duration::from_secs(3)).unwrap()
            {
                break completion;
            }
        }
    })
    .await
    .unwrap()
}

async fn wait_held(tail: &StreamTailGate, count: usize) {
    tokio::time::timeout(Duration::from_secs(2), async {
        while tail.held.load(std::sync::atomic::Ordering::SeqCst) != count {
            tail.reached.notified().await;
        }
    })
    .await
    .unwrap_or_else(|_| {
        panic!(
            "Expected {count} held HTTP responses; observed {}",
            tail.held.load(std::sync::atomic::Ordering::SeqCst)
        )
    });
}

fn response_calls(fixture: &Fixture) -> usize {
    fixture
        .state
        .lock()
        .unwrap()
        .records
        .iter()
        .filter(|record| record.path == "/v1/responses")
        .count()
}

fn open_parallel(server: &mut Server<ChatGptProvider>, connection: &Connection) -> Vec<Uri> {
    (0..4)
        .map(|index| {
            let uri: Uri = format!("file:///parallel-{index}.rs").parse().unwrap();
            server
                .notification(
                    connection,
                    lsp_server::Notification::new(
                        DidOpenTextDocument::METHOD.into(),
                        json!({"textDocument":{"uri":uri,"languageId":"rust","version":1,
                "text":format!("// Source {index}.\n\n// Other source.")}}),
                    ),
                )
                .unwrap();
            uri
        })
        .collect()
}

fn parallel_hover(server: &mut Server<ChatGptProvider>, id: i32, uri: &Uri) {
    assert!(
        server
            .hover(
                id.into(),
                parse(json!({
                    "textDocument":{"uri":uri}, "position":{"line":0,"character":4}
                }))
                .unwrap()
            )
            .unwrap()
            .is_none()
    );
}

fn command_notice(client: &Connection, id: i32, level: u64) -> RequestResult<Value> {
    let messages: Vec<_> = client.receiver.try_iter().collect();
    assert_eq!(
        messages.len(),
        2,
        "one acknowledgement and one notification: {messages:?}"
    );
    let response = messages
        .iter()
        .find_map(|m| {
            if let Message::Response(r) = m {
                Some(r)
            } else {
                None
            }
        })
        .unwrap();
    let notice = messages
        .iter()
        .find_map(|m| {
            if let Message::Notification(n) = m {
                Some(n)
            } else {
                None
            }
        })
        .unwrap();
    assert_eq!(response.id, RequestId::from(id));
    assert_eq!(notice.method, "window/showMessage");
    assert_eq!(notice.params["type"], level);
    if let Err(error) = &response.response_result {
        assert_eq!(notice.params["message"], error.message);
    }
    if level == 1
        && let Ok(value) = &response.response_result
    {
        // An admitted command stays acknowledged even when its later operation fails.
        assert!(value.is_null());
        Err(failure(
            ErrorCode::RequestFailed,
            notice.params["message"].as_str().unwrap(),
        ))
    } else {
        response.response_result.clone()
    }
}

#[tokio::test(flavor = "current_thread")]
async fn documentation_comments_reach_http_and_return_translated_hover() {
    for (language, source, body, translated) in [
        (
            "csharp",
            "/// <summary>Retry safely.</summary>\nclass Example {}",
            "<summary>Retry safely.</summary>",
            "<summary>安全に再試行します。</summary>",
        ),
        (
            "python",
            "\"\"\"Retry safely.\"\"\"",
            "Retry safely.",
            "安全に再試行します。",
        ),
    ] {
        let fixture = Fixture::new().await;
        fixture.state.lock().unwrap().translation = Some(translated.into());
        let auth = fixture.auth();
        fixture.connected(&auth, 3600).await;
        let provider = ChatGptProvider::synthetic(
            auth,
            format!("{}/v1", fixture.base),
            "synthetic-model".into(),
        );
        let (connection, client) = Connection::memory();
        let (finished, completions) = bounded(WORKER_EVENT_CHANNEL_CAPACITY);
        let mut server = Server::new_with_provider(finished, provider).unwrap();
        let uri: Uri = "file:///Example.cs".parse().unwrap();
        server.documents.insert(
            uri.clone(),
            Document::new(1, 1, language.into(), source.into()),
        );
        assert!(
            server
                .hover(
                    1.into(),
                    parse(json!({
                        "textDocument":{"uri":uri}, "position":{"line":0,"character":15}
                    }))
                    .unwrap()
                )
                .unwrap()
                .is_none()
        );
        server
            .finish(&connection, next(&completions).await)
            .unwrap();
        let state = fixture.state.lock().unwrap();
        let requests: Vec<_> = state
            .records
            .iter()
            .filter(|record| record.path == "/v1/responses")
            .collect();
        assert_eq!(requests.len(), 1);
        let request: Value = serde_json::from_str(&requests[0].body).unwrap();
        assert_eq!(request["input"][0]["content"], body);
        let Message::Response(reply) = client.receiver.try_recv().unwrap() else {
            panic!("Expected Hover response");
        };
        let hover = reply.response_result.unwrap();
        assert!(
            hover["contents"]["value"]
                .as_str()
                .unwrap()
                .contains(translated)
        );
        assert_eq!(server.documents[&uri].text(), Some(source));
        assert!(client.receiver.try_recv().is_err());
    }
}

fn markdown_actions(
    server: &mut Server<ChatGptProvider>,
    uri: &Uri,
    range: Range,
) -> Vec<CodeAction> {
    server
        .code_actions(
            parse(json!({
                "textDocument": {"uri": uri}, "range": range, "context": {"diagnostics": []}
            }))
            .unwrap(),
        )
        .unwrap()
}

async fn protocol_message(client: &Connection) -> Message {
    let receiver = client.receiver.clone();
    tokio::task::spawn_blocking(move || receiver.recv_timeout(Duration::from_secs(3)).unwrap())
        .await
        .unwrap()
}

async fn range_server(
    settings: Settings,
) -> (
    Fixture,
    Server<ChatGptProvider>,
    Connection,
    Connection,
    crossbeam_channel::Receiver<WorkerEvent<AccountKey>>,
    Uri,
) {
    let fixture = Fixture::new().await;
    let auth = fixture.auth();
    fixture.connected(&auth, 3600).await;
    let provider = ChatGptProvider::synthetic(
        auth,
        format!("{}/v1", fixture.base),
        "synthetic-model".into(),
    );
    let (connection, client) = Connection::memory();
    let (finished, completions) = bounded(WORKER_EVENT_CHANNEL_CAPACITY);
    let mut server = Server::configured(
        finished,
        provider,
        settings,
        Ui::new(
            connection.sender.clone(),
            UiCapabilities {
                show_document: true,
                ..Default::default()
            },
        ),
    )
    .unwrap();
    let uri: Uri = "file:///ranges.md".parse().unwrap();
    server.documents.insert(
        uri.clone(),
        Document::new(
            1,
            1,
            "markdown".into(),
            "Alpha sentence.\n\nBravo sentence.\n\nThird sentence.".into(),
        ),
    );
    (fixture, server, connection, client, completions, uri)
}
fn range_target(server: &Server<ChatGptProvider>, uri: &Uri, line: u32) -> TranslationTarget {
    let document = &server.documents[uri];
    TranslationTarget {
        uri: uri.clone(),
        version: document.version,
        session: document.session,
        range: Range::new(Position::new(line, 0), Position::new(line, 15)),
        target_language: "ja".into(),
    }
}
