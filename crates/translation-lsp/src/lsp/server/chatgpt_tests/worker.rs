//! What the worker does with deadlines, cancellation, and failed streams.
use super::*;

#[tokio::test(flavor = "current_thread")]
async fn worker_deadline_and_cancellation_stop_overload_backoff() {
    for cancel in [false, true] {
        let fixture = Fixture::new().await;
        fixture.state.lock().unwrap().response_failures.push_back((
            200,
            json!({"type":"error", "code":"server_is_overloaded", "message":"Overloaded"}),
        ));
        let auth = fixture.auth();
        fixture.connected(&auth, 3600).await;
        let provider = ChatGptProvider::synthetic(
            auth,
            format!("{}/v1", fixture.base),
            "synthetic-model".into(),
        );
        let (stop, deadline) = watch::channel(Some(Instant::now() + Duration::from_secs(5)));
        let (finished, events) = bounded(WORKER_EVENT_CHANNEL_CAPACITY);
        let job = Job {
            ticket: 1,
            deadline,
            task: Task::Translation {
                intent: crate::providers::Intent::Automatic,
                output: Output::Range,
                generation: 0,
                text: "Read this.".into(),
                target_language: "ja".into(),
                context: provider.context().unwrap(),
            },
        };
        let running = run_job(job, &finished, &provider);
        tokio::pin!(running);
        assert!(
            tokio::time::timeout(Duration::from_millis(100), &mut running)
                .await
                .is_err()
        );
        assert_eq!(response_calls(&fixture), 1);
        stop.send_replace(if cancel { None } else { Some(Instant::now()) });
        tokio::time::timeout(Duration::from_secs(1), &mut running)
            .await
            .unwrap();
        assert!(matches!(
            events.try_recv().unwrap(),
            WorkerEvent::Started(1)
        ));
        if !cancel {
            let WorkerEvent::Finished(completion) = events.try_recv().unwrap() else {
                panic!("Expected deadline failure");
            };
            assert!(
                completion
                    .error()
                    .is_some_and(|error| error.to_string() == "Translation timed out")
            );
        }
        if cancel {
            assert!(
                matches!(events.try_recv().unwrap(), WorkerEvent::Finished(f) if f.error().is_some())
            );
        }
        assert!(events.try_recv().is_err());
        assert_eq!(response_calls(&fixture), 1);
    }
}

#[tokio::test(flavor = "current_thread")]
async fn explicit_preview_reports_admission_refresh_and_stream_failures() {
    let source = "Private source sentence.";
    for mode in ["admission", "refresh", "stream", "wrapped-stream"] {
        let fixture = Fixture::new().await;
        let auth = fixture.auth();
        if mode != "admission" {
            fixture
                .connected(&auth, if mode == "refresh" { 0 } else { 3600 })
                .await;
        }
        if mode == "refresh" {
            fixture.state.lock().unwrap().refresh_failure = Some((
                400,
                json!({"error":"invalid_grant","error_description":"synthetic refresh invalid"}),
            ));
        } else if mode == "stream" {
            fixture.state.lock().unwrap().stream_error = Some(
                json!({"type":"error","code":"unsupported_value","param":"reasoning.effort","message":format!("unsupported: {source} synthetic-access-0")}),
            );
        } else if mode == "wrapped-stream" {
            fixture.state.lock().unwrap().stream_error =
                Some(json!({"type":"error","sequence_number":1,"error":{
                    "code":"unsupported_value","param":"reasoning.effort",
                    "message":format!("unsupported: {source} synthetic-access-0")}}));
        }
        let before = auth.store.snapshot().unwrap();
        let provider = ChatGptProvider::synthetic(
            auth.clone(),
            format!("{}/v1", fixture.base),
            "synthetic-model".into(),
        );
        let (connection, client) = Connection::memory();
        let (finished, completions) = bounded(WORKER_EVENT_CHANNEL_CAPACITY);
        let mut server = Server::configured(
            finished,
            provider,
            Settings::default(),
            Ui::new(
                connection.sender.clone(),
                UiCapabilities {
                    show_document: true,
                    ..Default::default()
                },
            ),
        )
        .unwrap();
        let uri: Uri = "file:///command-failure.md".parse().unwrap();
        server.documents.insert(
            uri.clone(),
            Document::new(1, 1, "markdown".into(), source.into()),
        );
        let range =
            json!({"start":{"line":0,"character":0},"end":{"line":0,"character":source.len()}});
        let action = json!({"command":COMMAND,"arguments":[{"uri":uri,"version":1,"session":1,"range":range,"targetLanguage":"ja"}]});
        server
            .request(
                &connection,
                Request::new(1.into(), ExecuteCommand::METHOD.into(), action),
            )
            .unwrap();
        if mode != "admission" {
            server
                .finish(&connection, next(&completions).await)
                .unwrap();
        }
        let error = command_notice(&client, 1, 1).unwrap_err();
        assert_eq!(error.code, ErrorCode::RequestFailed as i32);
        assert!(!error.message.contains(source));
        assert!(!error.message.contains("synthetic-access-0"));
        assert!(server.cache.latest(&uri).is_none());
        assert_eq!(server.documents[&uri].text(), Some(source));
        if mode == "refresh" {
            assert!(error.message.contains("code=invalid_grant"));
            assert!(error.message.contains("request_id=req-synthetic"));
            assert!(error.message.contains("reconnect the saved account"));
            let before = before.unwrap().state;
            let after = auth.store.snapshot().unwrap().unwrap().state;
            let id = before.active_registration.unwrap();
            assert_eq!(after.active_registration, Some(id));
            assert_eq!(after.ext_agent_host_id, before.ext_agent_host_id);
            assert_eq!(
                after.registrations[&id].client_id,
                before.registrations[&id].client_id
            );
            assert!(after.registrations[&id].credentials.is_none());
        } else if matches!(mode, "stream" | "wrapped-stream") {
            assert!(error.message.contains("event=error"));
            assert!(error.message.contains("code=unsupported_value"));
            assert!(error.message.contains("param=reasoning.effort"));
            assert!(error.message.contains("request_id=req-synthetic"));
            assert!(!error.message.contains("event_keys="));
            assert!(error.message.contains("[redacted]"));
        }
        let records = fixture.state.lock().unwrap().records.len();
        server
            .request(
                &connection,
                Request::new(
                    2.into(),
                    CodeActionRequest::METHOD.into(),
                    json!({"textDocument":{"uri":uri},"range":range,"context":{"diagnostics":[]}}),
                ),
            )
            .unwrap();
        let Message::Response(response) = client.receiver.try_recv().unwrap() else {
            panic!("Expected local candidates");
        };
        assert_eq!(
            response.response_result.unwrap().as_array().unwrap().len(),
            if mode == "admission" || mode == "refresh" {
                1
            } else {
                3
            }
        );
        assert!(client.receiver.try_recv().is_err());
        assert_eq!(fixture.state.lock().unwrap().records.len(), records);
    }
}

