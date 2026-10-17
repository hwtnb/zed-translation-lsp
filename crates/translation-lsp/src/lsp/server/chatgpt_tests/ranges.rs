//! Range work kept past its requests, comment Hover, and retries.
use super::*;

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
    assert!(!server.scheduler.pending().contains_key(&2.into()));
    server
        .cancel(&connection, &1.into(), ErrorCode::RequestCanceled)
        .unwrap();
    server
        .cancel(&connection, &2.into(), ErrorCode::RequestCanceled)
        .unwrap();
    markdown_actions(&mut server, &uri, Range::default());
    assert_eq!(server.scheduler.translations().len(), 1);
    tail.release.notify_one();
    server
        .finish(&connection, next(&completions).await)
        .unwrap();
    assert!(
        server
            .cache
            .exact(&uri, target.range, Output::Range)
            .is_some()
    );
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
    assert_eq!(server.scheduler.translations().len(), 3);
    wait_held(&tail, 2).await;
    markdown_actions(&mut server, &uri, Range::default());
    let canceled = server
        .range_ticket(&range_target(&server, &uri, 2))
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
                .exact(&uri, range_target(&server, &uri, line).range, Output::Range)
                .is_some()
        );
    }
    assert!(
        server
            .cache
            .exact(&uri, range_target(&server, &uri, 2).range, Output::Range)
            .is_none()
    );
    assert!(server.scheduler.translations().is_empty());
    assert_eq!(response_calls(&fixture), 3);
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
        .set_waiter_deadline(&3.into(), Instant::now());
    server.expire(&connection).unwrap();
    assert_eq!(client.receiver.try_iter().count(), 3);
    assert_eq!(server.scheduler.automatic_count(), 3);
    assert!(server.scheduler.pending().is_empty());
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
    while !server.scheduler.translations().is_empty() {
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
async fn retry_action_refreshes_failed_range_without_discarding_readable_translation() {
    let (fixture, mut server, connection, _client, completions, uri) =
        range_server(Settings::default()).await;
    let target = range_target(&server, &uri, 0);
    let context = server.provider.context().unwrap();
    server.cache.insert(
        uri.clone(),
        1,
        Stored::Translation(
            Output::Range,
            Preview::new(
                context.clone(),
                target.range,
                "Previous translation".into(),
                false,
                None,
            ),
        ),
    );
    server.cache.insert(
        uri.clone(),
        2,
        Stored::failure(context, target.range, "Synthetic failure"),
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
