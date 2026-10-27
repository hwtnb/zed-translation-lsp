use serde_json::json;

use super::*;

#[test]
fn decode_error_retains_source_without_echoing_rejected_values() {
    use std::error::Error as _;
    let error = Settings::from_options(Some(json!({"provider": {"name": "synthetic-secret"}})))
        .err()
        .unwrap();
    assert!(matches!(error, ConfigError::Decode(_)));
    assert!(
        error
            .source()
            .unwrap()
            .downcast_ref::<serde_json::Error>()
            .is_some()
    );
    assert!(!error.to_string().contains("synthetic-secret"));
    assert!(!format!("{error:?}").contains("synthetic-secret"));
}

#[test]
fn no_setting_accepts_an_array_in_place_of_its_table_or_value() {
    let tab = "synthetic-tab";
    for options in [
        json!([]),
        json!([{}]),
        json!([[16, 1024], 4096]),
        json!({"provider": ["anthropic"]}),
        json!({"provider": ["chatgpt", "m"]}),
        json!({"provider": ["chatgpt", "m", "low"]}),
        json!({"provider": ["mock"]}),
        json!({"provider": [{"name": "mock"}]}),
        json!({"provider": {"name": "chatgpt", "model": "m", "document": [tab, "low"]}}),
        json!({"provider": {"name": "chatgpt", "model": "m", "document": [tab]}}),
        json!({"provider": {"name": "chatgpt", "model": "m", "document": [{"model": tab}]}}),
        json!({"provider": {"name": "anthropic", "document": [tab, "low", true]}}),
        json!({"provider": {"name": "anthropic", "document": [tab]}}),
        json!({"provider": {"name": "anthropic", "document": []}}),
        json!({"translation_limits": []}),
        json!({"translation_limits": [[16, 1024], 4096]}),
        json!({"translation_limits": {"input_kib": [16, 1024]}}),
        json!({"translation_limits": {"input_kib": []}}),
        json!({"translation_limits": {"work": []}}),
        json!({"translation_limits": {"work": [8, 2, 1, 3, 16]}}),
        json!({"translation_limits": {"cache": [64]}}),
        json!({"translation_limits": {"cache": [64, 16384]}}),
        // Nor in place of a single value.
        json!({"target_language": ["ja"]}),
        json!({"hover_timeout_ms": [20000]}),
        json!({"translation_limits": {"output_kib": [4096]}}),
        json!({"provider": {"name": "chatgpt", "model": ["m"]}}),
        json!({"provider": {"name": "anthropic", "effort": ["low"]}}),
        json!({"provider": {"name": "anthropic", "document": {"model": [tab]}}}),
    ] {
        let error = Settings::from_options(Some(options.clone()))
            .err()
            .unwrap_or_else(|| panic!("Accepted {options}"));
        assert!(matches!(error, ConfigError::Decode(_)), "{options}");
        assert_eq!(
            error.to_string(),
            "Invalid translation settings: check field names and value types"
        );
        assert!(!format!("{error:?}").contains(tab));
    }
    // The same values written as tables are settings.
    for options in [
        json!({}),
        json!({"provider": {"name": "anthropic", "document": {"model": tab, "effort": "low", "thinking": true}}}),
        json!({"provider": {"name": "chatgpt", "model": "m", "document": {"model": tab, "effort": "low", "speed": "fast"}}}),
        json!({"translation_limits": {"input_kib": {"hover": 16, "document": 1024}, "work": {}, "cache": {"entries": 64}}}),
    ] {
        assert!(Settings::from_options(Some(options)).is_ok());
    }
    // The named checks still report before the generic error.
    let error = Settings::from_options(Some(json!({
        "provider": {"name": "anthropic", "document": {"effort": "high"}},
        "translation_limits": []
    })))
    .err()
    .unwrap();
    assert_eq!(error.to_string(), "provider.document.model is required");
}

