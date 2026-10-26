//! Tab translations and results shared between outputs.
use std::{fs, sync::Arc};

use reqwest::Url;
use tokio::time::sleep;

use super::*;

#[tokio::test(flavor = "current_thread")]
async fn document_pipeline_opens_large_translation_and_does_not_publish_oversized_output() {
    for (limit, output_bytes, success) in
        [(4096, 96 * 1024, true), (1, 1024, true), (1, 1025, false)]
    {
        let fixture = Fixture::new().await;
        let auth = fixture.auth();
        fixture.connected(&auth, 3600).await;
        let translated = format!("{}é", "x".repeat(output_bytes - 2));
        fixture.state.lock().unwrap().translation = Some(translated.clone());
        let provider = ChatGptProvider::synthetic(
            auth,
            format!("{}/v1", fixture.base),
            "synthetic-model".into(),
        );
        let settings =
            Settings::from_options(Some(json!({"translation_limits":{"output_kib":limit}})))
                .unwrap();
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
        let uri: Uri = "file:///large-document.md".parse().unwrap();
        let source = "Read this sentence. ".repeat(2000);
        let range = Range::new(Position::new(0, 0), Position::new(0, source.len() as u32));
        server.documents.insert(
            uri.clone(),
            Document::new(1, 1, "markdown".into(), source.clone()),
        );
        let target = TranslationTarget {
            uri: uri.clone(),
            version: 1,
            session: 1,
            range,
            target_language: "ja".into(),
        };
        server
            .queue_translation(target, TranslationRequest::Document)
            .unwrap();
        server
            .finish(&connection, next(&completions).await)
            .unwrap();
        assert_eq!(server.documents[&uri].text(), Some(source.as_str()));
        assert_eq!(server.cache.latest(&uri).is_some(), success);
        let messages: Vec<_> = client.receiver.try_iter().collect();
        if success {
            let open = messages
                .iter()
                .find_map(|message| match message {
                    Message::Request(request) if request.method == "window/showDocument" => {
                        Some(request)
                    }
                    _ => None,
                })
                .unwrap();
            let path = Url::parse(open.params["uri"].as_str().unwrap())
                .unwrap()
                .to_file_path()
                .unwrap();
            assert_eq!(fs::read_to_string(&path).unwrap(), translated);
            drop(server);
            assert!(!path.exists());
        } else {
            assert!(!messages.iter().any(|message| match message {
                Message::Request(request) => request.method == "window/showDocument",
                _ => false,
            }));
            assert!(messages.iter().any(|message| {
                match message {
                    Message::Notification(notification) => notification
                        .params
                        .to_string()
                        .contains("translation_limits.output_kib"),
                    _ => false,
                }
            }));
        }
    }
}

#[tokio::test(flavor = "current_thread")]
async fn document_promotion_uses_original_start_and_survives_hover_expiry() {
    let settings = Settings {
        hover_timeout_ms: 80,
        selection_timeout_ms: 150,
        document_timeout_ms: 1000,
        ..Default::default()
    };
    let (fixture, mut server, connection, client, completions, uri) = range_server(settings).await;
    let tail = Arc::new(StreamTailGate::default());
    fixture.state.lock().unwrap().stream_tail_gate = Some(tail.clone());
    let target = range_target(&server, &uri, 0);
    server
        .queue(
            1.into(),
            target.clone(),
            PendingReply::Hover(Position::new(0, 3)),
        )
        .unwrap();
    let ticket = server.scheduler.pending()[&1.into()].ticket;
    let start = server.scheduler.translations()[&ticket].started.unwrap();
    wait_held(&tail, 1).await;
    server
        .queue_translation(target.clone(), TranslationRequest::Document)
        .unwrap();
    assert_eq!(
        server.scheduler.translations()[&ticket].execution_deadline(),
        Some(start + Duration::from_millis(1000))
    );
    sleep(Duration::from_millis(100)).await;
    server.expire(&connection).unwrap();
    assert!(server.scheduler.pending().is_empty());
    assert!(server.scheduler.translations()[&ticket].owners.tab());
    server
        .queue_translation(target, TranslationRequest::Document)
        .unwrap();
    assert_eq!(
        server.scheduler.translations()[&ticket].execution_deadline(),
        Some(start + Duration::from_millis(1000))
    );
    tail.release.notify_one();
    server
        .finish(&connection, next(&completions).await)
        .unwrap();
    let messages: Vec<_> = client.receiver.try_iter().collect();
    assert!(messages.iter().any(|m| matches!(m, Message::Response(r) if r.id == RequestId::from(1) && r.response_result.is_err())));
    assert_eq!(
        messages
            .iter()
            .filter(|m| matches!(m, Message::Request(r) if r.method == "window/showDocument"))
            .count(),
        1
    );
    assert_eq!(response_calls(&fixture), 1);
    assert!(
        server
            .cache
            .hover(&uri, Position::new(0, 3), |_| true)
            .is_none()
    );
}

