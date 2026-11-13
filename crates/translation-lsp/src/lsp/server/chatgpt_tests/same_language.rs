//! Text that the provider finds already in the target language.
use std::sync::Arc;

use super::*;
use crate::{
    lsp::server::menu::RETRY,
    providers::prompt::{SAME_LANGUAGE, asks_for_languages},
};

/// A Rust comment of two Japanese lines, already in the target language.
const COMMENT: &str = "// これは日本語の文です。\n// 二つ目の文です。\nfn main() {}\n";

/// Opens [`COMMENT`] in `server`.
fn comment_document(server: &mut Server<ChatGptProvider>) -> Uri {
    let uri: Uri = "file:///same.rs".parse().unwrap();
    server.documents.insert(
        uri.clone(),
        Document::new(1, 1, "rust".into(), COMMENT.into()),
    );
    uri
}

/// The body of the comment's first line, as a target of its own.
fn first_line(uri: &Uri) -> TranslationTarget {
    let line = COMMENT.lines().next().unwrap();
    TranslationTarget {
        uri: uri.clone(),
        version: 1,
        session: 1,
        range: Range::new(
            Position::new(0, 3),
            Position::new(0, line.encode_utf16().count() as u32),
        ),
        target_language: "ja".into(),
    }
}

/// What `server` answers at once to Hover at `position`: `None` while the
/// answer waits for a translation.
fn hover_at(
    server: &mut Server<ChatGptProvider>,
    id: i32,
    uri: &Uri,
    position: Position,
) -> Option<Value> {
    server
        .hover(
            id.into(),
            parse(json!({"textDocument": {"uri": uri}, "position": position})).unwrap(),
        )
        .unwrap()
}

/// The messages that `client` has received since the last call.
fn received(client: &Connection) -> Vec<Message> {
    client.receiver.try_iter().collect()
}

fn ready(messages: &[Message]) -> usize {
    messages
        .iter()
        .filter(|m| matches!(m, Message::Notification(n) if n.params["message"].as_str().is_some_and(|m| m.contains(TRANSLATION_READY))))
        .count()
}

fn opened(messages: &[Message]) -> usize {
    messages
        .iter()
        .filter(|m| matches!(m, Message::Request(r) if r.method == "window/showDocument"))
        .count()
}

/// The result of the response to `id` in `messages`.
fn response(messages: &[Message], id: i32) -> Value {
    messages
        .iter()
        .find_map(|m| match m {
            Message::Response(r) if r.id == id.into() => Some(r.response_result.clone().unwrap()),
            _ => None,
        })
        .unwrap()
}

/// The instructions of each translation request the fixture received.
fn instructions(fixture: &Fixture) -> Vec<String> {
    fixture
        .state
        .lock()
        .unwrap()
        .records
        .iter()
        .filter(|record| record.path == "/v1/responses")
        .map(|record| {
            let request: Value = serde_json::from_str(&record.body).unwrap();
            request["instructions"].as_str().unwrap().to_owned()
        })
        .collect()
}

fn hover(target: &TranslationTarget) -> PendingReply {
    PendingReply::Hover(target.range.start, Instant::now())
}

