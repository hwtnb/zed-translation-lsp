use std::collections::HashSet;

use serde::Deserialize;
use serde_json::Value;

use super::{SERVICE, key};
use crate::{
    config::{
        AnthropicSpeed,
        limits::{SizeLimit, SizeSetting},
    },
    providers::{
        Output,
        error::{Cause, Error, RemoteError, RemoteKind},
        sse,
        transport::{network_error, safe},
    },
};

// The request header that names a workspace. This server never sends it.
const WORKSPACE_HEADER: &str = "anthropic-workspace-id";
const BILLING_ADVICE: &str =
    "Check the Billing page in the Claude Console at https://platform.claude.com/";

#[derive(Clone, Copy)]
enum Advice {
    Key,
    Workspace,
    Billing,
    Model,
}

impl Advice {
    // `configured` is the output whose settings named the request's model.
    fn text(self, configured: Output) -> &'static str {
        match (self, configured) {
            (Self::Key, _) => key::REJECTED,
            (Self::Workspace, _) => key::SEVERAL_WORKSPACES,
            (Self::Billing, _) => BILLING_ADVICE,
            (Self::Model, Output::Range) => "Check provider.model",
            (Self::Model, Output::Document) => "Check provider.document.model",
        }
    }
}

// Documented status and `error.type` pairs. A stream error arrives after HTTP
// 200, so the type is matched first and the status covers bodies without one.
const FAILURES: [(u16, &str, RemoteKind, Option<Advice>); 9] = [
    (
        401,
        "authentication_error",
        RemoteKind::Configuration,
        Some(Advice::Key),
    ),
    (
        402,
        "billing_error",
        RemoteKind::UsageLimit,
        Some(Advice::Billing),
    ),
    (403, "permission_error", RemoteKind::NotEligible, None),
    (
        404,
        "not_found_error",
        RemoteKind::Configuration,
        Some(Advice::Model),
    ),
    (400, "invalid_request_error", RemoteKind::Failed, None),
    (413, "request_too_large", RemoteKind::Failed, None),
    (429, "rate_limit_error", RemoteKind::Temporary, None),
    (500, "api_error", RemoteKind::Temporary, None),
    (529, "overloaded_error", RemoteKind::Temporary, None),
];

fn remote_error(
    status: u16,
    error_type: Option<&str>,
    detail: Option<&str>,
    request_id: Option<&str>,
    secrets: &[&str],
    configured: Output,
) -> Error {
    let detail = detail
        .filter(|detail| !detail.trim().is_empty())
        .unwrap_or("Anthropic request failed");
    // The API requires the workspace header of a key that works in more than
    // one workspace. No request carries the header, so an invalid request that
    // names it means such a key, which only another key can correct.
    let (kind, advice) =
        if error_type == Some("invalid_request_error") && detail.contains(WORKSPACE_HEADER) {
            (RemoteKind::Configuration, Some(Advice::Workspace))
        } else {
            FAILURES
                .iter()
                .find(|failure| Some(failure.1) == error_type)
                .or_else(|| FAILURES.iter().find(|failure| failure.0 == status))
                .map_or_else(
                    || {
                        if status >= 500 {
                            (RemoteKind::Temporary, None)
                        } else {
                            (RemoteKind::Failed, None)
                        }
                    },
                    |failure| (failure.2, failure.3),
                )
        };
    RemoteError {
        service: SERVICE,
        kind,
        status,
        detail: safe(detail, secrets, 512),
        code: error_type.map(|value| safe(value, secrets, 128)),
        param: None,
        request_id: request_id.map(|value| safe(value, secrets, 128)),
        advice: advice.map(|advice| advice.text(configured)),
    }
    .into()
}

fn request_id(headers: &reqwest::header::HeaderMap) -> Option<String> {
    headers
        .get("request-id")
        .and_then(|id| id.to_str().ok())
        .map(str::to_owned)
}

pub(super) async fn failure(
    mut response: reqwest::Response,
    secrets: &[&str],
    configured: Output,
) -> Error {
    let status = response.status().as_u16();
    let request_id = request_id(response.headers());
    let mut bytes = Vec::new();
    let body = loop {
        match response.chunk().await {
            Err(error) => return network_error(SERVICE)(error),
            // Classify an oversized error body from its status alone.
            Ok(Some(chunk)) if bytes.len().saturating_add(chunk.len()) > 1024 * 1024 => {
                break Value::Null;
            }
            Ok(Some(chunk)) => bytes.extend_from_slice(&chunk),
            Ok(None) => {
                break serde_json::from_slice(&bytes).unwrap_or_else(|_| {
                    Value::String(String::from_utf8_lossy(&bytes).into_owned())
                });
            }
        }
    };
    // An error body may come from a proxy in any shape, so it is read leniently.
    let error = body.get("error").filter(|error| error.is_object());
    remote_error(
        status,
        error
            .and_then(|error| error.get("type"))
            .and_then(Value::as_str),
        error
            .and_then(|error| error.get("message"))
            .and_then(Value::as_str)
            .or_else(|| body.as_str()),
        request_id
            .as_deref()
            .or_else(|| body.get("request_id").and_then(Value::as_str)),
        secrets,
        configured,
    )
}

