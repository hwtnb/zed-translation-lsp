use serde_json::json;

use super::*;

#[tokio::test(flavor = "current_thread")]
async fn worker_keeps_non_send_futures_and_one_independent_admin_slot() {
    #[derive(Clone)]
    struct GatedProvider {
        started: tokio::sync::mpsc::UnboundedSender<&'static str>,
        admin: std::sync::Arc<tokio::sync::Semaphore>,
        translation: std::sync::Arc<tokio::sync::Semaphore>,
    }
    impl TranslationProvider for GatedProvider {
        async fn translate(
            &self,
            _intent: crate::providers::Intent,
            text: &str,
            _: &str,
            context: Context,
        ) -> Result<Translated, ProviderError> {
            let text = std::rc::Rc::new(text.to_owned());
            self.started.send("translation").unwrap();
            self.translation.acquire().await.unwrap().forget();
            Ok(Translated {
                text: (*text).clone(),
                context,
            })
        }
        async fn administer(&self, _: &str, _: &impl Interaction) -> Result<String, ProviderError> {
            let text = std::rc::Rc::new("admin".to_owned());
            self.started.send("admin").unwrap();
            self.admin.acquire().await.unwrap().forget();
            Ok((*text).clone())
        }
    }
    let (started, mut observed) = tokio::sync::mpsc::unbounded_channel();
    let admin = std::sync::Arc::new(tokio::sync::Semaphore::new(0));
    let translation = std::sync::Arc::new(tokio::sync::Semaphore::new(0));
    let provider = GatedProvider {
        started,
        admin: admin.clone(),
        translation,
    };
    let (jobs, received_jobs) = queue::channel(4);
    let (administrations, received_admin) = queue::channel(2);
    let (finished, completions) = bounded(MAX_JOBS);
    let (connection, _client) = Connection::memory();
    let ui = Ui::new(connection.sender.clone(), UiCapabilities::default());
    let mut stops = Vec::new();
    for ticket in 0..6 {
        let (stop, deadline) = watch::channel(Some(Instant::now() + Duration::from_secs(5)));
        stops.push(stop);
        let task = if ticket < 4 {
            Task::Translation {
                intent: crate::providers::Intent::Automatic,
                generation: 0,
                text: format!("source {ticket}"),
                target_language: "ja".into(),
                context: Context::Mock,
            }
        } else {
            Task::Administration {
                command: "synthetic-admin".into(),
                ui: ui.clone(),
            }
        };
        let sender = if ticket < 4 { &jobs } else { &administrations };
        sender
            .try_send(Job {
                ticket,
                deadline,
                task,
            })
            .unwrap();
    }
    let exercise = async move {
        let mut messages = Vec::new();
        for _ in 0..3 {
            messages.push(observed.recv().await.unwrap());
        }
        assert_eq!(messages.iter().filter(|&&m| m == "translation").count(), 2);
        assert_eq!(messages.iter().filter(|&&m| m == "admin").count(), 1);
        assert!(observed.try_recv().is_err());
        admin.add_permits(1);
        assert_eq!(observed.recv().await.unwrap(), "admin");
        assert!(observed.try_recv().is_err());
        for stop in stops {
            stop.send_replace(None);
        }
        drop(jobs);
        drop(administrations);
    };
    tokio::time::timeout(Duration::from_secs(2), async {
        tokio::join!(
            worker(received_jobs, received_admin, finished, provider, 2),
            exercise
        );
    })
    .await
    .unwrap();
    let events: Vec<_> = completions.try_iter().collect();
    assert_eq!(
        events
            .iter()
            .filter(|e| matches!(e, WorkerEvent::Started(_)))
            .count(),
        2
    );
    assert_eq!(
        events
            .iter()
            .filter(|e| matches!(e, WorkerEvent::Finished(f) if f.ticket == 4 && f.result.is_ok()))
            .count(),
        1
    );
}

