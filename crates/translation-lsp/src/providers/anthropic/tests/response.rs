use serde_json::{Value, json};

use super::{PAUSED, automatic, translate};
use crate::{
    config::limits::SizeLimit,
    providers::{
        Intent,
        anthropic::test_support::{
            Fixture, Reply, completed, event, started, stopped, text_block, thinking_block,
        },
        error::ErrorKind,
    },
};

#[tokio::test(flavor = "current_thread")]
async fn complete_streams_return_text_blocks_and_skip_everything_else() {
    let translation = "`retry_count` を読みます。\n🚀";
    let end = || stopped(json!({"stop_reason":"end_turn","stop_sequence":null}));
    let ping = event("ping", json!({"type":"ping"}));
    let with_thinking = format!(
        "{}{ping}{}{ping}{}{}",
        started(),
        thinking_block(0),
        text_block(1, translation),
        end()
    );
    let unknown_events = format!(
        "{}event: future_event\ndata: not json\n\n{}{}{}",
        started(),
        event(
            "content_block_start",
            json!({"type":"content_block_start","index":0,
                "content_block":{"type":"future_block","text":"not a translation"}}),
        ),
        text_block(3, translation),
        end()
    );
    let two_blocks = format!(
        "{}{}{}{}",
        started(),
        text_block(0, "`retry_count` を"),
        text_block(1, "読みます。\n🚀"),
        end()
    );
    let unnamed: String = completed(translation)
        .lines()
        .filter(|line| !line.starts_with("event:"))
        .map(|line| format!("{line}\n"))
        .collect();
    for stream in [
        completed(translation),
        completed(translation).replace('\n', "\r\n"),
        with_thinking,
        unknown_events,
        two_blocks,
        unnamed,
    ] {
        let fixture = Fixture::new().await;
        fixture.reply(Reply::stream(stream.clone()));
        let provider = fixture.provider();
        assert_eq!(
            translate(&provider, Intent::Automatic)
                .await
                .map_err(|error| error.to_string()),
            Ok(translation.to_owned()),
            "{stream}"
        );
        assert_eq!(fixture.calls(), 1);
    }
}

#[tokio::test(flavor = "current_thread")]
async fn unfinished_or_stopped_streams_are_never_returned() {
    let partial = || format!("{}{}", started(), text_block(0, "途中まで"));
    let delta = |index: u64| {
        event(
            "content_block_delta",
            json!({"type":"content_block_delta","index":index,
                "delta":{"type":"text_delta","text":"途中"}}),
        )
    };
    for (stream, message) in [
        (
            format!(
                "{}{}",
                partial(),
                stopped(json!({"stop_reason":"max_tokens"}))
            ),
            "output token limit",
        ),
        (
            format!(
                "{}{}",
                partial(),
                stopped(json!({"stop_reason":"refusal",
                    "stop_details":{"type":"refusal","category":"cyber"}}))
            ),
            "declined to translate this text (category=cyber)",
        ),
        (
            format!(
                "{}{}",
                started(),
                stopped(json!({"stop_reason":"refusal","stop_details":null}))
            ),
            "declined to translate this text;",
        ),
        (
            format!(
                "{}{}",
                partial(),
                stopped(json!({"stop_reason":"pause_turn"}))
            ),
            "without completing the translation",
        ),
        (
            format!("{}{}", partial(), stopped(json!({"stop_reason":null}))),
            "without completing the translation",
        ),
        (
            format!(
                "{}{}",
                started(),
                stopped(json!({"stop_reason":"end_turn"}))
            ),
            "without translation text",
        ),
        (partial(), "without message_stop"),
        (
            format!(
                "{}{}",
                partial(),
                event(
                    "message_delta",
                    json!({"type":"message_delta","delta":{"stop_reason":"end_turn"}})
                )
            ),
            "without message_stop",
        ),
        (
            format!("{}data: {{\"type\":\"message_stop\"", completed("完成")),
            "ended inside an event",
        ),
        (
            format!("{}{}", completed("完成"), delta(0)),
            "continued after message_stop",
        ),
        (
            format!(
                "{}{}{}",
                started(),
                delta(0),
                stopped(json!({"stop_reason":"end_turn"}))
            ),
            "outside a text content block",
        ),
        (
            format!(
                "{}{}{}{}",
                started(),
                thinking_block(0),
                delta(0),
                stopped(json!({"stop_reason":"end_turn"}))
            ),
            "outside a text content block",
        ),
        (
            format!(
                "{}event: content_block_delta\ndata: {{broken}}\n\n",
                started()
            ),
            "invalid JSON",
        ),
        (
            format!(
                "{}{}",
                started(),
                event("message_stop", json!({"type":"content_block_delta"}))
            ),
            "does not match its data",
        ),
    ] {
        let fixture = Fixture::new().await;
        fixture.always(Some(Reply::stream(stream.clone())));
        let provider = fixture.provider();
        let error = translate(&provider, Intent::Automatic).await.unwrap_err();
        assert_eq!(error.kind(), ErrorKind::Failed, "{stream}");
        assert!(error.to_string().contains(message), "{error}");
        assert!(error.to_string().starts_with("Anthropic"), "{error}");
        assert!(!error.to_string().contains(PAUSED));
        // Neither a refusal nor a broken stream is replayed or pauses automatic work.
        assert_eq!(fixture.calls(), 1);
        automatic(&provider).unwrap();
    }
}

