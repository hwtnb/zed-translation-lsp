//! Pauses for usage limits and translations without a usable authorization.
use super::*;

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
    let (finished, completions) = bounded(WORKER_EVENT_CHANNEL_CAPACITY);
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
        let (finished, _completions) = bounded(WORKER_EVENT_CHANNEL_CAPACITY);
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
                    context: None,
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
    assert!(
        server
            .cache
            .exact(&uri, target.range, Output::Range)
            .is_some()
    );
    server
        .provider
        .admit(
            &server.provider.context().unwrap(),
            crate::providers::Intent::Automatic,
        )
        .unwrap();
}