#[tokio::test(flavor = "current_thread")]
async fn text_in_the_target_language_shows_nothing_until_an_explicit_request_or_an_edit() {
    let (fixture, mut server, connection, client, completions, uri) =
        range_server(Settings::default()).await;
    fixture.state.lock().unwrap().same_language = true;
    let target = range_target(&server, &uri, 0);
    server
        .queue(1.into(), target.clone(), hover(&target))
        .unwrap();
    server
        .finish(&connection, next(&completions).await)
        .unwrap();
    let messages = received(&client);
    assert_eq!(response(&messages, 1), Value::Null);
    assert_eq!(ready(&messages), 0);
    assert!(server.scheduler.translations().is_empty());
    assert_eq!(response_calls(&fixture), 1);
    assert!(asks_for_languages(&instructions(&fixture)[0]));
    // Automatic requests reuse the result without sending the text again.
    assert_eq!(
        server
            .queue(2.into(), target.clone(), hover(&target))
            .unwrap(),
        Some(Value::Null)
    );
    assert_eq!(
        server
            .queue_translation(target.clone(), TranslationRequest::Selection)
            .unwrap(),
        None
    );
    assert!(server.scheduler.translations().is_empty());
    assert_eq!(response_calls(&fixture), 1);
    // An explicit request sends it, and Hover then shows its translation.
    server
        .queue(
            3.into(),
            target.clone(),
            PendingReply::Command { notice: true },
        )
        .unwrap();
    server
        .finish(&connection, next(&completions).await)
        .unwrap();
    assert_eq!(ready(&received(&client)), 1);
    assert_eq!(response_calls(&fixture), 2);
    assert!(!asks_for_languages(&instructions(&fixture)[1]));
    let shown = server
        .queue(4.into(), target.clone(), hover(&target))
        .unwrap();
    assert!(shown.unwrap().to_string().contains("訳Alpha sentence."));
    assert_eq!(response_calls(&fixture), 2);
    // An edit discards the result, so the text is sent again.
    server.notification(&connection, lsp_server::Notification::new(DidChangeTextDocument::METHOD.into(), json!({"textDocument":{"uri":uri,"version":2},"contentChanges":[{"text":"Alpha sentence.\n\nBravo sentence."}]}))).unwrap();
    let target = range_target(&server, &uri, 0);
    server
        .queue(5.into(), target.clone(), hover(&target))
        .unwrap();
    server
        .finish(&connection, next(&completions).await)
        .unwrap();
    assert_eq!(response(&received(&client), 5), Value::Null);
    assert_eq!(response_calls(&fixture), 3);
    assert!(server.cache.same_language(&uri, target.range));
}

/// The fixture holds the automatic request after it is sent, until the
/// explicit requests in `join` are added; the server then sends the text again
/// as an explicit request, once. Gives the messages until that ends, and what
/// Hover then shows.
async fn join_after_sending(join: Vec<TranslationRequest>) -> (Vec<Message>, Option<Value>) {
    let (fixture, mut server, connection, client, completions, uri) =
        range_server(Settings::default()).await;
    let tail = Arc::new(StreamTailGate::default());
    {
        let mut state = fixture.state.lock().unwrap();
        state.same_language = true;
        state.stream_tail_gate = Some(tail.clone());
    }
    let target = range_target(&server, &uri, 0);
    server
        .queue(1.into(), target.clone(), hover(&target))
        .unwrap();
    wait_held(&tail, 1).await;
    fixture.state.lock().unwrap().stream_tail_gate = None;
    for request in join {
        server.queue_translation(target.clone(), request).unwrap();
    }
    assert_eq!(server.scheduler.translations().len(), 1);
    tail.release.notify_one();
    server
        .finish(&connection, next(&completions).await)
        .unwrap();
    let mut messages = received(&client);
    // The automatic waiter gets nothing, and nothing else happens yet.
    assert_eq!(response(&messages, 1), Value::Null);
    assert_eq!((ready(&messages), opened(&messages)), (0, 0));
    assert_eq!(server.scheduler.translations().len(), 1);
    server
        .finish(&connection, next(&completions).await)
        .unwrap();
    messages.extend(received(&client));
    assert!(server.scheduler.translations().is_empty());
    let sent = instructions(&fixture);
    assert_eq!(sent.len(), 2);
    assert!(asks_for_languages(&sent[0]));
    assert!(!asks_for_languages(&sent[1]));
    let shown = server
        .queue(3.into(), target.clone(), hover(&target))
        .unwrap();
    assert_eq!(response_calls(&fixture), 2);
    (messages, shown)
}

#[tokio::test(flavor = "current_thread")]
async fn a_code_action_that_joins_after_sending_sends_the_text_again_explicitly() {
    let (messages, shown) = join_after_sending(vec![TranslationRequest::Reply(
        2.into(),
        PendingReply::Command { notice: true },
    )])
    .await;
    assert_eq!((ready(&messages), opened(&messages)), (1, 0));
    assert!(shown.unwrap().to_string().contains("訳Alpha sentence."));
}

