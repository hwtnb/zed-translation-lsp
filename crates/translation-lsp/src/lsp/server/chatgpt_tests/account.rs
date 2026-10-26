//! The account menu, account operations, and pauses for usage limits.
use std::{env, ffi::OsString, fs, iter};

use tokio::task::spawn_blocking;
use uuid::Uuid;

use crate::providers::chatgpt;

use super::*;

#[tokio::test(flavor = "current_thread")]
async fn activity_cancellation_leaves_account_picker_and_its_deadline_alive() {
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
                work_done_progress: true,
                server_status: true,
            },
        ),
    )
    .unwrap();
    // Keep translation work out of the worker until activity cancellation is observed.
    let (held_sender, mut held_jobs) =
        queue::channel(server.settings.translation_limits.work.concurrent);
    let worker_jobs = server.jobs.replace(held_sender).unwrap();
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
    assert!(server.scheduler.translations().is_empty());
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
    assert_eq!(server.scheduler.admitted(), 1);
    let job = held_jobs.recv().await.unwrap();
    let ticket = job.ticket;
    assert!(job.deadline.borrow().is_none());
    worker_jobs.try_send(job).unwrap();
    server.jobs = Some(worker_jobs);
    let canceled = next(&completions).await;
    assert_eq!(canceled.ticket(), ticket);
    assert!(
        canceled
            .error()
            .is_some_and(|error| error.kind() == ErrorKind::Cancelled)
    );
    server.finish(&connection, canceled).unwrap();
    assert_eq!(server.scheduler.admitted(), 0);
    assert!(fixture.state.lock().unwrap().records.is_empty());
    assert_eq!(
        *server.administrations[&2.into()].stop.borrow(),
        Some(deadline)
    );
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

#[tokio::test(flavor = "current_thread")]
async fn administration_reports_admission_worker_and_deadline_failures_but_not_cancel() {
    let fixture = Fixture::new().await;
    let provider = ChatGptProvider::synthetic(
        fixture.auth(),
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
            Finished::Administration(Completion {
                ticket,
                result: Err(ProviderError::failed("late completion")),
            }),
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
    assert_eq!(server.scheduler.translations().len(), 1);
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
        let uri: Uri = "file:///picker.rs".parse().unwrap();
        server.documents.insert(
            uri.clone(),
            Document::new(1, 1, "rust".into(), "// sentence".into()),
        );
        server.cache.insert(
            uri.clone(),
            0,
            Stored::Translation(
                Output::Range,
                Preview::new(
                    AccountKey::synthetic(),
                    Range::new(Position::new(0, 0), Position::new(0, 11)),
                    "cached".into(),
                    true,
                    None,
                ),
            ),
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
        let expected: Vec<_> = chatgpt::management_commands(state)
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
    let commands = serde_json::to_value(capabilities::<ChatGptProvider>()).unwrap()["capabilities"]
        ["executeCommandProvider"]["commands"]
        .clone();
    for command in iter::once(COMMAND)
        .chain(
            ChatGptProvider::capabilities()
                .commands
                .iter()
                .map(|command| command.command),
        )
        .chain(iter::once(SETTINGS_COMMAND))
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
    let directory = env::temp_dir().join(format!("zed-actions-synthetic-{}", Uuid::new_v4()));
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
    let saved = fs::read(&path).unwrap();
    fs::write(&path, b"{").unwrap();
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
        fs::read(&path).unwrap(),
        b"{",
        "Explicit failed recovery must not rewrite the store"
    );
    fs::write(&path, saved).unwrap();
    assert_eq!(actions(&server), ready);
    assert!(fixture.state.lock().unwrap().records.is_empty());
    let mut files: Vec<_> = fs::read_dir(&directory)
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect();
    files.sort();
    assert_eq!(
        files,
        vec![OsString::from("auth.json"), OsString::from("auth.lock")]
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
        spawn_blocking(move || receiver.recv_timeout(Duration::from_secs(2)).unwrap())
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
        spawn_blocking(move || receiver.recv_timeout(Duration::from_secs(2)).unwrap())
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
