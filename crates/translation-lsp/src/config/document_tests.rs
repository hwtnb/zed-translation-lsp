use serde_json::{Value, json};

use super::*;

fn provider(name: &str, fields: Value) -> Result<Settings, ConfigError> {
    let mut provider = json!({"name": name});
    provider
        .as_object_mut()
        .unwrap()
        .extend(fields.as_object().unwrap().clone());
    Settings::from_options(Some(json!({"provider": provider})))
}

fn anthropic(fields: Value) -> Result<Settings, ConfigError> {
    provider("anthropic", fields)
}

fn chatgpt(fields: Value) -> Result<Settings, ConfigError> {
    let mut fields = fields;
    fields["model"] = json!("synthetic-model");
    provider("chatgpt", fields)
}

type Sent = (String, AnthropicEffort, bool);

// What the range output and the tab output send, as the server resolves them.
fn anthropic_sent(fields: Value) -> (Sent, Option<Sent>) {
    let settings = anthropic(fields).unwrap();
    let ProviderSettings::Anthropic {
        model,
        effort,
        thinking,
        document,
        ..
    } = settings.provider.clone()
    else {
        panic!("Expected Anthropic settings")
    };
    assert_eq!(settings.provider.separates_documents(), document.is_some());
    let sent = |model: String, effort, thinking| {
        let thinking = anthropic_thinking(&model, effort, thinking);
        (model, effort, thinking)
    };
    (
        sent(model, effort, thinking),
        document.map(|document| sent(document.model, document.effort, document.thinking)),
    )
}

fn sent(model: &str, effort: AnthropicEffort, thinking: bool) -> Sent {
    (model.into(), effort, thinking)
}

const HAIKU: &str = "claude-haiku-5-5";
const SONNET: &str = "claude-sonnet-5-5";

#[test]
fn document_settings_take_their_own_defaults_and_inherit_nothing() {
    use AnthropicEffort::{Low, Max, Medium};
    for (fields, range, document) in [
        (json!({}), sent(HAIKU, Low, false), None),
        (
            json!({"document": {"model": SONNET}}),
            sent(HAIKU, Low, false),
            Some(sent(SONNET, Low, true)),
        ),
        (
            json!({"effort": "medium", "document": {"model": SONNET}}),
            sent(HAIKU, Medium, false),
            Some(sent(SONNET, Low, true)),
        ),
        (
            json!({"thinking": true, "document": {"model": HAIKU}}),
            sent(HAIKU, Low, true),
            Some(sent(HAIKU, Low, false)),
        ),
        (
            json!({"document": {"model": HAIKU, "effort": "max"}}),
            sent(HAIKU, Low, false),
            Some(sent(HAIKU, Max, true)),
        ),
        // Explicit values are kept, and the same values as outside still count
        // as tab settings.
        (
            json!({"document": {"model": SONNET, "effort": "medium", "thinking": false}}),
            sent(HAIKU, Low, false),
            Some(sent(SONNET, Medium, false)),
        ),
        (
            json!({"model": SONNET, "effort": "max", "document": {"model": SONNET, "effort": "max"}}),
            sent(SONNET, Max, true),
            Some(sent(SONNET, Max, true)),
        ),
    ] {
        assert_eq!(
            anthropic_sent(fields.clone()),
            (range, document),
            "{fields}"
        );
    }

    for (fields, range, document) in [
        (json!({}), "low", None),
        (
            json!({"effort": "medium", "document": {"model": "m"}}),
            "medium",
            Some(("m", "low")),
        ),
        (
            json!({"document": {"model": "m", "effort": "high"}}),
            "low",
            Some(("m", "high")),
        ),
        (
            json!({"effort": "none", "document": {"model": "synthetic-model", "effort": "max"}}),
            "none",
            Some(("synthetic-model", "max")),
        ),
    ] {
        let settings = chatgpt(fields.clone()).unwrap();
        let ProviderSettings::Chatgpt {
            model,
            effort,
            document: tab,
            ..
        } = settings.provider.clone()
        else {
            panic!("Expected ChatGPT settings")
        };
        assert_eq!(model, "synthetic-model");
        assert_eq!(json!(effort), json!(range), "{fields}");
        assert_eq!(
            tab.map(|tab| (tab.model, json!(tab.effort))),
            document.map(|(model, effort)| (model.to_owned(), json!(effort))),
            "{fields}"
        );
        assert_eq!(
            settings.provider.separates_documents(),
            document.is_some(),
            "{fields}"
        );
    }
    for options in [
        None,
        Some(json!({})),
        Some(json!({"provider": {"name": "mock"}})),
    ] {
        assert!(
            !Settings::from_options(options)
                .unwrap()
                .provider
                .separates_documents()
        );
    }
}

