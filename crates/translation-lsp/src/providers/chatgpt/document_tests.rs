use serde_json::{Value, json};

use super::{ChatGptProvider, ModelOptions, test_support::Fixture};
use crate::{
    config::{ChatgptSpeed, ReasoningEffort},
    providers::{
        Context, Intent, Output, OutputOptions, TranslationProvider,
        error::{Error, ErrorKind},
    },
};

const RANGE_MODEL: &str = "synthetic-range-model";
const TAB_MODEL: &str = "synthetic-tab-model";

fn options(model: &str, effort: ReasoningEffort, speed: Option<ChatgptSpeed>) -> ModelOptions {
    ModelOptions {
        model: model.into(),
        effort,
        speed,
    }
}

async fn provider(
    fixture: &Fixture,
    range: ModelOptions,
    document: Option<ModelOptions>,
) -> (ChatGptProvider, Context) {
    let auth = fixture.auth();
    fixture.connected(&auth, 3600).await;
    let provider =
        ChatGptProvider::synthetic(auth, format!("{}/v1", fixture.base), range.model.clone())
            .with_options(OutputOptions::new(range, document));
    let context = provider.context().unwrap();
    (provider, context)
}

async fn translate(
    provider: &ChatGptProvider,
    context: &Context,
    intent: Intent,
    output: Output,
) -> Result<String, Error> {
    provider
        .translate(intent, output, "Read this.", "ja", context.clone())
        .await
        .map(|translated| translated.text)
}

// The model, `reasoning`, and `service_tier` of each translation request, in
// order. `service_tier` is `null` when the request did not carry the field.
fn sent(fixture: &Fixture) -> Vec<Value> {
    let state = fixture.state.lock().unwrap();
    state
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
        .collect()
}

#[tokio::test(flavor = "current_thread")]
async fn each_output_sends_its_own_model_reasoning_and_speed() {
    use ChatgptSpeed::{Fast, Flex, Priority};
    use ReasoningEffort::{High, Low, Medium};
    for (range, document, range_sent, tab_sent) in [
        (
            options(RANGE_MODEL, Low, None),
            Some(options(TAB_MODEL, Medium, Some(Fast))),
            json!([RANGE_MODEL, {"effort": "low"}, null]),
            json!([TAB_MODEL, {"effort": "medium"}, "fast"]),
        ),
        (
            options(RANGE_MODEL, ReasoningEffort::None, Some(Priority)),
            Some(options(RANGE_MODEL, High, None)),
            json!([RANGE_MODEL, {"effort": "none"}, "priority"]),
            json!([RANGE_MODEL, {"effort": "high"}, null]),
        ),
        // Without tab settings, a tab translation sends what a range sends.
        (
            options(RANGE_MODEL, Low, Some(Flex)),
            None,
            json!([RANGE_MODEL, {"effort": "low"}, "flex"]),
            json!([RANGE_MODEL, {"effort": "low"}, "flex"]),
        ),
    ] {
        let fixture = Fixture::new().await;
        let (provider, context) = provider(&fixture, range, document).await;
        for output in [Output::Range, Output::Document, Output::Range] {
            translate(&provider, &context, Intent::Explicit, output)
                .await
                .unwrap();
        }
        assert_eq!(sent(&fixture), [range_sent.clone(), tab_sent, range_sent]);
    }
}

#[tokio::test(flavor = "current_thread")]
async fn a_stop_caused_by_one_output_pauses_and_resumes_both() {
    for (failing, resuming) in [
        (Output::Document, Output::Range),
        (Output::Range, Output::Document),
    ] {
        let fixture = Fixture::new().await;
        fixture.state.lock().unwrap().response_failures.push_back((
            400,
            json!({"error": {
                "code": "model_not_found", "param": "model",
                "message": "The configured model is unsupported"
            }}),
        ));
        let (provider, context) = provider(
            &fixture,
            options(RANGE_MODEL, ReasoningEffort::Low, None),
            Some(options(TAB_MODEL, ReasoningEffort::Low, None)),
        )
        .await;
        let error = translate(&provider, &context, Intent::Explicit, failing)
            .await
            .unwrap_err();
        assert_eq!(error.kind(), ErrorKind::Configuration);

        // Automatic work is rejected before any request, whichever output stopped.
        assert_eq!(
            provider
                .admit(&context, Intent::Automatic)
                .unwrap_err()
                .kind(),
            ErrorKind::Configuration
        );
        let rejected = translate(&provider, &context, Intent::Automatic, Output::Range)
            .await
            .unwrap_err();
        assert_eq!(rejected.kind(), ErrorKind::Configuration);
        assert_eq!(sent(&fixture).len(), 1);

        // An explicit success with the other output's settings resumes it.
        translate(&provider, &context, Intent::Explicit, resuming)
            .await
            .unwrap();
        provider.admit(&context, Intent::Automatic).unwrap();
        translate(&provider, &context, Intent::Automatic, Output::Range)
            .await
            .unwrap();
        let models: Vec<_> = sent(&fixture)
            .into_iter()
            .map(|sent| sent[0].clone())
            .collect();
        let model = |output| match output {
            Output::Range => RANGE_MODEL,
            Output::Document => TAB_MODEL,
        };
        assert_eq!(models, [model(failing), model(resuming), RANGE_MODEL]);
    }
}