#[test]
fn resource_limits_accept_boundaries_and_reject_excess_with_typed_details() {
    for (path, maximum) in [
        (vec!["input_kib", "hover"], 1024),
        (vec!["input_kib", "document"], 16384),
        (vec!["output_kib"], 16384),
        (vec!["cache", "capacity_kib"], 262144),
        (vec!["cache", "entries"], 4096),
        (vec!["work", "admitted"], 64),
        (vec!["work", "concurrent"], 8),
        (vec!["work", "documents"], 8),
        (vec!["work", "automatic"], 63),
        (vec!["work", "hover_waiters"], 256),
    ] {
        let settings = |value| {
            let mut options = json!({"translation_limits": {"work": {"admitted": 64, "concurrent": 8}, "input_kib": {"document": 1024}}});
            // Use fewer accepted operations for the maximum document size.
            if path == ["input_kib", "document"] {
                options["translation_limits"]["work"]["admitted"] = json!(8);
            }
            // Use fewer workers for the maximum output size.
            if path == ["output_kib"] {
                options["translation_limits"]["work"]["concurrent"] = json!(2);
            }
            let mut slot = &mut options["translation_limits"];
            for name in &path {
                slot = &mut slot[*name];
            }
            *slot = json!(value);
            Settings::from_options(Some(options))
        };
        assert!(settings(maximum).is_ok(), "{path:?}");
        let error = settings(maximum + 1).err().unwrap();
        let ConfigError::OutOfRange {
            setting,
            value,
            minimum,
            maximum: bound,
        } = error
        else {
            panic!("Wrong error: {error}")
        };
        assert_eq!(setting, format!("translation_limits.{}", path.join(".")));
        assert_eq!((value, minimum, bound), (maximum + 1, 1, maximum));
    }
}

#[test]
fn combined_budgets_reject_individually_valid_settings() {
    use error::Budget;
    let options = json!({"translation_limits": {"input_kib":{"document":4096},"output_kib":8192,"work":{"admitted":64,"concurrent":8}}});
    assert!(Settings::from_options(Some(options.clone())).is_ok());
    for (field, value, input) in [("document", 4097, true), ("output_kib", 8193, false)] {
        let mut options = options.clone();
        if input {
            options["translation_limits"]["input_kib"][field] = json!(value);
        } else {
            options["translation_limits"][field] = json!(value);
        }
        let error = Settings::from_options(Some(options)).err().unwrap();
        let ConfigError::Budget {
            budget,
            required_bytes,
            maximum_bytes,
        } = error
        else {
            panic!("Wrong error: {error}")
        };
        assert!(required_bytes > maximum_bytes);
        assert_eq!(matches!(budget, Budget::AcceptedInput), input);
    }
}

#[test]
fn translation_limits_default_at_every_level_and_validate_kib_values() {
    for options in [
        json!({}),
        json!({"translation_limits":{}}),
        json!({"translation_limits":{"input_kib":{}}}),
    ] {
        let limits = Settings::from_options(Some(options))
            .unwrap()
            .translation_limits;
        assert_eq!(limits.input_kib.hover.bytes(), 16 * 1024);
        assert_eq!(limits.input_kib.document.bytes(), 1024 * 1024);
        assert_eq!(limits.output_kib.bytes(), 4 * 1024 * 1024);
    }
    let limits = Settings::from_options(Some(
        json!({"translation_limits":{"input_kib":{"document":2048}}}),
    ))
    .unwrap()
    .translation_limits;
    assert_eq!(limits.input_kib.document.bytes(), 2 * 1024 * 1024);
    assert_eq!(limits.input_kib.hover.bytes(), 16 * 1024);
    assert_eq!(limits.output_kib.bytes(), 4 * 1024 * 1024);
    for value in [
        json!(0),
        json!(-1),
        json!(1.5),
        json!("16"),
        json!(true),
        Value::Null,
        json!(u64::MAX),
    ] {
        for limits in [
            json!({"input_kib":{"hover":value}}),
            json!({"input_kib":{"document":value}}),
            json!({"output_kib":value}),
        ] {
            assert!(Settings::from_options(Some(json!({"translation_limits":limits}))).is_err());
        }
    }
    for limits in [
        Value::Null,
        json!({"input_kib":null}),
        json!({"input_kib":{"unknown":16}}),
        json!({"unknown":16}),
    ] {
        assert!(Settings::from_options(Some(json!({"translation_limits":limits}))).is_err());
    }
    let limits = Settings::from_options(Some(
        json!({"translation_limits":{"input_kib":{"hover":1},"output_kib":2}}),
    ))
    .unwrap()
    .translation_limits;
    assert!(
        limits
            .input_kib
            .hover
            .check("é".repeat(512).len(), limits::SizeSetting::HoverInput)
            .is_ok()
    );
    assert!(
        limits
            .input_kib
            .hover
            .check("é".repeat(513).len(), limits::SizeSetting::HoverInput)
            .is_err()
    );
    assert_eq!(limits.output_kib.bytes(), 2048);
}

