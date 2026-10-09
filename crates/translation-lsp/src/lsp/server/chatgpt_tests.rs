use super::*;
use crate::providers::chatgpt::test_support::{Fixture, StreamTailGate};
use serde_json::json;

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
        let (finished, completions) = bounded(MAX_JOBS);
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
        let (finished, _completions) = bounded(MAX_JOBS);
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
        let (finished, completions) = bounded(MAX_JOBS);
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
            let path = reqwest::Url::parse(open.params["uri"].as_str().unwrap())
                .unwrap()
                .to_file_path()
                .unwrap();
            assert_eq!(std::fs::read_to_string(&path).unwrap(), translated);
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
        let (finished, events) = bounded(MAX_JOBS);
        let job = Job {
            ticket: 1,
            deadline,
            task: Task::Translation {
                intent: crate::providers::Intent::Automatic,
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
                    .result
                    .is_err_and(|error| error.to_string() == "Translation timed out")
            );
        }
        if cancel {
            assert!(
                matches!(events.try_recv().unwrap(), WorkerEvent::Finished(f) if f.result.is_err())
            );
        }
        assert!(events.try_recv().is_err());
        assert_eq!(response_calls(&fixture), 1);
    }
}

#[tokio::test(flavor = "current_thread")]
async fn configured_auto_toggle_keeps_non_ja_manual_and_comment_translation() {
    for automatic in [false, true] {
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
            "target_language":"en", "markdown_auto_translate":automatic,
            "hover_timeout_ms":1000, "selection_timeout_ms":1500, "selection_debounce_ms":100,
            "provider":{"name":"chatgpt","model":"synthetic-model"}
        })))
        .unwrap();
        let (connection, client) = Connection::memory();
        let (finished, completions) = bounded(MAX_JOBS);
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
        assert_eq!(actions[0].title, "Translate selection to en");
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
            assert!(server.scheduler.translations.is_empty());
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
                server.scheduler.translations.is_empty(),
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
                    .starts_with("Translate the text into en. Source language: auto.")
            );
        }
        assert!(client.receiver.is_empty());
    }
}

#[tokio::test(flavor = "current_thread")]
async fn activity_problem_survives_busy_store_and_clears_confirmed_external_invalidation() {
    for plan_disabled in [false, true] {
        let fixture = Fixture::new().await;
        fixture.state.lock().unwrap().failure = Some((
            400,
            serde_json::json!({"error":{"message":"Synthetic translation failure"}}),
        ));
        let auth = fixture.auth();
        fixture.connected(&auth, 3600).await;
        let provider = ChatGptProvider::synthetic(
            auth.clone(),
            format!("{}/v1", fixture.base),
            "synthetic-model".into(),
        );
        let (connection, client) = Connection::memory();
        let (finished, completions) = bounded(MAX_JOBS);
        let mut server = Server::configured(
            finished,
            provider,
            Settings::default(),
            Ui::new(
                connection.sender.clone(),
                UiCapabilities {
                    server_status: true,
                    ..Default::default()
                },
            ),
        )
        .unwrap();
        let uri: Uri = "file:///problem-account.rs".parse().unwrap();
        server.documents.insert(
            uri.clone(),
            Document::new(1, 1, "rust".into(), "// Translate this.".into()),
        );
        server
            .hover(
                1.into(),
                parse(json!({"textDocument":{"uri":uri},"position":{"line":0,"character":4}}))
                    .unwrap(),
            )
            .unwrap();
        server
            .finish(&connection, next(&completions).await)
            .unwrap();
        let Message::Response(failed) = client.receiver.try_recv().unwrap() else {
            panic!("Expected failure");
        };
        assert!(failed.response_result.is_err());
        server.publish_activity();
        let Message::Notification(problem) = client.receiver.try_recv().unwrap() else {
            panic!("Expected warning");
        };
        assert_eq!(problem.params["health"], "warning");
        let mut locked = auth.store.lock(false).await.unwrap().unwrap();
        assert_eq!(auth.ui_state(), AuthUiState::Unavailable);
        server.publish_activity();
        assert!(
            client.receiver.is_empty(),
            "Busy is not a confirmed account change"
        );
        let id = locked.state.active_registration.unwrap();
        let registration = locked.state.registrations.get_mut(&id).unwrap();
        if plan_disabled {
            registration.credentials.as_mut().unwrap().scopes.clear();
        } else {
            registration.credentials = None;
        }
        locked.save().unwrap();
        drop(locked);
        assert_eq!(
            auth.ui_state(),
            if plan_disabled {
                AuthUiState::PlanDisabled
            } else {
                AuthUiState::SignedOut
            }
        );
        server.publish_activity();
        let Message::Notification(clear) = client.receiver.try_recv().unwrap() else {
            panic!("Expected clear");
        };
        assert_eq!(clear.params, json!({"health":"ok","quiescent":true}));
        server.publish_activity();
        assert!(client.receiver.is_empty());
        assert_eq!(response_calls(&fixture), 1);
    }
}

