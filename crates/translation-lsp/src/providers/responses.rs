//! The streamed replies of the OpenAI Responses API, which the ChatGPT and
//! OpenAI API providers share.
mod output;

use reqwest::header::HeaderMap;
use serde_json::{Value, json};

use super::{
    error::{Error, Service},
    sse,
    transport::safe,
};
use crate::config::limits::{SizeLimit, SizeSetting};
use output::Output;

/// What tells the providers of the Responses API apart.
pub(crate) struct Reading {
    pub service: Service,
    /// Reported when a message holds something besides output text.
    pub not_text: &'static str,
    /// The error that a failed response or a stream error reports, from its
    /// status, body, request ID, and the texts it must not show.
    pub failure: fn(u16, &Value, Option<&str>, &[&str]) -> Error,
    /// The error of a response that ended incomplete, from its event.
    pub incomplete: fn(&Value) -> Error,
}

pub(crate) fn request_id(headers: &HeaderMap) -> Option<&str> {
    headers.get("x-request-id").and_then(|id| id.to_str().ok())
}

pub(crate) async fn response_text(
    response: reqwest::Response,
    secrets: &[&str],
    output_limit: SizeLimit,
    reading: &Reading,
) -> Result<String, Error> {
    let status = response.status().as_u16();
    let request_id = request_id(response.headers()).map(str::to_owned);
    let mut output = Output::new(reading.not_text);
    sse::read_events(response, reading.service, output_limit, |event| {
        let value: Value = serde_json::from_str(&event.data)
            .map_err(|_| Error::failed("OpenAI SSE event contains invalid JSON"))?;
        let kind = value
            .get("type")
            .and_then(Value::as_str)
            .ok_or_else(|| Error::failed("OpenAI SSE event has no type"))?;
        if !event.event.is_empty() && event.event != "message" && event.event != kind {
            return Err(Error::failed(
                "OpenAI SSE event type does not match its data",
            ));
        }
        match kind {
            "response.failed" | "error" => {
                let error = if kind == "response.failed" {
                    json!({"error":value["response"]["error"]})
                } else if let Some(error) = value.get("error").filter(|error| error.is_object()) {
                    json!({"error":error})
                } else {
                    json!({"error":value})
                };
                let fields = &error["error"];
                let unknown = fields.get("code").and_then(Value::as_str).is_none()
                    || fields.get("message").and_then(Value::as_str).is_none();
                let error = (reading.failure)(status, &error, request_id.as_deref(), secrets);
                let diagnostics = if unknown {
                    format!(
                        " (event_keys=[{}], response_keys=[{}], error_keys=[{}])",
                        error_keys(&value, secrets),
                        error_keys(&value["response"], secrets),
                        error_keys(fields, secrets),
                    )
                } else {
                    String::new()
                };
                Err(error.in_stream(reading.service, kind.to_owned(), diagnostics))
            }
            "response.incomplete" => Err((reading.incomplete)(&value)),
            _ => output.accept(kind, &value, output_limit),
        }
    })
    .await?;
    output.finish()
}

fn error_keys(value: &Value, secrets: &[&str]) -> String {
    value.as_object().map_or_else(String::new, |object| {
        object
            .keys()
            .take(8)
            .map(|key| {
                if key
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
                {
                    safe(key, secrets, 64)
                } else {
                    "[invalid key]".into()
                }
            })
            .collect::<Vec<_>>()
            .join(",")
    })
}