// The speed each output requests: the range's, and the tab's when
// `provider.document` is present.
fn speeds(name: &str, fields: Value) -> (Option<Value>, Option<Option<Value>>) {
    fn value(speed: Option<impl serde::Serialize>) -> Option<Value> {
        speed.map(|speed| json!(speed))
    }
    match provider(name, fields).unwrap().provider {
        ProviderSettings::Chatgpt {
            speed, document, ..
        } => (value(speed), document.map(|tab| value(tab.speed))),
        ProviderSettings::Anthropic {
            speed, document, ..
        } => (value(speed), document.map(|tab| value(tab.speed))),
        ProviderSettings::Mock {} => panic!("Expected provider settings"),
    }
}

#[test]
fn speed_is_requested_only_where_it_is_written() {
    for (name, range, tab) in [
        ("chatgpt", "fast", "flex"),
        ("anthropic", "fast", "standard"),
    ] {
        for (fields, sent) in [
            (json!({}), (None, None)),
            (json!({"speed": range}), (Some(json!(range)), None)),
            (
                json!({"speed": range, "document": {"model": "m"}}),
                (Some(json!(range)), Some(None)),
            ),
            (
                json!({"document": {"model": "m", "speed": tab}}),
                (None, Some(Some(json!(tab)))),
            ),
            (
                json!({"speed": range, "document": {"model": "m", "speed": tab}}),
                (Some(json!(range)), Some(Some(json!(tab)))),
            ),
        ] {
            assert_eq!(speeds(name, fields.clone()), sent, "{name} {fields}");
        }
    }
}

#[test]
fn document_settings_name_the_rejected_field_without_its_value() {
    const SECRET: &str = "synthetic-secret";
    let named = |result: Result<Settings, ConfigError>, message: &str| {
        let error = result.err().unwrap();
        assert_eq!(error.to_string(), message);
        assert_eq!(format!("{error:?}"), message);
    };

    for document in [
        json!({}),
        json!({"effort": "high"}),
        json!({"thinking": true}),
        json!({"speed": "fast"}),
    ] {
        named(
            anthropic(json!({"model": SECRET, "document": document})),
            "provider.document.model is required",
        );
    }
    for document in [
        json!({}),
        json!({"effort": "low"}),
        json!({"speed": "fast"}),
    ] {
        named(
            chatgpt(json!({"document": document})),
            "provider.document.model is required",
        );
    }

    let long = "m".repeat(257);
    for model in ["", " ", "\t", "synthetic\u{0}secret", long.as_str()] {
        for result in [
            anthropic(json!({"document": {"model": model}})),
            chatgpt(json!({"document": {"model": model}})),
        ] {
            named(
                result,
                "provider.document.model must be nonempty, without control characters, and at most 256 bytes",
            );
        }
    }
    assert!(anthropic(json!({"document": {"model": "m".repeat(256)}})).is_ok());
    for model in ["synthetic secret", "synthetic\u{3000}secret"] {
        for result in [
            anthropic(json!({"document": {"model": model}})),
            chatgpt(json!({"document": {"model": model}})),
        ] {
            named(
                result,
                "provider.document.model must not contain whitespace",
            );
        }
    }

    for effort in ["none", "minimal", "ultra", "", "LOW", SECRET] {
        named(
            anthropic(json!({"document": {"model": SONNET, "effort": effort}})),
            "provider.document.effort must be one of low, medium, high, xhigh, max",
        );
    }
    for effort in ["xhigh", "max"] {
        for model in [HAIKU, SONNET] {
            named(
                anthropic(json!({
                    "thinking": true,
                    "document": {"model": model, "effort": effort, "thinking": false}
                })),
                &format!(
                    "provider.document.thinking must not be false when provider.document.effort is {effort}"
                ),
            );
        }
    }
    for (effort, thinking) in [("high", false), ("max", true), ("xhigh", true)] {
        assert!(
            anthropic(json!({
                "document": {"model": SONNET, "effort": effort, "thinking": thinking}
            }))
            .is_ok()
        );
    }

    // The checks of the surrounding settings keep their own names.
    let document = json!({"model": SONNET, "effort": "max"});
    named(
        anthropic(json!({"model": "claude haiku", "document": document})),
        "provider.model must not contain whitespace",
    );
    named(
        anthropic(json!({"model": " ", "document": document})),
        "provider.model must be nonempty, without control characters, and at most 256 bytes",
    );
    named(
        anthropic(json!({"effort": "none", "document": document})),
        "provider.effort must be one of low, medium, high, xhigh, max",
    );
    named(
        anthropic(json!({"effort": "max", "thinking": false, "document": document})),
        "provider.thinking must not be false when provider.effort is max",
    );
}