#[test]
fn an_omitted_provider_is_unset_and_explicit_settings_are_validated() {
    let settings = Settings::from_options(None).unwrap();
    assert!(matches!(
        Settings::from_options(Some(Value::Null)).unwrap().provider,
        ProviderSettings::Unset
    ));
    for name in ["unset", "Unset"] {
        assert!(matches!(
            Settings::from_options(Some(json!({"provider": {"name": name}})))
                .err()
                .unwrap(),
            ConfigError::Decode(_)
        ));
    }
    for effort in ["none", "minimal", "low", "medium", "high", "xhigh", "max"] {
        assert!(
            Settings::from_options(Some(
                json!({"provider":{"name":"chatgpt","model":"synthetic-model","effort":effort}})
            ))
            .is_ok()
        );
    }
    assert_eq!(settings.target_language, "ja");
    assert_eq!(settings.source_language, "auto");
    assert!(matches!(settings.provider, ProviderSettings::Unset));
    for options in [None, Some(Value::Null), Some(json!({}))] {
        let settings = Settings::from_options(options).unwrap();
        assert_eq!(settings.hover_timeout_ms, 20_000);
        assert_eq!(settings.selection_timeout_ms, 30_000);
        assert_eq!(settings.document_timeout_ms, 180_000);
        assert_eq!(settings.selection_debounce_ms, 450);
        assert_eq!(settings.comment_parse_timeout_ms, 1_000);
    }
    let settings = Settings::from_options(Some(json!({
            "target_language":"en", "provider":{"name":"chatgpt", "model":"gpt-6.1-sol", "effort":"medium"}
        }))).unwrap();
    assert!(matches!(
        settings.provider,
        ProviderSettings::Chatgpt {
            effort: ReasoningEffort::Medium,
            ..
        }
    ));
    for invalid in [
        json!({"provider":{"name":"chatgpt","model":" "}}),
        json!({"provider":{"name":"chatgpt","model":"gpt-6.1-sol","effort":"ultra"}}),
        json!({"provider":{"name":"chatgpt","model":"gpt-6.1-sol","reasoning_effort":"low"}}),
        json!({"provider":{"name":"mock","model":"unexpected"}}),
        json!({"target_language":""}),
        json!({"api_key":"not-a-supported-setting"}),
    ] {
        assert!(Settings::from_options(Some(invalid)).is_err());
    }
}

