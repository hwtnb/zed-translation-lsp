//! Several translations at once: HTTP slots, queues, and the credential store.
use super::*;

#[tokio::test(flavor = "current_thread")]
async fn two_http_translations_run_while_other_documents_stay_queued() {
    let fixture = Fixture::new().await;
    let tail = std::sync::Arc::new(StreamTailGate::default());
    {
        let mut state = fixture.state.lock().unwrap();
        state.response_mode = "delta_compact".into();
        state.stream_tail_gate = Some(tail.clone());
    }
    let auth = fixture.auth();
    fixture.connected(&auth, 3600).await;
    let provider = ChatGptProvider::synthetic(
        auth,
        format!("{}/v1", fixture.base),
        "synthetic-model".into(),
    );
    let (connection, client) = Connection::memory();
    let (finished, completions) = bounded(WORKER_EVENT_CHANNEL_CAPACITY);
    let mut settings = Settings::default();
    settings.translation_limits.work.automatic = 5;
    let mut server = Server::configured(
        finished,
        provider,
        settings,
        Ui::new(connection.sender.clone(), UiCapabilities::default()),
    )
    .unwrap();
    let mut uris = Vec::new();
    for index in 0..4 {
        let uri: Uri = format!("file:///parallel-{index}.rs").parse().unwrap();
        server
            .notification(
                &connection,
                lsp_server::Notification::new(
                    DidOpenTextDocument::METHOD.into(),
                    json!({"textDocument":{"uri":uri,"languageId":"rust","version":1,
                "text":format!("// Source {index}.\n\n// Other source.")}}),
                ),
            )
            .unwrap();
        uris.push(uri);
    }
    let request = |uri: &Uri, line| {
        parse(json!({"textDocument":{"uri":uri},"position":{"line":line,"character":4}})).unwrap()
    };
    assert!(
        server
            .hover(1.into(), request(&uris[0], 0))
            .unwrap()
            .is_none()
    );
    wait_held(&tail, 1).await;
    assert!(
        server
            .hover(2.into(), request(&uris[1], 0))
            .unwrap()
            .is_none()
    );
    wait_held(&tail, 2).await;
    assert!(
        server
            .hover(6.into(), request(&uris[0], 2))
            .unwrap()
            .is_none()
    );
    assert!(
        server
            .hover(3.into(), request(&uris[2], 0))
            .unwrap()
            .is_none()
    );
    assert!(
        server
            .hover(4.into(), request(&uris[3], 0))
            .unwrap()
            .is_none()
    );
    assert!(
        server
            .hover(5.into(), request(&uris[0], 0))
            .unwrap()
            .is_none()
    );
    assert_eq!(
        server.scheduler.pending()[&1.into()].ticket,
        server.scheduler.pending()[&5.into()].ticket
    );
    assert_eq!(server.scheduler.translations().len(), 5);
    assert_eq!(
        fixture
            .state
            .lock()
            .unwrap()
            .records
            .iter()
            .filter(|record| record.path == "/v1/responses")
            .count(),
        2
    );
    assert!((server.cache.successes() == 0));
    for _ in 0..5 {
        tail.release.notify_one();
        let completion = next(&completions).await;
        assert!(completion.is_ok());
        server.finish(&connection, completion).unwrap();
        if server.scheduler.translations().len() >= 2 {
            wait_held(&tail, 2).await;
        }
    }
    assert!(server.scheduler.translations().is_empty() && server.scheduler.pending().is_empty());
    assert_eq!(
        fixture
            .state
            .lock()
            .unwrap()
            .records
            .iter()
            .filter(|record| record.path == "/v1/responses")
            .count(),
        5
    );
    assert_eq!(tail.peak.load(std::sync::atomic::Ordering::SeqCst), 2);
    let replies: Vec<_> = client.receiver.try_iter().collect();
    assert_eq!(replies.len(), 6);
    assert!(replies.iter().all(
        |message| matches!(message, Message::Response(response) if response.response_result.is_ok())
    ));
    drop(completions);
    drop(server);
    wait_held(&tail, 0).await;
}

