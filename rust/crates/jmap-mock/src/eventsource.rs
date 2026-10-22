// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Handle the `GET /eventsource` push endpoint (RFC 8620 section 7.3).
//!
//! Run one dedicated thread for each subscriber.
//! Write chunked data through `Request::into_writer` so each update flushes.

use std::collections::{BTreeMap, BTreeSet};
use std::io::Write;
use std::sync::Mutex;
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::time::Duration;

use jmap_proto::push::StateChange;

use crate::state::ServerState;

/// Parsed query parameters for `/eventsource`.
struct EventSourceParams {
    /// No type filter when this field is `None`.
    types: Option<BTreeSet<String>>,
    close_after_state: bool,
    ping_interval: Option<Duration>,
}

fn parse_params(query: &str) -> EventSourceParams {
    let fields: BTreeMap<String, String> = query
        .split('&')
        .filter(|pair| !pair.is_empty())
        .filter_map(|pair| pair.split_once('='))
        .map(|(key, value)| (key.to_owned(), value.to_owned()))
        .collect();
    EventSourceParams {
        types: fields.get("types").and_then(|value| {
            (value != "*").then(|| {
                value
                    .split(',')
                    .filter(|name| !name.is_empty())
                    .map(str::to_owned)
                    .collect()
            })
        }),
        close_after_state: fields.get("closeafter").map(String::as_str) == Some("state"),
        ping_interval: fields
            .get("ping")
            .and_then(|value| value.parse::<u64>().ok())
            .filter(|seconds| *seconds > 0)
            .map(Duration::from_secs),
    }
}

/// Subscribe this request and stream events on one dedicated thread.
pub fn spawn_and_respond(request: tiny_http::Request, query: &str, state: &Mutex<ServerState>) {
    let params = parse_params(query);
    let receiver = state
        .lock()
        .expect("mock state lock")
        .event_source
        .subscribe(params.types.clone());
    std::thread::spawn(move || {
        let mut writer = request.into_writer();
        let head = "HTTP/1.1 200 OK\r\n\
             Content-Type: text/event-stream\r\n\
             Cache-Control: no-cache\r\n\
             Transfer-Encoding: chunked\r\n\
             \r\n";
        if writer.write_all(head.as_bytes()).is_err() || writer.flush().is_err() {
            return;
        }
        stream_events(
            writer.as_mut(),
            &receiver,
            params.ping_interval,
            params.close_after_state,
        );
    });
}

/// Stream `state` and optional `ping` events as flushed chunks.
fn stream_events(
    writer: &mut dyn Write,
    receiver: &Receiver<Vec<u8>>,
    ping_interval: Option<Duration>,
    close_after_state: bool,
) {
    loop {
        let pushed = match ping_interval {
            Some(interval) => match receiver.recv_timeout(interval) {
                Ok(chunk) => Ok(Some(chunk)),
                Err(RecvTimeoutError::Timeout) => Ok(None),
                Err(RecvTimeoutError::Disconnected) => Err(()),
            },
            None => receiver.recv().map(Some).map_err(|_| ()),
        };
        let Ok(pushed) = pushed else { return };
        let (bytes, was_pushed) = match pushed {
            Some(chunk) => (chunk, true),
            None => (
                format_ping_event(ping_interval.expect("a timeout implies an interval")),
                false,
            ),
        };
        if write_chunk(writer, &bytes).is_err() {
            return;
        }
        if was_pushed && close_after_state {
            let _ = end_chunked_body(writer);
            return;
        }
    }
}

/// Write one HTTP chunk from `data`, then flush.
fn write_chunk(writer: &mut dyn Write, data: &[u8]) -> std::io::Result<()> {
    write!(writer, "{:x}\r\n", data.len())?;
    writer.write_all(data)?;
    writer.write_all(b"\r\n")?;
    writer.flush()
}

/// Write the trailing zero-length chunk for `closeafter=state`.
fn end_chunked_body(writer: &mut dyn Write) -> std::io::Result<()> {
    writer.write_all(b"0\r\n\r\n")?;
    writer.flush()
}

/// Encode one `StateChange` as `event: state` with JSON `data`.
/// Skip the `id` resume field.
pub fn format_state_event(change: &StateChange) -> Vec<u8> {
    let data = serde_json::to_string(change).expect("StateChange serializes");
    format!("event: state\ndata: {data}\n\n").into_bytes()
}

/// Encode a `ping` event with the interval in seconds.
fn format_ping_event(interval: Duration) -> Vec<u8> {
    format!(
        "event: ping\ndata: {{\"interval\":{}}}\n\n",
        interval.as_secs()
    )
    .into_bytes()
}

#[cfg(test)]
mod tests {
    use super::parse_params;

    #[test]
    fn closeafter_state_is_recognised() {
        let params = parse_params("types=*&closeafter=state&ping=0");
        assert!(params.close_after_state);
        assert_eq!(params.ping_interval, None);
    }

    #[test]
    fn closeafter_no_persists_the_connection() {
        let params = parse_params("types=*&closeafter=no&ping=0");
        assert!(!params.close_after_state);
    }

    #[test]
    fn a_zero_ping_sends_no_pings() {
        let params = parse_params("ping=0");
        assert_eq!(params.ping_interval, None);
    }

    #[test]
    fn a_positive_ping_is_an_interval_in_seconds() {
        let params = parse_params("ping=300");
        assert_eq!(
            params.ping_interval,
            Some(std::time::Duration::from_secs(300))
        );
    }

    #[test]
    fn no_params_at_all_is_a_persistent_unpinged_connection() {
        let params = parse_params("");
        assert!(!params.close_after_state);
        assert_eq!(params.ping_interval, None);
    }

    #[test]
    fn a_wildcard_types_is_no_filter() {
        assert_eq!(parse_params("types=*").types, None);
    }

    #[test]
    fn an_absent_types_is_no_filter() {
        assert_eq!(parse_params("closeafter=no").types, None);
    }

    #[test]
    fn a_single_type_is_a_one_element_filter() {
        assert_eq!(
            parse_params("types=Mailbox").types,
            Some(std::collections::BTreeSet::from(["Mailbox".to_owned()]))
        );
    }

    #[test]
    fn a_comma_separated_list_is_a_multi_element_filter() {
        assert_eq!(
            parse_params("types=Mailbox,Email").types,
            Some(std::collections::BTreeSet::from([
                "Mailbox".to_owned(),
                "Email".to_owned()
            ]))
        );
    }
}