#[test]
fn anthropic_settings_default_to_light_haiku_and_name_rejected_values() {
    let anthropic = |fields: Value| {
        let mut provider = json!({"name": "anthropic"});
        provider
            .as_object_mut()
            .unwrap()
            .extend(fields.as_object().unwrap().clone());
        Settings::from_options(Some(json!({"provider": provider})))
    };
    let ProviderSettings::Anthropic {
        model,
        effort,
        thinking,
        ..
    } = anthropic(json!({})).unwrap().provider
    else {
        panic!("Expected Anthropic settings")
    };
    assert_eq!(model, "claude-haiku-5-5");
    assert_eq!(effort, AnthropicEffort::Low);
    assert_eq!(thinking, None);
    let ProviderSettings::Anthropic {
        model,
        effort,
        thinking,
        ..
    } = anthropic(json!({"model": "claude-opus-5", "effort": "max", "thinking": true}))
        .unwrap()
        .provider
    else {
        panic!("Expected Anthropic settings")
    };
    assert_eq!(model, "claude-opus-5");
    assert_eq!(effort, AnthropicEffort::Max);
    assert_eq!(thinking, Some(true));
    for (effort, thinking) in [
        ("low", false),
        ("medium", false),
        ("high", false),
        ("low", true),
        ("xhigh", true),
        ("max", true),
    ] {
        assert!(anthropic(json!({"effort": effort, "thinking": thinking})).is_ok());
    }

    for effort in ["none", "minimal", "ultra", "", "LOW"] {
        let error = anthropic(json!({"effort": effort, "model": "synthetic-secret"}))
            .err()
            .unwrap();
        assert!(matches!(
            error,
            ConfigError::Choice {
                setting: "provider.effort",
                ..
            }
        ));
        assert_eq!(
            error.to_string(),
            "provider.effort must be one of low, medium, high, xhigh, max"
        );
    }
    for effort in ["xhigh", "max"] {
        for model in ["claude-haiku-5-5", "claude-opus-5-5"] {
            let error = anthropic(json!({"model": model, "effort": effort, "thinking": false}))
                .err()
                .unwrap();
            assert!(matches!(error, ConfigError::ThinkingRequired { .. }));
            assert_eq!(
                error.to_string(),
                format!("provider.thinking must not be false when provider.effort is {effort}")
            );
        }
    }
    for model in ["", " ", "\t"] {
        assert!(matches!(
            anthropic(json!({"model": model})).err().unwrap(),
            ConfigError::Text {
                setting: "provider.model",
                ..
            }
        ));
    }
    assert!(matches!(
        anthropic(json!({"model": "claude haiku"})).err().unwrap(),
        ConfigError::Whitespace { .. }
    ));
    for fields in [
        json!({"api_key": "synthetic-secret"}),
        json!({"reasoning_effort": "low"}),
        json!({"base_url": "http://127.0.0.1"}),
        json!({"effort": 1}),
        json!({"effort": null}),
        json!({"thinking": "false"}),
        json!({"thinking": null}),
        json!({"model": null}),
    ] {
        let error = anthropic(fields).err().unwrap();
        assert!(matches!(error, ConfigError::Decode(_)));
        assert!(!error.to_string().contains("synthetic-secret"));
    }

    // The Anthropic fields and checks do not leak into the other providers.
    for options in [
        json!({"provider": {"name": "chatgpt", "model": "synthetic-model", "thinking": true}}),
        json!({"provider": {"name": "mock", "effort": "low"}}),
        json!({"provider": {"name": "mock", "speed": "fast"}}),
    ] {
        assert!(matches!(
            Settings::from_options(Some(options)).err().unwrap(),
            ConfigError::Decode(_)
        ));
    }
    assert!(matches!(
        Settings::from_options(Some(json!({
            "provider": {"name": "chatgpt", "effort": "none"}
        })))
        .unwrap()
        .provider,
        ProviderSettings::Chatgpt {
            effort: ReasoningEffort::None,
            ..
        }
    ));
    assert!(matches!(
        Settings::from_options(Some(json!({"provider": {"name": "mock"}})))
            .unwrap()
            .provider,
        ProviderSettings::Mock {}
    ));
}

#[test]
fn chatgpt_settings_default_to_light_luna_and_take_no_reasoning_effort() {
    let ProviderSettings::Chatgpt {
        model,
        effort,
        speed,
        document,
    } = Settings::from_options(Some(json!({"provider": {"name": "chatgpt"}})))
        .unwrap()
        .provider
    else {
        panic!("Expected ChatGPT settings")
    };
    assert_eq!(model, "gpt-6-luna");
    assert_eq!(effort, ReasoningEffort::Low);
    assert_eq!(speed, None);
    assert!(document.is_none());
    // The former name is an unknown field, inside `provider.document` too.
    for provider in [
        json!({"name": "chatgpt", "reasoning_effort": "low"}),
        json!({"name": "chatgpt", "document": {"model": "m", "reasoning_effort": "low"}}),
    ] {
        assert!(matches!(
            Settings::from_options(Some(json!({"provider": provider})))
                .err()
                .unwrap(),
            ConfigError::Decode(_)
        ));
    }
}