#[tokio::test(flavor = "current_thread")]
async fn parallel_retirement_refills_slots_without_caching_retired_work() {
    for operation in [
        "cancel-active",
        "cancel-queued",
        "timeout-active",
        "edit",
        "reopen",
    ] {
        let fixture = Fixture::new().await;
        let tail = std::sync::Arc::new(StreamTailGate::default());
        {
            let mut state = fixture.state.lock().unwrap();
            state.response_mode = "delta_compact".into();
            state.stream_tail_gate = Some(tail.clone());
        }
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
        let uris = open_parallel(&mut server, &connection);
        parallel_hover(&mut server, 1, &uris[0]);
        wait_held(&tail, 1).await;
        parallel_hover(&mut server, 2, &uris[1]);
        wait_held(&tail, 2).await;
        parallel_hover(&mut server, 3, &uris[2]);
        assert_eq!(response_calls(&fixture), 2);
        // Later HTTP requests finish independently of the held initial streams.
        let following = std::sync::Arc::new(StreamTailGate::default());
        fixture.state.lock().unwrap().stream_tail_gate = Some(following.clone());
        let queued = operation.ends_with("queued");
        let id = RequestId::from(if queued { 3 } else { 1 });
        let ticket = server.scheduler.pending()[&id].ticket;
        let expected = match operation {
            "timeout-active" => {
                server.scheduler.translations()[&ticket]
                    .deadline
                    .send_replace(Some(Instant::now()));
                let completion = next(&completions).await;
                assert_eq!(completion.ticket(), ticket);
                assert!(
                    completion
                        .error()
                        .is_some_and(|error| error.to_string().contains("timed out"))
                );
                server.finish(&connection, completion).unwrap();
                ErrorCode::RequestFailed
            }
            "edit" => {
                server
                    .notification(
                        &connection,
                        lsp_server::Notification::new(
                            DidChangeTextDocument::METHOD.into(),
                            json!({"textDocument":{"uri":uris[0],"version":2},
                        "contentChanges":[{"text":"// Changed source."}]}),
                        ),
                    )
                    .unwrap();
                ErrorCode::ContentModified
            }
            "reopen" => {
                let session = server.documents[&uris[0]].session;
                server
                    .notification(
                        &connection,
                        lsp_server::Notification::new(
                            DidCloseTextDocument::METHOD.into(),
                            json!({"textDocument":{"uri":uris[0]}}),
                        ),
                    )
                    .unwrap();
                server
                    .notification(
                        &connection,
                        lsp_server::Notification::new(
                            DidOpenTextDocument::METHOD.into(),
                            json!({"textDocument":{"uri":uris[0],"version":1,
                        "languageId":"rust","text":"// Reopened source."}}),
                        ),
                    )
                    .unwrap();
                assert_ne!(server.documents[&uris[0]].session, session);
                ErrorCode::ContentModified
            }
            _ => {
                server
                    .cancel_range(&connection, ticket, ErrorCode::RequestCanceled)
                    .unwrap();
                ErrorCode::RequestCanceled
            }
        };
        let Message::Response(retired) = client.receiver.try_recv().unwrap() else {
            panic!("Expected retired response");
        };
        assert_eq!(retired.id, id);
        assert_eq!(retired.response_result.unwrap_err().code, expected as i32);
        assert!(
            server
                .cache
                .latest(&uris[if queued { 2 } else { 0 }])
                .is_none()
        );
        if !queued {
            if operation != "timeout-active" {
                let canceled = next(&completions).await;
                assert!(
                    canceled
                        .error()
                        .is_some_and(|error| error.kind() == ErrorKind::Cancelled)
                );
                server.finish(&connection, canceled).unwrap();
            }
            // The queued HTTP starts only after the canceled worker releases its slot.
            wait_held(&following, 1).await;
            assert_eq!(response_calls(&fixture), 3);
        }
        tail.release.notify_waiters();
        following.release.notify_waiters();
        while !server.scheduler.translations().is_empty() {
            let completion = next(&completions).await;
            assert!(
                completion.is_ok(),
                "{operation}: {}",
                completion
                    .error()
                    .map(|e| e.to_string())
                    .unwrap_or_default()
            );
            server.finish(&connection, completion).unwrap();
        }
        assert!(server.scheduler.pending().is_empty());
        assert_eq!(response_calls(&fixture), if queued { 2 } else { 3 });
        let successful: Vec<_> = client.receiver.try_iter().collect();
        assert_eq!(successful.len(), 2);
        assert!(successful.iter().all(|message| matches!(message, Message::Response(response) if response.response_result.is_ok())), "{operation}: {successful:?}");
        assert!(
            server
                .cache
                .latest(&uris[if queued { 2 } else { 0 }])
                .is_none()
        );
        if matches!(operation, "edit" | "reopen") {
            fixture.state.lock().unwrap().stream_tail_gate = None;
            parallel_hover(&mut server, 5, &uris[0]);
            let completion = next(&completions).await;
            assert!(completion.is_ok());
            server.finish(&connection, completion).unwrap();
            assert!(
                server
                    .cache
                    .latest(&uris[0])
                    .as_ref()
                    .unwrap()
                    .translation
                    .contains(if operation == "edit" {
                        "Changed source."
                    } else {
                        "Reopened source."
                    })
            );
            assert_eq!(response_calls(&fixture), 4);
        }
        drop(completions);
        drop(server);
    }
}

