//! Markdown selections and automatic translation of them.
use super::*;

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
    let (finished, completions) = bounded(WORKER_EVENT_CHANNEL_CAPACITY);
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
        Document::new(1, 1, "markdown".into(), source.into()),
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
        assert!(server.cache.exact(&uri, range, Output::Range).is_some());
        assert!(server.scheduler.translations().is_empty());
    }
    server.publish_activity();
    let Message::Notification(status) = client.receiver.try_recv().unwrap() else {
        panic!("Expected cleared failure status");
    };
    assert_eq!(status.params["health"], "ok");
    assert_eq!(server.documents[&uri].text().unwrap(), source);
    assert!(server.scheduler.pending().is_empty());
    assert!(server.scheduler.translations().is_empty());
    assert_eq!(response_calls(&fixture), 4);
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
        assert!(server.scheduler.translations().is_empty());
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
            let sent = prompt::source(body["input"][0]["content"].as_str().unwrap());
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
            assert!(server.scheduler.translations().is_empty());
            assert_eq!(fixture.state.lock().unwrap().records.len(), before);
        }
        assert_eq!(server.documents[&uri].text(), Some(text));
        assert!(client.receiver.try_recv().is_err());
    }
    drop(completions);
    drop(server);
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
async fn empty_selection_preserves_adjustment_chain_but_visiting_another_range_breaks_it() {
    let (fixture, mut server, connection, _client, completions, uri) =
        range_server(Settings::default()).await;
    let tail = std::sync::Arc::new(StreamTailGate::default());
    fixture.state.lock().unwrap().stream_tail_gate = Some(tail.clone());
    let first = range_target(&server, &uri, 0);
    markdown_actions(&mut server, &uri, first.range);
    server.selection.as_mut().unwrap().start = Some(Instant::now());
    server.expire(&connection).unwrap();
    let old = server.range_ticket(&first).unwrap();
    wait_held(&tail, 1).await;
    markdown_actions(&mut server, &uri, Range::default());
    let adjusted = Range::new(Position::new(0, 0), Position::new(0, 5));
    markdown_actions(&mut server, &uri, adjusted);
    server.selection.as_mut().unwrap().start = Some(Instant::now());
    server.expire(&connection).unwrap();
    assert!(!server.scheduler.translations().contains_key(&old));
    server
        .finish(&connection, next(&completions).await)
        .unwrap();
    let kept = server
        .scheduler
        .translations()
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
        server.scheduler.translations().contains_key(&kept),
        "Returning from B must not cancel the earlier A"
    );
    assert_eq!(server.scheduler.translations().len(), 3);
    // A fourth automatic selection remains an unaccepted candidate, and is replaced by a later candidate.
    for end in [6, 9] {
        let range = Range::new(Position::new(4, 0), Position::new(4, end));
        markdown_actions(&mut server, &uri, range);
        server.selection.as_mut().unwrap().start = Some(Instant::now());
        server.expire(&connection).unwrap();
        assert!(server.selection.as_ref().unwrap().start.is_some());
        assert_eq!(server.scheduler.translations().len(), 3);
    }
    server.cancel_translations(&connection).unwrap();
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
    while !server.scheduler.translations().is_empty() {
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