#[test]
fn effort_and_speed_take_the_values_of_their_provider_and_name_them() {
    const CHATGPT_EFFORT: &str = "none, minimal, low, medium, high, xhigh, max";
    const CHATGPT_SPEED: &str = "auto, default, flex, scale, priority, fast, ultrafast";
    const ANTHROPIC_SPEED: &str = "standard, fast";
    let settings = |name: &str, field: &str, value: Value, inside: bool| {
        let mut provider = json!({"name": name});
        if inside {
            provider["document"] = json!({"model": "m", (field): value});
        } else {
            provider[field] = value;
        }
        Settings::from_options(Some(json!({"provider": provider})))
    };
    for (name, field, accepted) in [
        ("chatgpt", "speed", CHATGPT_SPEED),
        ("anthropic", "speed", ANTHROPIC_SPEED),
        ("chatgpt", "effort", CHATGPT_EFFORT),
        ("openai_api", "speed", CHATGPT_SPEED),
        ("openai_api", "effort", CHATGPT_EFFORT),
    ] {
        for value in accepted.split(", ") {
            for inside in [false, true] {
                assert!(
                    settings(name, field, json!(value), inside).is_ok(),
                    "{name} {field} {value}"
                );
            }
        }
    }
    for (name, field, value, choices) in [
        // The values of the other provider.
        ("chatgpt", "speed", json!("standard"), CHATGPT_SPEED),
        ("anthropic", "speed", json!("priority"), ANTHROPIC_SPEED),
        ("anthropic", "speed", json!("ultrafast"), ANTHROPIC_SPEED),
        (
            "anthropic",
            "effort",
            json!("minimal"),
            AnthropicEffort::CHOICES,
        ),
        ("chatgpt", "effort", json!("ultra"), CHATGPT_EFFORT),
        ("chatgpt", "speed", json!("Fast"), CHATGPT_SPEED),
        (
            "anthropic",
            "speed",
            json!("synthetic-secret"),
            ANTHROPIC_SPEED,
        ),
        ("openai_api", "speed", json!("standard"), CHATGPT_SPEED),
        ("openai_api", "effort", json!("ultra"), CHATGPT_EFFORT),
        // `null` is no way to write that no speed is requested.
        ("chatgpt", "speed", Value::Null, CHATGPT_SPEED),
        ("openai_api", "speed", Value::Null, CHATGPT_SPEED),
        ("anthropic", "speed", Value::Null, ANTHROPIC_SPEED),
    ] {
        for (inside, scope) in [(false, "provider"), (true, "provider.document")] {
            let error = settings(name, field, value.clone(), inside).err().unwrap();
            assert_eq!(
                error.to_string(),
                format!("{scope}.{field} must be one of {choices}"),
                "{name} {value}"
            );
        }
    }
    for name in ["chatgpt", "anthropic", "openai_api"] {
        for value in [json!(1), json!(true), json!({"tier": "fast"})] {
            for inside in [false, true] {
                assert!(matches!(
                    settings(name, "speed", value.clone(), inside)
                        .err()
                        .unwrap(),
                    ConfigError::Decode(_)
                ));
            }
        }
    }
}

#[test]
fn omitted_anthropic_thinking_follows_the_model_and_effort() {
    let resolved = |fields: Value| {
        let mut provider = json!({"name": "anthropic"});
        provider
            .as_object_mut()
            .unwrap()
            .extend(fields.as_object().unwrap().clone());
        let ProviderSettings::Anthropic {
            model,
            effort,
            thinking,
            ..
        } = Settings::from_options(Some(json!({"provider": provider})))
            .unwrap()
            .provider
        else {
            panic!("Expected Anthropic settings")
        };
        anthropic_thinking(&model, effort, thinking)
    };
    let efforts = [
        ("low", false),
        ("medium", false),
        ("high", false),
        ("xhigh", true),
        ("max", true),
    ];
    // The default model keeps thinking off up to `high`, named or not.
    for (effort, expected) in efforts {
        assert_eq!(resolved(json!({"effort": effort})), expected, "{effort}");
        assert_eq!(
            resolved(json!({"model": "claude-haiku-5-5", "effort": effort})),
            expected,
            "{effort}"
        );
    }
    assert!(!resolved(json!({})));
    assert!(!resolved(json!({"model": "claude-haiku-5-5"})));
    // Any other model name enables it, including near matches of the default.
    for model in [
        "claude-opus-5-5",
        "claude-sonnet-5-5",
        "claude-haiku-4-5",
        "claude-haiku-5-5-20260101",
        "anthropic.claude-haiku-5-5",
        "claude-haiku-5",
        "Claude-Haiku-5-5",
    ] {
        assert!(resolved(json!({"model": model})), "{model}");
        for (effort, _) in efforts {
            assert!(
                resolved(json!({"model": model, "effort": effort})),
                "{model} {effort}"
            );
        }
    }
    // An explicit value is sent as written wherever the settings accept it.
    for model in ["claude-haiku-5-5", "claude-opus-5-5"] {
        for (effort, required) in efforts {
            assert!(resolved(
                json!({"model": model, "effort": effort, "thinking": true})
            ));
            if !required {
                assert!(!resolved(
                    json!({"model": model, "effort": effort, "thinking": false})
                ));
            }
        }
    }
}