#[tokio::test(flavor = "current_thread")]
async fn parallel_account_change_and_shutdown_retire_all_jobs() {
    for operation in ["account-change", "shutdown"] {
        let fixture = Fixture::new().await;
        let tail = std::sync::Arc::new(StreamTailGate::default());
        {
            let mut state = fixture.state.lock().unwrap();
            state.response_mode = "delta_compact".into();
            state.stream_tail_gate = Some(tail.clone());
        }
        let auth = fixture.auth();
        fixture.connected(&auth, 3600).await;
        let provider = ChatGptProvider::synthetic(
            auth.clone(),
            format!("{}/v1", fixture.base),
            "synthetic-model".into(),
        );
        let (connection, client) = Connection::memory();
        let (finished, completions) = bounded(WORKER_EVENT_CHANNEL_CAPACITY);
        let mut settings = Settings::default();
        settings.translation_limits.work.automatic = 4;
        let mut server = Server::configured(
            finished,
            provider,
            settings,
            Ui::new(connection.sender.clone(), UiCapabilities::default()),
        )
        .unwrap();
        server.settings.translation_limits.work.documents = 2;
        let uris = open_parallel(&mut server, &connection);
        parallel_hover(&mut server, 1, &uris[0]);
        wait_held(&tail, 1).await;
        parallel_hover(&mut server, 2, &uris[1]);
        wait_held(&tail, 2).await;
        parallel_hover(&mut server, 3, &uris[2]);
        parallel_hover(&mut server, 4, &uris[3]);
        let targets: Vec<_> = server
            .scheduler
            .translations()
            .values()
            .map(|work| work.target.clone())
            .collect();
        for target in targets {
            server
                .queue_translation(target, TranslationRequest::Document)
                .unwrap();
        }
        if operation == "account-change" {
            // Another process changes the account: running requests complete and
            // their results are discarded, and queued ones are not sent.
            fixture.connected(&auth, 3600).await;
            tail.release.notify_waiters();
            for _ in 0..4 {
                let completion = next(&completions).await;
                server.finish(&connection, completion).unwrap();
            }
        } else {
            server.cancel_all_work(&connection).unwrap();
        }
        assert!(
            server.scheduler.pending().is_empty() && server.scheduler.translations().is_empty()
        );
        assert!((server.cache.successes() == 0));
        assert_eq!(response_calls(&fixture), 2);
        let replies: Vec<_> = client.receiver.try_iter().collect();
        assert_eq!(replies.len(), 4);
        assert!(replies.iter().all(|message| matches!(message, Message::Response(response) if
            response.response_result.as_ref().is_err_and(|error| error.code == if operation == "shutdown" {
                ErrorCode::RequestCanceled as i32
            } else {ErrorCode::ContentModified as i32}))));
        if operation == "account-change" {
            fixture.state.lock().unwrap().stream_tail_gate = None;
            parallel_hover(&mut server, 5, &uris[0]);
            let completion = next(&completions).await;
            assert!(
                completion.is_ok(),
                "{operation}: {}",
                completion
                    .error()
                    .map(|e| e.to_string())
                    .unwrap_or_default()
            );
            server.finish(&connection, completion).unwrap();
            assert!(server.cache.latest(&uris[0]).is_some());
            assert_eq!(response_calls(&fixture), 3);
        }
        let started = Instant::now();
        drop(completions);
        drop(server);
        assert!(started.elapsed() < Duration::from_millis(500));
        // These are synthetic server-side response bodies, not occupied client slots.
        tail.release.notify_waiters();
        wait_held(&tail, 0).await;
    }
}

