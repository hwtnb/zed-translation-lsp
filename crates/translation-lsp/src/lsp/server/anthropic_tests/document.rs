//! How tab translations and range translations of one target share work.
use serde_json::{Value, json};

use super::{actions, next, target};
use crate::{
    lsp::server::*,
    providers::anthropic::{
        key::KeyId,
        test_support::{Fixture, Reply, completed},
    },
};

const TAB_MODEL: &str = "claude-sonnet-5-5";
const RANGE_TEXT: &str = "範囲の訳";
const TAB_TEXT: &str = "タブの訳";

type Editor = (
    Server<AnthropicProvider>,
    Connection,
    Connection,
    crossbeam_channel::Receiver<WorkerEvent<KeyId>>,
);

fn settings(separate: bool, work: Value) -> Settings {
    let mut provider = json!({"name": "anthropic", "effort": "medium"});
    if separate {
        provider["document"] = json!({"model": TAB_MODEL});
    }
    Settings::from_options(Some(json!({
        "provider": provider, "translation_limits": {"work": work}
    })))
    .unwrap()
}

/// The provider sends what `serve` resolves from the same settings.
fn editor(fixture: &Fixture, settings: Settings) -> Editor {
    let ProviderSettings::Anthropic {
        model,
        effort,
        thinking,
        speed,
        document,
    } = settings.provider.clone()
    else {
        panic!("Expected Anthropic settings")
    };
    let provider = fixture.provider().with_options(OutputOptions::new(
        AnthropicModel {
            model,
            effort,
            thinking,
            speed,
        },
        document,
    ));
    let (connection, client) = Connection::memory();
    let (finished, completions) = bounded(WORKER_EVENT_CHANNEL_CAPACITY);
    let server = Server::configured(
        finished,
        provider,
        settings,
        Ui::new(
            connection.sender.clone(),
            UiCapabilities {
                show_document: true,
                work_done_progress: true,
                ..Default::default()
            },
        ),
    )
    .unwrap();
    (server, connection, client, completions)
}

fn open(server: &mut Server<AnthropicProvider>, language: &str, source: &str) -> Uri {
    let uri: Uri = "file:///notes.txt".parse().unwrap();
    server.documents.insert(
        uri.clone(),
        Document::new(1, 1, language.into(), source.into()),
    );
    uri
}

fn line(line: u32) -> Range {
    Range::new(Position::new(line, 0), Position::new(line, 15))
}

fn execute(
    server: &mut Server<AnthropicProvider>,
    connection: &Connection,
    command: &str,
    argument: Value,
) -> Option<Value> {
    server
        .execute_command(
            connection,
            1.into(),
            ExecuteCommandParams {
                command: command.into(),
                arguments: vec![argument],
                work_done_progress_params: Default::default(),
            },
        )
        .unwrap()
}

fn translate(
    server: &mut Server<AnthropicProvider>,
    connection: &Connection,
    command: &str,
    target: &TranslationTarget,
) -> u64 {
    execute(server, connection, command, to_value(target).unwrap());
    let output = if command == DOCUMENT_COMMAND {
        Output::Document
    } else {
        Output::Range
    };
    server
        .scheduler
        .ticket(target, output, server.sharing())
        .unwrap()
}

/// Dispatched jobs wait here until the test hands them to the worker, so an
/// operation stays in flight for as long as the test needs it.
struct Held {
    worker: JobSender<KeyId>,
    jobs: JobReceiver<KeyId>,
    waiting: Vec<Job<KeyId>>,
}

impl Held {
    fn new(server: &mut Server<AnthropicProvider>) -> Self {
        let worker = server.jobs.take().unwrap();
        let (sender, jobs) = queue::channel(8);
        server.jobs = Some(sender);
        Self {
            worker,
            jobs,
            waiting: Vec::new(),
        }
    }

    fn dispatched(&mut self) -> Vec<u64> {
        while let Ok(job) = self.jobs.try_recv() {
            self.waiting.push(job);
        }
        self.waiting.iter().map(|job| job.ticket).collect()
    }

    fn release(&mut self, ticket: u64) {
        let index = self
            .dispatched()
            .iter()
            .position(|dispatched| *dispatched == ticket)
            .expect("The operation was dispatched");
        self.worker.try_send(self.waiting.remove(index)).unwrap();
    }
}