#[test]
fn generic_worker_receives_comment_bodies_and_adjacent_blocks_take_precedence() {
    #[derive(Clone)]
    struct RecordingProvider(Sender<(String, String)>);
    impl TranslationProvider for RecordingProvider {
        async fn translate(
            &self,
            _intent: crate::providers::Intent,
            text: &str,
            target: &str,
            context: Context,
        ) -> Result<Translated, ProviderError> {
            self.0.send((text.into(), target.into())).unwrap();
            Ok(Translated {
                text: format!("fake: {text}"),
                context,
            })
        }
    }
    let (record, recorded) = bounded(1);
    let (completed, completions) = bounded(MAX_JOBS);
    let mut server = Server::new_with_provider(completed, RecordingProvider(record)).unwrap();
    let (connection, client) = Connection::memory();
    let uri: Uri = "file:///unsaved.rs".parse().unwrap();
    let source = concat!(
        "  //\t`id` 🚀  \r\n",
        "  //  second\r\n",
        "/** \tkeep\r\n * star */ /* left *//*\u{a0}right */",
    );
    server.documents.insert(
        uri.clone(),
        Document::new(1, 1, "rust".into(), source.into()),
    );
    let params = |position: Position| {
        parse(json!({"textDocument": {"uri": uri}, "position": position})).unwrap()
    };
    let finish = |server: &mut Server<RecordingProvider>, body: &str| {
        let completion = loop {
            if let WorkerEvent::Finished(completion) =
                completions.recv_timeout(Duration::from_secs(2)).unwrap()
            {
                break completion;
            }
        };
        assert_eq!(recorded.recv().unwrap(), (body.to_owned(), "ja".into()));
        server.finish(&connection, completion).unwrap();
        let Message::Response(response) = client.receiver.try_recv().unwrap() else {
            panic!("Expected Hover response");
        };
        assert!(
            response.response_result.unwrap()["contents"]["value"]
                .as_str()
                .unwrap()
                .contains(&format!("fake: {body}"))
        );
    };
    // Indentation before a comment token cannot start work.
    for position in [Position::new(0, 1), Position::new(1, 0)] {
        assert_eq!(
            server.hover(RequestId::from(1), params(position)).unwrap(),
            Some(Value::Null)
        );
    }
    assert!(
        server
            .hover(RequestId::from(2), params(Position::new(0, 14)))
            .unwrap()
            .is_none()
    );
    finish(&mut server, "\t`id` 🚀  \r\n second");
    assert!(
        server
            .hover(RequestId::from(3), params(Position::new(2, 0)))
            .unwrap()
            .is_none()
    );
    finish(&mut server, "\tkeep\r\n * star ");
    let left_start = source.find("/* left").unwrap();
    let left_range = server
        .documents
        .get_mut(&uri)
        .unwrap()
        .comment_range(&uri, left_start)
        .unwrap();
    assert!(
        server
            .hover(RequestId::from(4), params(left_range.start))
            .unwrap()
            .is_none()
    );
    finish(&mut server, "left ");
    assert!(
        server
            .hover(RequestId::from(5), params(left_range.end))
            .unwrap()
            .is_none()
    );
    finish(&mut server, "\u{a0}right ");
    assert_eq!(server.documents[&uri].text(), Some(source));
    assert!(server.scheduler.pending.is_empty());
    assert!(server.scheduler.translations.is_empty());
}
#[test]
fn stale_completion_ticket_cannot_answer_a_reused_hover_request_id() {
    let (connection, client) = Connection::memory();
    let (finished, _completions) = bounded(MAX_JOBS);
    let mut server = Server::new(
        MockOptions {
            delay: Duration::from_secs(30),
            ..Default::default()
        },
        finished,
    )
    .unwrap();
    let uri: Uri = "file:///ticket.rs".parse().unwrap();
    server.documents.insert(
        uri.clone(),
        Document::new(
            1,
            1,
            "rust".into(),
            include_str!("../../../../../samples/comment.rs").into(),
        ),
    );
    let params = || {
        parse(json!({
            "textDocument": { "uri": uri },
            "position": { "line": 1, "character": 10 }
        }))
        .unwrap()
    };
    let id = RequestId::from("reused".to_owned());
    assert!(server.hover(id.clone(), params()).unwrap().is_none());
    let old_ticket = server.scheduler.pending[&id].ticket;
    server
        .cancel_range(&connection, old_ticket, ErrorCode::RequestCanceled)
        .unwrap();
    let Message::Response(canceled) = client.receiver.try_recv().unwrap() else {
        panic!("Expected cancellation response");
    };
    assert!(canceled.response_result.is_err());
    assert!(server.hover(id.clone(), params()).unwrap().is_none());
    let new_ticket = server.scheduler.pending[&id].ticket;
    assert_ne!(old_ticket, new_ticket);
    server
        .finish(
            &connection,
            Finished {
                ticket: old_ticket,
                result: Ok(Outcome::Translation(Translated {
                    text: "stale result".into(),
                    context: Context::Mock,
                })),
            },
        )
        .unwrap();
    assert!(client.receiver.try_recv().is_err());
    assert!(server.cache.latest(&uri).is_none());
    assert_eq!(server.scheduler.pending[&id].ticket, new_ticket);
    server
        .finish(
            &connection,
            Finished {
                ticket: new_ticket,
                result: Ok(Outcome::Translation(Translated {
                    text: "current result".into(),
                    context: Context::Mock,
                })),
            },
        )
        .unwrap();
    let Message::Response(response) = client.receiver.try_recv().unwrap() else {
        panic!("Expected retained Hover response");
    };
    assert_eq!(response.id, id);
    let value = response.response_result.unwrap();
    assert!(
        value["contents"]["value"]
            .as_str()
            .unwrap()
            .contains("current result")
    );
    assert!(!value.to_string().contains("stale result"));
    assert!(server.scheduler.pending.is_empty());
    assert!(server.scheduler.translations.is_empty());
}