#[test]
fn timing_options_accept_inclusive_bounds_and_reject_wrong_types_and_names() {
    for (field, minimum, maximum) in [
        ("hover_timeout_ms", 1_000, 120_000),
        ("selection_timeout_ms", 1_000, 120_000),
        ("document_timeout_ms", 1_000, 600_000),
        ("selection_debounce_ms", 100, 2_000),
        ("comment_parse_timeout_ms", 100, 60_000),
    ] {
        for value in [minimum, maximum] {
            assert!(Settings::from_options(Some(json!({(field):value}))).is_ok());
        }
        for value in [minimum - 1, maximum + 1] {
            let error = Settings::from_options(Some(json!({(field):value})))
                .err()
                .unwrap();
            assert!(error.to_string().starts_with(field), "{field}: {error}");
        }
        for value in [
            json!(-1),
            json!(1.5),
            json!("1000"),
            json!(true),
            Value::Null,
            json!(u64::MAX),
        ] {
            assert!(Settings::from_options(Some(json!({(field):value}))).is_err());
        }
    }
    assert!(Settings::from_options(Some(json!({"unknown_setting":true}))).is_err());
}

#[test]
fn openai_api_settings_default_like_chatgpt_and_take_no_endpoint() {
    let ProviderSettings::OpenAiApi {
        model,
        effort,
        speed,
        document,
    } = Settings::from_options(Some(json!({"provider": {"name": "openai_api"}})))
        .unwrap()
        .provider
    else {
        panic!("Expected the OpenAI API provider")
    };
    assert_eq!(model, "gpt-6-luna");
    assert_eq!(effort, ReasoningEffort::Low);
    assert_eq!(speed, None);
    assert!(document.is_none());

    let error = Settings::from_options(Some(json!({
        "provider": {"name": "openai_api", "document": {"effort": "high"}}
    })))
    .err()
    .unwrap();
    assert_eq!(error.to_string(), "provider.document.model is required");
    for field in ["base_url", "thinking", "api_key", "key"] {
        let mut provider = json!({"name": "openai_api"});
        provider[field] = json!("https://api.openai.com/v1");
        assert!(matches!(
            Settings::from_options(Some(json!({"provider": provider})))
                .err()
                .unwrap(),
            ConfigError::Decode(_)
        ));
    }
}

fn compatible(fields: Value) -> Result<ProviderSettings, ConfigError> {
    let mut provider = json!({
        "name": "openai_compatible",
        "base_url": "https://openrouter.ai/api/v1",
        "model": "synthetic/model",
    });
    for (field, value) in fields.as_object().unwrap() {
        if value.is_null() {
            provider.as_object_mut().unwrap().remove(field);
        } else {
            provider[field] = value.clone();
        }
    }
    Settings::from_options(Some(json!({"provider": provider}))).map(|settings| settings.provider)
}

