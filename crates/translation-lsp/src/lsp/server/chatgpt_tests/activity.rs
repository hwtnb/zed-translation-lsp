//! The progress and status the server reports while it translates.
use super::*;

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
    let (finished, completions) = bounded(WORKER_EVENT_CHANNEL_CAPACITY);
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
    let (held_sender, mut held_jobs) =
        queue::channel(server.settings.translation_limits.work.concurrent);
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
    let ticket = server.scheduler.pending()[&1.into()].ticket;
    let original_context = server.scheduler.translations()[&ticket].context.clone();
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
    server.scheduler.mark_running(ticket);
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
        .set_waiter_deadline(&1.into(), Instant::now());
    server.expire(&connection).unwrap();
    server.publish_activity();
    let Message::Response(expired) = client.receiver.try_recv().unwrap() else {
        panic!("Expected expired Hover");
    };
    assert_eq!(
        expired.response_result.unwrap_err().code,
        ErrorCode::RequestFailed as i32
    );
    assert!(server.scheduler.translations()[&ticket].owners.selection());
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
