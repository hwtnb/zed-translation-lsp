use std::time::Duration;

use openidconnect::{HttpRequest, HttpResponse};
use serde_json::Value;

use crate::providers::error::{Error, ErrorKind};

#[derive(Clone)]
pub(crate) struct Http {
    pub client: reqwest::Client,
    #[cfg(test)]
    revocation_fixture: Option<String>,
}

pub(super) fn request_id(headers: &reqwest::header::HeaderMap) -> Option<&str> {
    headers.get("x-request-id").and_then(|id| id.to_str().ok())
}

pub(super) fn response_error(
    status: u16,
    body: &Value,
    request_id: Option<&str>,
    secrets: &[&str],
) -> Error {
    let error = body
        .get("error")
        .filter(|error| error.is_object())
        .unwrap_or(body);
    let code = error
        .get("code")
        .and_then(Value::as_str)
        .or_else(|| body.get("error").and_then(Value::as_str))
        .unwrap_or("");
    let param = error.get("param").and_then(Value::as_str);
    let detail = error
        .get("message")
        .or_else(|| body.get("detail"))
        .or_else(|| body.get("error_description"))
        .and_then(Value::as_str)
        .or_else(|| body.as_str())
        .unwrap_or("OpenAI request failed");
    let kind = match code {
        "subscription_sharing_usage_limit_exceeded" => ErrorKind::UsageLimit,
        "subscription_sharing_user_not_eligible" => ErrorKind::NotEligible,
        "subscription_sharing_usage_unavailable"
        | "subscription_sharing_user_unavailable"
        | "server_is_overloaded" => ErrorKind::Temporary,
        "subscription_sharing_unsupported_capability"
        | "invalid_client"
        | "model_not_found"
        | "unsupported_value" => ErrorKind::Configuration,
        "invalid_grant"
        | "invalid_refresh_token"
        | "token_expired"
        | "refresh_token_expired"
        | "refresh_token_invalidated"
        | "refresh_token_reused" => ErrorKind::RefreshUnusable,
        _ if status >= 500 => ErrorKind::Temporary,
        _ if status == 400 && matches!(param, Some("model" | "reasoning.effort")) => {
            ErrorKind::Configuration
        }
        _ => ErrorKind::Failed,
    };
    let mut message = format!("OpenAI HTTP {status}: {}", safe(detail, secrets, 512));
    if !code.is_empty() {
        message.push_str(&format!(" (code={})", safe(code, secrets, 128)));
    }
    if let Some(param) = param {
        message.push_str(&format!(" (param={})", safe(param, secrets, 128)));
    }
    if let Some(id) = request_id {
        message.push_str(&format!(" (request_id={})", safe(id, secrets, 128)));
    }
    if kind == ErrorKind::UsageLimit {
        message.push_str(
            ". New ChatGPT plan requests are paused; check https://chatgpt.com/settings/usage",
        );
    }
    if kind == ErrorKind::RefreshUnusable {
        message.push_str(
            ". Choose Continue with ChatGPT / Choose account and reconnect the saved account.",
        );
    }
    Error { kind, message }
}

impl Http {
    pub fn new() -> Result<Self, Error> {
        let client = Self::client_builder()
            .timeout(Duration::from_secs(15))
            .build()
            .map_err(|_| Error::failed("Could not prepare the OpenAI HTTP client"))?;
        Ok(Self {
            client,
            #[cfg(test)]
            revocation_fixture: None,
        })
    }

    fn client_builder() -> reqwest::ClientBuilder {
        reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .retry(reqwest::retry::never())
            .connect_timeout(Duration::from_secs(5))
    }

    pub fn responses_client() -> Result<reqwest::Client, Error> {
        // The worker owns the changing total deadline; OAuth keeps its 15-second cap.
        Self::client_builder()
            .build()
            .map_err(|_| Error::failed("Could not prepare the OpenAI Responses client"))
    }

    #[cfg(test)]
    pub fn synthetic_revocation(base: &str) -> Self {
        let mut http = Self::new().unwrap();
        http.revocation_fixture = Some(base.to_owned());
        http
    }

