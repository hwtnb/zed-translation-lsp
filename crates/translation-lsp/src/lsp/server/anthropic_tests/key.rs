//! The key is saved, replaced, and removed while the server runs. What was
//! translated, refused, or stopped under one key is not kept for another.
use std::{fs, sync::Arc};

use serde_json::{Value, json};
use tokio::sync::Semaphore;

use super::{actions, next, server, target};
use crate::{
    lsp::server::*,
    providers::{
        AccountChange, TranslationProvider,
        anthropic::test_support::{Fixture, KEY, KeyDirectory, Reply, Terminal},
    },
};

const NEW_KEY: &str = "sk-ant-synthetic-replacing-key";
const MENU: &str = "Anthropic API key…";
const SAVE: &str = "translation-lsp.save-anthropic-key";
const REMOVE: &str = "translation-lsp.remove-anthropic-key";
const MISSING: &str =
    "Anthropic API key is not saved. Run \"Anthropic API key…\" from the code actions to save it";
const SOURCE: &str = "Alpha sentence.\n\nBravo sentence.\n\nThird sentence.";

fn line(line: u32) -> Range {
    Range::new(Position::new(line, 0), Position::new(line, 15))
}

fn open(server: &mut Server<AnthropicProvider>, name: &str, language: &str, text: &str) -> Uri {
    let uri: Uri = format!("file:///{name}").parse().unwrap();
    server.documents.insert(
        uri.clone(),
        Document::new(1, 1, language.into(), text.into()),
    );
    uri
}

fn titles(server: &mut Server<AnthropicProvider>, uri: &Uri, range: Range) -> Vec<String> {
    actions(server, uri, range)
        .into_iter()
        .map(|action| action.title)
        .collect()
}

fn hover(uri: &Uri, line: u32) -> HoverParams {
    parse(json!({"textDocument": {"uri": uri}, "position": {"line": line, "character": 4}}))
        .unwrap()
}

/// The key the last request to the fixture carried.
fn sent(fixture: &Fixture) -> String {
    let state = fixture.state.lock().unwrap();
    let key = &state.records.last().unwrap().headers["x-api-key"];
    key.to_str().unwrap().to_owned()
}