/// The model and options of each request the fixture received, in order.
fn sent(fixture: &Fixture) -> Vec<Value> {
    let state = fixture.state.lock().unwrap();
    state
        .records
        .iter()
        .map(|record| {
            let request: Value = serde_json::from_str(&record.body).unwrap();
            json!([
                request["model"],
                request["output_config"]["effort"],
                request["thinking"]["type"]
            ])
        })
        .collect()
}

/// The `max_tokens` of each request the fixture received, in order.
fn max_tokens(fixture: &Fixture) -> Vec<u64> {
    let state = fixture.state.lock().unwrap();
    state
        .records
        .iter()
        .map(|record| {
            let request: Value = serde_json::from_str(&record.body).unwrap();
            request["max_tokens"].as_u64().unwrap()
        })
        .collect()
}

fn range_sent() -> Value {
    json!(["claude-haiku-5-5", "medium", "disabled"])
}

fn tab_sent() -> Value {
    json!([TAB_MODEL, "low", "adaptive"])
}

/// The contents of each tab the server asked the editor to open.
fn opened(messages: &[Message]) -> Vec<String> {
    messages
        .iter()
        .filter_map(|message| match message {
            Message::Request(request) if request.method == "window/showDocument" => {
                let path = reqwest::Url::parse(request.params["uri"].as_str().unwrap())
                    .unwrap()
                    .to_file_path()
                    .unwrap();
                Some(std::fs::read_to_string(path).unwrap())
            }
            _ => None,
        })
        .collect()
}

fn ready(messages: &[Message]) -> usize {
    messages
        .iter()
        .filter(|message| {
            matches!(message, Message::Notification(notice)
                if notice.params["message"].as_str().is_some_and(|text| text.contains(TRANSLATION_READY)))
        })
        .count()
}

fn hover(server: &mut Server<AnthropicProvider>, uri: &Uri) -> Value {
    server
        .hover(
            9.into(),
            parse(json!({"textDocument": {"uri": uri}, "position": {"line": 0, "character": 3}}))
                .unwrap(),
        )
        .unwrap()
        .unwrap()
}

fn titles(actions: &[CodeAction]) -> Vec<&str> {
    actions.iter().map(|action| action.title.as_str()).collect()
}

#[tokio::test(flavor = "current_thread")]
async fn completed_results_stay_with_the_output_that_requested_them() {
    for commands in [[COMMAND, DOCUMENT_COMMAND], [DOCUMENT_COMMAND, COMMAND]] {
        let fixture = Fixture::new().await;
        let (mut server, connection, client, completions) =
            editor(&fixture, settings(true, json!({})));
        let uri = open(&mut server, "plaintext", "Alpha sentence.");
        let target = target(&server, &uri, line(0));
        let text = |command| {
            if command == DOCUMENT_COMMAND {
                TAB_TEXT
            } else {
                RANGE_TEXT
            }
        };
        for (index, command) in commands.into_iter().enumerate() {
            // The other output's completed result does not answer this request.
            fixture.reply(Reply::stream(completed(text(command))));
            translate(&mut server, &connection, command, &target);
            assert_eq!(server.scheduler.translations().len(), 1);
            server
                .finish(&connection, next(&completions).await)
                .unwrap();
            assert_eq!(fixture.calls(), index + 1);
        }
        let expected = |command| {
            if command == DOCUMENT_COMMAND {
                tab_sent()
            } else {
                range_sent()
            }
        };
        assert_eq!(sent(&fixture), commands.map(expected));
        let messages: Vec<_> = client.receiver.try_iter().collect();
        assert_eq!(opened(&messages), [TAB_TEXT]);
        assert_eq!(ready(&messages), 1);
        let shown = hover(&mut server, &uri);
        let shown = shown["contents"]["value"].as_str().unwrap();
        assert!(shown.contains(RANGE_TEXT), "{shown}");
        assert!(!shown.contains(TAB_TEXT), "{shown}");

        // Each output reuses its own result without another request.
        for command in commands {
            execute(
                &mut server,
                &connection,
                command,
                to_value(&target).unwrap(),
            );
        }
        assert!(server.scheduler.translations().is_empty());
        assert_eq!(fixture.calls(), 2);
        let messages: Vec<_> = client.receiver.try_iter().collect();
        assert_eq!(opened(&messages), [TAB_TEXT]);
        assert_eq!(ready(&messages), 1);
    }
}

