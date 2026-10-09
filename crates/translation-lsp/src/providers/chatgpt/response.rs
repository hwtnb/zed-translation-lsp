use super::{Error, SizeLimit, http, response_error};
use crate::config::limits::SizeSetting;
use crate::providers::{error::Service, sse, transport::safe};
use serde_json::{Value, json};
mod output;
use output::Output;

pub(super) async fn response_text(
    response: reqwest::Response,
    secrets: &[&str],
    output_limit: SizeLimit,
) -> Result<String, Error> {
    let status = response.status().as_u16();
    let request_id = http::request_id(response.headers()).map(str::to_owned);
    let mut output = Output::default();
    sse::read_events(response, Service::OpenAi, output_limit, |event| {
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
                let error = response_error(status, &error, request_id.as_deref(), secrets);
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
                Err(error.in_stream(Service::OpenAi, kind.to_owned(), diagnostics))
            }
            "response.incomplete" => Err(Error::failed(
                "ChatGPT response is incomplete; no partial translation was saved",
            )),
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