/// Waits until the fixture has received this many requests. The limit only ends
/// a test that would otherwise hang.
async fn received(fixture: &Fixture, calls: usize) {
    tokio::time::timeout(Duration::from_secs(30), async {
        while fixture.calls() < calls {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("The request did not reach the fixture");
}

async fn message(client: &Connection) -> Message {
    let receiver = client.receiver.clone();
    tokio::task::spawn_blocking(move || receiver.recv_timeout(Duration::from_secs(30)).unwrap())
        .await
        .unwrap()
}

/// Holds every response of the fixture until the test lets one go.
fn gate(fixture: &Fixture) -> Arc<Semaphore> {
    let gate = Arc::new(Semaphore::new(0));
    fixture.state.lock().unwrap().gate = Some(gate.clone());
    gate
}

#[tokio::test(flavor = "current_thread")]
async fn without_a_usable_key_the_code_actions_offer_only_the_key_menu() {
    let fixture = Fixture::new().await;
    let directory = KeyDirectory::new();
    let provider = AnthropicProvider::synthetic(format!("{}/v1", fixture.base), &directory.path);
    let (mut server, _connection, _client, _completions) = server(provider, Settings::default());
    let comment = open(&mut server, "missing-key.rs", "rust", "// Read this.");
    let range = Range::new(Position::new(0, 0), Position::new(0, 13));
    let markdown = open(&mut server, "missing-key.md", "markdown", SOURCE);

    for contents in [None, Some(""), Some("not a key")] {
        if let Some(contents) = contents {
            directory.replace(contents);
        }
        assert_eq!(titles(&mut server, &comment, range), [MENU]);
        assert_eq!(titles(&mut server, &markdown, line(0)), [MENU]);
        // A selection is no candidate for automatic translation without a key.
        assert!(server.selection.is_none());
    }
    fs::remove_file(directory.file()).unwrap();

    // A command the editor still holds is answered with where to save the key.
    let target = target(&server, &comment, range);
    let explicit = server
        .queue(1.into(), target, PendingReply::Command)
        .unwrap_err();
    let hovered = server.hover(2.into(), hover(&comment, 0)).unwrap_err();
    for error in [explicit, hovered] {
        assert_eq!(error.code, ErrorCode::RequestFailed as i32);
        assert_eq!(error.message, MISSING);
    }
    assert!(server.scheduler.translations.is_empty());
    assert_eq!(fixture.calls(), 0);
    assert!(directory.names().is_empty());

    // The key saved by the command in the terminal is in use at the next request.
    directory.replace(NEW_KEY);
    assert_eq!(
        titles(&mut server, &comment, range),
        ["Translate selection to Japanese (ja)", MENU]
    );
    titles(&mut server, &markdown, line(0));
    assert!(server.selection.is_some());
}

#[tokio::test(flavor = "current_thread")]
async fn key_menu_opens_the_terminal_without_disturbing_work_in_progress() {
    let fixture = Fixture::new().await;
    let gate = gate(&fixture);
    let terminal = Terminal::default();
    let (mut server, connection, client, completions) = server(
        fixture.provider().with_terminal(terminal.clone()),
        Settings::default(),
    );
    let uri = open(&mut server, "menu.rs", "rust", "// Read this.");
    assert!(server.hover(1.into(), hover(&uri, 0)).unwrap().is_none());
    received(&fixture, 1).await;

    assert!(
        server
            .execute_command(
                &connection,
                2.into(),
                parse(json!({"command": SETTINGS_COMMAND})).unwrap(),
            )
            .unwrap()
            .is_none()
    );
    let Message::Request(picker) = message(&client).await else {
        panic!("Expected the menu")
    };
    assert_eq!(picker.method, "window/showMessageRequest");
    assert_eq!(
        picker.params["actions"],
        json!([
            {"title": "Replace Anthropic API key"},
            {"title": "Remove Anthropic API key"}
        ])
    );
    server.ui.response(Response::new_ok(
        picker.id,
        json!({"title": "Replace Anthropic API key"}),
    ));
    server
        .finish(&connection, next(&completions).await)
        .unwrap();
    assert_eq!(server.administrations[&2.into()].command, SAVE);
    server
        .finish(&connection, next(&completions).await)
        .unwrap();
    assert!(server.administrations.is_empty());

    // The command ends once the window is open. It does not wait for the key.
    let Message::Notification(notice) = message(&client).await else {
        panic!("Expected the notice")
    };
    assert_eq!(notice.method, "window/showMessage");
    assert_eq!(notice.params["type"], 3);
    let text = notice.params["message"].as_str().unwrap();
    assert!(
        text.starts_with("A terminal window was opened. Enter the Anthropic API key there."),
        "{text}"
    );
    assert!(text.ends_with(" anthropic-key"), "{text}");
    assert!(!text.contains(KEY));
    let Message::Response(response) = message(&client).await else {
        panic!("Expected the command response")
    };
    assert_eq!(response.id, 2.into());
    assert_eq!(response.response_result.unwrap(), Value::Null);
    assert_eq!(
        *terminal.opened.lock().unwrap(),
        [std::env::current_exe().unwrap()]
    );

    // The translation that was running is still wanted, and still arrives.
    gate.add_permits(1);
    server
        .finish(&connection, next(&completions).await)
        .unwrap();
    let Message::Response(response) = message(&client).await else {
        panic!("Expected the Hover response")
    };
    assert_eq!(response.id, 1.into());
    assert!(
        response.response_result.unwrap()["contents"]["value"]
            .as_str()
            .unwrap()
            .contains("訳Read this.")
    );
    assert_eq!(fixture.calls(), 1);
    assert_eq!(fs::read(fixture.keys.file()).unwrap(), KEY.as_bytes());
}

#[tokio::test(flavor = "current_thread")]
async fn removing_the_key_cancels_work_and_discards_previews_only_once_confirmed() {
    assert_eq!(
        Server::<AnthropicProvider>::account_change(SAVE),
        AccountChange::Never
    );
    assert_eq!(
        Server::<AnthropicProvider>::account_change(REMOVE),
        AccountChange::IfConfirmed
    );

    let fixture = Fixture::new().await;
    let (mut server, connection, client, completions) =
        server(fixture.provider(), Settings::default());
    let uri = open(
        &mut server,
        "remove.rs",
        "rust",
        "// Read this.\n\n// Read that.",
    );
    let range = Range::new(Position::new(0, 0), Position::new(0, 13));
    // One comment is translated, and the other is with the API.
    assert!(server.hover(1.into(), hover(&uri, 0)).unwrap().is_none());
    server
        .finish(&connection, next(&completions).await)
        .unwrap();
    assert!(matches!(message(&client).await, Message::Response(_)));
    let gate = gate(&fixture);
    assert!(server.hover(2.into(), hover(&uri, 2)).unwrap().is_none());
    received(&fixture, 2).await;
    fs::write(fixture.keys.path.join("auth.json"), "synthetic").unwrap();

    let remove = |server: &mut Server<AnthropicProvider>, id: i32| {
        assert!(
            server
                .execute_command(
                    &connection,
                    id.into(),
                    parse(json!({"command": REMOVE})).unwrap(),
                )
                .unwrap()
                .is_none()
        );
    };
    let confirmation = |message| {
        let Message::Request(confirmation) = message else {
            panic!("Expected the confirmation")
        };
        assert_eq!(confirmation.method, "window/showMessageRequest");
        assert_eq!(
            confirmation.params["message"],
            concat!(
                "Remove the saved Anthropic API key from this computer? The key stays valid ",
                "until you delete it in the Claude Console."
            )
        );
        assert_eq!(confirmation.params["actions"], json!([{"title": "Remove"}]));
        confirmation.id
    };
    let notice = |message| {
        let Message::Notification(notice) = message else {
            panic!("Expected the notice")
        };
        assert_eq!(notice.method, "window/showMessage");
        assert_eq!(notice.params["type"], 3);
        notice.params["message"].as_str().unwrap().to_owned()
    };

    // Closing the question leaves the key, the preview, and the work as they were.
    remove(&mut server, 3);
    let asked = confirmation(message(&client).await);
    assert_eq!(fs::read(fixture.keys.file()).unwrap(), KEY.as_bytes());
    server.ui.response(Response::new_ok(asked, Value::Null));
    server
        .finish(&connection, next(&completions).await)
        .unwrap();
    assert_eq!(
        notice(message(&client).await),
        "The Anthropic API key was not removed."
    );
    let Message::Response(response) = message(&client).await else {
        panic!("Expected the command response")
    };
    assert_eq!(response.id, 3.into());
    assert!(fixture.keys.file().exists());
    assert!(server.cache.latest(&uri).is_some());
    assert_eq!(server.scheduler.translations.len(), 1);
    assert_eq!(server.scheduler.pending.len(), 1);
    fixture.open.wait(0).await;

    // Agreeing removes the key. The work in progress is cancelled then, and the
    // preview is discarded, as when signing out of ChatGPT.
    remove(&mut server, 4);
    let asked = confirmation(message(&client).await);
    assert!(server.cache.latest(&uri).is_some());
    server
        .ui
        .response(Response::new_ok(asked, json!({"title": "Remove"})));
    server
        .finish(&connection, next(&completions).await)
        .unwrap();
    let Message::Response(cancelled) = message(&client).await else {
        panic!("Expected the Hover that was waiting to be answered")
    };
    assert_eq!(cancelled.id, 2.into());
    assert_eq!(
        cancelled.response_result.unwrap_err().code,
        ErrorCode::RequestCanceled as i32
    );
    assert_eq!(
        notice(message(&client).await),
        "The Anthropic API key was removed."
    );
    let Message::Response(response) = message(&client).await else {
        panic!("Expected the command response")
    };
    assert_eq!(response.id, 4.into());
    assert!(server.cache.latest(&uri).is_none());
    assert!(server.scheduler.translations.is_empty());
    // The worker drops the request it had open.
    let dropped = next(&completions).await;
    assert!(dropped.error().is_some());
    server.finish(&connection, dropped).unwrap();
    gate.add_permits(1);

    // Only the key file is gone.
    assert_eq!(fixture.keys.names(), ["auth.json"]);
    assert_eq!(
        fs::read(fixture.keys.path.join("auth.json")).unwrap(),
        b"synthetic"
    );
    assert_eq!(titles(&mut server, &uri, range), [MENU]);
    let error = server.hover(5.into(), hover(&uri, 0)).unwrap_err();
    assert_eq!(error.message, MISSING);
    assert!(client.receiver.try_recv().is_err());
    assert_eq!(fixture.calls(), 2);
}

#[tokio::test(flavor = "current_thread")]
async fn translation_finished_after_the_key_was_replaced_is_not_published() {
    let fixture = Fixture::new().await;
    let gate = gate(&fixture);
    let (mut server, connection, client, completions) =
        server(fixture.provider(), Settings::default());
    let uri = open(&mut server, "replaced.rs", "rust", "// Read this.");
    assert!(server.hover(1.into(), hover(&uri, 0)).unwrap().is_none());
    received(&fixture, 1).await;
    assert_eq!(sent(&fixture), KEY);

    // The request is with the API when another key takes the place of its key.
    fixture.keys.replace(NEW_KEY);
    gate.add_permits(1);
    let completion = next(&completions).await;
    assert!(completion.is_ok(), "The API answered the request it had");
    server.finish(&connection, completion).unwrap();
    let Message::Response(response) = message(&client).await else {
        panic!("Expected the Hover response")
    };
    let error = response.response_result.unwrap_err();
    assert_eq!(error.code, ErrorCode::ContentModified as i32);
    assert_eq!(
        error.message,
        "Translation authorization changed during translation"
    );
    assert!(server.cache.latest(&uri).is_none());
    assert!(
        server
            .cache
            .failed_range(&uri, Position::new(0, 4))
            .is_none()
    );
    // Nothing is shown for it either.
    assert!(client.receiver.try_recv().is_err());

    // The same comment is translated again, with the key now saved.
    assert!(server.hover(2.into(), hover(&uri, 0)).unwrap().is_none());
    received(&fixture, 2).await;
    assert_eq!(sent(&fixture), NEW_KEY);
    gate.add_permits(1);
    server
        .finish(&connection, next(&completions).await)
        .unwrap();
    let Message::Response(response) = message(&client).await else {
        panic!("Expected the Hover response")
    };
    assert!(response.response_result.is_ok());
    assert_eq!(
        server.cache.latest(&uri).unwrap().context,
        server.provider.context().unwrap()
    );
}

#[tokio::test(flavor = "current_thread")]
async fn work_queued_under_one_key_is_dropped_when_it_starts_under_another() {
    let fixture = Fixture::new().await;
    let (mut server, connection, client, completions) =
        server(fixture.provider(), Settings::default());
    let uri = open(&mut server, "queued.rs", "rust", "// Read this.");
    // Keep the job from the worker until the key has been replaced.
    let worker_jobs = server.jobs.take().unwrap();
    let (hold, mut held) = queue::channel(1);
    server.jobs = Some(hold);
    assert!(server.hover(1.into(), hover(&uri, 0)).unwrap().is_none());
    fixture.keys.replace(NEW_KEY);
    worker_jobs.try_send(held.recv().await.unwrap()).unwrap();
    server.jobs = Some(worker_jobs);

    server
        .finish(&connection, next(&completions).await)
        .unwrap();
    let Message::Response(response) = message(&client).await else {
        panic!("Expected the Hover response")
    };
    let error = response.response_result.unwrap_err();
    assert_eq!(error.code, ErrorCode::ContentModified as i32);
    assert_eq!(
        error.message,
        "Anthropic API key changed; retry the translation"
    );
    assert!(client.receiver.try_recv().is_err());
    assert_eq!(fixture.calls(), 0);
}

#[tokio::test(flavor = "current_thread")]
async fn results_and_failures_kept_for_one_key_are_not_used_for_another() {
    let fixture = Fixture::new().await;
    let (mut server, connection, client, completions) =
        server(fixture.provider(), Settings::default());
    let uri = open(&mut server, "kept.md", "markdown", SOURCE);
    fixture.reply(Reply::stream(
        crate::providers::anthropic::test_support::completed("古いキーの訳"),
    ));
    fixture.reply(Reply::failure(400, "invalid_request_error", "Rejected"));
    for (id, index) in [(1, 0), (2, 2)] {
        let target = target(&server, &uri, line(index));
        server
            .queue(id.into(), target, PendingReply::Command)
            .unwrap();
        server
            .finish(&connection, next(&completions).await)
            .unwrap();
    }
    let cursor = |line| Range::new(Position::new(line, 3), Position::new(line, 3));
    assert!(server.cache.exact(&uri, line(0), Output::Range).is_some());
    assert!(
        titles(&mut server, &uri, cursor(2)).contains(&"Retry translation of lines 3-3".to_owned())
    );
    let shown = server.hover(3.into(), hover(&uri, 0)).unwrap().unwrap();
    assert!(
        shown["contents"]["value"]
            .as_str()
            .unwrap()
            .contains("古いキーの訳")
    );
    assert_eq!(fixture.calls(), 2);
    client.receiver.try_iter().count();

    fixture.keys.replace(NEW_KEY);
    // Neither the translation nor the failure is shown for the new key.
    assert_eq!(
        server.hover(4.into(), hover(&uri, 0)).unwrap(),
        Some(Value::Null)
    );
    assert_eq!(
        server.hover(5.into(), hover(&uri, 2)).unwrap(),
        Some(Value::Null)
    );
    assert!(
        !titles(&mut server, &uri, cursor(2))
            .iter()
            .any(|title| title.starts_with("Retry"))
    );
    assert!(server.cache.latest(&uri).is_none());
    assert!(server.cache.exact(&uri, line(0), Output::Range).is_none());

    // The same range is sent again, with the new key.
    let target = target(&server, &uri, line(0));
    server
        .queue(6.into(), target, PendingReply::Command)
        .unwrap();
    server
        .finish(&connection, next(&completions).await)
        .unwrap();
    assert_eq!(fixture.calls(), 3);
    assert_eq!(sent(&fixture), NEW_KEY);
    assert_eq!(
        server
            .cache
            .exact(&uri, line(0), Output::Range)
            .unwrap()
            .context,
        server.provider.context().unwrap()
    );
}

#[tokio::test(flavor = "current_thread")]
async fn automatic_translation_stopped_for_a_rejected_key_starts_again_with_the_next_key() {
    let fixture = Fixture::new().await;
    fixture.always(Some(Reply::failure(
        401,
        "authentication_error",
        "invalid x-api-key",
    )));
    let (mut server, connection, client, completions) =
        server(fixture.provider(), Settings::default());
    let uri = open(&mut server, "stopped.md", "markdown", SOURCE);
    let target = target(&server, &uri, line(0));
    server
        .queue(1.into(), target, PendingReply::Command)
        .unwrap();
    server
        .finish(&connection, next(&completions).await)
        .unwrap();
    let notice = client
        .receiver
        .try_iter()
        .find_map(|message| match message {
            Message::Notification(notice) => Some(notice),
            _ => None,
        })
        .unwrap();
    let text = notice.params["message"].as_str().unwrap();
    assert!(
        text.contains(concat!(
            "The API key was rejected. Run \"Anthropic API key…\" from the code actions to ",
            "replace it. Automatic translation is paused."
        )),
        "{text}"
    );
    assert!(!text.contains(KEY));

    // The selection is a candidate, and the stop keeps it from starting.
    let start = |server: &mut Server<AnthropicProvider>| {
        actions(server, &uri, line(2));
        server.selection.as_mut().unwrap().start = Some(Instant::now());
        server.start_selection();
    };
    start(&mut server);
    assert!(server.scheduler.translations.is_empty());
    assert_eq!(fixture.calls(), 1);

    fixture.keys.replace(NEW_KEY);
    fixture.always(None);
    start(&mut server);
    assert_eq!(server.scheduler.translations.len(), 1);
    server
        .finish(&connection, next(&completions).await)
        .unwrap();
    assert_eq!(fixture.calls(), 2);
    assert_eq!(sent(&fixture), NEW_KEY);
    assert!(server.cache.exact(&uri, line(2), Output::Range).is_some());
}