#[tokio::test(flavor = "current_thread")]
async fn a_tab_and_a_range_for_one_target_run_as_two_operations() {
    let fixture = Fixture::new().await;
    let (mut server, connection, client, completions) = editor(
        &fixture,
        settings(true, json!({"concurrent": 2, "documents": 1})),
    );
    let uri = open(&mut server, "plaintext", "Alpha sentence.\nBravo sentence.");
    let mut held = Held::new(&mut server);
    let shared = target(&server, &uri, line(0));
    let range = translate(&mut server, &connection, COMMAND, &shared);
    let tab = translate(&mut server, &connection, DOCUMENT_COMMAND, &shared);
    assert_ne!(range, tab);
    assert_eq!(server.scheduler.admitted(), 2);

    // Both execution slots are taken, so another range waits for one.
    let other = target(&server, &uri, line(1));
    let waiting = translate(&mut server, &connection, COMMAND, &other);
    assert_eq!(held.dispatched(), [range, tab]);
    assert!(server.scheduler.translations()[&waiting].started.is_none());

    // A repeated request joins the operation of its own output.
    assert_eq!(translate(&mut server, &connection, COMMAND, &shared), range);
    assert_eq!(
        translate(&mut server, &connection, DOCUMENT_COMMAND, &shared),
        tab
    );
    assert_eq!(server.scheduler.admitted(), 3);
    let work = &server.scheduler.translations();
    assert!(work[&range].owners.manual() && !work[&range].owners.tab());
    assert!(work[&tab].owners.tab() && !work[&tab].owners.range_output());
    assert_eq!(fixture.calls(), 0);

    // The range completes while the tab is still in flight.
    fixture.reply(Reply::stream(completed(RANGE_TEXT)));
    held.release(range);
    server
        .finish(&connection, next(&completions).await)
        .unwrap();
    assert!(server.scheduler.translations().contains_key(&tab));
    assert_eq!(held.dispatched(), [tab, waiting]);
    let messages: Vec<_> = client.receiver.try_iter().collect();
    assert!(opened(&messages).is_empty());
    assert_eq!(ready(&messages), 1);
    assert!(
        hover(&mut server, &uri)["contents"]["value"]
            .as_str()
            .unwrap()
            .contains(RANGE_TEXT)
    );

    fixture.reply(Reply::stream(completed(TAB_TEXT)));
    held.release(tab);
    server
        .finish(&connection, next(&completions).await)
        .unwrap();
    let messages: Vec<_> = client.receiver.try_iter().collect();
    assert_eq!(opened(&messages), [TAB_TEXT]);
    assert_eq!(ready(&messages), 0);
    assert_eq!(sent(&fixture), [range_sent(), tab_sent()]);
    assert_eq!(
        server
            .scheduler
            .translations()
            .keys()
            .copied()
            .collect::<Vec<_>>(),
        [waiting]
    );
}