#[tokio::test(flavor = "current_thread")]
async fn handled_events_must_carry_their_required_fields() {
    let start = |fields: Value| event("content_block_start", fields);
    let delta = |fields: Value| event("content_block_delta", fields);
    let text_start = || {
        start(json!({"type":"content_block_start","index":0,
            "content_block":{"type":"text","text":""}}))
    };
    let first = || {
        delta(json!({"type":"content_block_delta","index":0,
            "delta":{"type":"text_delta","text":"first"}}))
    };
    let end = || stopped(json!({"stop_reason":"end_turn","stop_sequence":null}));
    let complete = |events: String| format!("{}{}{events}{}", started(), text_start(), end());
    for (case, stream) in [
        (
            "delta without a type after accepted text",
            complete(format!(
                "{}{}",
                first(),
                delta(json!({"type":"content_block_delta","index":0,"delta":{"text":"second"}}))
            )),
        ),
        (
            "delta without a delta object",
            complete(format!(
                "{}{}",
                first(),
                delta(json!({"type":"content_block_delta","index":0}))
            )),
        ),
        (
            "delta without an index",
            complete(format!(
                "{}{}",
                first(),
                delta(json!({"type":"content_block_delta",
                    "delta":{"type":"thinking_delta","thinking":"synthetic"}}))
            )),
        ),
        (
            "text delta without text",
            complete(format!(
                "{}{}",
                first(),
                delta(json!({"type":"content_block_delta","index":0,
                    "delta":{"type":"text_delta"}}))
            )),
        ),
        (
            "text delta with null text",
            complete(format!(
                "{}{}",
                first(),
                delta(json!({"type":"content_block_delta","index":0,
                    "delta":{"type":"text_delta","text":null}}))
            )),
        ),
        (
            "text delta with numeric text",
            complete(format!(
                "{}{}",
                first(),
                delta(json!({"type":"content_block_delta","index":0,
                    "delta":{"type":"text_delta","text":2}}))
            )),
        ),
        (
            "block start without a block type",
            complete(format!(
                "{}{}",
                first(),
                start(json!({"type":"content_block_start","index":1,
                    "content_block":{"text":"second"}}))
            )),
        ),
        (
            "block start without a block",
            complete(format!(
                "{}{}",
                first(),
                start(json!({"type":"content_block_start","index":1}))
            )),
        ),
        (
            "block start without an index",
            complete(format!(
                "{}{}",
                first(),
                start(json!({"type":"content_block_start",
                    "content_block":{"type":"thinking","thinking":""}}))
            )),
        ),
        (
            "block start with a text index",
            complete(format!(
                "{}{}",
                first(),
                start(json!({"type":"content_block_start","index":"1",
                    "content_block":{"type":"text","text":""}}))
            )),
        ),
        (
            "block start with numeric text",
            complete(format!(
                "{}{}",
                first(),
                start(json!({"type":"content_block_start","index":1,
                    "content_block":{"type":"text","text":2}}))
            )),
        ),
        (
            "message delta without a delta",
            format!(
                "{}{}{}{}{}",
                started(),
                text_start(),
                first(),
                event("message_delta", json!({"type":"message_delta","usage":{}})),
                end()
            ),
        ),
        (
            "message delta with a numeric stop reason",
            format!(
                "{}{}{}{}{}",
                started(),
                text_start(),
                first(),
                stopped(json!({"stop_reason":1})),
                end()
            ),
        ),
        (
            "error without an error type",
            format!(
                "{}{}{}{}",
                started(),
                text_start(),
                first(),
                event(
                    "error",
                    json!({"type":"error","error":{"message":"Overloaded"}})
                )
            ),
        ),
        (
            "error without an error object",
            format!(
                "{}{}{}{}",
                started(),
                text_start(),
                first(),
                event("error", json!({"type":"error","error":"overloaded_error"}))
            ),
        ),
        (
            "text block start without text",
            complete(format!(
                "{}{}",
                first(),
                start(json!({"type":"content_block_start","index":1,
                    "content_block":{"type":"text"}}))
            )),
        ),
        (
            "text block start with null text",
            complete(format!(
                "{}{}",
                first(),
                start(json!({"type":"content_block_start","index":1,
                    "content_block":{"type":"text","text":null}}))
            )),
        ),
        (
            "message stop without a type",
            format!(
                "{}{}{}{}{}",
                started(),
                text_start(),
                first(),
                event(
                    "message_delta",
                    json!({"type":"message_delta","delta":{"stop_reason":"end_turn"}})
                ),
                event("message_stop", json!({}))
            ),
        ),
        (
            "message stop with a null type",
            format!(
                "{}{}{}{}{}",
                started(),
                text_start(),
                first(),
                event(
                    "message_delta",
                    json!({"type":"message_delta","delta":{"stop_reason":"end_turn"}})
                ),
                event("message_stop", json!({"type":null}))
            ),
        ),
        (
            "block start without a type",
            format!(
                "{}{}{}{}",
                started(),
                start(json!({"index":0,"content_block":{"type":"text","text":""}})),
                first(),
                end()
            ),
        ),
        (
            "block start with a null type",
            format!(
                "{}{}{}{}",
                started(),
                start(json!({"type":null,"index":0,
                    "content_block":{"type":"text","text":""}})),
                first(),
                end()
            ),
        ),
        (
            "block delta without a type",
            complete(format!(
                "{}{}",
                first(),
                delta(json!({"index":0,"delta":{"type":"text_delta","text":"second"}}))
            )),
        ),
        (
            "block delta with a null type",
            complete(format!(
                "{}{}",
                first(),
                delta(json!({"type":null,"index":0,
                    "delta":{"type":"text_delta","text":"second"}}))
            )),
        ),
        (
            "message delta without a type",
            format!(
                "{}{}{}{}{}",
                started(),
                text_start(),
                first(),
                event("message_delta", json!({"delta":{"stop_reason":"end_turn"}})),
                event("message_stop", json!({"type":"message_stop"}))
            ),
        ),
        (
            "message delta with a null type",
            format!(
                "{}{}{}{}{}",
                started(),
                text_start(),
                first(),
                event(
                    "message_delta",
                    json!({"type":null,"delta":{"stop_reason":"end_turn"}})
                ),
                event("message_stop", json!({"type":"message_stop"}))
            ),
        ),
        (
            "error without a type",
            format!(
                "{}{}{}{}",
                started(),
                text_start(),
                first(),
                event(
                    "error",
                    json!({"error":{"type":"overloaded_error","message":"Overloaded"}})
                )
            ),
        ),
        (
            "error with a null type",
            format!(
                "{}{}{}{}",
                started(),
                text_start(),
                first(),
                event(
                    "error",
                    json!({"type":null,
                        "error":{"type":"overloaded_error","message":"Overloaded"}})
                )
            ),
        ),
    ] {
        let fixture = Fixture::new().await;
        fixture.state.lock().unwrap().retry_after = Some("0".into());
        fixture.always(Some(Reply::stream(stream)));
        let provider = fixture.provider();
        let error = match translate(&provider, Intent::Automatic).await {
            Ok(text) => panic!("{case}: published {text:?}"),
            Err(error) => error,
        };
        assert_eq!(error.kind(), ErrorKind::Failed, "{case}: {error}");
        assert_eq!(
            error.to_string(),
            "Anthropic SSE event lacks a required field or has an unexpected value",
            "{case}"
        );
        assert_eq!(fixture.calls(), 1, "{case}");
        automatic(&provider).unwrap();
    }
}