#[tokio::test(flavor = "current_thread")]
async fn late_hover_join_does_not_extend_http_execution_cap() {
    let settings = Settings {
        hover_timeout_ms: 150,
        ..Default::default()
    };
    let (fixture, mut server, connection, client, completions, uri) = range_server(settings).await;
    let tail = Arc::new(StreamTailGate::default());
    fixture.state.lock().unwrap().stream_tail_gate = Some(tail.clone());
    let target = range_target(&server, &uri, 0);
    server
        .queue(
            1.into(),
            target.clone(),
            PendingReply::Hover(Position::new(0, 3)),
        )
        .unwrap();
    wait_held(&tail, 1).await;
    let cap = server
        .scheduler
        .translations()
        .values()
        .next()
        .unwrap()
        .execution_deadline();
    sleep(Duration::from_millis(60)).await;
    server
        .queue(2.into(), target, PendingReply::Hover(Position::new(0, 4)))
        .unwrap();
    assert_eq!(
        server
            .scheduler
            .translations()
            .values()
            .next()
            .unwrap()
            .execution_deadline(),
        cap
    );
    sleep(Duration::from_millis(120)).await;
    server.expire(&connection).unwrap();
    server
        .finish(&connection, next(&completions).await)
        .unwrap();
    assert!(server.scheduler.translations().is_empty());
    let responses: Vec<_> = client
        .receiver
        .try_iter()
        .filter_map(|m| {
            if let Message::Response(r) = m {
                Some(r)
            } else {
                None
            }
        })
        .collect();
    assert_eq!(responses.len(), 2);
    assert!(responses.iter().all(|r| r.response_result.is_err()));
    assert_eq!(response_calls(&fixture), 1);
}

#[tokio::test(flavor = "current_thread")]
async fn explicit_outputs_reuse_completed_ranges_without_automatic_hover_promotion() {
    for tab_first in [false, true] {
        let (fixture, mut server, connection, client, completions, uri) =
            range_server(Settings::default()).await;
        let target = range_target(&server, &uri, 0);
        let request = |tab| {
            if tab {
                TranslationRequest::Document
            } else {
                TranslationRequest::Reply(1.into(), PendingReply::Command)
            }
        };
        server
            .queue_translation(target.clone(), request(tab_first))
            .unwrap();
        server
            .finish(&connection, next(&completions).await)
            .unwrap();
        let opened = |client: &Connection| {
            client.receiver.try_iter().find_map(|m| match m {
                Message::Request(r) if r.method == "window/showDocument" => {
                    Some(r.params["uri"].clone())
                }
                _ => None,
            })
        };
        let first_uri = opened(&client);
        if tab_first {
            let hover = server
                .hover(
                    2.into(),
                    parse(json!({"textDocument":{"uri":uri},"position":target.range.start}))
                        .unwrap(),
                )
                .unwrap();
            assert_eq!(hover, Some(Value::Null));
        }
        server
            .queue_translation(target.clone(), request(!tab_first))
            .unwrap();
        let second_uri = opened(&client);
        assert!(
            server.scheduler.translations().is_empty(),
            "Switching outputs must not enqueue another translation"
        );
        assert_eq!(response_calls(&fixture), 1);
        assert_eq!(
            server
                .cache
                .exact(&uri, target.range, Output::Document)
                .unwrap()
                .translation,
            server
                .cache
                .exact(&uri, target.range, Output::Range)
                .unwrap()
                .translation
        );
        server
            .queue_translation(target, TranslationRequest::Document)
            .unwrap();
        assert_eq!(
            opened(&client),
            first_uri.or(second_uri),
            "Reused outputs must preserve the temporary file identity"
        );
        assert_eq!(response_calls(&fixture), 1);
    }
}

#[tokio::test(flavor = "current_thread")]
async fn automatic_selection_and_retry_do_not_reuse_tab_results() {
    for retry in [false, true] {
        let (fixture, mut server, connection, client, completions, uri) =
            range_server(Settings::default()).await;
        let target = range_target(&server, &uri, 0);
        server
            .queue_translation(target.clone(), TranslationRequest::Document)
            .unwrap();
        server
            .finish(&connection, next(&completions).await)
            .unwrap();
        client.receiver.try_iter().for_each(drop);
        let request = if retry {
            TranslationRequest::Reply(1.into(), PendingReply::Retry)
        } else {
            TranslationRequest::Selection
        };
        server.queue_translation(target, request).unwrap();
        server
            .finish(&connection, next(&completions).await)
            .unwrap();
        assert_eq!(response_calls(&fixture), 2);
    }
}

#[tokio::test(flavor = "current_thread")]
async fn cross_output_reuse_checks_input_limits_and_document_freshness() {
    let settings = Settings::from_options(Some(
        json!({"translation_limits":{"input_kib":{"hover":1}}}),
    ))
    .unwrap();
    let (fixture, mut server, connection, client, completions, uri) = range_server(settings).await;
    let source = "A".repeat(1500);
    server.documents.insert(
        uri.clone(),
        Document::new(1, 1, "markdown".into(), source.clone()),
    );
    let mut target = range_target(&server, &uri, 0);
    target.range = server.documents[&uri].full_range().unwrap();
    server
        .queue_translation(target.clone(), TranslationRequest::Document)
        .unwrap();
    server
        .finish(&connection, next(&completions).await)
        .unwrap();
    client.receiver.try_iter().for_each(drop);
    let error = server
        .queue(1.into(), target.clone(), PendingReply::Command)
        .unwrap_err();
    assert_eq!(error.code, ErrorCode::InvalidParams as i32);
    assert!(
        server
            .cache
            .exact(&uri, target.range, Output::Range)
            .is_none()
    );
    assert_eq!(response_calls(&fixture), 1);
    server.notification(&connection, lsp_server::Notification::new(DidChangeTextDocument::METHOD.into(), json!({"textDocument":{"uri":uri,"version":2},"contentChanges":[{"text":"Changed."}]}))).unwrap();
    let error = server
        .queue_translation(target, TranslationRequest::Document)
        .unwrap_err();
    assert_eq!(error.code, ErrorCode::ContentModified as i32);
    assert!(server.cache.latest(&uri).is_none());
    assert_eq!(response_calls(&fixture), 1);
}
