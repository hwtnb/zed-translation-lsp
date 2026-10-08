use super::{Error, SizeLimit, http, response_error, temporary_network};
use crate::config::limits::SizeSetting;
use crate::providers::error::{TransportExceeded, TransportScope};
use eventsource_stream::Eventsource;
use futures_util::StreamExt;
use serde_json::{Value, json};
use std::collections::{BTreeMap, HashSet};

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

struct TextPart {
    item_id: String,
    text: String,
    done: bool,
}

struct OutputItem {
    id: Option<String>,
    texts: Vec<String>,
}

fn output_item(value: &Value, output_limit: SizeLimit) -> Result<OutputItem, Error> {
    let mut texts = Vec::new();
    match value["type"].as_str() {
        Some("reasoning") => (),
        Some("message") => {
            if value["status"] != "completed" || value["role"] != "assistant" {
                return Err(Error::failed("OpenAI output message is incomplete"));
            }
            let content = value["content"]
                .as_array()
                .ok_or_else(|| Error::failed("OpenAI output content is invalid"))?;
            let mut bytes = 0usize;
            for part in content {
                if part["type"] != "output_text" {
                    return Err(Error::failed("ChatGPT did not return translation text"));
                }
                let text = part["text"]
                    .as_str()
                    .ok_or_else(|| Error::failed("OpenAI output text is invalid"))?;
                bytes = bytes.saturating_add(text.len());
                output_limit.check(bytes, SizeSetting::Output)?;
                texts.push(text.to_owned());
            }
        }
        _ => return Err(Error::failed("OpenAI returned unexpected output")),
    }
    let id = match value.get("id") {
        None => None,
        Some(Value::String(id)) if !id.is_empty() => Some(id.clone()),
        _ => return Err(Error::failed("OpenAI output item ID is invalid")),
    };
    Ok(OutputItem { id, texts })
}

pub(super) async fn response_text(
    response: reqwest::Response,
    secrets: &[&str],
    output_limit: SizeLimit,
) -> Result<String, Error> {
    let status = response.status().as_u16();
    let request_id = http::request_id(response.headers()).map(str::to_owned);
    let mut raw = RawLimit::default();
    let mut completed = None;
    let mut parts = BTreeMap::<(u64, u64), TextPart>::new();
    let mut done_items = BTreeMap::<u64, OutputItem>::new();
    let mut streamed_bytes = 0usize;
    let mut item_bytes = 0usize;
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
                "response.output_text.delta" | "response.output_text.done" => {
                    let output_index = value["output_index"]
                        .as_u64()
                        .ok_or_else(|| Error::failed("OpenAI text output index is invalid"))?;
                    let content_index = value["content_index"]
                        .as_u64()
                        .ok_or_else(|| Error::failed("OpenAI text content index is invalid"))?;
                    let item_id = value["item_id"]
                        .as_str()
                        .filter(|id| !id.is_empty())
                        .ok_or_else(|| Error::failed("OpenAI text item ID is invalid"))?;
                    let done = kind == "response.output_text.done";
                    let text = value
                        .get(if done { "text" } else { "delta" })
                        .and_then(Value::as_str)
                        .ok_or_else(|| Error::failed("OpenAI streamed text is invalid"))?;
                    if completed.is_some() || done_items.contains_key(&output_index) {
                        return Err(Error::failed("OpenAI text follows completion"));
                    }
                    let key = (output_index, content_index);
                    if let Some(part) = parts.get_mut(&key) {
                        if part.item_id != item_id || part.done {
                            return Err(Error::failed("OpenAI text item is inconsistent"));
                        }
                        if done {
                            if part.text != text {
                                return Err(Error::failed(
                                    "OpenAI text completion is inconsistent",
                                ));
                            }
                            part.done = true;
                        } else {
                            streamed_bytes = streamed_bytes.saturating_add(text.len());
                            output_limit.check(streamed_bytes, SizeSetting::Output)?;
                            part.text.push_str(text);
                        }
                    } else {
                        streamed_bytes = streamed_bytes.saturating_add(text.len());
                        output_limit.check(streamed_bytes, SizeSetting::Output)?;
                        parts.insert(
                            key,
                            TextPart {
                                item_id: item_id.into(),
                                text: text.into(),
                                done,
                            },
                        );
                    }
                }
                "response.output_item.done" => {
                    let index = value["output_index"]
                        .as_u64()
                        .ok_or_else(|| Error::failed("OpenAI output item index is invalid"))?;
                    let item = output_item(&value["item"], output_limit)?;
                    if completed.is_some() || done_items.contains_key(&index) || item.id.is_none() {
                        return Err(Error::failed("OpenAI output item completion is invalid"));
                    }
                    item_bytes = item
                        .texts
                        .iter()
                        .fold(item_bytes, |bytes, text| bytes.saturating_add(text.len()));
                    output_limit.check(item_bytes, SizeSetting::Output)?;
                    done_items.insert(index, item);
                }
                "response.completed" => {
                    if completed.is_some()
                        || value["response"]["status"] != "completed"
                        || !value["response"]["error"].is_null()
                    {
                        return Err(Error::failed("OpenAI completion status is invalid"));
                    }
                    let output = value["response"]["output"]
                        .as_array()
                        .ok_or_else(|| Error::failed("OpenAI completion has no output"))?;
                    let mut items = BTreeMap::new();
                    for (index, value) in output.iter().enumerate() {
                        items.insert(index as u64, output_item(value, output_limit)?);
                    }
                    for output in [&done_items, &items] {
                        let mut ids = HashSet::new();
                        for item in output.values() {
                            if item.id.as_ref().is_some_and(|id| !ids.insert(id)) {
                                return Err(Error::failed("OpenAI output item ID is duplicated"));
                            }
                        }
                    }
                    if items.is_empty() {
                        items = std::mem::take(&mut done_items);
                    } else {
                        for (index, done) in &done_items {
                            let Some(item) = items.get(index) else {
                                return Err(Error::failed("OpenAI completed output omits an item"));
                            };
                            if item.texts != done.texts
                                || item
                                    .id
                                    .as_ref()
                                    .is_some_and(|id| Some(id) != done.id.as_ref())
                            {
                                return Err(Error::failed(
                                    "OpenAI completed output is inconsistent",
                                ));
                            }
                        }
                    }
                    for ((index, content_index), part) in &parts {
                        let Some(item) = items.get(index) else {
                            return Err(Error::failed(
                                "OpenAI completed output omits streamed text",
                            ));
                        };
                        let item_id = item
                            .id
                            .as_ref()
                            .or_else(|| done_items.get(index).and_then(|done| done.id.as_ref()));
                        if item_id.is_some_and(|id| id != &part.item_id)
                            || usize::try_from(*content_index)
                                .ok()
                                .and_then(|index| item.texts.get(index))
                                != Some(&part.text)
                        {
                            return Err(Error::failed(
                                "OpenAI streamed text does not match its completed output",
                            ));
                        }
                    }
                    let mut text = String::new();
                    for item in items.into_values() {
                        for part in item.texts {
                            output_limit.check(
                                text.len().saturating_add(part.len()),
                                SizeSetting::Output,
                            )?;
                            text.push_str(&part);
                        }
                    }
                    if text.is_empty() {
                        return Err(Error::failed("OpenAI completed without translation text"));
                    }
                    completed = Some(text);
                }
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
                _ => (),
            }
        }
    }
    if raw.event != 0 {
        return Err(Error::failed("OpenAI SSE stream ended inside an event"));
    }
    completed.ok_or_else(|| Error::failed("OpenAI stream ended without response.completed"))
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