#[tokio::test(flavor = "current_thread")]
async fn chatgpt_worker_never_caches_failed_compact_streams() {
    for mode in [
        "empty_text",
        "item_role",
        "item_id_mismatch",
        "text_done_mismatch",
        "compact_truncated",
        "compact_incomplete",
        "compact_late_failed",
    ] {
        let fixture = Fixture::new().await;
        fixture.state.lock().unwrap().response_mode = mode.into();
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
            provider.clone(),
            Settings::default(),
            Ui::new(
                connection.sender.clone(),
                UiCapabilities {
                    show_document: true,
                    ..Default::default()
                },
            ),
        )
        .unwrap();
        let uri: Uri = "file:///failed-stream.rs".parse().unwrap();
        server.documents.insert(
            uri.clone(),
            Document::new(1, 1, "rust".into(), "// Translate this.\n".into()),
        );
        let hover = || {
            parse(json!({
                "textDocument":{"uri":uri}, "position":{"line":0,"character":4}
            }))
            .unwrap()
        };
        assert!(server.hover(RequestId::from(1), hover()).unwrap().is_none());
        server
            .finish(&connection, next(&completions).await)
            .unwrap();
        assert!(server.cache.latest(&uri).is_none(), "SSE mode {mode}");
        assert!(server.scheduler.pending().is_empty());
        assert!(server.scheduler.translations().is_empty());
        let Message::Response(response) = client.receiver.try_recv().unwrap() else {
            panic!("Expected failed Hover response");
        };
        assert!(response.response_result.is_err());
        if mode == "compact_late_failed" {
            assert!(
                provider
                    .admit(
                        &provider.context().unwrap(),
                        crate::providers::Intent::Automatic
                    )
                    .is_err()
            );
            assert_eq!(
                fixture
                    .state
                    .lock()
                    .unwrap()
                    .records
                    .iter()
                    .filter(|record| record.path == "/v1/responses")
                    .count(),
                1
            );
        } else {
            fixture.state.lock().unwrap().response_mode = "delta_compact".into();
            let failure = server.hover(RequestId::from(2), hover()).unwrap().unwrap();
            assert!(
                failure["contents"]["value"]
                    .as_str()
                    .unwrap()
                    .contains("translation failed")
            );
            let target = TranslationTarget {
                uri: uri.clone(),
                version: 1,
                session: 1,
                range: Range::new(Position::new(0, 0), Position::new(0, 18)),
                target_language: "ja".into(),
            };
            server
                .queue(4.into(), target, PendingReply::Command)
                .unwrap();
            server
                .finish(&connection, next(&completions).await)
                .unwrap();
            assert!(server.cache.latest(&uri).is_some());
            assert!(server.hover(RequestId::from(3), hover()).unwrap().is_some());
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
        }
        assert_eq!(server.documents[&uri].text(), Some("// Translate this.\n"));
    }
}