#[tokio::test(flavor = "current_thread")]
async fn cached_translation_survives_store_lock_but_not_reauthorization() {
    let fixture = Fixture::new().await;
    let auth = fixture.auth();
    let registration = fixture.connected(&auth, 3600).await;
    let provider = ChatGptProvider::synthetic(
        auth.clone(),
        format!("{}/v1", fixture.base),
        "synthetic-model".into(),
    );
    let (connection, client) = Connection::memory();
    let (finished, completions) = bounded(WORKER_EVENT_CHANNEL_CAPACITY);
    let mut server = Server::configured(
        finished,
        provider.clone(),
        Settings::default(),
        Ui::new(connection.sender.clone(), UiCapabilities::default()),
    )
    .unwrap();
    let uri: Uri = "file:///cached.rs".parse().unwrap();
    server.documents.insert(
        uri.clone(),
        Document::new(1, 1, "rust".into(), "// Read this.\n".into()),
    );
    let hover =
        || parse(json!({"textDocument":{"uri":uri},"position":{"line":0,"character":4}})).unwrap();
    assert!(server.hover(1.into(), hover()).unwrap().is_none());
    server
        .finish(&connection, next(&completions).await)
        .unwrap();
    client.receiver.try_recv().unwrap();
    assert!(server.cache.latest(&uri).is_some());
    let mut writer = auth.store.lock(false).await.unwrap().unwrap();
    assert!(server.hover(2.into(), hover()).is_err());
    assert!(server.cache.latest(&uri).is_some());
    writer.save().unwrap();
    drop(writer);
    assert!(server.hover(3.into(), hover()).unwrap().is_some());
    assert_eq!(response_calls(&fixture), 1);
    writer = auth.store.lock(false).await.unwrap().unwrap();
    writer
        .state
        .registrations
        .get_mut(&registration)
        .unwrap()
        .authorization_id = uuid::Uuid::new_v4();
    writer.save().unwrap();
    drop(writer);
    fixture.state.lock().unwrap().translation = Some("New authorization translation".into());
    assert!(server.hover(4.into(), hover()).unwrap().is_none());
    assert!(server.cache.latest(&uri).is_none());
    server
        .finish(&connection, next(&completions).await)
        .unwrap();
    assert_eq!(
        server.cache.latest(&uri).as_ref().unwrap().translation,
        "New authorization translation"
    );
    assert_eq!(response_calls(&fixture), 2);
}

#[tokio::test(flavor = "current_thread")]
async fn concurrent_refresh_preserves_authorization_and_completes_both_requests() {
    let fixture = Fixture::new().await;
    fixture.state.lock().unwrap().response_mode = "delta_compact".into();
    let auth = fixture.auth();
    fixture.connected(&auth, 0).await;
    let provider = ChatGptProvider::synthetic(
        auth,
        format!("{}/v1", fixture.base),
        "synthetic-model".into(),
    );
    let context = provider.context().unwrap();
    let (first, second) = tokio::join!(
        provider.translate(
            crate::providers::Intent::Automatic,
            crate::providers::Output::Range,
            "First source.",
            "ja",
            context.clone()
        ),
        provider.translate(
            crate::providers::Intent::Automatic,
            crate::providers::Output::Range,
            "Second source.",
            "ja",
            context
        ),
    );
    let mut successes = 0;
    for result in [first, second] {
        match result {
            Ok(translated) => {
                successes += 1;
                assert_eq!(translated.context, provider.context().unwrap());
            }
            Err(error) => assert_eq!(error.kind(), ErrorKind::Modified),
        }
    }
    assert_eq!(successes, 2);
    assert_eq!(fixture.state.lock().unwrap().refreshes, 1);
    assert_eq!(response_calls(&fixture), 2);
    let retry = provider
        .translate(
            crate::providers::Intent::Automatic,
            crate::providers::Output::Range,
            "Retry source.",
            "ja",
            provider.context().unwrap(),
        )
        .await
        .unwrap();
    assert!(retry.text.contains("Retry source."));
    assert_eq!(fixture.state.lock().unwrap().refreshes, 1);
    assert_eq!(response_calls(&fixture), 3);
}