#[tokio::test(flavor = "current_thread")]
async fn handled_events_tolerate_unknown_types_and_extra_fields() {
    let end = || stopped(json!({"stop_reason":"end_turn","stop_sequence":null}));
    let stream = format!(
        "{}{}{}{}{}{}{}{}{}",
        started(),
        event(
            "content_block_start",
            json!({"type":"content_block_start","index":0,"future":{"nested":[1]},
                "content_block":{"type":"text","text":"訳","citations":null,"future":true}}),
        ),
        event(
            "content_block_delta",
            json!({"type":"content_block_delta","index":0,"future":1,
                "delta":{"type":"text_delta","text":"文","future":[]}}),
        ),
        // Unknown delta and block types are skipped, whatever else they carry.
        event(
            "content_block_delta",
            json!({"type":"content_block_delta","index":0,
                "delta":{"type":"future_delta","text":"not a translation"}}),
        ),
        event(
            "content_block_start",
            json!({"type":"content_block_start","index":1,
                "content_block":{"type":"future_block","text":2}}),
        ),
        event(
            "content_block_delta",
            json!({"type":"content_block_delta","index":1,
                "delta":{"type":"future_delta","text":2}}),
        ),
        event(
            "message_delta",
            json!({"type":"message_delta","delta":{},"usage":{"output_tokens":1},"future":1}),
        ),
        event("future_event", json!({"type":"future_event","delta":1})),
        end()
    );
    let fixture = Fixture::new().await;
    fixture.reply(Reply::stream(stream));
    assert_eq!(
        translate(&fixture.provider(), Intent::Automatic)
            .await
            .map_err(|error| error.to_string()),
        Ok("訳文".to_owned())
    );

    // Refusal details are read only from the delta, where the API reports them.
    for (delta, root, message) in [
        (
            json!({"stop_reason":"refusal","stop_details":{"type":"refusal","category":"bio"}}),
            Value::Null,
            "declined to translate this text (category=bio);",
        ),
        (
            json!({"stop_reason":"refusal"}),
            json!({"type":"refusal","category":"cyber"}),
            "declined to translate this text;",
        ),
        (
            json!({"stop_reason":"refusal","stop_details":{"type":"refusal","category":null}}),
            Value::Null,
            "declined to translate this text;",
        ),
    ] {
        let fixture = Fixture::new().await;
        fixture.reply(Reply::stream(format!(
            "{}{}{}",
            started(),
            event(
                "message_delta",
                json!({"type":"message_delta","delta":delta,"stop_details":root}),
            ),
            event("message_stop", json!({"type":"message_stop","future":1})),
        )));
        let error = translate(&fixture.provider(), Intent::Automatic)
            .await
            .unwrap_err();
        assert!(error.to_string().contains(message), "{error}");
    }
}