#[test]
fn openai_compatible_settings_need_a_service_and_a_model() {
    let ProviderSettings::OpenAiCompatible {
        base_url,
        model,
        effort,
        document,
    } = compatible(json!({})).unwrap()
    else {
        panic!("Expected the OpenAI-compatible provider")
    };
    assert_eq!(base_url, "https://openrouter.ai/api/v1");
    assert_eq!(model, "synthetic/model");
    assert_eq!(effort, None);
    assert!(document.is_none());

    for (field, setting) in [
        ("base_url", "provider.base_url"),
        ("model", "provider.model"),
    ] {
        let error = compatible(json!({(field): null})).err().unwrap();
        assert_eq!(error.to_string(), format!("{setting} is required"));
    }
    let error = compatible(json!({"document": {"effort": "low"}}))
        .err()
        .unwrap();
    assert_eq!(error.to_string(), "provider.document.model is required");

    let ProviderSettings::OpenAiCompatible {
        effort, document, ..
    } = compatible(json!({
        "effort": "Ultra",
        "document": {"model": "synthetic/large", "effort": "max"}
    }))
    .unwrap()
    else {
        panic!("Expected the OpenAI-compatible provider")
    };
    // Any value a service may take is sent as written.
    assert_eq!(effort.as_deref(), Some("Ultra"));
    let document = document.unwrap();
    assert_eq!(document.model, "synthetic/large");
    assert_eq!(document.effort.as_deref(), Some("max"));
    // A document without effort sends none, whatever `provider.effort` is.
    let ProviderSettings::OpenAiCompatible { document, .. } = compatible(json!({
        "effort": "low",
        "document": {"model": "synthetic/large"}
    }))
    .unwrap() else {
        panic!("Expected the OpenAI-compatible provider")
    };
    assert_eq!(document.unwrap().effort, None);
}

#[test]
fn openai_compatible_settings_name_rejected_values_without_repeating_them() {
    for (fields, message) in [
        (json!({"model": " "}), "provider.model must be nonempty"),
        (
            json!({"model": "a b"}),
            "provider.model must not contain whitespace",
        ),
        (
            json!({"effort": "very high"}),
            "provider.effort must not contain whitespace",
        ),
        (json!({"effort": ""}), "provider.effort must be nonempty"),
        (
            json!({"document": {"model": "m", "effort": "a\tb"}}),
            "provider.document.effort must be nonempty",
        ),
        (
            json!({"document": {"model": "a b"}}),
            "provider.document.model must not contain whitespace",
        ),
    ] {
        let error = compatible(fields.clone()).err().unwrap().to_string();
        assert!(error.starts_with(message), "{fields}: {error}");
    }
    for fields in [
        json!({"effort": null, "speed": "fast"}),
        json!({"thinking": true}),
        json!({"document": {"model": "m", "base_url": "https://example.com"}}),
        json!({"effort": 1}),
    ] {
        assert!(
            matches!(compatible(fields.clone()), Err(ConfigError::Decode(_))),
            "{fields}"
        );
    }
}

#[test]
fn openai_compatible_services_are_https_or_this_computer() {
    for base_url in [
        "https://openrouter.ai/api/v1",
        "https://generativelanguage.googleapis.com/v1beta/openai/",
        "https://llm.example:8443",
        "http://localhost:11434/v1",
        "http://127.0.0.1:1234/v1",
        "http://[::1]:8080/v1",
    ] {
        assert!(
            compatible(json!({"base_url": base_url})).is_ok(),
            "{base_url}"
        );
    }
    for base_url in [
        "http://openrouter.ai/api/v1",
        "http://192.168.1.20:11434/v1",
        "https://user:synthetic-secret@openrouter.ai/api/v1",
        "https://openrouter.ai/api/v1?key=synthetic-secret",
        "https://openrouter.ai/api/v1#synthetic-secret",
        "ftp://localhost/v1",
        "openrouter.ai/api/v1",
    ] {
        let error = compatible(json!({"base_url": base_url})).err().unwrap();
        assert!(matches!(error, ConfigError::BaseUrl), "{base_url}");
        assert!(!error.to_string().contains("synthetic-secret"));
    }
    let error = compatible(json!({"base_url": format!("https://{}.example", "a".repeat(2048))}))
        .err()
        .unwrap();
    assert_eq!(
        error.to_string(),
        "provider.base_url must be nonempty, without control characters, and at most 2048 bytes"
    );
}
