use std::time::Duration;

use openidconnect::{HttpRequest, HttpResponse, http::Response, url::form_urlencoded};
use reqwest::Request;
use serde_json::Value;

#[cfg(test)]
use crate::providers::error::ErrorKind;
use crate::providers::{
    error::{Error, RemoteError, RemoteKind, Service},
    responses::request_id,
    transport::{self, safe},
};

#[derive(Clone)]
pub(crate) struct Http {
    pub client: reqwest::Client,
    #[cfg(test)]
    revocation_fixture: Option<String>,
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
        "subscription_sharing_usage_limit_exceeded" => RemoteKind::UsageLimit,
        "subscription_sharing_user_not_eligible" => RemoteKind::NotEligible,
        "subscription_sharing_usage_unavailable"
        | "subscription_sharing_user_unavailable"
        | "server_is_overloaded" => RemoteKind::Temporary,
        "subscription_sharing_unsupported_capability"
        | "invalid_client"
        | "model_not_found"
        | "unsupported_value" => RemoteKind::Configuration,
        "invalid_grant"
        | "invalid_refresh_token"
        | "token_expired"
        | "refresh_token_expired"
        | "refresh_token_invalidated"
        | "refresh_token_reused" => RemoteKind::RefreshUnusable,
        _ if status >= 500 => RemoteKind::Temporary,
        _ if status == 400 && matches!(param, Some("model" | "reasoning.effort")) => {
            RemoteKind::Configuration
        }
        _ => RemoteKind::Failed,
    };
    RemoteError {
        service: Service::OpenAi,
        kind,
        status,
        detail: safe(detail, secrets, 512),
        code: (!code.is_empty()).then(|| safe(code, secrets, 128)),
        param: param.map(|value| safe(value, secrets, 128)),
        request_id: request_id.map(|value| safe(value, secrets, 128)),
        advice: match kind {
            RemoteKind::UsageLimit => {
                Some("Check ChatGPT usage at https://chatgpt.com/settings/usage".into())
            }
            RemoteKind::RefreshUnusable => Some(
                "Choose Continue with ChatGPT / Choose account and reconnect the saved account."
                    .into(),
            ),
            _ => None,
        },
    }
    .into()
}

impl Http {
    pub fn new() -> Result<Self, Error> {
        let client = transport::client_builder()
            .timeout(Duration::from_secs(15))
            .build()
            .map_err(|_| Error::failed("Could not prepare the OpenAI HTTP client"))?;
        Ok(Self {
            client,
            #[cfg(test)]
            revocation_fixture: None,
        })
    }

    pub fn responses_client() -> Result<reqwest::Client, Error> {
        // The worker owns the changing total deadline; OAuth keeps its 15-second cap.
        transport::client_builder()
            .build()
            .map_err(|_| Error::failed("Could not prepare the OpenAI Responses client"))
    }

    #[cfg(test)]
    pub fn synthetic_revocation(base: &str) -> Self {
        let mut http = Self::new().unwrap();
        http.revocation_fixture = Some(base.to_owned());
        http
    }

    /// openidconnect 4 uses a public AsyncHttpClient closure. Its HTTP types are
    /// compatible with reqwest 0.13; response limits apply before SDK parsing.
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
        let secrets: Vec<String> = form_urlencoded::parse(request.body())
            .filter(|(key, _)| {
                matches!(
                    key.as_ref(),
                    "code" | "refresh_token" | "token" | "code_verifier"
                )
            })
            .map(|(_, value)| value.into_owned())
            .collect();
        let request =
            Request::try_from(request).map_err(|_| Error::failed("Invalid OIDC HTTP request"))?;
        let mut response = self
            .client
            .execute(request)
            .await
            .map_err(temporary_network)?;
        let mut builder = Response::builder().status(response.status());
        for (name, value) in response.headers() {
            builder = builder.header(name, value);
        }
        let mut body = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(temporary_network)? {
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

    /// The error that a non-success response reports.
    pub async fn failure(&self, response: reqwest::Response, secrets: &[&str]) -> Error {
        let status = response.status();
        let request_id = request_id(response.headers()).map(str::to_owned);
        match error_body(response).await {
            Ok(body) => response_error(status.as_u16(), &body, request_id.as_deref(), secrets),
            Err(error) => error,
        }
    }
}

/// The JSON of an error response, or its text when it is not JSON.
async fn error_body(mut response: reqwest::Response) -> Result<Value, Error> {
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(temporary_network)? {
        if bytes.len().saturating_add(chunk.len()) > 1024 * 1024 {
            return Err(Error::failed("OpenAI JSON response exceeds 1 MiB"));
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(serde_json::from_slice(&bytes)
        .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&bytes).into_owned())))
}

pub(super) fn temporary_network(source: reqwest::Error) -> Error {
    transport::network_error(Service::OpenAi)(source)
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
        assert_eq!(error.kind(), ErrorKind::Temporary);
    }

    #[test]
    fn errors_keep_status_detail_and_request_id_without_tokens() {
        let error = response_error(
            429,
            &json!({"error":{"code":"subscription_sharing_usage_limit_exceeded","message":"synthetic-secret"}}),
            Some("req-test"),
            &["synthetic-secret"],
        );
        assert_eq!(error.kind(), ErrorKind::UsageLimit);
        assert!(error.to_string().contains("req-test"));
        assert!(error.to_string().contains("settings/usage"));
        assert!(!error.to_string().contains("synthetic-secret"));
        let error = response_error(503, &json!({"detail":"Temporarily unavailable"}), None, &[]);
        assert_eq!(error.kind(), ErrorKind::Temporary);
        assert!(error.to_string().contains("Temporarily unavailable"));
    }
}