#[test]
fn provider_capabilities_drive_commands_and_account_actions() {
    use crate::providers::{Capabilities, ProviderCommand};
    #[derive(Clone)]
    struct OtherProvider;
    impl TranslationProvider for OtherProvider {
        fn capabilities() -> Capabilities {
            Capabilities {
                server_name: "Synthetic provider",
                automatic_selection: false,
                account_menu: Some("Synthetic account…"),
                commands: &[ProviderCommand {
                    command: "synthetic.inspect",
                    title: "Inspect synthetic account",
                    changes_account: false,
                }],
            }
        }
        async fn translate(
            &self,
            _intent: crate::providers::Intent,
            text: &str,
            _: &str,
            context: Context,
        ) -> Result<Translated, ProviderError> {
            Ok(Translated {
                text: text.into(),
                context,
            })
        }
        async fn administer(
            &self,
            command: &str,
            _: &impl Interaction,
        ) -> Result<String, ProviderError> {
            assert_eq!(command, "synthetic.inspect");
            Ok("Synthetic account inspected.".into())
        }
    }
    let initialized = capabilities::<OtherProvider>();
    assert_eq!(initialized.server_info.unwrap().name, "Synthetic provider");
    assert_eq!(
        initialized
            .capabilities
            .execute_command_provider
            .unwrap()
            .commands,
        [
            COMMAND,
            DOCUMENT_COMMAND,
            CANCEL_COMMAND,
            CANCEL_ALL_COMMAND,
            "synthetic.inspect",
            SETTINGS_COMMAND
        ]
    );
    let (connection, client) = Connection::memory();
    let (finished, completions) = bounded(MAX_JOBS);
    let mut server = Server::configured(
        finished,
        OtherProvider,
        Settings::default(),
        Ui::new(connection.sender.clone(), UiCapabilities::default()),
    )
    .unwrap();
    let actions = server
        .code_actions(
            parse(json!({
                "textDocument":{"uri":"file:///synthetic.txt"},
                "range":{"start":{"line":0,"character":0},"end":{"line":0,"character":0}},
                "context":{"diagnostics":[]}
            }))
            .unwrap(),
        )
        .unwrap();
    assert_eq!(actions.len(), 1);
    assert_eq!(actions[0].title, "Synthetic account…");
    assert!(
        server
            .execute_command(
                &connection,
                1.into(),
                parse(json!({"command":"synthetic.inspect","arguments":[]})).unwrap()
            )
            .unwrap()
            .is_none()
    );
    let WorkerEvent::Finished(completion) =
        completions.recv_timeout(Duration::from_secs(3)).unwrap()
    else {
        panic!("Expected account completion")
    };
    server.finish(&connection, completion).unwrap();
    let Message::Notification(notice) = client.receiver.try_recv().unwrap() else {
        panic!("Expected account notice")
    };
    assert_eq!(notice.params["message"], "Synthetic account inspected.");
    let Message::Response(response) = client.receiver.try_recv().unwrap() else {
        panic!("Expected command response")
    };
    assert!(response.response_result.is_ok());
}