#[tokio::test(flavor = "current_thread")]
async fn chatgpt_worker_drops_outbound_on_cancel_deadline_edit_and_shutdown() {
    for operation in ["cancel", "deadline", "edit", "shutdown"] {
        let fixture = Fixture::new().await;
        let tail = std::sync::Arc::new(StreamTailGate::default());
        {
            let mut state = fixture.state.lock().unwrap();
            state.response_mode = "delta_compact".into();
            state.stream_tail_gate = Some(tail.clone());
        }
        // Only the deadline case lets the execution budget run out. Its budget is
        // wide enough for the steps before it to finish on a loaded machine.
        let execution_budget = Duration::from_secs(if operation == "deadline" { 2 } else { 20 });
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
            Settings {
                hover_timeout_ms: execution_budget.as_millis() as u64,
                ..Default::default()
            },
            Ui::new(
                connection.sender.clone(),
                UiCapabilities {
                    show_document: true,
                    ..Default::default()
                },
            ),
        )
        .unwrap();
        let uri: Uri = "file:///cancel-synthetic.rs".parse().unwrap();
        server.documents.insert(
            uri.clone(),
            Document::new(1, 1, "rust".into(), "// Translate this.\n".into()),
        );
        server
            .hover(
                RequestId::from(1),
                parse(json!({"textDocument":{"uri":uri},"position":{"line":0,"character":4}}))
                    .unwrap(),
            )
            .unwrap();
        // Wait for the worker to start and for its response to stay open, rather
        // than for a fixed time.
        let ticket = server.scheduler.pending()[&1.into()].ticket;
        let started = {
            let completions = completions.clone();
            tokio::task::spawn_blocking(move || completions.recv_timeout(Duration::from_secs(3)))
                .await
                .unwrap()
                .unwrap()
        };
        assert!(matches!(started, WorkerEvent::Started(started) if started == ticket));
        wait_held(&tail, 1).await;
        let target = TranslationTarget {
            uri: uri.clone(),
            version: 1,
            session: 1,
            range: Range::new(Position::new(0, 0), Position::new(0, 18)),
            target_language: "ja".into(),
        };
        server
            .queue(2.into(), target, PendingReply::Hover(Position::new(0, 5)))
            .unwrap();
        assert_eq!(server.scheduler.pending()[&2.into()].ticket, ticket);
        let execution_deadline = server.scheduler.translations()[&ticket]
            .execution_deadline()
            .unwrap();
        assert_eq!(
            *server.scheduler.translations()[&ticket].deadline.borrow(),
            Some(execution_deadline)
        );
        assert!(
            completions.is_empty(),
            "Stream EOF has not completed the worker"
        );
        assert!(server.cache.latest(&uri).is_none());
        assert_eq!(
            fixture
                .state
                .lock()
                .unwrap()
                .records
                .iter()
                .filter(|record| record.path == "/v1/responses")
                .count(),
            1
        );
        match operation {
            "deadline" => {
                // Canceling a display waiter does not change the execution deadline.
                server
                    .cancel(&connection, &2.into(), ErrorCode::RequestCanceled)
                    .unwrap();
                assert_eq!(
                    *server.scheduler.translations()[&ticket].deadline.borrow(),
                    Some(execution_deadline)
                );
                assert_eq!(
                    server.scheduler.translations()[&ticket].execution_deadline(),
                    Some(execution_deadline)
                );
                // Expiry holds at any time after the deadline, however late this runs.
                tokio::time::sleep_until(execution_deadline.into()).await;
                server.expire(&connection).unwrap();
            }
            "edit" => {
                server.notification(&connection,lsp_server::Notification::new("textDocument/didChange".into(),json!({"textDocument":{"uri":uri,"version":2},"contentChanges":[{"text":"// Changed source.\n"}]}))).unwrap();
            }
            _ => {
                server.cancel_all_work(&connection).unwrap();
            }
        }
        for _ in 0..2 {
            let Message::Response(response) = client.receiver.try_recv().unwrap() else {
                panic!("Expected failed waiter without notification");
            };
            let expected = match operation {
                "edit" => ErrorCode::ContentModified,
                "deadline" if response.id == RequestId::from(1) => ErrorCode::RequestFailed,
                _ => ErrorCode::RequestCanceled,
            };
            assert_eq!(response.response_result.unwrap_err().code, expected as i32);
        }
        assert!(client.receiver.try_recv().is_err());
        assert!(server.scheduler.pending().is_empty());
        assert!(server.scheduler.translations().is_empty());
        assert!(server.cache.latest(&uri).is_none());
        // The worker abandons the open response instead of waiting for it to end.
        wait_held(&tail, 0).await;
        let start = Instant::now();
        drop(completions);
        drop(server);
        assert!(start.elapsed() < Duration::from_millis(500));
    }
}