#[tokio::test(flavor = "current_thread")]
async fn a_tab_that_joins_after_sending_opens_once() {
    let (messages, shown) = join_after_sending(vec![TranslationRequest::Document]).await;
    assert_eq!((ready(&messages), opened(&messages)), (0, 1));
    // The tab's translation is not for Hover, which still shows nothing.
    assert_eq!(shown, Some(Value::Null));
    // One request answers both.
    let (messages, shown) = join_after_sending(vec![
        TranslationRequest::Document,
        TranslationRequest::Reply(2.into(), PendingReply::Command { notice: true }),
    ])
    .await;
    assert_eq!((ready(&messages), opened(&messages)), (1, 1));
    assert!(shown.unwrap().to_string().contains("訳Alpha sentence."));
}

#[tokio::test(flavor = "current_thread")]
async fn an_explicit_reply_of_the_mark_is_a_translation_and_is_not_sent_again() {
    let (fixture, mut server, connection, client, completions, uri) =
        range_server(Settings::default()).await;
    fixture.state.lock().unwrap().translation = Some(SAME_LANGUAGE.into());
    let target = range_target(&server, &uri, 0);
    server
        .queue(
            1.into(),
            target.clone(),
            PendingReply::Command { notice: true },
        )
        .unwrap();
    server
        .finish(&connection, next(&completions).await)
        .unwrap();
    assert_eq!(ready(&received(&client)), 1);
    assert!(server.scheduler.translations().is_empty());
    assert_eq!(response_calls(&fixture), 1);
    assert_eq!(
        server
            .cache
            .exact(&uri, target.range, Output::Range)
            .unwrap()
            .translation,
        SAME_LANGUAGE
    );
}

#[tokio::test(flavor = "current_thread")]
async fn an_explicit_request_that_joins_before_sending_sends_once_explicitly() {
    let settings = Settings::from_options(Some(
        json!({"translation_limits":{"work":{"concurrent":1,"documents":1}}}),
    ))
    .unwrap();
    let (fixture, mut server, connection, client, completions, uri) = range_server(settings).await;
    let tail = Arc::new(StreamTailGate::default());
    {
        let mut state = fixture.state.lock().unwrap();
        state.same_language = true;
        state.stream_tail_gate = Some(tail.clone());
    }
    // Another range takes the only slot, so the first waits unsent.
    let other = range_target(&server, &uri, 2);
    server
        .queue_translation(other, TranslationRequest::Selection)
        .unwrap();
    wait_held(&tail, 1).await;
    fixture.state.lock().unwrap().stream_tail_gate = None;
    let target = range_target(&server, &uri, 0);
    server
        .queue(1.into(), target.clone(), hover(&target))
        .unwrap();
    server
        .queue(
            2.into(),
            target.clone(),
            PendingReply::Command { notice: true },
        )
        .unwrap();
    tail.release.notify_one();
    server
        .finish(&connection, next(&completions).await)
        .unwrap();
    server
        .finish(&connection, next(&completions).await)
        .unwrap();
    let messages = received(&client);
    assert!(
        response(&messages, 1)
            .to_string()
            .contains("訳Alpha sentence.")
    );
    assert_eq!(ready(&messages), 1);
    assert!(server.scheduler.translations().is_empty());
    let sent = instructions(&fixture);
    assert_eq!(sent.len(), 2);
    assert!(asks_for_languages(&sent[0]));
    assert!(!asks_for_languages(&sent[1]));
}

#[tokio::test(flavor = "current_thread")]
async fn an_automatic_markdown_selection_in_the_target_language_shows_nothing() {
    let (fixture, mut server, connection, client, completions, uri) =
        range_server(Settings::default()).await;
    fixture.state.lock().unwrap().same_language = true;
    let target = range_target(&server, &uri, 2);
    server
        .queue_translation(target.clone(), TranslationRequest::Selection)
        .unwrap();
    server
        .finish(&connection, next(&completions).await)
        .unwrap();
    assert_eq!(ready(&received(&client)), 0);
    assert!(server.cache.same_language(&uri, target.range));
    assert!(server.cache.lenses(&uri).is_empty());
    // Hover over the selection shows nothing and sends nothing.
    assert_eq!(
        server
            .queue(1.into(), target.clone(), hover(&target))
            .unwrap(),
        Some(Value::Null)
    );
    assert!(server.scheduler.translations().is_empty());
    assert_eq!(response_calls(&fixture), 1);
}