#[tokio::test(flavor = "current_thread")]
async fn each_operation_of_one_target_has_its_own_cancel_action() {
    for cancel_tab in [true, false] {
        let fixture = Fixture::new().await;
        let (mut server, connection, client, completions) = editor(
            &fixture,
            settings(true, json!({"concurrent": 2, "documents": 1})),
        );
        let uri = open(&mut server, "plaintext", "Alpha sentence.");
        let mut held = Held::new(&mut server);
        let shared = target(&server, &uri, line(0));
        let range = translate(&mut server, &connection, COMMAND, &shared);
        let tab = translate(&mut server, &connection, DOCUMENT_COMMAND, &shared);

        let offered = actions(&mut server, &uri, line(0));
        let cancels: Vec<_> = offered
            .iter()
            .filter_map(|action| action.command.as_ref())
            .filter(|command| command.command == CANCEL_COMMAND)
            .map(|command| (command.title.as_str(), command.arguments.clone().unwrap()))
            .collect();
        assert_eq!(
            cancels,
            [
                ("Cancel translation of lines 1-1", vec![json!(range)]),
                ("Cancel tab translation of lines 1-1", vec![json!(tab)]),
            ]
        );
        assert!(titles(&offered).contains(&"Cancel all translations"));

        let (canceled, kept) = if cancel_tab {
            (tab, range)
        } else {
            (range, tab)
        };
        execute(&mut server, &connection, CANCEL_COMMAND, json!(canceled));
        assert_eq!(
            server
                .scheduler
                .translations()
                .keys()
                .copied()
                .collect::<Vec<_>>(),
            [kept]
        );
        let remaining = actions(&mut server, &uri, line(0));
        assert_eq!(
            titles(&remaining).contains(&"Cancel tab translation of lines 1-1"),
            !cancel_tab
        );
        assert_eq!(
            titles(&remaining).contains(&"Cancel translation of lines 1-1"),
            cancel_tab
        );

        // The kept operation completes, and the canceled one sends nothing.
        fixture.reply(Reply::stream(completed(if cancel_tab {
            RANGE_TEXT
        } else {
            TAB_TEXT
        })));
        held.release(canceled);
        held.release(kept);
        for _ in 0..2 {
            server
                .finish(&connection, next(&completions).await)
                .unwrap();
        }
        assert_eq!(
            sent(&fixture),
            [if cancel_tab { range_sent() } else { tab_sent() }]
        );
        let messages: Vec<_> = client.receiver.try_iter().collect();
        if cancel_tab {
            assert!(opened(&messages).is_empty());
            assert_eq!(ready(&messages), 1);
            assert!(server.cache.exact(&uri, line(0), Output::Range).is_some());
            assert!(
                server
                    .cache
                    .exact(&uri, line(0), Output::Document)
                    .is_none()
            );
        } else {
            assert_eq!(opened(&messages), [TAB_TEXT]);
            assert_eq!(ready(&messages), 0);
            assert!(server.cache.exact(&uri, line(0), Output::Range).is_none());
            assert!(
                server
                    .cache
                    .exact(&uri, line(0), Output::Document)
                    .is_some()
            );
        }
        assert!(server.scheduler.translations().is_empty());
    }
}

/// The text of the progress report the editor shows while work is in flight.
fn progress(server: &mut Server<AnthropicProvider>, client: &Connection) -> String {
    server.publish_activity();
    let created = client
        .receiver
        .try_iter()
        .find_map(|message| match message {
            Message::Request(request) if request.method == "window/workDoneProgress/create" => {
                Some(request.id)
            }
            _ => None,
        })
        .unwrap();
    assert!(
        server
            .activity
            .response(&Response::new_ok(created, Value::Null))
    );
    server.publish_activity();
    client
        .receiver
        .try_iter()
        .find_map(|message| match message {
            Message::Notification(notice) if notice.method == "$/progress" => {
                notice.params["value"]["message"]
                    .as_str()
                    .map(str::to_owned)
            }
            _ => None,
        })
        .unwrap()
}

#[tokio::test(flavor = "current_thread")]
async fn automatic_selection_does_not_take_a_tab_operation_for_its_own() {
    for separate in [true, false] {
        let fixture = Fixture::new().await;
        // Two accepted operations fill the queue.
        let (mut server, connection, client, _completions) = editor(
            &fixture,
            settings(
                separate,
                json!({"admitted": 2, "automatic": 1, "concurrent": 1, "documents": 1}),
            ),
        );
        let uri = open(
            &mut server,
            "markdown",
            "Alpha sentence.\n\nBravo sentence.",
        );
        let _held = Held::new(&mut server);
        let selected = target(&server, &uri, line(0));
        let tab = translate(&mut server, &connection, DOCUMENT_COMMAND, &selected);
        let other = target(&server, &uri, line(2));
        let manual = translate(&mut server, &connection, COMMAND, &other);

        actions(&mut server, &uri, line(0));
        let candidate = server.selection.as_ref().unwrap();
        assert!(candidate.target == selected && candidate.start.is_some());
        // Without tab settings the selection would join the tab's operation.
        // With them it still needs a place of its own in the full queue.
        assert_eq!(server.range_ticket(&selected), (!separate).then_some(tab));
        assert_eq!(server.can_start_selection(candidate), !separate);
        assert_eq!(
            progress(&mut server, &client),
            if separate {
                "Queued: 2; selection not yet accepted"
            } else {
                "Queued: 2"
            }
        );
        if !separate {
            continue;
        }

        // Once a place frees up, the selection starts its own range operation.
        server
            .cancel_range(&connection, manual, ErrorCode::RequestCanceled)
            .unwrap();
        assert!(server.can_start_selection(server.selection.as_ref().unwrap()));
        server.selection.as_mut().unwrap().start = Some(Instant::now());
        server.start_selection();
        let automatic = server.range_ticket(&selected).unwrap();
        assert_ne!(automatic, tab);
        assert_eq!(server.adjusting_ticket, Some(automatic));
        let work = &server.scheduler.translations();
        assert_eq!(work.len(), 2);
        assert!(work[&automatic].owners.selection() && !work[&automatic].owners.tab());
        assert!(work[&tab].owners.tab() && !work[&tab].owners.range_output());
    }
}