#[tokio::test(flavor = "current_thread")]
async fn unknown_and_unnamed_events_keep_their_lenient_handling() {
    let unnamed = |value: Value| format!("data: {value}\n\n");
    let text_start = json!({"type":"content_block_start","index":0,
        "content_block":{"type":"text","text":""}});
    let text_delta = json!({"type":"content_block_delta","index":0,
        "delta":{"type":"text_delta","text":"訳文"}});
    let stop_delta = json!({"type":"message_delta","delta":{"stop_reason":"end_turn"}});
    let stop = json!({"type":"message_stop"});
    for (case, stream) in [
        (
            "unknown names are skipped without reading their data",
            format!(
                "{}{}event: future_event\ndata: not json\n\n{}{}{}{}",
                started(),
                event("content_block_start", text_start.clone()),
                event("future_event", json!({})),
                event("future_event", json!({"type":null,"index":"x","delta":1})),
                event("content_block_delta", text_delta.clone()),
                stopped(json!({"stop_reason":"end_turn"}))
            ),
        ),
        (
            "unnamed events are identified by their type",
            format!(
                "{}{}{}{}",
                unnamed(text_start.clone()),
                unnamed(text_delta.clone()),
                unnamed(stop_delta.clone()),
                unnamed(stop.clone())
            ),
        ),
        (
            "unnamed events without a usable type are skipped",
            format!(
                "{}{}{}{}{}{}{}",
                unnamed(text_start.clone()),
                unnamed(json!({})),
                unnamed(json!({"index":0,"delta":{"type":"text_delta","text":"x"}})),
                unnamed(json!({"type":"future_event","index":0,
                    "delta":{"type":"text_delta","text":"x"}})),
                unnamed(text_delta.clone()),
                unnamed(stop_delta.clone()),
                unnamed(stop.clone())
            ),
        ),
        (
            "a message delta may carry usage alone",
            format!(
                "{}{}{}{}{}{}",
                started(),
                event("content_block_start", text_start.clone()),
                event("content_block_delta", text_delta.clone()),
                event(
                    "message_delta",
                    json!({"type":"message_delta","delta":{},"usage":{"output_tokens":1}})
                ),
                event(
                    "message_delta",
                    json!({"type":"message_delta",
                        "delta":{"stop_reason":"end_turn","stop_details":null}})
                ),
                event(
                    "message_delta",
                    json!({"type":"message_delta",
                        "delta":{"stop_reason":null,"stop_details":{"category":null}}})
                ) + &event("message_stop", stop.clone())
            ),
        ),
    ] {
        let fixture = Fixture::new().await;
        fixture.reply(Reply::stream(stream));
        assert_eq!(
            translate(&fixture.provider(), Intent::Automatic)
                .await
                .map_err(|error| error.to_string()),
            Ok("訳文".to_owned()),
            "{case}"
        );
    }

    let fixture = Fixture::new().await;
    fixture.reply(Reply::stream(format!(
        "{}{}",
        started(),
        event("message_stop", json!({"type":"message_delta"}))
    )));
    let error = translate(&fixture.provider(), Intent::Automatic)
        .await
        .unwrap_err();
    assert_eq!(
        error.to_string(),
        "Anthropic SSE event type does not match its data"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn output_and_transport_limits_scale_with_the_configured_output() {
    for (bytes, accepted) in [(1024, true), (1025, false)] {
        let fixture = Fixture::new().await;
        let translation = format!("{}é", "x".repeat(bytes - 2));
        fixture.reply(Reply::stream(completed(&translation)));
        let mut provider = fixture.provider();
        provider.output_limit = SizeLimit::try_from(1).unwrap();
        let result = translate(&provider, Intent::Automatic).await;
        if accepted {
            assert_eq!(result.unwrap(), translation);
        } else {
            let error = result.unwrap_err();
            assert!(error.to_string().contains("translation_limits.output_kib"));
            assert!(error.to_string().contains("1025"));
        }
    }
    for (stream, scope) in [
        (
            format!("{}data: {}", started(), "x".repeat(2 * 1024 * 1024)),
            "Anthropic SSE event exceeds",
        ),
        (
            event(
                "ping",
                json!({"type":"ping","padding":"x".repeat(16 * 1024)}),
            )
            .repeat(140),
            "Anthropic stream exceeds",
        ),
    ] {
        let fixture = Fixture::new().await;
        fixture.reply(Reply::stream(stream));
        let mut provider = fixture.provider();
        provider.output_limit = SizeLimit::try_from(1).unwrap();
        let error = translate(&provider, Intent::Automatic).await.unwrap_err();
        assert_eq!(error.kind(), ErrorKind::Failed);
        assert!(error.to_string().starts_with(scope), "{error}");
        assert_eq!(fixture.calls(), 1);
    }
}