#[tokio::test(flavor = "current_thread")]
async fn fast_finishes_refill_slots_and_admit_new_work_without_read_lock_failures() {
    let fixture = Fixture::new().await;
    fixture.state.lock().unwrap().response_mode = "delta_compact".into();
    let auth = fixture.auth();
    fixture.connected(&auth, 3600).await;
    let provider = ChatGptProvider::synthetic(
        auth,
        format!("{}/v1", fixture.base),
        "synthetic-model".into(),
    );
    for round in 0..10 {
        let tail = std::sync::Arc::new(StreamTailGate::default());
        fixture.state.lock().unwrap().stream_tail_gate = Some(tail.clone());
        let (connection, client) = Connection::memory();
        let (finished, completions) = bounded(WORKER_EVENT_CHANNEL_CAPACITY);
        let mut settings = Settings::default();
        settings.translation_limits.work.automatic = 4;
        let mut server = Server::configured(
            finished,
            provider.clone(),
            settings,
            Ui::new(connection.sender.clone(), UiCapabilities::default()),
        )
        .unwrap();
        let uris = open_parallel(&mut server, &connection);
        parallel_hover(&mut server, 1, &uris[0]);
        wait_held(&tail, 1).await;
        parallel_hover(&mut server, 2, &uris[1]);
        parallel_hover(&mut server, 3, &uris[2]);
        parallel_hover(&mut server, 4, &uris[3]);
        fixture.state.lock().unwrap().stream_tail_gate = None;
        tail.release.notify_waiters();
        // Finish and admit while the worker starts and completes following HTTPs.
        let completion = next(&completions).await;
        assert!(completion.is_ok());
        server.finish(&connection, completion).unwrap();
        let uri: Uri = "file:///new-admission.rs".parse().unwrap();
        server
            .notification(
                &connection,
                lsp_server::Notification::new(
                    DidOpenTextDocument::METHOD.into(),
                    json!({"textDocument":{"uri":uri,"version":1,
                "languageId":"rust","text":"// New source."}}),
                ),
            )
            .unwrap();
        parallel_hover(&mut server, 5, &uri);
        while !server.scheduler.translations().is_empty() {
            let completion = next(&completions).await;
            assert!(completion.is_ok());
            server.finish(&connection, completion).unwrap();
        }
        assert_eq!(response_calls(&fixture), (round + 1) * 5);
        assert!(server.scheduler.pending().is_empty());
        assert_eq!(server.cache.successes(), 5);
        let replies: Vec<_> = client.receiver.try_iter().collect();
        assert_eq!(replies.len(), 5);
        assert!(replies.iter().all(|message| matches!(message, Message::Response(response) if response.response_result.is_ok())), "{replies:?}");
        drop(completions);
        drop(server);
    }
}