#[test]
fn malformed_document_settings_report_the_generic_error() {
    const SECRET: &str = "synthetic-secret";
    let generic = |result: Result<Settings, ConfigError>, case: &Value| {
        let error = result.err().unwrap_or_else(|| panic!("Accepted {case}"));
        assert!(matches!(error, ConfigError::Decode(_)), "{case}: {error}");
        assert_eq!(
            error.to_string(),
            "Invalid translation settings: check field names and value types"
        );
        assert!(!format!("{error:?}").contains(SECRET));
    };
    for document in [
        Value::Null,
        json!(SECRET),
        json!(true),
        json!([{"model": SONNET}]),
        json!({"model": SONNET, "unknown": SECRET}),
        json!({"model": SONNET, "reasoning_effort": "low"}),
        json!({"model": SONNET, "document": {"model": SONNET}}),
        json!({"model": SONNET, "name": "anthropic"}),
        json!({"model": null}),
        json!({"model": 1}),
        json!({"model": SONNET, "effort": null}),
        json!({"model": SONNET, "effort": 1}),
        json!({"model": SONNET, "thinking": null}),
        json!({"model": SONNET, "thinking": "false"}),
        json!({"model": SONNET, "speed": 1}),
        json!({"model": SONNET, "speed": true}),
    ] {
        generic(anthropic(json!({"document": document})), &document);
    }
    for document in [
        Value::Null,
        json!(SECRET),
        json!({"model": "m", "unknown": SECRET}),
        json!({"model": "m", "reasoning_effort": "low"}),
        json!({"model": "m", "thinking": true}),
        json!({"model": null}),
        json!({"model": "m", "reasoning_effort": SECRET}),
        json!({"model": "m", "reasoning_effort": 1}),
        json!({"model": "m", "effort": 1}),
        json!({"model": "m", "effort": null}),
        json!({"model": "m", "speed": 1}),
    ] {
        generic(chatgpt(json!({"document": document})), &document);
    }
    // The mock has no such setting, with or without a model inside.
    for document in [
        Value::Null,
        json!({}),
        json!({"model": SECRET}),
        json!({"effort": "ultra"}),
    ] {
        generic(provider("mock", json!({"document": document})), &document);
    }
    // Nor is it a top-level setting.
    let document = json!({"model": SONNET});
    generic(
        Settings::from_options(Some(json!({"document": document}))),
        &document,
    );
}