pub(super) struct Completed {
    pub text: String,
    // The speed the response reports it was served at.
    pub speed: Option<String>,
}

// The response value comes from outside, and is only written to the log.
fn reported_speed(value: Option<&str>, secrets: &[&str]) -> Option<String> {
    value
        .map(|value| safe(value, secrets, 32))
        .filter(|value| !value.is_empty())
}

// Fast mode costs more, so a request that names a speed tells in the log the
// speed that `usage.speed` reports it was served at.
pub(super) fn log_speed(requested: AnthropicSpeed, completed: &Completed) {
    eprintln!("{}", speed_line(requested, completed.speed.as_deref()));
}

fn speed_line(requested: AnthropicSpeed, served: Option<&str>) -> String {
    let requested = match requested {
        AnthropicSpeed::Standard => "standard",
        AnthropicSpeed::Fast => "fast",
    };
    format!(
        "translation preview: Anthropic speed requested {requested}, served {}",
        served.unwrap_or("(not reported)")
    )
}

pub(super) async fn text(
    response: reqwest::Response,
    secrets: &[&str],
    output_limit: SizeLimit,
    configured: Output,
) -> Result<Completed, Error> {
    let status = response.status().as_u16();
    let request_id = request_id(response.headers());
    let mut message = Message::default();
    sse::read_events(response, SERVICE, output_limit, |event| {
        match Kind::of(&event.event, &event.data)? {
            Kind::MessageStart => message.started(parse(&event.data)?),
            Kind::ContentBlockStart => message.start(parse(&event.data)?, output_limit),
            Kind::ContentBlockDelta => message.delta(parse(&event.data)?, output_limit),
            Kind::MessageDelta => message.stopping(parse(&event.data)?),
            Kind::MessageStop => message.stop(),
            Kind::Error => {
                let StreamError {
                    error,
                    request_id: reported,
                } = parse(&event.data)?;
                Err(remote_error(
                    status,
                    Some(&error.kind),
                    error.message.as_deref(),
                    request_id.as_deref().or(reported.as_deref()),
                    secrets,
                    configured,
                )
                .in_stream(SERVICE, "error".into(), String::new()))
            }
            Kind::Other => Ok(()),
        }
    })
    .await?;
    message.finish(secrets)
}

// The events that decide the result, and `message_start`, which reports the
// speed. The API may add event types, so every other name, including
// `content_block_stop` and `ping`, is skipped without inspecting its data.
#[derive(Clone, Copy, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
enum Kind {
    MessageStart,
    ContentBlockStart,
    ContentBlockDelta,
    MessageDelta,
    MessageStop,
    Error,
    #[serde(other)]
    Other,
}

// Each handled event repeats its name as `type` in its data.
#[derive(Deserialize)]
struct Declared {
    #[serde(rename = "type")]
    kind: String,
}

// Without an SSE event name, `type` alone identifies the event, and data
// without one is not an event this server handles.
#[derive(Deserialize)]
struct Unnamed {
    #[serde(rename = "type")]
    kind: Option<String>,
}

const MALFORMED: &str = "Anthropic SSE event lacks a required field or has an unexpected value";

impl Kind {
    fn named(name: &str) -> Self {
        let name = serde::de::value::StrDeserializer::<serde::de::value::Error>::new(name);
        Self::deserialize(name).unwrap_or(Self::Other)
    }

    fn of(name: &str, data: &str) -> Result<Self, Error> {
        if name.is_empty() || name == "message" {
            let kind = json::<Unnamed>(data)?.kind;
            return Ok(kind.map_or(Self::Other, |kind| Self::named(&kind)));
        }
        let named = Self::named(name);
        if named == Self::Other {
            return Ok(named);
        }
        if named != Self::named(&json::<Declared>(data)?.kind) {
            return Err(Error::failed(
                "Anthropic SSE event type does not match its data",
            ));
        }
        Ok(named)
    }
}

fn json<'a, T: Deserialize<'a>>(data: &'a str) -> Result<T, Error> {
    serde_json::from_str(data).map_err(|error| {
        Error::failed(if error.is_data() {
            MALFORMED
        } else {
            "Anthropic SSE event contains invalid JSON"
        })
    })
}

// A handled event must carry its required fields with the documented types.
// Fields this server does not read are ignored. `Kind::of` has already
// accepted the data as JSON.
fn parse<'a, T: Deserialize<'a>>(data: &'a str) -> Result<T, Error> {
    serde_json::from_str(data).map_err(|_| Error::failed(MALFORMED))
}

// Only `usage.speed` is read. It is reported here and not in later usage, and
// its shape is not checked: a speed is never a reason to reject a translation.
#[derive(Deserialize)]
struct MessageStart {
    #[serde(default)]
    message: Value,
}