#[tokio::test(flavor = "current_thread")]
async fn activity_keeps_background_owner_after_hover_expiry_and_allows_token_refresh() {
    let fixture = Fixture::new().await;
    fixture.state.lock().unwrap().stream_tail_delay = Duration::from_millis(300);
    let auth = fixture.auth();
    fixture.connected(&auth, 0).await;
    let token_release = std::sync::Arc::new(tokio::sync::Notify::new());
    fixture.state.lock().unwrap().token_release = Some(token_release.clone());
    let provider = ChatGptProvider::synthetic(
        auth.clone(),
        format!("{}/v1", fixture.base),
        "synthetic-model".into(),
    );
    let (connection, client) = Connection::memory();
    let (finished, completions) = bounded(MAX_JOBS);
    let mut server = Server::configured(
        finished,
        provider.clone(),
        Settings::default(),
        Ui::new(
            connection.sender.clone(),
            UiCapabilities {
                work_done_progress: true,
                server_status: true,
                ..Default::default()
            },
        ),
    )
    .unwrap();
    // Hold admission until the test owns the store lock, without delaying the worker.
    let (held_sender, mut held_jobs) = queue::channel(MAX_JOBS);
    let worker_jobs = server.jobs.replace(held_sender).unwrap();
    let uri: Uri = "file:///activity-background.md".parse().unwrap();
    let range = Range::new(Position::new(0, 0), Position::new(0, 15));
    server.documents.insert(
        uri.clone(),
        Document::new(1, 1, "markdown".into(), "First sentence.".into()),
    );
    markdown_actions(&mut server, &uri, range);
    server.selection.as_mut().unwrap().start = Some(Instant::now());
    server.expire(&connection).unwrap();
    server
        .hover(
            1.into(),
            parse(json!({"textDocument":{"uri":uri},"position":{"line":0,"character":4}})).unwrap(),
        )
        .unwrap();
    let ticket = server.scheduler.pending[&1.into()].ticket;
    let original_context = server.scheduler.translations[&ticket].context.clone();
    server.publish_activity();
    let Message::Request(create) = client.receiver.try_recv().unwrap() else {
        panic!("Expected create");
    };
    assert!(
        server
            .activity
            .response(&Response::new_ok(create.id, Value::Null))
    );
    server.publish_activity();
    let Message::Notification(begin) = client.receiver.try_recv().unwrap() else {
        panic!("Expected begin");
    };
    assert_eq!(begin.params["value"]["message"], "Queued: 1");
    let locked = auth.store.lock(false).await.unwrap().unwrap();
    worker_jobs
        .try_send(held_jobs.recv().await.unwrap())
        .unwrap();
    server.jobs = Some(worker_jobs);
    let events = completions.clone();
    let started =
        tokio::task::spawn_blocking(move || events.recv_timeout(Duration::from_secs(2)).unwrap())
            .await
            .unwrap();
    let WorkerEvent::Started(started_ticket) = started else {
        panic!("Expected worker Started");
    };
    assert_eq!(started_ticket, ticket);
    server
        .scheduler
        .translations
        .get_mut(&ticket)
        .unwrap()
        .phase = Phase::Running;
    assert!(matches!(
        auth.store.snapshot(),
        Err(crate::providers::error::credentials::CredentialError::Busy)
    ));
    assert!(provider.context().is_err());
    tokio::time::sleep(Duration::from_millis(160)).await;
    server.publish_activity();
    let Message::Notification(running) = client.receiver.try_recv().unwrap() else {
        panic!("Expected running report during store lock");
    };
    assert_eq!(running.params["value"]["kind"], "report");
    assert_eq!(
        running.params["value"]["message"],
        "Translating: 1, queued: 0"
    );
    server
        .scheduler
        .pending
        .get_mut(&1.into())
        .unwrap()
        .deadline = Instant::now();
    server.expire(&connection).unwrap();
    server.publish_activity();
    let Message::Response(expired) = client.receiver.try_recv().unwrap() else {
        panic!("Expected expired Hover");
    };
    assert_eq!(
        expired.response_result.unwrap_err().code,
        ErrorCode::RequestFailed as i32
    );
    assert!(server.scheduler.translations[&ticket].automatic);
    assert!(
        client.receiver.is_empty(),
        "Remaining background owner keeps progress alive"
    );
    drop(locked);
    tokio::time::timeout(Duration::from_secs(2), async {
        while !fixture
            .state
            .lock()
            .unwrap()
            .records
            .iter()
            .any(|record| record.path == "/token")
        {
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
    })
    .await
    .unwrap();
    assert!(matches!(
        auth.store.snapshot(),
        Err(crate::providers::error::credentials::CredentialError::Busy)
    ));
    server.publish_activity();
    assert!(
        client.receiver.is_empty(),
        "HTTP refresh holding auth lock keeps progress alive"
    );
    token_release.notify_one();
    let completion = next(&completions).await;
    assert_eq!(provider.context().unwrap(), original_context);
    assert_eq!(fixture.state.lock().unwrap().refreshes, 1);
    server.publish_activity();
    assert!(
        client.receiver.is_empty(),
        "Credential revision change must not end valid work"
    );
    server.finish(&connection, completion).unwrap();
    server.publish_activity();
    let Message::Notification(end) = client.receiver.try_recv().unwrap() else {
        panic!("Expected end");
    };
    assert_eq!(
        end.params,
        json!({"token":create.params["token"],"value":{"kind":"end"}})
    );
    assert!(server.cache.latest(&uri).is_some());
    assert!(
        client.receiver.is_empty(),
        "Background completion adds no status or INFO"
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
}

#[tokio::test(flavor = "current_thread")]
async fn activity_cancellation_leaves_account_picker_and_its_deadline_alive() {
    let fixture = Fixture::new().await;
    fixture.state.lock().unwrap().delay = Duration::from_secs(1);
    let auth = fixture.auth();
    fixture.connected(&auth, 3600).await;
    let provider = ChatGptProvider::synthetic(
        auth,
        format!("{}/v1", fixture.base),
        "synthetic-model".into(),
    );
    let (connection, client) = Connection::memory();
    let (finished, completions) = bounded(MAX_JOBS);
    let mut server = Server::configured(
        finished,
        provider,
        Settings::default(),
        Ui::new(
            connection.sender.clone(),
            UiCapabilities {
                show_document: true,
                work_done_progress: true,
                server_status: true,
            },
        ),
    )
    .unwrap();
    let uri: Uri = "file:///activity-picker.md".parse().unwrap();
    let range = Range::new(Position::new(0, 0), Position::new(0, 15));
    server.documents.insert(
        uri.clone(),
        Document::new(1, 1, "markdown".into(), "First sentence.".into()),
    );
    markdown_actions(&mut server, &uri, range);
    server.selection.as_mut().unwrap().start = Some(Instant::now());
    server.expire(&connection).unwrap();
    server.publish_activity();
    let Message::Request(create) = client.receiver.try_recv().unwrap() else {
        panic!("Expected create");
    };
    server
        .activity
        .response(&Response::new_ok(create.id, Value::Null));
    server.publish_activity();
    let _begin = client.receiver.try_recv().unwrap();
    server
        .request(
            &connection,
            Request::new(
                2.into(),
                ExecuteCommand::METHOD.into(),
                json!({"command":SETTINGS_COMMAND}),
            ),
        )
        .unwrap();
    let Message::Request(picker) = protocol_message(&client).await else {
        panic!("Expected picker");
    };
    assert_eq!(picker.method, "window/showMessageRequest");
    let deadline = server.administrations[&2.into()].deadline;
    server
        .notification(
            &connection,
            lsp_server::Notification::new(
                "window/workDoneProgress/cancel".into(),
                json!({"token":create.params["token"]}),
            ),
        )
        .unwrap();
    server.publish_activity();
    assert!(server.scheduler.translations.is_empty());
    assert!(server.selection.as_ref().is_none_or(|s| s.start.is_none()));
    assert_eq!(
        *server.administrations[&2.into()].stop.borrow(),
        Some(deadline)
    );
    let Message::Notification(end) = client.receiver.try_recv().unwrap() else {
        panic!("Expected end");
    };
    assert_eq!(end.params["value"]["kind"], "end");
    assert!(client.receiver.is_empty(), "Picker must remain pending");
    server.ui.response(Response::new_ok(picker.id, Value::Null));
    while !server.administrations.is_empty() {
        server
            .finish(&connection, next(&completions).await)
            .unwrap();
    }
    let Message::Response(dismissed) = client.receiver.try_recv().unwrap() else {
        panic!("Expected picker dismissal");
    };
    assert_eq!(dismissed.id, 2.into());
    assert!(dismissed.response_result.unwrap().is_null());
    assert!(client.receiver.is_empty());
    assert!(fixture.state.lock().unwrap().records.is_empty());
}

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
    let (finished, completions) = bounded(MAX_JOBS);
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
        server.scheduler.pending[&1.into()].ticket,
        server.scheduler.pending[&5.into()].ticket
    );
    assert_eq!(server.scheduler.translations.len(), 5);
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
        assert!(completion.result.is_ok());
        server.finish(&connection, completion).unwrap();
        if server.scheduler.translations.len() >= 2 {
            wait_held(&tail, 2).await;
        }
    }
    assert!(server.scheduler.translations.is_empty() && server.scheduler.pending.is_empty());
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
        let (finished, completions) = bounded(MAX_JOBS);
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
        let ticket = server.scheduler.pending[&id].ticket;
        let expected = match operation {
            "timeout-active" => {
                server.scheduler.translations[&ticket]
                    .deadline
                    .send_replace(Some(Instant::now()));
                let completion = next(&completions).await;
                assert_eq!(completion.ticket, ticket);
                assert!(
                    completion
                        .result
                        .as_ref()
                        .is_err_and(|error| error.to_string().contains("timed out"))
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
                    matches!(&canceled.result, Err(error) if error.kind() == ErrorKind::Cancelled)
                );
                server.finish(&connection, canceled).unwrap();
            }
            // The queued HTTP starts only after the canceled worker releases its slot.
            wait_held(&following, 1).await;
            assert_eq!(response_calls(&fixture), 3);
        }
        tail.release.notify_waiters();
        following.release.notify_waiters();
        while !server.scheduler.translations.is_empty() {
            let completion = next(&completions).await;
            assert!(
                completion.result.is_ok(),
                "{operation}: {}",
                completion
                    .result
                    .as_ref()
                    .err()
                    .map(|e| e.to_string())
                    .unwrap_or_default()
            );
            server.finish(&connection, completion).unwrap();
        }
        assert!(server.scheduler.pending.is_empty());
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
            assert!(completion.result.is_ok());
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
        let (finished, completions) = bounded(MAX_JOBS);
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
            .translations
            .values()
            .map(|work| work.target.clone())
            .collect();
        for target in targets {
            server
                .queue_translation(target, TranslationRequest::Document)
                .unwrap();
        }
        if operation == "account-change" {
            fixture.connected(&auth, 3600).await;
            for _ in 0..4 {
                let completion = next(&completions).await;
                assert!(completion.result.is_err());
                server.finish(&connection, completion).unwrap();
            }
        } else {
            server.cancel_all(&connection).unwrap();
        }
        assert!(server.scheduler.pending.is_empty() && server.scheduler.translations.is_empty());
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
                completion.result.is_ok(),
                "{operation}: {}",
                completion
                    .result
                    .as_ref()
                    .err()
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
    let (finished, completions) = bounded(MAX_JOBS);
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
            "First source.",
            "ja",
            context.clone()
        ),
        provider.translate(
            crate::providers::Intent::Automatic,
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
        let (finished, completions) = bounded(MAX_JOBS);
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
        assert!(completion.result.is_ok());
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
        while !server.scheduler.translations.is_empty() {
            let completion = next(&completions).await;
            assert!(completion.result.is_ok());
            server.finish(&connection, completion).unwrap();
        }
        assert_eq!(response_calls(&fixture), (round + 1) * 5);
        assert!(server.scheduler.pending.is_empty());
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
    let (finished, completions) = bounded(MAX_JOBS);
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
        assert!(completion.result.as_ref().is_err_and(|e| {
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
        assert!(completion.result.is_ok());
        server.finish(&connection, completion).unwrap();
    }
    assert!(server.scheduler.pending.is_empty() && server.scheduler.translations.is_empty());
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
        let (finished, completions) = bounded(MAX_JOBS);
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

#[tokio::test(flavor = "current_thread")]
async fn document_promotion_uses_original_start_and_survives_hover_expiry() {
    let settings = Settings {
        hover_timeout_ms: 80,
        selection_timeout_ms: 150,
        document_timeout_ms: 1000,
        ..Default::default()
    };
    let (fixture, mut server, connection, client, completions, uri) = range_server(settings).await;
    let tail = std::sync::Arc::new(StreamTailGate::default());
    fixture.state.lock().unwrap().stream_tail_gate = Some(tail.clone());
    let target = range_target(&server, &uri, 0);
    server
        .queue(
            1.into(),
            target.clone(),
            PendingReply::Hover(Position::new(0, 3)),
        )
        .unwrap();
    let ticket = server.scheduler.pending[&1.into()].ticket;
    let start = server.scheduler.translations[&ticket].started.unwrap();
    wait_held(&tail, 1).await;
    server
        .queue_translation(target.clone(), TranslationRequest::Document)
        .unwrap();
    assert_eq!(
        server.scheduler.translations[&ticket].execution_deadline(),
        Some(start + Duration::from_millis(1000))
    );
    tokio::time::sleep(Duration::from_millis(100)).await;
    server.expire(&connection).unwrap();
    assert!(server.scheduler.pending.is_empty());
    assert!(server.scheduler.translations[&ticket].document);
    server
        .queue_translation(target, TranslationRequest::Document)
        .unwrap();
    assert_eq!(
        server.scheduler.translations[&ticket].execution_deadline(),
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
            .hover(&uri, Position::new(0, 3), false, |_| true)
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
    let tail = std::sync::Arc::new(StreamTailGate::default());
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
        .translations
        .values()
        .next()
        .unwrap()
        .execution_deadline();
    tokio::time::sleep(Duration::from_millis(60)).await;
    server
        .queue(2.into(), target, PendingReply::Hover(Position::new(0, 4)))
        .unwrap();
    assert_eq!(
        server
            .scheduler
            .translations
            .values()
            .next()
            .unwrap()
            .execution_deadline(),
        cap
    );
    tokio::time::sleep(Duration::from_millis(120)).await;
    server.expire(&connection).unwrap();
    server
        .finish(&connection, next(&completions).await)
        .unwrap();
    assert!(server.scheduler.translations.is_empty());
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
        let (finished, completions) = bounded(MAX_JOBS);
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
async fn administration_reports_admission_worker_and_deadline_failures_but_not_cancel() {
    let fixture = Fixture::new().await;
    let provider = ChatGptProvider::synthetic(
        fixture.auth(),
        format!("{}/v1", fixture.base),
        "synthetic-model".into(),
    );
    let (connection, client) = Connection::memory();
    let (finished, completions) = bounded(MAX_JOBS);
    let mut server = Server::configured(
        finished,
        provider,
        Settings::default(),
        Ui::new(connection.sender.clone(), UiCapabilities::default()),
    )
    .unwrap();
    server
        .request(
            &connection,
            Request::new(
                1.into(),
                ExecuteCommand::METHOD.into(),
                json!({"command":"translation-lsp.connect","arguments":["unexpected"]}),
            ),
        )
        .unwrap();
    assert_eq!(
        command_notice(&client, 1, 1).unwrap_err().code,
        ErrorCode::InvalidParams as i32
    );
    let usage = json!({"command":"translation-lsp.usage"});
    server
        .request(
            &connection,
            Request::new(2.into(), ExecuteCommand::METHOD.into(), usage.clone()),
        )
        .unwrap();
    server
        .finish(&connection, next(&completions).await)
        .unwrap();
    assert!(
        command_notice(&client, 2, 1)
            .unwrap_err()
            .message
            .contains("window/showDocument")
    );
    server
        .request(
            &connection,
            Request::new(3.into(), ExecuteCommand::METHOD.into(), usage.clone()),
        )
        .unwrap();
    let work = server.administrations.get_mut(&3.into()).unwrap();
    let ticket = work.ticket;
    work.deadline = Instant::now();
    server.expire(&connection).unwrap();
    let error = command_notice(&client, 3, 1).unwrap_err();
    assert_eq!(error.code, ErrorCode::RequestFailed as i32);
    assert!(error.message.contains("timed out"));
    server
        .finish(
            &connection,
            Finished {
                ticket,
                result: Err(ProviderError::failed("late completion")),
            },
        )
        .unwrap();
    assert!(client.receiver.try_recv().is_err());
    server
        .finish(&connection, next(&completions).await)
        .unwrap();
    server
        .request(
            &connection,
            Request::new(4.into(), ExecuteCommand::METHOD.into(), usage),
        )
        .unwrap();
    server
        .cancel(&connection, &4.into(), ErrorCode::RequestCanceled)
        .unwrap();
    let Message::Response(response) = client.receiver.try_recv().unwrap() else {
        panic!("Expected cancellation response");
    };
    assert_eq!(
        response.response_result.unwrap_err().code,
        ErrorCode::RequestCanceled as i32
    );
    assert!(client.receiver.try_recv().is_err());
    assert!(fixture.state.lock().unwrap().records.is_empty());
}

#[tokio::test(flavor = "current_thread")]
async fn chatgpt_worker_coalesces_and_invalidates_completed_cache_on_account_changes() {
    let fixture = Fixture::new().await;
    fixture.state.lock().unwrap().response_mode = "delta_compact".into();
    let auth = fixture.auth();
    fixture.connected(&auth, 0).await;
    let provider = ChatGptProvider::synthetic(
        auth.clone(),
        format!("{}/v1", fixture.base),
        "synthetic-model".into(),
    );
    let (connection, client) = Connection::memory();
    let (finished, completions) = bounded(MAX_JOBS);
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
    let uri: Uri = "file:///synthetic.rs".parse().unwrap();
    server.documents.insert(
        uri.clone(),
        Document::new(
            1,
            1,
            "rust".into(),
            "// Translate this.\nlet value = 1;\n".into(),
        ),
    );
    let hover =
        || parse(json!({"textDocument":{"uri":uri},"position":{"line":0,"character":4}})).unwrap();
    let worker_jobs = server.jobs.take().unwrap();
    let (held_sender, mut held) = queue::channel(2);
    server.jobs = Some(held_sender);
    assert!(server.hover(RequestId::from(1), hover()).unwrap().is_none());
    assert!(server.hover(RequestId::from(2), hover()).unwrap().is_none());
    assert_eq!(server.scheduler.translations.len(), 1);
    worker_jobs.try_send(held.recv().await.unwrap()).unwrap();
    server.jobs = Some(worker_jobs);
    server
        .cancel(&connection, &RequestId::from(1), ErrorCode::RequestCanceled)
        .unwrap();
    server
        .finish(&connection, next(&completions).await)
        .unwrap();
    assert!(server.cache.latest(&uri).is_some());
    assert!(server.hover(RequestId::from(3), hover()).unwrap().is_some());
    let records = fixture.state.lock().unwrap().records.clone();
    assert_eq!(
        records
            .iter()
            .filter(|record| record.path == "/v1/responses")
            .count(),
        1
    );
    assert!(
        records
            .iter()
            .filter(|record| record.path.starts_with("/v1/"))
            .all(|record| record.authorization == "Bearer synthetic-access-1")
    );
    let _ = client.receiver.try_recv().unwrap();
    let _ = client.receiver.try_recv().unwrap();
    // A second registration changes the key and starts a fresh inference.
    fixture.connected(&auth, 3600).await;
    assert!(server.hover(RequestId::from(4), hover()).unwrap().is_none());
    assert!(server.cache.latest(&uri).is_none());
    server
        .finish(&connection, next(&completions).await)
        .unwrap();
    assert_eq!(
        fixture
            .state
            .lock()
            .unwrap()
            .records
            .iter()
            .filter(|record| record.path == "/v1/models")
            .count(),
        0
    );
    assert_eq!(
        server.documents[&uri].text(),
        Some("// Translate this.\nlet value = 1;\n")
    );
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
        let (finished, completions) = bounded(MAX_JOBS);
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
        assert!(server.scheduler.pending.is_empty());
        assert!(server.scheduler.translations.is_empty());
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
        {
            let mut state = fixture.state.lock().unwrap();
            state.response_mode = "delta_compact".into();
            state.stream_tail_delay = Duration::from_secs(5);
        }
        let auth = fixture.auth();
        fixture.connected(&auth, 3600).await;
        let provider = ChatGptProvider::synthetic(
            auth,
            format!("{}/v1", fixture.base),
            "synthetic-model".into(),
        );
        let (connection, client) = Connection::memory();
        let (finished, completions) = bounded(MAX_JOBS);
        let mut server = Server::configured(
            finished,
            provider,
            Settings {
                hover_timeout_ms: 250,
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
        tokio::time::sleep(Duration::from_millis(120)).await;
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
        let ticket = server.scheduler.pending[&1.into()].ticket;
        assert_eq!(server.scheduler.pending[&2.into()].ticket, ticket);
        assert_eq!(
            *server.scheduler.translations[&ticket].deadline.borrow(),
            server.scheduler.translations[&ticket].execution_deadline()
        );
        assert!(
            matches!(completions.try_recv().unwrap(), WorkerEvent::Started(started) if started == ticket)
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
                    *server.scheduler.translations[&ticket].deadline.borrow(),
                    server.scheduler.translations[&ticket].execution_deadline()
                );
                tokio::time::sleep(Duration::from_millis(150)).await;
                server.expire(&connection).unwrap();
            }
            "edit" => {
                server.notification(&connection,lsp_server::Notification::new("textDocument/didChange".into(),json!({"textDocument":{"uri":uri,"version":2},"contentChanges":[{"text":"// Changed source.\n"}]}))).unwrap();
            }
            _ => {
                server.cancel_all(&connection).unwrap();
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
        assert!(server.scheduler.pending.is_empty());
        assert!(server.scheduler.translations.is_empty());
        assert!(server.cache.latest(&uri).is_none());
        let start = Instant::now();
        drop(completions);
        drop(server);
        assert!(start.elapsed() < Duration::from_millis(500));
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

#[tokio::test(flavor = "current_thread")]
async fn markdown_repeated_selections_report_failure_and_recover_manually() {
    let fixture = Fixture::new().await;
    let auth = fixture.auth();
    fixture.connected(&auth, 3600).await;
    let provider = ChatGptProvider::synthetic(
        auth,
        format!("{}/v1", fixture.base),
        "synthetic-model".into(),
    );
    let (connection, client) = Connection::memory();
    let (finished, completions) = bounded(MAX_JOBS);
    let mut server = Server::configured(
        finished,
        provider,
        Settings::default(),
        Ui::new(
            connection.sender.clone(),
            UiCapabilities {
                server_status: true,
                ..Default::default()
            },
        ),
    )
    .unwrap();
    let uri: Uri = "file:///repeated.md".parse().unwrap();
    let source = "First sentence.\nSecond sentence.";
    server.documents.insert(
        uri.clone(),
        Document::new(1, 1, "Markdown".into(), source.into()),
    );
    let first = Range::new(Position::new(0, 0), Position::new(0, 15));
    let second = Range::new(Position::new(1, 0), Position::new(1, 16));
    for (range, mode) in [(first, ""), (second, "empty_text")] {
        fixture.state.lock().unwrap().response_mode = mode.into();
        markdown_actions(&mut server, &uri, range);
        server.selection.as_mut().unwrap().start = Some(Instant::now());
        server.expire(&connection).unwrap();
        server
            .finish(&connection, next(&completions).await)
            .unwrap();
        if mode.is_empty() {
            assert_eq!(server.cache.latest(&uri).as_ref().unwrap().range, first);
        }
    }
    assert!(server.cache.latest(&uri).is_some());
    server.publish_activity();
    let Message::Notification(status) = client.receiver.try_recv().unwrap() else {
        panic!("Expected background failure status");
    };
    assert_eq!(status.method, "experimental/serverStatus");
    let message = status.params["message"].as_str().unwrap();
    assert!(message.contains("OpenAI completed without translation text"));
    let target = server.selection.as_ref().unwrap().target.clone();
    let reply = server
        .queue(1.into(), target.clone(), PendingReply::Command)
        .unwrap()
        .unwrap();
    respond(&connection, 1.into(), Ok(reply)).unwrap();
    server
        .finish(&connection, next(&completions).await)
        .unwrap();
    assert!(
        command_notice(&client, 1, 1)
            .unwrap_err()
            .message
            .contains(message)
    );
    fixture.state.lock().unwrap().response_mode.clear();
    let reply = server
        .queue(2.into(), target, PendingReply::Command)
        .unwrap()
        .unwrap();
    respond(&connection, 2.into(), Ok(reply)).unwrap();
    server
        .finish(&connection, next(&completions).await)
        .unwrap();
    command_notice(&client, 2, 3).unwrap();
    assert_eq!(server.cache.latest(&uri).as_ref().unwrap().range, second);
    for range in [first, second] {
        markdown_actions(&mut server, &uri, range);
        server.selection.as_mut().unwrap().start = Some(Instant::now());
        server.expire(&connection).unwrap();
        assert!(server.cache.exact(&uri, range, false).is_some());
        assert!(server.scheduler.translations.is_empty());
    }
    server.publish_activity();
    let Message::Notification(status) = client.receiver.try_recv().unwrap() else {
        panic!("Expected cleared failure status");
    };
    assert_eq!(status.params["health"], "ok");
    assert_eq!(server.documents[&uri].text().unwrap(), source);
    assert!(server.scheduler.pending.is_empty());
    assert!(server.scheduler.translations.is_empty());
    assert_eq!(response_calls(&fixture), 4);
}

#[tokio::test(flavor = "current_thread")]
async fn retained_manual_and_auto_work_survive_waiter_and_selection_cancellation() {
    let (fixture, mut server, connection, client, completions, uri) =
        range_server(Settings::default()).await;
    let tail = std::sync::Arc::new(StreamTailGate::default());
    fixture.state.lock().unwrap().stream_tail_gate = Some(tail.clone());
    let target = range_target(&server, &uri, 0);
    markdown_actions(&mut server, &uri, target.range);
    server.selection.as_mut().unwrap().start = Some(Instant::now());
    server.expire(&connection).unwrap();
    wait_held(&tail, 1).await;
    server
        .queue(
            1.into(),
            target.clone(),
            PendingReply::Hover(Position::new(0, 3)),
        )
        .unwrap();
    assert_eq!(
        server
            .queue(2.into(), target.clone(), PendingReply::Command)
            .unwrap(),
        Some(Value::Null)
    );
    assert!(!server.scheduler.pending.contains_key(&2.into()));
    server
        .cancel(&connection, &1.into(), ErrorCode::RequestCanceled)
        .unwrap();
    server
        .cancel(&connection, &2.into(), ErrorCode::RequestCanceled)
        .unwrap();
    markdown_actions(&mut server, &uri, Range::default());
    assert_eq!(server.scheduler.translations.len(), 1);
    tail.release.notify_one();
    server
        .finish(&connection, next(&completions).await)
        .unwrap();
    assert!(server.cache.exact(&uri, target.range, false).is_some());
    assert_eq!(response_calls(&fixture), 1);
    assert!(client.receiver.try_iter().any(|m| matches!(m, Message::Notification(n) if n.params["message"].as_str().is_some_and(|m| m.contains("ranges.md:1-1")))));
}

#[tokio::test(flavor = "current_thread")]
async fn distinct_automatic_ranges_persist_and_range_cancel_only_removes_its_ticket() {
    let (fixture, mut server, connection, _client, completions, uri) =
        range_server(Settings::default()).await;
    let tail = std::sync::Arc::new(StreamTailGate::default());
    fixture.state.lock().unwrap().stream_tail_gate = Some(tail.clone());
    for line in [0, 2, 4] {
        let target = range_target(&server, &uri, line);
        markdown_actions(&mut server, &uri, target.range);
        server.selection.as_mut().unwrap().start = Some(Instant::now());
        server.expire(&connection).unwrap();
    }
    assert_eq!(server.scheduler.translations.len(), 3);
    wait_held(&tail, 2).await;
    markdown_actions(&mut server, &uri, Range::default());
    let canceled = server
        .scheduler
        .ticket(&range_target(&server, &uri, 2))
        .unwrap();
    server
        .cancel_range(&connection, canceled, ErrorCode::RequestCanceled)
        .unwrap();
    server
        .finish(&connection, next(&completions).await)
        .unwrap();
    wait_held(&tail, 2).await;
    for _ in 0..2 {
        tail.release.notify_one();
        server
            .finish(&connection, next(&completions).await)
            .unwrap();
    }
    for line in [0, 4] {
        assert!(
            server
                .cache
                .exact(&uri, range_target(&server, &uri, line).range, false)
                .is_some()
        );
    }
    assert!(
        server
            .cache
            .exact(&uri, range_target(&server, &uri, 2).range, false)
            .is_none()
    );
    assert!(server.scheduler.translations.is_empty());
    assert_eq!(response_calls(&fixture), 3);
}

#[tokio::test(flavor = "current_thread")]
async fn automatic_selection_excludes_code_whitespace_non_markdown_and_ineligible_auth() {
    for (mode, language, text) in [
        ("code", "markdown", "`code`"),
        ("space", "markdown", " "),
        ("large", "markdown", "sentence"),
        ("rust", "rust", "sentence"),
        ("signed-out", "markdown", "sentence"),
        ("disabled", "markdown", "sentence"),
    ] {
        let fixture = Fixture::new().await;
        let auth = fixture.auth();
        if mode != "signed-out" {
            let id = fixture.connected(&auth, 3600).await;
            if mode == "disabled" {
                let mut locked = auth.store.lock(false).await.unwrap().unwrap();
                locked
                    .state
                    .registrations
                    .get_mut(&id)
                    .unwrap()
                    .credentials
                    .as_mut()
                    .unwrap()
                    .scopes
                    .clear();
                locked.save().unwrap();
            }
        }
        let provider = ChatGptProvider::synthetic(
            auth.clone(),
            format!("{}/v1", fixture.base),
            "synthetic-model".into(),
        );
        let (connection, client) = Connection::memory();
        let (finished, completions) = bounded(MAX_JOBS);
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
        let uri: Uri = "file:///excluded.md".parse().unwrap();
        let text = if mode == "large" {
            "a".repeat(16 * 1024 + 1)
        } else {
            text.into()
        };
        let range = Range::new(Position::new(0, 0), Position::new(0, text.len() as u32));
        server
            .documents
            .insert(uri.clone(), Document::new(1, 1, language.into(), text));
        markdown_actions(&mut server, &uri, range);
        server.expire(&connection).unwrap();
        assert!(server.selection.is_none());
        assert!(server.scheduler.translations.is_empty());
        assert!(fixture.state.lock().unwrap().records.is_empty());
        assert!(client.receiver.try_recv().is_err());
        if mode == "signed-out" {
            assert!(auth.store.snapshot().unwrap().is_none());
        }
        drop(completions);
        drop(server);
    }
}

#[tokio::test(flavor = "current_thread")]
async fn automatic_selection_uses_document_code_context_and_sends_only_selected_bytes() {
    let fixture = Fixture::new().await;
    let auth = fixture.auth();
    fixture.connected(&auth, 3600).await;
    let provider = ChatGptProvider::synthetic(
        auth,
        format!("{}/v1", fixture.base),
        "synthetic-model".into(),
    );
    let (connection, client) = Connection::memory();
    let (finished, completions) = bounded(MAX_JOBS);
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
    let uri: Uri = "file:///context.md".parse().unwrap();
    for (index, (text, selected, prose)) in [
        (
            "```rust\nlet retry_count = 3;\n```\n",
            "let retry_count = 3;",
            false,
        ),
        ("```rust\nlet retry_count = 3;\n```\n", "retry_count", false),
        (
            "```rust\nlet retry_count = 3;\n```\n",
            "```rust\nlet retry_count = 3;\n```",
            false,
        ),
        ("Keep `retry_count` unchanged.", "`retry_count`", false),
        ("Keep `retry_count` unchanged.", "retry_count", false),
        (
            "Read [the guide](https://example.test/path).",
            "https://example.test/path",
            false,
        ),
        (
            "Read [the guide](https://example.test/path).",
            "example.test",
            false,
        ),
        (
            "Read <https://example.test/path>.",
            "https://example.test/path",
            false,
        ),
        (
            "    let retry_count = 3;\n",
            "    let retry_count = 3;",
            false,
        ),
        ("    let retry_count = 3;\n", "retry_count", false),
        ("<div>\nhidden text\n</div>\n", "hidden text", false),
        (
            "Do not send the entire document. First sentence. Keep this too.",
            "First sentence.",
            true,
        ),
        (
            "Read [the guide](https://example.test/path).",
            "the guide",
            true,
        ),
        (
            "Outside. Keep `retry_count` unchanged. Outside.",
            "Keep `retry_count` unchanged.",
            true,
        ),
        (
            "Outside. Read [the guide](https://example.test/path). Outside.",
            "Read [the guide](https://example.test/path).",
            true,
        ),
        (
            "Outside.\n\n```rust\nlet retry_count = 3;\n```\n\nFirst sentence.\nOutside.",
            "let retry_count = 3;\n```\n\nFirst sentence.",
            false,
        ),
        (
            "Outside.\n\n```rust\nlet retry_count = 3;\n```\n\nFirst sentence.\nOutside.",
            "```rust\nlet retry_count = 3;\n```\n\nFirst sentence.",
            true,
        ),
        (
            "Keep `retry_count` unchanged.",
            "retry_count` unchanged.",
            false,
        ),
        (
            "Read [the guide](https://example.test/path). First sentence.",
            "example.test/path). First sentence.",
            false,
        ),
        (
            "First sentence. Keep `retry_count` unchanged.",
            "First sentence. Keep `retry",
            false,
        ),
    ]
    .into_iter()
    .enumerate()
    {
        let start = text.find(selected).unwrap();
        let position = |offset: usize| {
            let prefix = &text[..offset];
            Position::new(
                prefix.bytes().filter(|&byte| byte == b'\n').count() as u32,
                prefix.rsplit('\n').next().unwrap().encode_utf16().count() as u32,
            )
        };
        let range = Range::new(position(start), position(start + selected.len()));
        server.documents.insert(
            uri.clone(),
            Document::new(1, index as u64, "markdown".into(), text.into()),
        );
        let before = fixture.state.lock().unwrap().records.len();
        markdown_actions(&mut server, &uri, range);
        if prose {
            assert!(server.selection.is_some(), "{selected:?}");
            server.selection.as_mut().unwrap().start = Some(Instant::now());
            server.expire(&connection).unwrap();
            server
                .finish(&connection, next(&completions).await)
                .unwrap();
            let state = fixture.state.lock().unwrap();
            assert_eq!(state.records.len(), before + 1, "{selected:?}");
            let body: Value = serde_json::from_str(&state.records.last().unwrap().body).unwrap();
            let sent = body["input"][0]["content"].as_str().unwrap();
            assert_eq!(sent, selected);
            for literal in ["retry_count", "https://example.test/path"] {
                if selected.contains(literal) {
                    assert!(
                        server
                            .cache
                            .latest(&uri)
                            .as_ref()
                            .unwrap()
                            .translation
                            .contains(literal)
                    );
                }
            }
        } else {
            assert!(server.selection.is_none(), "{selected:?}");
            server.expire(&connection).unwrap();
            assert!(server.scheduler.translations.is_empty());
            assert_eq!(fixture.state.lock().unwrap().records.len(), before);
        }
        assert_eq!(server.documents[&uri].text(), Some(text));
        assert!(client.receiver.try_recv().is_err());
    }
    drop(completions);
    drop(server);
}

async fn protocol_message(client: &Connection) -> Message {
    let receiver = client.receiver.clone();
    tokio::task::spawn_blocking(move || receiver.recv_timeout(Duration::from_secs(3)).unwrap())
        .await
        .unwrap()
}

#[tokio::test(flavor = "current_thread")]
async fn live_loop_starts_after_stability_and_settings_dismissal_preserves_background() {
    let fixture = Fixture::new().await;
    fixture.state.lock().unwrap().stream_tail_delay = Duration::from_millis(800);
    let auth = fixture.auth();
    fixture.connected(&auth, 3600).await;
    let before = auth.store.snapshot().unwrap().unwrap().revision;
    let provider = ChatGptProvider::synthetic(
        auth.clone(),
        format!("{}/v1", fixture.base),
        "synthetic-model".into(),
    );
    let (connection, client) = Connection::memory();
    let server = thread::spawn(move || {
        let ui = Ui::new(
            connection.sender.clone(),
            UiCapabilities {
                show_document: true,
                ..Default::default()
            },
        );
        serve_initialized(
            &connection,
            Settings::from_options(Some(json!({"selection_debounce_ms":700}))).unwrap(),
            ui,
            provider,
        )
        .unwrap();
    });
    let uri: Uri = "file:///loop.md".parse().unwrap();
    client
        .sender
        .send(
            lsp_server::Notification::new(
                "textDocument/didOpen".into(),
                json!({
                    "textDocument": {
                        "uri": uri, "languageId": "markdown", "version": 1,
                        "text": "First sentence."
                    }
                }),
            )
            .into(),
        )
        .unwrap();
    let params = json!({
        "textDocument": {"uri": uri},
        "range": {"start": {"line": 0, "character": 0}, "end": {"line": 0, "character": 15}},
        "context": {"diagnostics": []}
    });
    let started = Instant::now();
    client
        .sender
        .send(Request::new(1.into(), CodeActionRequest::METHOD.into(), params.clone()).into())
        .unwrap();
    let Message::Response(response) = protocol_message(&client).await else {
        panic!("Expected candidates");
    };
    assert_eq!(
        response.response_result.unwrap().as_array().unwrap().len(),
        3
    );
    assert!(started.elapsed() < Duration::from_millis(200));
    tokio::time::sleep(Duration::from_millis(250)).await;
    assert!(fixture.state.lock().unwrap().records.is_empty());
    tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            if fixture
                .state
                .lock()
                .unwrap()
                .records
                .iter()
                .any(|record| record.path == "/v1/responses")
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    assert!(started.elapsed() >= Duration::from_millis(690));
    let picker = Instant::now();
    client
        .sender
        .send(
            Request::new(
                2.into(),
                ExecuteCommand::METHOD.into(),
                json!({"command":SETTINGS_COMMAND}),
            )
            .into(),
        )
        .unwrap();
    let Message::Request(request) = protocol_message(&client).await else {
        panic!("Expected picker");
    };
    assert_eq!(request.method, "window/showMessageRequest");
    assert!(
        picker.elapsed() < Duration::from_millis(400),
        "Picker must not wait for translation EOF"
    );
    assert_eq!(
        request.params["actions"],
        json!([
            {"title": "Change ChatGPT account"},
            {"title": "Sign out of ChatGPT"},
            {"title": "ChatGPT Usage"}
        ])
    );
    assert_eq!(request.params["message"], "");
    client
        .sender
        .send(
            Response {
                id: request.id,
                response_result: Ok(Value::Null),
            }
            .into(),
        )
        .unwrap();
    let Message::Response(response) = protocol_message(&client).await else {
        panic!("Expected dismissal");
    };
    assert_eq!(response.id, 2.into());
    assert!(response.response_result.unwrap().is_null());
    client
        .sender
        .send(
            Request::new(
                3.into(),
                HoverRequest::METHOD.into(),
                json!({"textDocument":{"uri":uri},"position":{"line":0,"character":4}}),
            )
            .into(),
        )
        .unwrap();
    let Message::Response(response) = protocol_message(&client).await else {
        panic!("Expected completed Hover");
    };
    assert_eq!(response.id, 3.into());
    assert!(
        response.response_result.unwrap()["contents"]["value"]
            .as_str()
            .unwrap()
            .contains("訳First sentence.")
    );
    assert_eq!(auth.store.snapshot().unwrap().unwrap().revision, before);
    client
        .sender
        .send(Request::new(4.into(), CodeActionRequest::METHOD.into(), params).into())
        .unwrap();
    let Message::Response(response) = protocol_message(&client).await else {
        panic!("Expected cached candidates");
    };
    assert_eq!(response.id, 4.into());
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
    assert!(client.receiver.try_recv().is_err());
    client
        .sender
        .send(Request::new(99.into(), Shutdown::METHOD.into(), Value::Null).into())
        .unwrap();
    client
        .sender
        .send(lsp_server::Notification::new(Exit::METHOD.into(), Value::Null).into())
        .unwrap();
    let Message::Response(response) = protocol_message(&client).await else {
        panic!("Expected shutdown");
    };
    assert_eq!(response.id, 99.into());
    tokio::task::spawn_blocking(move || server.join().unwrap())
        .await
        .unwrap();
}

#[tokio::test(flavor = "current_thread")]
async fn account_picker_filters_states_and_closing_it_has_no_effects() {
    for mode in ["signed-out", "disabled", "ready", "unavailable"] {
        let fixture = Fixture::new().await;
        let auth = fixture.auth();
        if mode != "signed-out" {
            let id = fixture.connected(&auth, 3600).await;
            if mode == "disabled" {
                let mut locked = auth.store.lock(false).await.unwrap().unwrap();
                locked
                    .state
                    .registrations
                    .get_mut(&id)
                    .unwrap()
                    .credentials
                    .as_mut()
                    .unwrap()
                    .scopes
                    .clear();
                locked.save().unwrap();
            }
        }
        let before = auth
            .store
            .snapshot()
            .unwrap()
            .map(|snapshot| snapshot.revision);
        let locked = if mode == "unavailable" {
            auth.store.lock(false).await.unwrap()
        } else {
            None
        };
        let provider = ChatGptProvider::synthetic(
            auth.clone(),
            format!("{}/v1", fixture.base),
            "synthetic-model".into(),
        );
        let (connection, client) = Connection::memory();
        let (finished, completions) = bounded(MAX_JOBS);
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
        let uri: Uri = "file:///picker.rs".parse().unwrap();
        server.documents.insert(
            uri.clone(),
            Document::new(1, 1, "rust".into(), "// sentence".into()),
        );
        server.cache.insert(
            uri.clone(),
            0,
            Preview::new(
                Context::Mock,
                Range::new(Position::new(0, 0), Position::new(0, 11)),
                "cached".into(),
                true,
            ),
            false,
            false,
        );
        let state = server.provider.auth_ui_state();
        server
            .execute_command(
                &connection,
                7.into(),
                parse(json!({"command":SETTINGS_COMMAND})).unwrap(),
            )
            .unwrap();
        let Message::Request(request) = protocol_message(&client).await else {
            panic!("Expected management picker");
        };
        assert_eq!(request.method, "window/showMessageRequest");
        let expected: Vec<_> = crate::providers::chatgpt::management_commands(state)
            .iter()
            .map(|command| json!({"title":command.title}))
            .collect();
        assert_eq!(request.params["actions"], json!(expected));
        server.ui.response(Response {
            id: request.id,
            response_result: Ok(Value::Null),
        });
        server
            .finish(&connection, next(&completions).await)
            .unwrap();
        let Message::Response(response) = protocol_message(&client).await else {
            panic!("Expected silent dismissal");
        };
        assert_eq!(response.id, 7.into());
        assert!(response.response_result.unwrap().is_null());
        assert!(client.receiver.try_recv().is_err());
        assert_eq!(
            server.cache.latest(&uri).as_ref().unwrap().translation,
            "cached"
        );
        drop(locked);
        assert_eq!(
            auth.store
                .snapshot()
                .unwrap()
                .map(|snapshot| snapshot.revision),
            before
        );
        assert!(fixture.state.lock().unwrap().records.is_empty());
        drop(completions);
        drop(server);
    }
}

#[tokio::test(flavor = "current_thread")]
async fn account_picker_dispatches_only_the_selected_command() {
    let fixture = Fixture::new().await;
    let auth = fixture.auth();
    fixture.connected(&auth, 3600).await;
    let before = auth.store.snapshot().unwrap().unwrap().revision;
    let provider = ChatGptProvider::synthetic(
        auth.clone(),
        format!("{}/v1", fixture.base),
        "synthetic-model".into(),
    );
    let (connection, client) = Connection::memory();
    let (finished, completions) = bounded(MAX_JOBS);
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
    let commands = serde_json::to_value(capabilities::<ChatGptProvider>()).unwrap()["capabilities"]
        ["executeCommandProvider"]["commands"]
        .clone();
    for command in std::iter::once(COMMAND)
        .chain(
            ChatGptProvider::capabilities()
                .commands
                .iter()
                .map(|command| command.command),
        )
        .chain(std::iter::once(SETTINGS_COMMAND))
    {
        assert!(commands.as_array().unwrap().contains(&json!(command)));
    }
    server
        .execute_command(
            &connection,
            1.into(),
            parse(json!({"command":SETTINGS_COMMAND})).unwrap(),
        )
        .unwrap();
    let Message::Request(request) = protocol_message(&client).await else {
        panic!("Expected picker");
    };
    assert_eq!(request.params["message"], "");
    assert_eq!(
        request.params["actions"],
        json!([
            {"title": "Change ChatGPT account"},
            {"title": "Sign out of ChatGPT"},
            {"title": "ChatGPT Usage"}
        ])
    );
    server.ui.response(Response {
        id: request.id,
        response_result: Ok(json!({"title":"ChatGPT Usage"})),
    });
    server
        .finish(&connection, next(&completions).await)
        .unwrap();
    assert_eq!(
        server.administrations[&1.into()].command,
        "translation-lsp.usage"
    );
    let Message::Request(request) = protocol_message(&client).await else {
        panic!("Expected chosen usage UI");
    };
    assert_eq!(request.method, "window/showDocument");
    assert_eq!(request.params["uri"], "https://chatgpt.com/settings/usage");
    server.ui.response(Response {
        id: request.id,
        response_result: Ok(json!({"success":true})),
    });
    server
        .finish(&connection, next(&completions).await)
        .unwrap();
    command_notice(&client, 1, 3).unwrap();
    assert!(fixture.state.lock().unwrap().records.is_empty());
    assert_eq!(auth.store.snapshot().unwrap().unwrap().revision, before);
    drop(completions);
    drop(server);
}

#[tokio::test(flavor = "current_thread")]
async fn candidates_follow_auth_transitions_without_http_or_store_writes() {
    use crate::providers::chatgpt::{
        auth::Auth,
        auth_store::{AuthStore, DIRECT_SCOPE},
        http::Http,
    };
    let fixture = Fixture::new().await;
    let directory =
        std::env::temp_dir().join(format!("zed-actions-synthetic-{}", uuid::Uuid::new_v4()));
    let auth = Auth::synthetic(
        AuthStore::synthetic(directory.clone()),
        Http::synthetic_revocation(&fixture.base),
        fixture.base.clone(),
    );
    let provider = ChatGptProvider::synthetic(
        auth.clone(),
        format!("{}/v1", fixture.base),
        "synthetic-model".into(),
    );
    let (connection, client) = Connection::memory();
    let (finished, completions) = bounded(MAX_JOBS);
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
    let uri: Uri = "file:///state-actions.rs".parse().unwrap();
    server.documents.insert(
        uri.clone(),
        Document::new(1, 1, "rust".into(), "// Translate this.\n".into()),
    );
    let actions = |server: &Server<ChatGptProvider>| -> Vec<(String, String)> {
        server
            .provider
            .account_commands()
            .into_iter()
            .map(|command| (command.command.to_owned(), command.title.to_owned()))
            .collect()
    };
    let connect = (
        "translation-lsp.connect".into(),
        "Continue with ChatGPT / Choose account".into(),
    );
    let usage = ("translation-lsp.usage".into(), "ChatGPT Usage".into());
    let signed_out = vec![connect.clone(), usage.clone()];
    let unknown = vec![
        (
            "translation-lsp.connect".into(),
            "Check ChatGPT connection…".into(),
        ),
        usage.clone(),
    ];
    assert_eq!(actions(&server), signed_out);
    assert!(
        !directory.exists(),
        "Candidate discovery must not create auth/lock files"
    );
    let id = fixture.connected(&auth, 3600).await;
    let ready = vec![
        (
            "translation-lsp.connect".into(),
            "Change ChatGPT account".into(),
        ),
        (
            "translation-lsp.sign-out".into(),
            "Sign out of ChatGPT".into(),
        ),
        usage.clone(),
    ];
    let before = auth.store.snapshot().unwrap().unwrap().revision;
    assert_eq!(actions(&server), ready);
    assert_eq!(auth.store.snapshot().unwrap().unwrap().revision, before);
    // No token expiry probing or refresh happens in discovery.
    {
        let mut locked = auth.store.lock(false).await.unwrap().unwrap();
        let credentials = locked
            .state
            .registrations
            .get_mut(&id)
            .unwrap()
            .credentials
            .as_mut()
            .unwrap();
        credentials.expires_in = 0;
        locked.save().unwrap();
    }
    assert_eq!(actions(&server), ready);
    {
        let mut locked = auth.store.lock(false).await.unwrap().unwrap();
        locked
            .state
            .registrations
            .get_mut(&id)
            .unwrap()
            .credentials
            .as_mut()
            .unwrap()
            .scopes
            .retain(|scope| scope != DIRECT_SCOPE);
        locked.save().unwrap();
    }
    assert_eq!(
        actions(&server),
        vec![
            connect.clone(),
            (
                "translation-lsp.enable".into(),
                "Enable ChatGPT plan use".into()
            ),
            (
                "translation-lsp.sign-out".into(),
                "Sign out of ChatGPT".into()
            ),
            usage.clone()
        ]
    );
    assert!(
        auth.key()
            .unwrap_err()
            .to_string()
            .contains("Enable ChatGPT plan use")
    );
    {
        let mut locked = auth.store.lock(false).await.unwrap().unwrap();
        // While another process owns the store, use a non-assertive recovery title.
        assert_eq!(actions(&server), unknown);
        let before = locked.revision().unwrap();
        server
            .request(
                &connection,
                Request::new(
                    9.into(),
                    ExecuteCommand::METHOD.into(),
                    json!({"command":"translation-lsp.connect"}),
                ),
            )
            .unwrap();
        assert!(
            server
                .code_actions(
                    parse(json!({
                        "textDocument": {"uri": uri},
                        "range": {
                            "start": {"line": 0, "character": 0},
                            "end": {"line": 0, "character": 18}
                        },
                        "context": {"diagnostics": []}
                    }))
                    .unwrap()
                )
                .unwrap()
                .is_empty()
        );
        server
            .finish(&connection, next(&completions).await)
            .unwrap();
        assert_eq!(
            command_notice(&client, 9, 1).unwrap_err().code,
            ErrorCode::RequestFailed as i32
        );
        assert_eq!(locked.revision().unwrap(), before);
        assert!(fixture.state.lock().unwrap().records.is_empty());
        locked.state.registrations.get_mut(&id).unwrap().credentials = None;
        locked.save().unwrap();
    }
    assert_eq!(actions(&server), signed_out);
    let error = auth.key().unwrap_err();
    assert!(
        error
            .to_string()
            .contains("Continue with ChatGPT / Choose account")
    );
    assert!(!error.to_string().contains("Enable"));
    fixture.connected(&auth, 3600).await;
    assert_eq!(actions(&server), ready);
    // An invalid local snapshot also retains the explicit recovery entry.
    let path = directory.join("auth.json");
    let saved = std::fs::read(&path).unwrap();
    std::fs::write(&path, b"{").unwrap();
    assert_eq!(actions(&server), unknown);
    server
        .request(
            &connection,
            Request::new(
                10.into(),
                ExecuteCommand::METHOD.into(),
                json!({"command":"translation-lsp.connect"}),
            ),
        )
        .unwrap();
    server
        .finish(&connection, next(&completions).await)
        .unwrap();
    let error = command_notice(&client, 10, 1).unwrap_err();
    assert_eq!(error.code, ErrorCode::RequestFailed as i32);
    assert!(!error.message.contains("synthetic-access"));
    assert_eq!(
        std::fs::read(&path).unwrap(),
        b"{",
        "Explicit failed recovery must not rewrite the store"
    );
    std::fs::write(&path, saved).unwrap();
    assert_eq!(actions(&server), ready);
    assert!(fixture.state.lock().unwrap().records.is_empty());
    let mut files: Vec<_> = std::fs::read_dir(&directory)
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect();
    files.sort();
    assert_eq!(
        files,
        vec![
            std::ffi::OsString::from("auth.json"),
            std::ffi::OsString::from("auth.lock")
        ]
    );
    drop(completions);
    drop(server);
}

#[tokio::test(flavor = "current_thread")]
async fn candidates_are_local_and_explicit_commands_use_standard_lsp_ui() {
    let fixture = Fixture::new().await;
    let auth = fixture.auth();
    let provider = ChatGptProvider::synthetic(
        auth.clone(),
        format!("{}/v1", fixture.base),
        "synthetic-model".into(),
    );
    let (connection, client) = Connection::memory();
    let (finished, completions) = bounded(MAX_JOBS);
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
    let uri: Uri = "file:///actions.rs".parse().unwrap();
    server.documents.insert(
        uri.clone(),
        Document::new(1, 1, "rust".into(), "// Translate this.\n".into()),
    );
    let actions = server.code_actions(parse(json!({"textDocument":{"uri":uri},"range":{"start":{"line":0,"character":0},"end":{"line":0,"character":0}},"context":{"diagnostics":[]}})).unwrap()).unwrap();
    assert_eq!(actions.len(), 1);
    assert_eq!(actions[0].title, "ChatGPT account…");
    assert_eq!(
        actions[0].command.as_ref().unwrap().command,
        SETTINGS_COMMAND
    );
    assert!(fixture.state.lock().unwrap().records.is_empty());
    assert!(auth.store.snapshot().unwrap().is_none());
    server
        .execute_command(
            &connection,
            RequestId::from(1),
            parse(json!({"command":"translation-lsp.connect"})).unwrap(),
        )
        .unwrap();
    let receiver = client.receiver.clone();
    let Message::Request(request) =
        tokio::task::spawn_blocking(move || receiver.recv_timeout(Duration::from_secs(2)).unwrap())
            .await
            .unwrap()
    else {
        panic!("Expected account picker");
    };
    assert_eq!(request.method, "window/showMessageRequest");
    server
        .ui
        .response(Response::new_ok(request.id, Value::Null));
    server
        .finish(&connection, next(&completions).await)
        .unwrap();
    assert!(auth.store.snapshot().unwrap().is_none());
    command_notice(&client, 1, 3).unwrap();
    fixture.connected(&auth, 3600).await;
    let before = auth.store.snapshot().unwrap().unwrap().revision;
    let commands = capabilities::<ChatGptProvider>()
        .capabilities
        .execute_command_provider
        .unwrap()
        .commands;
    assert!(commands.iter().any(|command| command == SETTINGS_COMMAND));
    assert!(
        !commands
            .iter()
            .any(|command| command == "translation-lsp.models")
    );
    let error = server
        .execute_command(
            &connection,
            RequestId::from(2),
            parse(json!({"command":"translation-lsp.models"})).unwrap(),
        )
        .unwrap_err();
    assert_eq!(error.code, ErrorCode::InvalidParams as i32);
    assert!(client.receiver.try_recv().is_err());
    assert!(server.administrations.is_empty());
    assert!(fixture.state.lock().unwrap().records.is_empty());
    assert_eq!(auth.store.snapshot().unwrap().unwrap().revision, before);
    server
        .execute_command(
            &connection,
            RequestId::from(3),
            parse(json!({"command":"translation-lsp.usage"})).unwrap(),
        )
        .unwrap();
    let receiver = client.receiver.clone();
    let Message::Request(request) =
        tokio::task::spawn_blocking(move || receiver.recv_timeout(Duration::from_secs(2)).unwrap())
            .await
            .unwrap()
    else {
        panic!("Expected usage URL");
    };
    assert_eq!(request.method, "window/showDocument");
    assert_eq!(request.params["uri"], "https://chatgpt.com/settings/usage");
    server
        .ui
        .response(Response::new_ok(request.id, json!({"success":true})));
    server
        .finish(&connection, next(&completions).await)
        .unwrap();
    command_notice(&client, 3, 3).unwrap();
}

#[tokio::test(flavor = "current_thread")]
async fn explicit_translation_recovers_from_usage_pause_through_worker() {
    let fixture = Fixture::new().await;
    let auth = fixture.auth();
    fixture.connected(&auth, 3600).await;
    fixture.state.lock().unwrap().failure = Some((
        429,
        json!({
            "error":{"code":"subscription_sharing_usage_limit_exceeded","message":"Limit reached"}
        }),
    ));
    let provider = ChatGptProvider::synthetic(
        auth,
        format!("{}/v1", fixture.base),
        "synthetic-model".into(),
    );
    let (connection, client) = Connection::memory();
    let (finished, completions) = bounded(MAX_JOBS);
    let mut server = Server::configured(
        finished,
        provider.clone(),
        Settings::default(),
        Ui::new(connection.sender.clone(), UiCapabilities::default()),
    )
    .unwrap();
    let uri: Uri = "file:///retry.rs".parse().unwrap();
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
    assert!(server.cache.latest(&uri).is_none());
    client.receiver.try_recv().unwrap();
    let failure = server.hover(2.into(), hover()).unwrap().unwrap();
    assert!(
        failure["contents"]["value"]
            .as_str()
            .unwrap()
            .contains("translation failed")
    );
    assert_eq!(response_calls(&fixture), 1);
    fixture.state.lock().unwrap().failure = None;
    let actions = markdown_actions(
        &mut server,
        &uri,
        Range::new(Position::new(0, 4), Position::new(0, 4)),
    );
    let retry = actions
        .into_iter()
        .filter_map(|a| a.command)
        .find(|c| c.command == RETRY_COMMAND)
        .unwrap();
    assert_eq!(
        retry.arguments.as_ref().unwrap()[0]["range"],
        json!(Range::new(
            Position::new(0, 0),
            Position::new(0, source.len() as u32)
        ))
    );
    server
        .execute_command(
            &connection,
            3.into(),
            ExecuteCommandParams {
                command: retry.command,
                arguments: retry.arguments.unwrap(),
                work_done_progress_params: Default::default(),
            },
        )
        .unwrap();
    server
        .finish(&connection, next(&completions).await)
        .unwrap();
    assert!(server.cache.latest(&uri).is_some());
    assert_eq!(response_calls(&fixture), 2);
    provider
        .admit(
            &provider.context().unwrap(),
            crate::providers::Intent::Automatic,
        )
        .unwrap();
    assert!(server.hover(4.into(), hover()).unwrap().is_some());
    assert_eq!(response_calls(&fixture), 2);
}

#[tokio::test(flavor = "current_thread")]
async fn unrelated_hover_is_null_when_translation_authorization_is_unavailable() {
    for state in ["signed-out", "plan-disabled", "busy", "administration"] {
        let fixture = Fixture::new().await;
        let auth = fixture.auth();
        if state != "signed-out" {
            let registration = fixture.connected(&auth, 3600).await;
            if state == "plan-disabled" {
                let mut writer = auth.store.lock(false).await.unwrap().unwrap();
                writer
                    .state
                    .registrations
                    .get_mut(&registration)
                    .unwrap()
                    .credentials
                    .as_mut()
                    .unwrap()
                    .scopes
                    .clear();
                writer.save().unwrap();
            }
        }
        let _locked = if state == "busy" {
            auth.store.lock(false).await.unwrap()
        } else {
            None
        };
        let provider = ChatGptProvider::synthetic(
            auth,
            format!("{}/v1", fixture.base),
            "synthetic-model".into(),
        );
        let (connection, _client) = Connection::memory();
        let (finished, _completions) = bounded(MAX_JOBS);
        let mut server = Server::configured(
            finished,
            provider,
            Settings::default(),
            Ui::new(connection.sender.clone(), UiCapabilities::default()),
        )
        .unwrap();
        if state == "administration" {
            let deadline = Instant::now() + Duration::from_secs(180);
            let (stop, _receiver) = watch::channel(Some(deadline));
            server.administrations.insert(
                99.into(),
                Administration {
                    command: SETTINGS_COMMAND.into(),
                    ticket: 1,
                    stop,
                    deadline,
                },
            );
        }
        let uri: Uri = "file:///unrelated.rs".parse().unwrap();
        server.documents.insert(
            uri.clone(),
            Document::new(
                1,
                1,
                "rust".into(),
                "let value = 1;   \n// Read this.".into(),
            ),
        );
        for (uri, line, character) in [
            (uri.as_str(), 0, 4),
            (uri.as_str(), 0, 15),
            (uri.as_str(), 50, 0),
            ("file:///not-open.rs", 0, 0),
        ] {
            let params = parse(json!({
                "textDocument": {"uri": uri},
                "position": {"line": line, "character": character}
            }))
            .unwrap();
            let result = server.hover(1.into(), params);
            assert_eq!(
                result.unwrap(),
                Some(Value::Null),
                "{state}: {uri}:{line}:{character}"
            );
        }
        assert!(
            server
                .hover(
                    2.into(),
                    parse(json!({"textDocument":{"uri":uri},"position":{"line":1,"character":4}}))
                        .unwrap()
                )
                .is_err()
        );
        assert_eq!(response_calls(&fixture), 0);
    }
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
    let (finished, completions) = bounded(MAX_JOBS);
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
        assert_eq!(server.scheduler.translations.len(), 4);
        server
            .cancel_range(
                &connection,
                server.scheduler.pending[&id.into()].ticket,
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
    while !server.scheduler.translations.is_empty() {
        server
            .finish(&connection, next(&completions).await)
            .unwrap();
    }
    assert_eq!(response_calls(&fixture), 4);
    assert_eq!(client.receiver.try_iter().count(), 4);
    assert!(uris.iter().all(|uri| server.cache.latest(uri).is_some()));
}

async fn range_server(
    settings: Settings,
) -> (
    Fixture,
    Server<ChatGptProvider>,
    Connection,
    Connection,
    crossbeam_channel::Receiver<WorkerEvent>,
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
    let (finished, completions) = bounded(MAX_JOBS);
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

#[tokio::test(flavor = "current_thread")]
async fn usage_stop_drains_old_admissions_without_http_or_duplicate_notices_and_allows_fresh_retry()
{
    let mut settings = Settings::default();
    settings.translation_limits.work.concurrent = 1;
    let (fixture, mut server, connection, client, completions, uri) = range_server(settings).await;
    fixture.state.lock().unwrap().failure = Some((
        429,
        json!({"error":{"code":"subscription_sharing_usage_limit_exceeded","message":"Synthetic limit"}}),
    ));
    let worker_jobs = server.jobs.take().unwrap();
    let (hold, mut held) = queue::channel(1);
    server.jobs = Some(hold);
    for line in [0, 2, 4] {
        let target = range_target(&server, &uri, line);
        assert_eq!(
            server
                .queue((line as i32 + 1).into(), target, PendingReply::Command)
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
    assert_eq!(response_calls(&fixture), 1);
    let notices: Vec<_> = client.receiver.try_iter().collect();
    assert_eq!(
        notices.len(),
        1,
        "one stop notification, not one per queued range"
    );
    assert!(matches!(&notices[0], Message::Notification(n) if n.params["type"] == 1));
    for line in [0, 2, 4] {
        let hover = server
            .hover(
                10.into(),
                parse(json!({"textDocument":{"uri":uri},"position":{"line":line,"character":3}}))
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
    // An unsuccessful fresh retry still reports its own error even while the pause is unchanged.
    let target = range_target(&server, &uri, 0);
    server
        .queue(20.into(), target.clone(), PendingReply::Command)
        .unwrap();
    server
        .finish(&connection, next(&completions).await)
        .unwrap();
    assert_eq!(response_calls(&fixture), 2);
    assert!(
        matches!(client.receiver.try_recv().unwrap(), Message::Notification(n) if n.params["type"] == 1)
    );
    fixture.state.lock().unwrap().failure = None;
    server
        .queue(21.into(), target.clone(), PendingReply::Command)
        .unwrap();
    server
        .finish(&connection, next(&completions).await)
        .unwrap();
    assert_eq!(response_calls(&fixture), 3);
    assert!(server.cache.exact(&uri, target.range, false).is_some());
    server
        .provider
        .admit(
            &server.provider.context().unwrap(),
            crate::providers::Intent::Automatic,
        )
        .unwrap();
}

#[tokio::test(flavor = "current_thread")]
async fn empty_selection_preserves_adjustment_chain_but_visiting_another_range_breaks_it() {
    let (fixture, mut server, connection, _client, completions, uri) =
        range_server(Settings::default()).await;
    let tail = std::sync::Arc::new(StreamTailGate::default());
    fixture.state.lock().unwrap().stream_tail_gate = Some(tail.clone());
    let first = range_target(&server, &uri, 0);
    markdown_actions(&mut server, &uri, first.range);
    server.selection.as_mut().unwrap().start = Some(Instant::now());
    server.expire(&connection).unwrap();
    let old = server.scheduler.ticket(&first).unwrap();
    wait_held(&tail, 1).await;
    markdown_actions(&mut server, &uri, Range::default());
    let adjusted = Range::new(Position::new(0, 0), Position::new(0, 5));
    markdown_actions(&mut server, &uri, adjusted);
    server.selection.as_mut().unwrap().start = Some(Instant::now());
    server.expire(&connection).unwrap();
    assert!(!server.scheduler.translations.contains_key(&old));
    server
        .finish(&connection, next(&completions).await)
        .unwrap();
    let kept = server
        .scheduler
        .translations
        .keys()
        .copied()
        .next()
        .unwrap();
    for range in [
        range_target(&server, &uri, 2).range,
        Range::new(Position::new(0, 0), Position::new(0, 7)),
    ] {
        markdown_actions(&mut server, &uri, range);
        server.selection.as_mut().unwrap().start = Some(Instant::now());
        server.expire(&connection).unwrap();
    }
    assert!(
        server.scheduler.translations.contains_key(&kept),
        "Returning from B must not cancel the earlier A"
    );
    assert_eq!(server.scheduler.translations.len(), 3);
    // A fourth automatic selection remains an unaccepted candidate, and is replaced by a later candidate.
    for end in [6, 9] {
        let range = Range::new(Position::new(4, 0), Position::new(4, end));
        markdown_actions(&mut server, &uri, range);
        server.selection.as_mut().unwrap().start = Some(Instant::now());
        server.expire(&connection).unwrap();
        assert!(server.selection.as_ref().unwrap().start.is_some());
        assert_eq!(server.scheduler.translations.len(), 3);
    }
    server.cancel_activity(&connection).unwrap();
    drop(completions);
    drop(server);
}

#[tokio::test(flavor = "current_thread")]
async fn reselecting_a_cached_overlap_updates_display_order_without_http() {
    let (fixture, mut server, connection, _client, completions, uri) =
        range_server(Settings::default()).await;
    let mut first = range_target(&server, &uri, 0);
    first.range.end.character = 10;
    let mut second = range_target(&server, &uri, 0);
    second.range.start.character = 5;
    for target in [first.clone(), second.clone()] {
        server
            .queue(1.into(), target, PendingReply::Command)
            .unwrap();
        server
            .finish(&connection, next(&completions).await)
            .unwrap();
    }
    let hover = |server: &mut Server<ChatGptProvider>| {
        server
            .hover(
                2.into(),
                parse(json!({"textDocument":{"uri":uri},"position":{"line":0,"character":7}}))
                    .unwrap(),
            )
            .unwrap()
            .unwrap()
    };
    assert!(
        hover(&mut server)["contents"]["value"]
            .as_str()
            .unwrap()
            .contains("訳 sentence.")
    );
    assert_eq!(
        server
            .queue(3.into(), first, PendingReply::Command)
            .unwrap(),
        Some(Value::Null)
    );
    assert!(
        hover(&mut server)["contents"]["value"]
            .as_str()
            .unwrap()
            .contains("訳Alpha sent")
    );
    assert_eq!(response_calls(&fixture), 2);
}

#[tokio::test(flavor = "current_thread")]
async fn comment_hovers_survive_cancellation_and_waiter_expiry_with_bounded_parallel_http() {
    let (fixture, mut server, connection, client, completions, uri) =
        range_server(Settings::default()).await;
    server.documents.insert(
        uri.clone(),
        Document::new(
            1,
            1,
            "rust".into(),
            "// Alpha sentence.\n\n// Bravo sentence.\n\n// Third sentence.\n\n// Fourth sentence."
                .into(),
        ),
    );
    let tail = std::sync::Arc::new(StreamTailGate::default());
    fixture.state.lock().unwrap().stream_tail_gate = Some(tail.clone());
    let hover = |line| {
        parse(json!({"textDocument":{"uri":uri},"position":{"line":line,"character":4}})).unwrap()
    };
    for (id, line) in [(1, 0), (2, 2), (3, 4)] {
        assert!(server.hover(id.into(), hover(line)).unwrap().is_none());
        if id < 3 {
            wait_held(&tail, id as usize).await;
        }
    }
    assert_eq!(response_calls(&fixture), 2);
    assert!(
        server
            .hover(4.into(), hover(6))
            .unwrap_err()
            .message
            .contains("queue is full")
    );
    // Both running requests can disappear; the queued waiter's own deadline can expire.
    for id in [1, 2] {
        server
            .cancel(&connection, &id.into(), ErrorCode::RequestCanceled)
            .unwrap();
    }
    server
        .scheduler
        .pending
        .get_mut(&3.into())
        .unwrap()
        .deadline = Instant::now();
    server.expire(&connection).unwrap();
    assert_eq!(client.receiver.try_iter().count(), 3);
    assert_eq!(server.scheduler.automatic_count(), 3);
    assert!(server.scheduler.pending.is_empty());
    let actions = server
        .cancel_actions(&CodeActionParams {
            text_document: lsp_types::TextDocumentIdentifier { uri: uri.clone() },
            range: Range::new(Position::new(0, 4), Position::new(0, 4)),
            context: lsp_types::CodeActionContext::default(),
            work_done_progress_params: Default::default(),
            partial_result_params: Default::default(),
        })
        .unwrap();
    assert!(
        actions
            .iter()
            .any(|a| a.command.as_ref().unwrap().command == CANCEL_COMMAND)
    );
    assert!(
        actions
            .iter()
            .any(|a| a.command.as_ref().unwrap().command == CANCEL_ALL_COMMAND)
    );
    fixture.state.lock().unwrap().stream_tail_gate = None;
    tail.release.notify_waiters();
    while !server.scheduler.translations.is_empty() {
        server
            .finish(&connection, next(&completions).await)
            .unwrap();
    }
    assert_eq!(response_calls(&fixture), 3);
    assert_eq!(tail.peak.load(std::sync::atomic::Ordering::SeqCst), 2);
    assert!(
        client.receiver.is_empty(),
        "Canceled responses and automatic completion must stay silent"
    );
    for (id, line, text) in [(5, 0, "Alpha"), (6, 2, "Bravo"), (7, 4, "Third")] {
        let cached = server.hover(id.into(), hover(line)).unwrap().unwrap();
        assert!(cached["contents"]["value"].as_str().unwrap().contains(text));
    }
    assert_eq!(
        response_calls(&fixture),
        3,
        "Revisits must use all three cached results"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn failed_retained_comment_is_visible_after_its_hover_was_canceled() {
    let (fixture, mut server, connection, client, completions, uri) =
        range_server(Settings::default()).await;
    server.documents.insert(
        uri.clone(),
        Document::new(1, 1, "rust".into(), "// Source.".into()),
    );
    fixture.state.lock().unwrap().failure = Some((
        400,
        json!({"error":{"code":"invalid_request_error","message":"Synthetic failure"}}),
    ));
    let hover =
        || parse(json!({"textDocument":{"uri":uri},"position":{"line":0,"character":4}})).unwrap();
    server.hover(1.into(), hover()).unwrap();
    server
        .cancel(&connection, &1.into(), ErrorCode::RequestCanceled)
        .unwrap();
    client.receiver.try_recv().unwrap();
    server
        .finish(&connection, next(&completions).await)
        .unwrap();
    let cached = server.hover(2.into(), hover()).unwrap().unwrap();
    assert!(
        cached["contents"]["value"]
            .as_str()
            .unwrap()
            .contains("translation failed")
    );
    assert_eq!(response_calls(&fixture), 1);
    assert!(client.receiver.is_empty());
}

#[tokio::test(flavor = "current_thread")]
async fn comment_and_selection_share_automatic_capacity_but_manual_promotion_frees_it() {
    let mut settings = Settings::default();
    settings.translation_limits.work.automatic = 1;
    let (_fixture, mut server, connection, _client, completions, markdown) =
        range_server(settings).await;
    let first = range_target(&server, &markdown, 0);
    server
        .queue_translation(first.clone(), TranslationRequest::Selection)
        .unwrap();
    let uri: Uri = "file:///capacity.rs".parse().unwrap();
    server.documents.insert(
        uri.clone(),
        Document::new(1, 2, "rust".into(), "// Read this.".into()),
    );
    let hover =
        || parse(json!({"textDocument":{"uri":uri},"position":{"line":0,"character":4}})).unwrap();
    assert!(
        server
            .hover(1.into(), hover())
            .unwrap_err()
            .message
            .contains("queue is full")
    );
    server
        .queue(2.into(), first, PendingReply::Command)
        .unwrap();
    assert!(server.hover(3.into(), hover()).unwrap().is_none());
    server
        .cancel(&connection, &3.into(), ErrorCode::RequestCanceled)
        .unwrap();
    assert!(server.hover(4.into(), hover()).unwrap().is_none());
    assert_eq!(
        server.scheduler.automatic_count(),
        1,
        "Revisiting a retained comment must join it"
    );
    let second = range_target(&server, &markdown, 2);
    assert!(
        server
            .queue_translation(second, TranslationRequest::Selection)
            .unwrap_err()
            .message
            .contains("queue is full")
    );
    while !server.scheduler.translations.is_empty() {
        server
            .finish(&connection, next(&completions).await)
            .unwrap();
    }
}

#[tokio::test(flavor = "current_thread")]
async fn unrelated_action_kinds_preserve_selection_candidate_and_adjustment_chain() {
    let (fixture, mut server, connection, _client, completions, uri) =
        range_server(Settings::default()).await;
    let selected = range_target(&server, &uri, 0);
    let full_range = server.documents[&uri].full_range().unwrap();
    let unrelated = |uri: &Uri, range, kind| {
        parse(json!({
            "textDocument":{"uri":uri},"range":range,"context":{"diagnostics":[],"only":[kind]}
        }))
        .unwrap()
    };
    assert!(
        server
            .code_actions(unrelated(&uri, full_range, "source.fixAll"))
            .unwrap()
            .is_empty()
    );
    assert!(server.selection.is_none());
    markdown_actions(&mut server, &uri, selected.range);
    let previous = server.previous_selection.clone();
    let deadline = server.selection.as_ref().unwrap().start;
    let other: Uri = "file:///other.md".parse().unwrap();
    assert!(
        server
            .code_actions(unrelated(&other, full_range, "source.organizeImports"))
            .unwrap()
            .is_empty()
    );
    assert!(server.previous_selection == previous);
    assert!(server.selection.as_ref().unwrap().target == selected);
    assert_eq!(server.selection.as_ref().unwrap().start, deadline);
    // A genuine request for the parent translation kind still observes a selection.
    server
        .code_actions(unrelated(&uri, selected.range, "translation"))
        .unwrap();
    tokio::time::sleep(Duration::from_millis(
        server.settings.selection_debounce_ms + 10,
    ))
    .await;
    server.start_selection();
    let completion = next(&completions).await;
    server.finish(&connection, completion).unwrap();
    assert_eq!(response_calls(&fixture), 1);
    assert_eq!(server.cache.latest(&uri).unwrap().range, selected.range);
}

#[tokio::test(flavor = "current_thread")]
async fn retry_action_refreshes_failed_range_without_discarding_readable_translation() {
    let (fixture, mut server, connection, _client, completions, uri) =
        range_server(Settings::default()).await;
    let target = range_target(&server, &uri, 0);
    let context = server.provider.context().unwrap();
    server.cache.insert(
        uri.clone(),
        1,
        Preview::new(
            context.clone(),
            target.range,
            "Previous translation".into(),
            false,
        ),
        false,
        false,
    );
    server.cache.insert(
        uri.clone(),
        2,
        Preview::new(context, target.range, "Synthetic failure".into(), false),
        false,
        true,
    );
    server.next_ticket = 2;
    let cursor = Range::new(Position::new(0, 5), Position::new(0, 5));
    let retry = markdown_actions(&mut server, &uri, cursor)
        .into_iter()
        .filter_map(|a| a.command)
        .find(|c| c.command == RETRY_COMMAND)
        .unwrap();
    let params = ExecuteCommandParams {
        command: retry.command,
        arguments: retry.arguments.unwrap(),
        work_done_progress_params: Default::default(),
    };
    let tail = std::sync::Arc::new(StreamTailGate::default());
    fixture.state.lock().unwrap().stream_tail_gate = Some(tail.clone());
    server
        .execute_command(&connection, 1.into(), params.clone())
        .unwrap();
    wait_held(&tail, 1).await;
    server
        .execute_command(&connection, 2.into(), params)
        .unwrap();
    assert_eq!(response_calls(&fixture), 1);
    assert_eq!(
        server.cache.latest(&uri).unwrap().translation,
        "Previous translation"
    );
    assert!(
        !markdown_actions(&mut server, &uri, cursor).iter().any(|a| a
            .command
            .as_ref()
            .is_some_and(|c| c.command == RETRY_COMMAND))
    );
    tail.release.notify_one();
    server
        .finish(&connection, next(&completions).await)
        .unwrap();
    assert!(
        server
            .cache
            .latest(&uri)
            .unwrap()
            .translation
            .contains("Alpha")
    );
    assert!(server.cache.failed_range(&uri, cursor.start).is_none());
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
            server.scheduler.translations.is_empty(),
            "Switching outputs must not enqueue another translation"
        );
        assert_eq!(response_calls(&fixture), 1);
        assert_eq!(
            server
                .cache
                .exact(&uri, target.range, true)
                .unwrap()
                .translation,
            server
                .cache
                .exact(&uri, target.range, false)
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
    assert!(server.cache.exact(&uri, target.range, false).is_none());
    assert_eq!(response_calls(&fixture), 1);
    server.notification(&connection, lsp_server::Notification::new(DidChangeTextDocument::METHOD.into(), json!({"textDocument":{"uri":uri,"version":2},"contentChanges":[{"text":"Changed."}]}))).unwrap();
    let error = server
        .queue_translation(target, TranslationRequest::Document)
        .unwrap_err();
    assert_eq!(error.code, ErrorCode::ContentModified as i32);
    assert!(server.cache.latest(&uri).is_none());
    assert_eq!(response_calls(&fixture), 1);
}
