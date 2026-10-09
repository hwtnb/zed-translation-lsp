// The settings decide what each output sends through `chatgpt_options`.
use serde_json::{Value, json};

use super::*;
use crate::providers::{Intent, chatgpt::test_support::Fixture};

const RANGE_MODEL: &str = "synthetic-range-model";
const TAB_MODEL: &str = "synthetic-tab-model";

// The model and `reasoning` each output sends with these provider settings.
// `reasoning` is `null` when the request did not carry the field.
async fn sent(provider: Value) -> (Value, Value) {
    let settings = Settings::from_options(Some(json!({"provider": provider}))).unwrap();
    let ProviderSettings::Chatgpt {
        model,
        reasoning_effort,
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
    .with_options(chatgpt_options(model, reasoning_effort, document));
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
            json!([request["model"], request.get("reasoning")])
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
    let low = json!([RANGE_MODEL, {"effort": "low"}]);
    assert_eq!(
        sent(settings(json!({"reasoning_effort": "low"}))).await,
        (low.clone(), low)
    );
    let unset = json!([RANGE_MODEL, null]);
    assert_eq!(sent(settings(json!({}))).await, (unset.clone(), unset));
}

#[tokio::test(flavor = "current_thread")]
async fn tab_settings_reach_only_the_tab_output() {
    for (fields, range, tab) in [
        (
            json!({
                "reasoning_effort": "low",
                "document": {"model": TAB_MODEL, "reasoning_effort": "medium"}
            }),
            json!([RANGE_MODEL, {"effort": "low"}]),
            json!([TAB_MODEL, {"effort": "medium"}]),
        ),
        // An effort omitted for tabs is not sent, whatever the range sends.
        (
            json!({"reasoning_effort": "low", "document": {"model": TAB_MODEL}}),
            json!([RANGE_MODEL, {"effort": "low"}]),
            json!([TAB_MODEL, null]),
        ),
        // Nor does the tab's effort reach the range.
        (
            json!({"document": {"model": TAB_MODEL, "reasoning_effort": "high"}}),
            json!([RANGE_MODEL, null]),
            json!([TAB_MODEL, {"effort": "high"}]),
        ),
        (
            json!({"reasoning_effort": "none", "document": {"model": RANGE_MODEL}}),
            json!([RANGE_MODEL, {"effort": "none"}]),
            json!([RANGE_MODEL, null]),
        ),
    ] {
        assert_eq!(
            sent(settings(fields.clone())).await,
            (range, tab),
            "{fields}"
        );
    }
}