#[derive(Deserialize)]
struct BlockStart {
    index: u64,
    content_block: Block,
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum Block {
    Text {
        text: String,
    },
    // Thinking and block types added later carry no translation text.
    #[serde(other)]
    Other,
}

#[derive(Deserialize)]
struct BlockDelta {
    index: u64,
    delta: Delta,
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum Delta {
    TextDelta {
        text: String,
    },
    #[serde(other)]
    Other,
}

#[derive(Deserialize)]
struct MessageDelta {
    delta: Stop,
}

#[derive(Deserialize)]
struct Stop {
    stop_reason: Option<StopReason>,
    stop_details: Option<StopDetails>,
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum StopReason {
    EndTurn,
    MaxTokens,
    Refusal,
    #[serde(other)]
    Other,
}

#[derive(Deserialize)]
struct StopDetails {
    category: Option<String>,
}

#[derive(Deserialize)]
struct StreamError {
    error: StreamErrorDetail,
    request_id: Option<String>,
}

#[derive(Deserialize)]
struct StreamErrorDetail {
    #[serde(rename = "type")]
    kind: String,
    message: Option<String>,
}

#[derive(Default)]
struct Message {
    text_blocks: HashSet<u64>,
    text: String,
    stop_reason: Option<StopReason>,
    refusal_category: Option<String>,
    speed: Option<String>,
    stopped: bool,
}

impl Message {
    fn open(&self) -> Result<(), Error> {
        if self.stopped {
            return Err(Error::failed(
                "Anthropic stream continued after message_stop",
            ));
        }
        Ok(())
    }

    fn started(&mut self, event: MessageStart) -> Result<(), Error> {
        self.speed = event.message["usage"]["speed"].as_str().map(str::to_owned);
        Ok(())
    }

    fn start(&mut self, event: BlockStart, output_limit: SizeLimit) -> Result<(), Error> {
        self.open()?;
        if let Block::Text { text } = event.content_block {
            self.text_blocks.insert(event.index);
            self.push(&text, output_limit)?;
        }
        Ok(())
    }

    fn delta(&mut self, event: BlockDelta, output_limit: SizeLimit) -> Result<(), Error> {
        self.open()?;
        if let Delta::TextDelta { text } = event.delta {
            if !self.text_blocks.contains(&event.index) {
                return Err(Error::failed(
                    "Anthropic sent text outside a text content block",
                ));
            }
            self.push(&text, output_limit)?;
        }
        Ok(())
    }

    fn stopping(&mut self, event: MessageDelta) -> Result<(), Error> {
        self.open()?;
        if let Some(reason) = event.delta.stop_reason {
            self.stop_reason = Some(reason);
        }
        if let Some(category) = event
            .delta
            .stop_details
            .and_then(|details| details.category)
        {
            self.refusal_category = Some(category);
        }
        Ok(())
    }

    fn stop(&mut self) -> Result<(), Error> {
        self.open()?;
        self.stopped = true;
        Ok(())
    }

    fn push(&mut self, text: &str, output_limit: SizeLimit) -> Result<(), Error> {
        output_limit.check(
            self.text.len().saturating_add(text.len()),
            SizeSetting::Output,
        )?;
        self.text.push_str(text);
        Ok(())
    }

    // Only the transport reader may call this, after EOF and truncation checks.
    fn finish(self, secrets: &[&str]) -> Result<Completed, Error> {
        if !self.stopped {
            return Err(Error::failed("Anthropic stream ended without message_stop"));
        }
        match self.stop_reason {
            Some(StopReason::EndTurn) if !self.text.is_empty() => Ok(Completed {
                speed: reported_speed(self.speed.as_deref(), secrets),
                text: self.text,
            }),
            Some(StopReason::EndTurn) => Err(Error::failed(
                "Anthropic completed without translation text",
            )),
            Some(StopReason::MaxTokens) => Err(Error::failed(
                "Anthropic response stopped at the output token limit; no partial translation was saved",
            )),
            Some(StopReason::Refusal) => Err(Cause::Refusal {
                service: SERVICE,
                category: self
                    .refusal_category
                    .map(|category| safe(&category, secrets, 64)),
            }
            .into()),
            Some(StopReason::Other) | None => Err(Error::failed(
                "Anthropic response ended without completing the translation; no partial translation was saved",
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn speed_line_names_both_values_or_that_none_was_reported() {
        assert_eq!(
            speed_line(AnthropicSpeed::Fast, Some("fast")),
            "translation preview: Anthropic speed requested fast, served fast"
        );
        assert_eq!(
            speed_line(AnthropicSpeed::Standard, None),
            "translation preview: Anthropic speed requested standard, served (not reported)"
        );
    }

    #[test]
    fn reported_speed_is_short_and_without_control_characters_or_secrets() {
        let secrets = ["synthetic-secret"];
        for (value, reported) in [
            (Some("fast"), Some("fast".to_owned())),
            (None, None),
            (Some(""), None),
            (Some("\u{7}\n"), None),
            (Some("fa\u{1b}[2Jst\r\n"), Some("fa[2Jst".to_owned())),
            (Some("x".repeat(40).as_str()), Some("x".repeat(32))),
            (Some("synthetic-secret"), Some("[redacted]".to_owned())),
        ] {
            assert_eq!(reported_speed(value, &secrets), reported, "{value:?}");
        }
    }
}