#[tokio::test(flavor = "current_thread")]
async fn an_explicit_translation_that_ends_after_the_comment_is_found_in_the_target_language_shows()
{
    let (fixture, mut server, connection, _client, completions, _) =
        range_server(Settings::default()).await;
    let uri = comment_document(&mut server);
    let tail = Arc::new(StreamTailGate::default());
    {
        let mut state = fixture.state.lock().unwrap();
        state.same_language = true;
        state.stream_tail_gate = Some(tail.clone());
    }
    let part = first_line(&uri);
    server
        .queue(1.into(), part, PendingReply::Command { notice: true })
        .unwrap();
    wait_held(&tail, 1).await;
    fixture.state.lock().unwrap().stream_tail_gate = None;
    // Hover over the whole comment ends first, with nothing to show.
    assert_eq!(hover_at(&mut server, 2, &uri, Position::new(1, 5)), None);
    server
        .finish(&connection, next(&completions).await)
        .unwrap();
    tail.release.notify_one();
    server
        .finish(&connection, next(&completions).await)
        .unwrap();
    let shown = hover_at(&mut server, 3, &uri, Position::new(0, 5)).unwrap();
    assert!(shown.to_string().contains("訳これは日本語の文です。"));
    assert!(
        server
            .cache
            .lenses(&uri)
            .iter()
            .any(|(_, lens)| matches!(lens, Lens::Translated(_)))
    );
}

#[tokio::test(flavor = "current_thread")]
async fn a_comment_found_in_the_target_language_keeps_the_translation_of_its_part() {
    let (fixture, mut server, connection, _client, completions, _) =
        range_server(Settings::default()).await;
    let uri = comment_document(&mut server);
    fixture.state.lock().unwrap().same_language = true;
    server
        .queue(
            1.into(),
            first_line(&uri),
            PendingReply::Command { notice: true },
        )
        .unwrap();
    server
        .finish(&connection, next(&completions).await)
        .unwrap();
    assert_eq!(hover_at(&mut server, 2, &uri, Position::new(1, 5)), None);
    server
        .finish(&connection, next(&completions).await)
        .unwrap();
    let shown = hover_at(&mut server, 3, &uri, Position::new(0, 5)).unwrap();
    assert!(shown.to_string().contains("訳これは日本語の文です。"));
    // The rest of the comment shows nothing, and sends nothing.
    assert_eq!(
        hover_at(&mut server, 4, &uri, Position::new(1, 5)),
        Some(Value::Null)
    );
    assert_eq!(response_calls(&fixture), 2);
}

/// A known limit: a result that shows nothing never hides an older
/// translation, even one of a wider range.
#[tokio::test(flavor = "current_thread")]
async fn an_older_wider_translation_still_shows_over_text_found_in_the_target_language() {
    let (fixture, mut server, connection, _client, completions, uri) =
        range_server(Settings::default()).await;
    let narrow = range_target(&server, &uri, 0);
    let mut wide = narrow.clone();
    wide.range.end = Position::new(2, 15);
    server
        .queue_translation(wide, TranslationRequest::Selection)
        .unwrap();
    server
        .finish(&connection, next(&completions).await)
        .unwrap();
    fixture.state.lock().unwrap().same_language = true;
    server
        .queue_translation(narrow.clone(), TranslationRequest::Selection)
        .unwrap();
    server
        .finish(&connection, next(&completions).await)
        .unwrap();
    assert!(server.cache.same_language(&uri, narrow.range));
    let shown = hover_at(&mut server, 1, &uri, Position::new(0, 3)).unwrap();
    assert!(shown.to_string().contains("訳Alpha sentence."));
}

#[tokio::test(flavor = "current_thread")]
async fn hover_joins_an_explicit_translation_of_text_found_in_the_target_language() {
    let (fixture, mut server, connection, client, completions, uri) =
        range_server(Settings::default()).await;
    fixture.state.lock().unwrap().same_language = true;
    let target = range_target(&server, &uri, 0);
    server
        .queue(1.into(), target.clone(), hover(&target))
        .unwrap();
    server
        .finish(&connection, next(&completions).await)
        .unwrap();
    server
        .execute_command(
            &connection,
            2.into(),
            ExecuteCommandParams {
                command: LENS_TRANSLATE_COMMAND.into(),
                arguments: vec![serde_json::to_value(&target).unwrap()],
                work_done_progress_params: Default::default(),
            },
        )
        .unwrap();
    assert_eq!(
        server
            .queue(3.into(), target.clone(), hover(&target))
            .unwrap(),
        None
    );
    server
        .finish(&connection, next(&completions).await)
        .unwrap();
    assert!(
        response(&received(&client), 3)
            .to_string()
            .contains("訳Alpha sentence.")
    );
    assert_eq!(response_calls(&fixture), 2);
}

