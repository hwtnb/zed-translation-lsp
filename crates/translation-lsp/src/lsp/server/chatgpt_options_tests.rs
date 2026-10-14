// The settings decide what each output sends through `chatgpt_options`.
use serde_json::{Value, json};

use super::*;
use crate::providers::{Intent, chatgpt::test_support::Fixture};

const RANGE_MODEL: &str = "synthetic-range-model";
const TAB_MODEL: &str = "synthetic-tab-model";

// The model, `reasoning`, and `service_tier` each output sends with these
// provider settings. `service_tier` is `null` when the request did not carry
// the field.
async fn sent(provider: Value) -> (Value, Value) {
    let settings = Settings::from_options(Some(json!({"provider": provider}))).unwrap();
    let ProviderSettings::Chatgpt {
        model,
        effort,
        speed,
        document,
    } = settings.provider
    else {
        panic!("Expected ChatGPT settings")
    };
    let fixture = Fixture::new().await;
    let auth = fixture.auth();
    fixture.connected(&auth, 3600).await;
    let provider = ChatGptProvider::synthetic(
        auth,
        format!("{}/v1", fixture.base),
        "replaced-by-the-settings".into(),
    )
    .with_options(chatgpt_options(model, effort, speed, document));
    let context = provider.context().unwrap();
    for output in [Output::Range, Output::Document] {
        provider
            .translate(
                Intent::Explicit,
                output,
                "Read this.",
                "ja",
                context.clone(),
            )
            .await
            .unwrap();
    }
    let state = fixture.state.lock().unwrap();
    let requests: Vec<_> = state
        .records
        .iter()
        .filter(|record| record.path == "/v1/responses")
        .map(|record| {
            let request: Value = serde_json::from_str(&record.body).unwrap();
            json!([
                request["model"],
                request["reasoning"],
                request.get("service_tier")
            ])
        })
        .collect();
    let [range, tab] = requests.try_into().unwrap();
    (range, tab)
}

fn settings(fields: Value) -> Value {
    let mut provider = json!({"name": "chatgpt", "model": RANGE_MODEL});
    provider
        .as_object_mut()
        .unwrap()
        .extend(fields.as_object().unwrap().clone());
    provider
}

#[tokio::test(flavor = "current_thread")]
async fn without_tab_settings_both_outputs_send_the_provider_values() {
    for (provider, sent_by_both) in [
        (
            json!({"name": "chatgpt"}),
            json!(["gpt-6-luna", {"effort": "low"}, null]),
        ),
        (
            settings(json!({})),
            json!([RANGE_MODEL, {"effort": "low"}, null]),
        ),
        (
            settings(json!({"effort": "medium", "speed": "fast"})),
            json!([RANGE_MODEL, {"effort": "medium"}, "fast"]),
        ),
        (
            settings(json!({"effort": "none", "speed": "ultrafast"})),
            json!([RANGE_MODEL, {"effort": "none"}, "ultrafast"]),
        ),
    ] {
        assert_eq!(
            sent(provider.clone()).await,
            (sent_by_both.clone(), sent_by_both),
            "{provider}"
        );
    }
}

#[tokio::test(flavor = "current_thread")]
async fn tab_settings_reach_only_the_tab_output() {
    for (fields, range, tab) in [
        (
            json!({
                "effort": "low",
                "document": {"model": TAB_MODEL, "effort": "medium", "speed": "flex"}
            }),
            json!([RANGE_MODEL, {"effort": "low"}, null]),
            json!([TAB_MODEL, {"effort": "medium"}, "flex"]),
        ),
        // A value omitted for tabs takes its default, whatever the range sends.
        (
            json!({"effort": "high", "speed": "fast", "document": {"model": TAB_MODEL}}),
            json!([RANGE_MODEL, {"effort": "high"}, "fast"]),
            json!([TAB_MODEL, {"effort": "low"}, null]),
        ),
        (
            json!({"speed": "priority", "document": {"model": RANGE_MODEL}}),
            json!([RANGE_MODEL, {"effort": "low"}, "priority"]),
            json!([RANGE_MODEL, {"effort": "low"}, null]),
        ),
    ] {
        assert_eq!(
            sent(settings(fields.clone())).await,
            (range, tab),
            "{fields}"
        );
    }
}
