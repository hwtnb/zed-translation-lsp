use std::mem;

use eventsource_stream::{Event, EventStreamError, Eventsource};
use futures_util::{StreamExt, stream};

use super::{
    error::{Error, Service, TransportExceeded, TransportScope},
    transport::network_error,
};
use crate::config::limits::SizeLimit;

pub(crate) struct RawLimit {
    service: Service,
    total: usize,
    event: usize,
    line_content: bool,
    cr: bool,
}

impl RawLimit {
    pub(crate) fn new(service: Service) -> Self {
        Self {
            service,
            total: 0,
            event: 0,
            line_content: false,
            cr: false,
        }
    }

    pub(crate) fn accept(&mut self, bytes: &[u8], output_limit: SizeLimit) -> Result<(), Error> {
        self.total = self.total.checked_add(bytes.len()).ok_or_else(|| {
            Error::protocol(
                self.service,
                "stream size exceeds the platform byte counter",
            )
        })?;
        // Allow JSON escaping, repeated snapshots and small delta events without
        // allocating these budgets up front. Bound empty or unrelated events too.
        let stream_limit = output_limit
            .bytes()
            .saturating_mul(1024)
            .saturating_add(1024 * 1024);
        if self.total > stream_limit {
            return Err(TransportExceeded {
                service: self.service,
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
                    service: self.service,
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

/// Passes each complete event to `accept` and returns once the stream ends on an
/// event boundary. The caller decides whether the events form a complete response.
pub(crate) async fn read_events(
    response: reqwest::Response,
    service: Service,
    output_limit: SizeLimit,
    mut accept: impl FnMut(Event) -> Result<(), Error>,
) -> Result<(), Error> {
    let mut raw = RawLimit::new(service);
    {
        // eventsource-stream reparses incomplete lines on every chunk. Feed whole
        // lines so large completed-output snapshots are parsed only once.
        let lines = stream::try_unfold(
            (response.bytes_stream(), Vec::new(), &mut raw),
            |(mut source, mut pending, raw)| async move {
                loop {
                    let Some(chunk) = source.next().await else {
                        return if pending.is_empty() {
                            Ok(None)
                        } else {
                            Ok(Some((mem::take(&mut pending), (source, pending, raw))))
                        };
                    };
                    let chunk = chunk.map_err(network_error(service))?;
                    raw.accept(&chunk, output_limit)?;
                    let end = chunk.iter().rposition(|byte| matches!(byte, b'\r' | b'\n'));
                    pending.extend_from_slice(&chunk);
                    if let Some(end) = end {
                        let split = pending.len() - chunk.len() + end + 1;
                        let tail = pending.split_off(split);
                        return Ok(Some((
                            mem::replace(&mut pending, tail),
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
                EventStreamError::Transport(error) => error,
                _ => Error::protocol(service, "SSE stream is invalid or truncated"),
            })?;
            accept(event)?;
        }
    }
    if raw.event != 0 {
        return Err(Error::protocol(service, "SSE stream ended inside an event"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn raw_stream_limits_scale_with_output_and_bound_unfinished_events() {
        let small = SizeLimit::try_from(1).unwrap();
        let large = SizeLimit::try_from(2).unwrap();
        let event_budget = 1024 * 6 + 1024 * 1024;
        let event = vec![b'x'; event_budget];
        let mut raw = RawLimit::new(Service::OpenAi);
        raw.accept(&event, small).unwrap();
        assert!(
            raw.accept(b"x", small)
                .unwrap_err()
                .to_string()
                .contains("SSE event")
        );
        RawLimit::new(Service::OpenAi)
            .accept(&[event, vec![b'x']].concat(), large)
            .unwrap();
        let stream_budget = 1024 * 1024 + 1024 * 1024;
        let stream = vec![b'\n'; stream_budget];
        let mut raw = RawLimit::new(Service::OpenAi);
        raw.accept(&stream, small).unwrap();
        assert!(
            raw.accept(b"\n", small)
                .unwrap_err()
                .to_string()
                .contains("stream")
        );
        RawLimit::new(Service::OpenAi)
            .accept(&[stream, vec![b'\n']].concat(), large)
            .unwrap();
    }
}