#[tokio::test(flavor = "current_thread")]
async fn a_failed_range_can_be_retried_while_its_tab_is_in_flight() {
    for separate in [true, false] {
        let fixture = Fixture::new().await;
        let (mut server, connection, _client, completions) = editor(
            &fixture,
            settings(separate, json!({"concurrent": 2, "documents": 1})),
        );
        let uri = open(&mut server, "plaintext", "Alpha sentence.");
        let failed = target(&server, &uri, line(0));
        fixture.reply(Reply::failure(400, "invalid_request_error", "Rejected"));
        translate(&mut server, &connection, COMMAND, &failed);
        server
            .finish(&connection, next(&completions).await)
            .unwrap();

        let _held = Held::new(&mut server);
        let tab = translate(&mut server, &connection, DOCUMENT_COMMAND, &failed);
        let cursor = Range::new(Position::new(0, 3), Position::new(0, 3));
        let offered = actions(&mut server, &uri, cursor);
        let retry = offered
            .iter()
            .filter_map(|action| action.command.as_ref())
            .find(|command| command.command == RETRY_COMMAND);
        // Without tab settings, the tab's operation is already translating this range.
        assert_eq!(retry.is_some(), separate, "{:?}", titles(&offered));
        let Some(retry) = retry else {
            continue;
        };
        assert_eq!(retry.title, "Retry translation of lines 1-1");
        execute(
            &mut server,
            &connection,
            RETRY_COMMAND,
            retry.arguments.clone().unwrap().remove(0),
        );
        let range = server.range_ticket(&failed).unwrap();
        assert_ne!(range, tab);
        assert_eq!(server.scheduler.translations().len(), 2);
        assert!(
            !titles(&actions(&mut server, &uri, cursor))
                .contains(&"Retry translation of lines 1-1")
        );
    }
}

#[tokio::test(flavor = "current_thread")]
async fn a_tab_asks_for_its_own_output_unless_the_range_was_already_sent() {
    // Each line has 15 bytes, so a range request asks for 16,384 + 15 * 4 tokens.
    const RANGE: u64 = 16_444;
    const TAB: u64 = 64_000;
    for sent in [true, false] {
        let fixture = Fixture::new().await;
        let (mut server, connection, client, completions) = editor(
            &fixture,
            settings(false, json!({"concurrent": 1, "documents": 1})),
        );
        let uri = open(&mut server, "plaintext", "Alpha sentence.\nBravo sentence.");
        let mut held = Held::new(&mut server);
        let shared = target(&server, &uri, line(0));
        // Otherwise another range takes the only slot, so this range waits unsent.
        let first = if sent {
            None
        } else {
            let other = target(&server, &uri, line(1));
            Some(translate(&mut server, &connection, COMMAND, &other))
        };
        let range = translate(&mut server, &connection, COMMAND, &shared);
        let tab = translate(&mut server, &connection, DOCUMENT_COMMAND, &shared);
        assert_eq!(tab == range, !sent);
        for (ticket, text) in [
            (first.unwrap_or(range), RANGE_TEXT),
            (if sent { tab } else { range }, TAB_TEXT),
        ] {
            fixture.reply(Reply::stream(completed(text)));
            held.release(ticket);
            server
                .finish(&connection, next(&completions).await)
                .unwrap();
        }
        assert!(server.scheduler.translations().is_empty());
        assert_eq!(max_tokens(&fixture), [RANGE, TAB], "sent {sent}");
        let messages: Vec<_> = client.receiver.try_iter().collect();
        assert_eq!(opened(&messages), [TAB_TEXT]);
    }
}