#[tokio::test(flavor = "current_thread")]
async fn a_card_retry_that_joins_after_sending_is_sent_again_despite_an_older_tab() {
    let (fixture, mut server, connection, client, completions, uri) =
        range_server(Settings::default()).await;
    let target = range_target(&server, &uri, 0);
    server
        .queue_translation(target.clone(), TranslationRequest::Document)
        .unwrap();
    server
        .finish(&connection, next(&completions).await)
        .unwrap();
    let context = server.provider.context().unwrap();
    server.next_ticket += 1;
    server.cache.insert(
        uri.clone(),
        server.next_ticket,
        Stored::failure(context, target.range, "Synthetic failure"),
    );
    received(&client);
    server
        .execute_command(
            &connection,
            1.into(),
            ExecuteCommandParams {
                command: SHOW_COMMAND.into(),
                arguments: vec![serde_json::to_value(&target).unwrap()],
                work_done_progress_params: Default::default(),
            },
        )
        .unwrap();
    let card = received(&client)
        .into_iter()
        .find_map(|m| match m {
            Message::Request(r) if r.method == "window/showMessageRequest" => Some(r.id),
            _ => None,
        })
        .unwrap();
    let tail = Arc::new(StreamTailGate::default());
    {
        let mut state = fixture.state.lock().unwrap();
        state.same_language = true;
        state.stream_tail_gate = Some(tail.clone());
    }
    server
        .queue_translation(target.clone(), TranslationRequest::Selection)
        .unwrap();
    wait_held(&tail, 1).await;
    fixture.state.lock().unwrap().stream_tail_gate = None;
    server
        .card_response(Response::new_ok(card, json!({"title": RETRY})))
        .unwrap();
    assert_eq!(server.scheduler.translations().len(), 1);
    tail.release.notify_one();
    server
        .finish(&connection, next(&completions).await)
        .unwrap();
    // The tab's translation does not stand in for the retry.
    assert_eq!(server.scheduler.translations().len(), 1);
    server
        .finish(&connection, next(&completions).await)
        .unwrap();
    assert_eq!(response_calls(&fixture), 3);
    assert!(!asks_for_languages(&instructions(&fixture)[2]));
    assert!(
        server
            .cache
            .exact(&uri, target.range, Output::Range)
            .is_some()
    );
}

#[tokio::test(flavor = "current_thread")]
async fn a_failure_that_ends_after_the_comment_is_found_in_the_target_language_is_not_kept() {
    let (fixture, mut server, connection, _client, completions, _) =
        range_server(Settings::default()).await;
    let uri = comment_document(&mut server);
    let tail = Arc::new(StreamTailGate::default());
    {
        let mut state = fixture.state.lock().unwrap();
        // An ordinary failure, which does not pause automatic translation.
        // The fixture holds the end of an error body as of a stream.
        state.failure = Some((
            400,
            json!({"error": {"code": "invalid_request_error", "message": "Synthetic failure"}}),
        ));
        state.stream_tail_gate = Some(tail.clone());
    }
    server
        .queue(
            1.into(),
            first_line(&uri),
            PendingReply::Command { notice: true },
        )
        .unwrap();
    wait_held(&tail, 1).await;
    {
        let mut state = fixture.state.lock().unwrap();
        state.failure = None;
        state.same_language = true;
        state.stream_tail_gate = None;
    }
    // Hover over the whole comment ends first, with nothing to show.
    assert_eq!(hover_at(&mut server, 2, &uri, Position::new(1, 5)), None);
    server
        .finish(&connection, next(&completions).await)
        .unwrap();
    assert_eq!(response_calls(&fixture), 2);
    tail.release.notify_one();
    server
        .finish(&connection, next(&completions).await)
        .unwrap();
    // The failure may be notified, but the first line shows nothing.
    assert_eq!(
        hover_at(&mut server, 3, &uri, Position::new(0, 5)),
        Some(Value::Null)
    );
}