    // openidconnect 4 uses a public AsyncHttpClient closure. Its HTTP types are
    // compatible with reqwest 0.13; response limits apply before SDK parsing.
    pub async fn oidc(&self, request: HttpRequest) -> Result<HttpResponse, Error> {
        // RFC 7009 makes the SDK require an HTTPS revocation URL. Only tests
        // intercept that one synthetic URL to inspect its real encoded form
        // on the localhost fixture; production never rewrites endpoints.
        #[cfg(test)]
        let request = {
            let mut request = request;
            if let Some(base) = &self.revocation_fixture
                && request.uri().to_string()
                    == format!("{}/revoke", base.replacen("http:", "https:", 1))
            {
                *request.uri_mut() = format!("{base}/revoke").parse().unwrap();
            }
            request
        };
        let secrets: Vec<String> = openidconnect::url::form_urlencoded::parse(request.body())
            .filter(|(key, _)| {
                matches!(
                    key.as_ref(),
                    "code" | "refresh_token" | "token" | "code_verifier"
                )
            })
            .map(|(_, value)| value.into_owned())
            .collect();
        let request = reqwest::Request::try_from(request)
            .map_err(|_| Error::failed("Invalid OIDC HTTP request"))?;
        let mut response = self
            .client
            .execute(request)
            .await
            .map_err(|_| temporary_network())?;
        let mut builder = openidconnect::http::Response::builder().status(response.status());
        for (name, value) in response.headers() {
            builder = builder.header(name, value);
        }
        let mut body = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(|_| temporary_network())? {
            if body.len().saturating_add(chunk.len()) > 512 * 1024 {
                return Err(Error::failed("OIDC response exceeds 512 KiB"));
            }
            body.extend_from_slice(&chunk);
        }
        let response = builder
            .body(body)
            .map_err(|_| Error::failed("Invalid OIDC response headers"))?;
        if !response.status().is_success() {
            let body = serde_json::from_slice(response.body()).unwrap_or_else(|_| {
                Value::String(String::from_utf8_lossy(response.body()).into_owned())
            });
            let secrets: Vec<_> = secrets.iter().map(String::as_str).collect();
            return Err(response_error(
                response.status().as_u16(),
                &body,
                request_id(response.headers()),
                &secrets,
            ));
        }
        Ok(response)
    }

    pub async fn json(
        &self,
        mut response: reqwest::Response,
        secrets: &[&str],
    ) -> Result<Value, Error> {
        let status = response.status();
        let request_id = request_id(response.headers()).map(str::to_owned);
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(|_| temporary_network())? {
            if bytes.len().saturating_add(chunk.len()) > 1024 * 1024 {
                return Err(Error::failed("OpenAI JSON response exceeds 1 MiB"));
            }
            bytes.extend_from_slice(&chunk);
        }
        let body = serde_json::from_slice(&bytes)
            .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&bytes).into_owned()));
        if !status.is_success() {
            return Err(response_error(
                status.as_u16(),
                &body,
                request_id.as_deref(),
                secrets,
            ));
        }
        if body.is_string() {
            return Err(Error::failed("OpenAI returned invalid JSON"));
        }
        Ok(body)
    }
}

pub(super) fn temporary_network() -> Error {
    Error {
        kind: ErrorKind::Temporary,
        message:
            "OpenAI network request failed or timed out; credentials are retained, retry later"
                .into(),
    }
}

pub(super) fn safe(value: &str, secrets: &[&str], maximum: usize) -> String {
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

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn streaming_overload_is_temporary_even_after_http_success() {
        let error = response_error(
            200,
            &json!({"error": {
                "code": "server_is_overloaded", "message": "Please try again later."
            }}),
            None,
            &[],
        );
        assert_eq!(error.kind, ErrorKind::Temporary);
    }

    #[test]
    fn errors_keep_status_detail_and_request_id_without_tokens() {
        let error = response_error(
            429,
            &json!({"error":{"code":"subscription_sharing_usage_limit_exceeded","message":"synthetic-secret"}}),
            Some("req-test"),
            &["synthetic-secret"],
        );
        assert_eq!(error.kind, ErrorKind::UsageLimit);
        assert!(error.message.contains("req-test"));
        assert!(error.message.contains("settings/usage"));
        assert!(!error.message.contains("synthetic-secret"));
        let error = response_error(503, &json!({"detail":"Temporarily unavailable"}), None, &[]);
        assert_eq!(error.kind, ErrorKind::Temporary);
        assert!(error.message.contains("Temporarily unavailable"));
    }
}
