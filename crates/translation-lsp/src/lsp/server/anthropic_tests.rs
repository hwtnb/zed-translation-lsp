use super::*;
use crate::providers::{
    Intent,
    anthropic::test_support::{Fixture, Reply, started, text_block},
};
use serde_json::json;

mod document;
mod key;
mod startup;

async fn next(receiver: &crossbeam_channel::Receiver<WorkerEvent>) -> Finished {
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

fn server(
    provider: AnthropicProvider,
    settings: Settings,
) -> (
    Server<AnthropicProvider>,
    Connection,
    Connection,
    crossbeam_channel::Receiver<WorkerEvent>,
) {
    let (connection, client) = Connection::memory();
    let (finished, completions) = bounded(WORKER_EVENT_CHANNEL_CAPACITY);
    let server = Server::configured(
        finished,
        provider,
        settings,
        Ui::new(connection.sender.clone(), UiCapabilities::default()),
    )
    .unwrap();
    (server, connection, client, completions)
}

fn actions(server: &mut Server<AnthropicProvider>, uri: &Uri, range: Range) -> Vec<CodeAction> {
    server
        .code_actions(
            parse(json!({
                "textDocument": {"uri": uri}, "range": range, "context": {"diagnostics": []}
            }))
            .unwrap(),
        )
        .unwrap()
}

fn target(server: &Server<AnthropicProvider>, uri: &Uri, range: Range) -> TranslationTarget {
    let document = &server.documents[uri];
    TranslationTarget {
        uri: uri.clone(),
        version: document.version,
        session: document.session,
        range,
        target_language: "ja".into(),
    }
}

#[tokio::test(flavor = "current_thread")]
async fn anthropic_declares_automatic_selection_and_the_key_menu() {
    let initialized = capabilities::<AnthropicProvider>();
    assert_eq!(
        initialized.server_info.unwrap().name,
        "Translation LSP (Anthropic)"
    );
    assert_eq!(
        initialized
            .capabilities
            .execute_command_provider
            .unwrap()
            .commands,
        [
            COMMAND,
            RETRY_COMMAND,
            DOCUMENT_COMMAND,
            CANCEL_COMMAND,
            CANCEL_ALL_COMMAND,
            "translation-lsp.save-anthropic-key",
            "translation-lsp.remove-anthropic-key",
            SETTINGS_COMMAND
        ]
    );
    let fixture = Fixture::new().await;
    let (mut server, _connection, _client, _completions) =
        server(fixture.provider(), Settings::default());
    let uri: Uri = "file:///selection.md".parse().unwrap();
    let source = "Alpha sentence.";
    let range = Range::new(Position::new(0, 0), Position::new(0, source.len() as u32));
    server.documents.insert(
        uri.clone(),
        Document::new(1, 1, "markdown".into(), source.into()),
    );
    let titles: Vec<_> = actions(&mut server, &uri, range)
        .into_iter()
        .map(|action| action.title)
        .collect();
    assert_eq!(
        titles,
        ["Translate selection to Japanese (ja)", "Anthropic API key…"]
    );
    // A stable Markdown selection becomes an automatic candidate once a key is saved.
    assert!(server.selection.is_some());
    assert_eq!(fixture.calls(), 0);
}

#[tokio::test(flavor = "current_thread")]
async fn worker_translation_is_cached_and_read_by_hover() {
    let fixture = Fixture::new().await;
    let (mut server, connection, client, completions) =
        server(fixture.provider(), Settings::default());
    let uri: Uri = "file:///comment.rs".parse().unwrap();
    let source = "// Read this.";
    server.documents.insert(
        uri.clone(),
        Document::new(1, 1, "rust".into(), source.into()),
    );
    let hover =
        || parse(json!({"textDocument":{"uri":uri},"position":{"line":0,"character":4}})).unwrap();
    assert!(server.hover(1.into(), hover()).unwrap().is_none());
    server
        .finish(&connection, next(&completions).await)
        .unwrap();
    let Message::Response(response) = client.receiver.try_recv().unwrap() else {
        panic!("Expected Hover response");
    };
    let pending = response.response_result.unwrap();
    assert!(
        pending["contents"]["value"]
            .as_str()
            .unwrap()
            .contains("訳Read this.")
    );
    let preview = server.cache.latest(&uri).unwrap();
    assert_eq!(preview.translation, "訳Read this.");
    assert_eq!(preview.context, server.provider.context().unwrap());
    assert_eq!(server.documents[&uri].text(), Some(source));

    let cached = server.hover(2.into(), hover()).unwrap().unwrap();
    assert_eq!(cached, pending);
    assert!(
        !cached["contents"]["value"]
            .as_str()
            .unwrap()
            .contains("Mock translation")
    );
    let state = fixture.state.lock().unwrap();
    assert_eq!(state.records.len(), 1);
    let request: Value = serde_json::from_str(&state.records[0].body).unwrap();
    assert_eq!(
        request["messages"],
        json!([{"role": "user", "content": "Read this."}])
    );
}

#[tokio::test(flavor = "current_thread")]
async fn billing_stop_drains_queued_work_with_one_notice_and_resumes_after_explicit_success() {
    let fixture = Fixture::new().await;
    fixture.always(Some(Reply::failure(402, "billing_error", "No credit")));
    let mut settings = Settings::default();
    settings.translation_limits.work.concurrent = 1;
    let (mut server, connection, client, completions) = server(fixture.provider(), settings);
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
    let line = |line: u32| Range::new(Position::new(line, 0), Position::new(line, 15));

    // Hold dispatch until all three ranges are admitted under the same generation.
    let worker_jobs = server.jobs.take().unwrap();
    let (hold, mut held) = queue::channel(1);
    server.jobs = Some(hold);
    for index in [0, 2, 4] {
        let target = target(&server, &uri, line(index));
        assert_eq!(
            server
                .queue((index as i32 + 1).into(), target, PendingReply::Command)
                .unwrap(),
            Some(Value::Null)
        );
    }
    worker_jobs.try_send(held.recv().await.unwrap()).unwrap();
    server.jobs = Some(worker_jobs);
    for _ in 0..3 {
        server
            .finish(&connection, next(&completions).await)
            .unwrap();
    }
    assert_eq!(fixture.calls(), 1);
    let notices: Vec<_> = client.receiver.try_iter().collect();
    assert_eq!(
        notices.len(),
        1,
        "one stop notification, not one per queued range"
    );
    let Message::Notification(notice) = &notices[0] else {
        panic!("Expected the stop notification")
    };
    assert_eq!(notice.params["type"], 1);
    let message = notice.params["message"].as_str().unwrap();
    assert!(message.contains("Anthropic HTTP 402"), "{message}");
    assert!(message.contains("Automatic translation is paused"));
    for index in [0, 2, 4] {
        let hover = server
            .hover(
                10.into(),
                parse(json!({"textDocument":{"uri":uri},"position":{"line":index,"character":3}}))
                    .unwrap(),
            )
            .unwrap()
            .unwrap();
        assert!(
            hover["contents"]["value"]
                .as_str()
                .unwrap()
                .contains("translation failed")
        );
    }

    // Automatic selection stays stopped: the candidate is observed but not started.
    actions(&mut server, &uri, line(2));
    assert!(server.selection.is_some());
    server.selection.as_mut().unwrap().start = Some(Instant::now());
    server.start_selection();
    assert!(server.scheduler.translations().is_empty());
    assert_eq!(fixture.calls(), 1);

    fixture.always(None);
    let target = target(&server, &uri, line(0));
    server
        .queue(20.into(), target.clone(), PendingReply::Command)
        .unwrap();
    server
        .finish(&connection, next(&completions).await)
        .unwrap();
    assert_eq!(fixture.calls(), 2);
    assert!(
        server
            .cache
            .exact(&uri, target.range, Output::Range)
            .is_some()
    );
    server
        .provider
        .admit(&server.provider.context().unwrap(), Intent::Automatic)
        .unwrap();
}

#[tokio::test(flavor = "current_thread")]
async fn worker_cancellation_and_deadline_close_the_open_request() {
    for operation in ["cancel", "deadline"] {
        let fixture = Fixture::new().await;
        {
            let mut state = fixture.state.lock().unwrap();
            state.hold = true;
            state.fallback = Some(Reply::stream(format!(
                "{}{}",
                started(),
                text_block(0, "途中まで")
            )));
        }
        let (mut server, connection, client, completions) = server(
            fixture.provider(),
            Settings {
                hover_timeout_ms: if operation == "deadline" {
                    1_000
                } else {
                    20_000
                },
                ..Default::default()
            },
        );
        let uri: Uri = "file:///cancel.rs".parse().unwrap();
        server.documents.insert(
            uri.clone(),
            Document::new(1, 1, "rust".into(), "// Translate this.\n".into()),
        );
        assert!(
            server
                .hover(
                    1.into(),
                    parse(json!({"textDocument":{"uri":uri},"position":{"line":0,"character":4}}))
                        .unwrap(),
                )
                .unwrap()
                .is_none()
        );
        fixture.open.wait(1).await;
        if operation == "cancel" {
            server.cancel_all_work(&connection).unwrap();
        }
        // The worker drops the translation on its own at the execution deadline.
        fixture.open.wait(0).await;
        let completion = next(&completions).await;
        assert!(completion.error().is_some());
        server.finish(&connection, completion).unwrap();
        server.expire(&connection).unwrap();
        let Message::Response(response) = client.receiver.try_recv().unwrap() else {
            panic!("Expected the failed Hover response");
        };
        assert!(response.response_result.is_err());
        assert!(server.cache.latest(&uri).is_none());
        assert_eq!(fixture.calls(), 1);
    }
}
