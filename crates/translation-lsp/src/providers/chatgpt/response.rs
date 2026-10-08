use super::{Error, SizeLimit, http, response_error, temporary_network};
use crate::config::limits::SizeSetting;
use crate::providers::error::{TransportExceeded, TransportScope};
use eventsource_stream::Eventsource;
use futures_util::StreamExt;
use serde_json::{Value, json};
mod output;
use output::Output;

#[derive(Default)]
pub(super) struct RawLimit {
    total: usize,
    event: usize,
    line_content: bool,
    cr: bool,
}

impl RawLimit {
    pub(super) fn accept(&mut self, bytes: &[u8], output_limit: SizeLimit) -> Result<(), Error> {
        self.total = self
            .total
            .checked_add(bytes.len())
            .ok_or_else(|| Error::failed("OpenAI stream size exceeds the platform byte counter"))?;
        // Allow JSON escaping, repeated snapshots and small delta events without
        // allocating these budgets up front. Bound empty or unrelated events too.
        let stream_limit = output_limit
            .bytes()
            .saturating_mul(1024)
            .saturating_add(1024 * 1024);
        if self.total > stream_limit {
            return Err(TransportExceeded {
                scope: TransportScope::Stream,
                actual_bytes: self.total,
                maximum_bytes: stream_limit,
            }
            .into());
        }
        let event_limit = output_limit
            .bytes()
            .saturating_mul(6)
            .saturating_add(1024 * 1024);
        for &byte in bytes {
            if byte == b'\n' && self.cr {
                self.cr = false;
                continue;
            }
            self.event += 1;
            self.cr = byte == b'\r';
            if matches!(byte, b'\n' | b'\r') {
                if !self.line_content {
                    self.event = 0;
                }
                self.line_content = false;
            } else {
                self.line_content = true;
            }
            if self.event > event_limit {
                return Err(TransportExceeded {
                    scope: TransportScope::Event,
                    actual_bytes: self.event,
                    maximum_bytes: event_limit,
                }
                .into());
            }
        }
        Ok(())
    }
}

pub(super) async fn response_text(
    response: reqwest::Response,
    secrets: &[&str],
    output_limit: SizeLimit,
) -> Result<String, Error> {
    let status = response.status().as_u16();
    let request_id = http::request_id(response.headers()).map(str::to_owned);
    let mut raw = RawLimit::default();
    let mut output = Output::default();
    {
        // eventsource-stream reparses incomplete lines on every chunk. Feed whole
        // lines so large completed-output snapshots are parsed only once.
        let lines = futures_util::stream::try_unfold(
            (response.bytes_stream(), Vec::new(), &mut raw),
            |(mut source, mut pending, raw)| async move {
                loop {
                    let Some(chunk) = source.next().await else {
                        return if pending.is_empty() {
                            Ok(None)
                        } else {
                            Ok(Some((std::mem::take(&mut pending), (source, pending, raw))))
                        };
                    };
                    let chunk = chunk.map_err(temporary_network)?;
                    raw.accept(&chunk, output_limit)?;
                    let end = chunk.iter().rposition(|byte| matches!(byte, b'\r' | b'\n'));
                    pending.extend_from_slice(&chunk);
                    if let Some(end) = end {
                        let split = pending.len() - chunk.len() + end + 1;
                        let tail = pending.split_off(split);
                        return Ok(Some((
                            std::mem::replace(&mut pending, tail),
                            (source, pending, raw),
                        )));
                    }
                }
            },
        );
        let stream = lines.eventsource();
        tokio::pin!(stream);
        while let Some(event) = stream.next().await {
            let event = event.map_err(|error| match error {
                eventsource_stream::EventStreamError::Transport(error) => error,
                _ => Error::failed("OpenAI SSE stream is invalid or truncated"),
            })?;
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
                    } else if let Some(error) = value.get("error").filter(|error| error.is_object())
                    {
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
                    let error = error.in_stream(kind.to_owned(), diagnostics);
                    return Err(error);
                }
                "response.incomplete" => {
                    return Err(Error::failed(
                        "ChatGPT response is incomplete; no partial translation was saved",
                    ));
                }
                _ => output.accept(kind, &value, output_limit)?,
            }
        }
    }
    if raw.event != 0 {
        return Err(Error::failed("OpenAI SSE stream ended inside an event"));
    }
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
                    http::safe(key, secrets, 64)
                } else {
                    "[invalid key]".into()
                }
            })
            .collect::<Vec<_>>()
            .join(",")
    })
}
