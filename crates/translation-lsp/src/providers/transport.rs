use std::time::Duration;

use reqwest::{
    Client,
    header::{HeaderMap, RETRY_AFTER},
    redirect::Policy,
    retry,
};

use super::error::{Cause, Error, Service};

pub(crate) fn client_builder() -> reqwest::ClientBuilder {
    Client::builder()
        .redirect(Policy::none())
        .retry(retry::never())
        .connect_timeout(Duration::from_secs(5))
}

pub(crate) fn network_error(service: Service) -> impl Fn(reqwest::Error) -> Error {
    move |source| {
        Cause::Network {
            service,
            source: source.without_url(),
        }
        .into()
    }
}

/// A date or invalid delay must not cause a retry earlier than requested.
pub(crate) fn retry_delay(headers: &HeaderMap) -> Option<Duration> {
    headers
        .get(RETRY_AFTER)
        .map_or(Some(Duration::from_secs(2)), |value| {
            value
                .to_str()
                .ok()?
                .parse::<u64>()
                .ok()
                .filter(|seconds| *seconds <= 120)
                .map(Duration::from_secs)
        })
}

pub(crate) fn safe(value: &str, secrets: &[&str], maximum: usize) -> String {
    let mut value = value.to_owned();
    for secret in secrets.iter().filter(|secret| !secret.is_empty()) {
        value = value.replace(secret, "[redacted]");
    }
    value
        .chars()
        .filter(|character| !character.is_control())
        .take(maximum)
        .collect()
}