#[tokio::test(flavor = "current_thread")]
async fn parallel_incomplete_streams_do_not_cache_and_release_slots() {
    let fixture = Fixture::new().await;
    let invalid = std::sync::Arc::new(StreamTailGate::default());
    let following = std::sync::Arc::new(StreamTailGate::default());
    {
        let mut state = fixture.state.lock().unwrap();
        state.response_mode = "item_without_completed".into();
        state.stream_tail_gate = Some(invalid.clone());
    }
    let auth = fixture.auth();
    fixture.connected(&auth, 3600).await;
    let provider = ChatGptProvider::synthetic(
        auth,
        format!("{}/v1", fixture.base),
        "synthetic-model".into(),
    );
    let (connection, client) = Connection::memory();
    let (finished, completions) = bounded(WORKER_EVENT_CHANNEL_CAPACITY);
    let mut settings = Settings::default();
    settings.translation_limits.work.automatic = 4;
    let mut server = Server::configured(
        finished,
        provider,
        settings,
        Ui::new(connection.sender.clone(), UiCapabilities::default()),
    )
    .unwrap();
    let uris = open_parallel(&mut server, &connection);
    parallel_hover(&mut server, 1, &uris[0]);
    wait_held(&invalid, 1).await;
    parallel_hover(&mut server, 2, &uris[1]);
    wait_held(&invalid, 2).await;
    {
        let mut state = fixture.state.lock().unwrap();
        state.response_mode = "delta_compact".into();
        state.stream_tail_gate = Some(following.clone());
    }
    parallel_hover(&mut server, 3, &uris[2]);
    parallel_hover(&mut server, 4, &uris[3]);
    assert_eq!(response_calls(&fixture), 2);
    assert!((server.cache.successes() == 0));
    invalid.release.notify_waiters();
    for _ in 0..2 {
        let completion = next(&completions).await;
        assert!(completion.error().is_some_and(|e| {
            e.to_string()
                .contains("OpenAI stream ended without response.completed")
        }));
        server.finish(&connection, completion).unwrap();
    }
    wait_held(&following, 2).await;
    assert_eq!(response_calls(&fixture), 4);
    following.release.notify_waiters();
    for _ in 0..2 {
        let completion = next(&completions).await;
        assert!(completion.is_ok());
        server.finish(&connection, completion).unwrap();
    }
    assert!(server.scheduler.pending().is_empty() && server.scheduler.translations().is_empty());
    for (index, uri) in uris.iter().enumerate() {
        assert_eq!(server.cache.latest(uri).is_some(), index >= 2);
    }
    let replies: Vec<_> = client.receiver.try_iter().collect();
    assert_eq!(replies.len(), 4);
    for message in replies {
        let Message::Response(response) = message else {
            panic!("Expected Hover reply");
        };
        if response.id == RequestId::from(1) || response.id == RequestId::from(2) {
            assert_eq!(
                response.response_result.unwrap_err().code,
                ErrorCode::RequestFailed as i32
            );
        } else {
            assert!(response.response_result.is_ok());
        }
    }
    drop(completions);
    drop(server);
}

#[tokio::test(flavor = "current_thread")]
async fn canceled_queued_jobs_release_capacity_while_active_requests_keep_running() {
    let fixture = Fixture::new().await;
    let tail = std::sync::Arc::new(StreamTailGate::default());
    fixture.state.lock().unwrap().stream_tail_gate = Some(tail.clone());
    let auth = fixture.auth();
    fixture.connected(&auth, 3600).await;
    let provider = ChatGptProvider::synthetic(
        auth,
        format!("{}/v1", fixture.base),
        "synthetic-model".into(),
    );
    let (connection, client) = Connection::memory();
    let (finished, completions) = bounded(WORKER_EVENT_CHANNEL_CAPACITY);
    let mut settings = Settings::default();
    settings.translation_limits.work.automatic = 4;
    let mut server = Server::configured(
        finished,
        provider,
        settings,
        Ui::new(connection.sender.clone(), UiCapabilities::default()),
    )
    .unwrap();
    let uris = open_parallel(&mut server, &connection);
    parallel_hover(&mut server, 1, &uris[0]);
    wait_held(&tail, 1).await;
    parallel_hover(&mut server, 2, &uris[1]);
    wait_held(&tail, 2).await;
    // Preserve this live queued request across repeated cancellation of another.
    parallel_hover(&mut server, 3, &uris[2]);
    for id in 4..14 {
        parallel_hover(&mut server, id, &uris[3]);
        assert_eq!(server.scheduler.translations().len(), 4);
        server
            .cancel_range(
                &connection,
                server.scheduler.pending()[&id.into()].ticket,
                ErrorCode::RequestCanceled,
            )
            .unwrap();
        let Message::Response(response) = client.receiver.try_recv().unwrap() else {
            panic!("Expected cancellation response")
        };
        assert_eq!(response.id, id.into());
        assert_eq!(
            response.response_result.unwrap_err().code,
            ErrorCode::RequestCanceled as i32
        );
        assert_eq!(response_calls(&fixture), 2);
    }
    parallel_hover(&mut server, 14, &uris[3]);
    fixture.state.lock().unwrap().stream_tail_gate = None;
    tail.release.notify_waiters();
    while !server.scheduler.translations().is_empty() {
        server
            .finish(&connection, next(&completions).await)
            .unwrap();
    }
    assert_eq!(response_calls(&fixture), 4);
    assert_eq!(client.receiver.try_iter().count(), 4);
    assert!(uris.iter().all(|uri| server.cache.latest(uri).is_some()));
}
