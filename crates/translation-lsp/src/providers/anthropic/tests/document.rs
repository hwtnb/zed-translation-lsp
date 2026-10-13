use serde_json::{Value, json};

use super::{PAUSED, SOURCE, automatic};
use crate::{
    config::AnthropicEffort,
    providers::{
        Intent, Output, OutputOptions, TranslationProvider,
        anthropic::{
            AnthropicProvider, ModelOptions,
            test_support::{Fixture, Reply},
        },
        error::{Error, ErrorKind},
    },
};

const RANGE_MODEL: &str = "claude-haiku-5-5";
const TAB_MODEL: &str = "claude-sonnet-5-5";

fn range() -> ModelOptions {
    ModelOptions {
        model: RANGE_MODEL.into(),
        effort: AnthropicEffort::Medium,
        thinking: false,
    }
}

fn tab() -> ModelOptions {
    ModelOptions {
        model: TAB_MODEL.into(),
        effort: AnthropicEffort::Low,
        thinking: true,
    }
}

async fn translate(
    provider: &AnthropicProvider,
    intent: Intent,
    output: Output,
) -> Result<String, Error> {
    provider
        .translate(intent, output, SOURCE, "ja", provider.context()?)
        .await
        .map(|translated| translated.text)
}

// The model and options of each request the fixture received, in order.
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

#[tokio::test(flavor = "current_thread")]
async fn each_output_sends_its_own_model_and_options() {
    let fixture = Fixture::new().await;
    let provider = fixture
        .provider()
        .with_options(OutputOptions::new(range(), Some(tab())));
    for output in [Output::Range, Output::Document, Output::Range] {
        translate(&provider, Intent::Explicit, output)
            .await
            .unwrap();
    }
    let range_sent = json!([RANGE_MODEL, "medium", "disabled"]);
    let tab_sent = json!([TAB_MODEL, "low", "adaptive"]);
    assert_eq!(
        sent(&fixture),
        [range_sent.clone(), tab_sent, range_sent.clone()]
    );

    // Without tab settings, a tab translation sends what a range sends.
    let fixture = Fixture::new().await;
    let provider = fixture
        .provider()
        .with_options(OutputOptions::new(range(), None));
    for output in [Output::Document, Output::Range] {
        translate(&provider, Intent::Explicit, output)
            .await
            .unwrap();
    }
    assert_eq!(sent(&fixture), [range_sent.clone(), range_sent]);
}

#[tokio::test(flavor = "current_thread")]
async fn a_stop_caused_by_one_output_pauses_and_resumes_both() {
    for (failing, resuming) in [
        (Output::Document, Output::Range),
        (Output::Range, Output::Document),
    ] {
        let fixture = Fixture::new().await;
        let provider = fixture
            .provider()
            .with_options(OutputOptions::new(range(), Some(tab())));
        fixture.reply(Reply::failure(404, "not_found_error", "No such model"));
        let error = translate(&provider, Intent::Explicit, failing)
            .await
            .unwrap_err();
        assert_eq!(error.kind(), ErrorKind::Configuration);
        let message = error.to_string();
        assert!(message.contains(PAUSED), "{message}");
        // The advice names the setting whose model was sent.
        assert_eq!(
            message.contains("Check provider.document.model"),
            failing == Output::Document,
            "{message}"
        );
        assert_eq!(
            message.contains("Check provider.model"),
            failing == Output::Range,
            "{message}"
        );

        // Automatic work is rejected before any request, whichever output stopped.
        assert_eq!(
            automatic(&provider).unwrap_err().kind(),
            ErrorKind::Configuration
        );
        let rejected = translate(&provider, Intent::Automatic, Output::Range)
            .await
            .unwrap_err();
        assert_eq!(rejected.kind(), ErrorKind::Configuration);
        assert_eq!(fixture.calls(), 1);

        // An explicit success with the other output's settings resumes it.
        translate(&provider, Intent::Explicit, resuming)
            .await
            .unwrap();
        automatic(&provider).unwrap();
        translate(&provider, Intent::Automatic, Output::Range)
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

#[tokio::test(flavor = "current_thread")]
async fn model_advice_names_the_only_model_setting_without_tab_settings() {
    let fixture = Fixture::new().await;
    let provider = fixture.provider();
    fixture.reply(Reply::failure(404, "not_found_error", "No such model"));
    let message = translate(&provider, Intent::Explicit, Output::Document)
        .await
        .unwrap_err()
        .to_string();
    assert!(message.contains("Check provider.model"), "{message}");
    assert!(!message.contains("provider.document"), "{message}");
}
