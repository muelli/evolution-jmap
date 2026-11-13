// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

#![no_main]

use libfuzzer_sys::fuzz_target;

use jmap_client::eventsource::{
    ALL_TYPES, EventSourceItem, EventSourceTimeouts, SharedHeaders, expand_url,
    parse_chunked_stream, parse_response_head, parse_stream,
};
use jmap_client::transport::CancelFlag;
use jmap_client::url::{origin, origin_of, split_http_scheme};

fuzz_target!(|data: &[u8]| {
    let cancel = CancelFlag::new();
    let (tx, rx) = std::sync::mpsc::channel();

    let _ = parse_stream(std::io::BufReader::new(data), &cancel, &tx);
    while let Ok(item) = rx.try_recv() {
        let _ = format!("{item:?}");
        let cloned = item.clone();
        let _ = item == cloned;
        if let EventSourceItem::State(ref state_change) = item {
            let _ = format!("{state_change:?}");
            let _ = &state_change.changed;
            let _ = &state_change.extra;
            let _ = &state_change.kind;
        }
    }

    let _ = parse_chunked_stream(std::io::BufReader::new(data), &cancel, &tx);
    while let Ok(item) = rx.try_recv() {
        let _ = format!("{item:?}");
        let cloned = item.clone();
        let _ = item == cloned;
        if let EventSourceItem::State(ref state_change) = item {
            let _ = format!("{state_change:?}");
            let _ = &state_change.changed;
            let _ = &state_change.extra;
            let _ = &state_change.kind;
        }
    }

    let cancelled = CancelFlag::new();
    cancelled.cancel();
    let (tx_c, rx_c) = std::sync::mpsc::channel();
    let _ = parse_stream(std::io::BufReader::new(data), &cancelled, &tx_c);
    let _ = parse_chunked_stream(std::io::BufReader::new(data), &cancelled, &tx_c);
    while rx_c.try_recv().is_ok() {}

    if data.len() > 8 {
        let dyn_cancel = CancelFlag::new();
        let (tx_d, rx_d) = std::sync::mpsc::channel();
        let mid = data.len() / 2;
        let _ = parse_stream(std::io::BufReader::new(&data[..mid]), &dyn_cancel, &tx_d);
        dyn_cancel.cancel();
        let _ = parse_stream(std::io::BufReader::new(&data[mid..]), &dyn_cancel, &tx_d);
        while rx_d.try_recv().is_ok() {}
    }

    let _ = parse_response_head(std::io::BufReader::new(data));

    let timeouts = EventSourceTimeouts::default();
    let _ = format!("{timeouts:?}");
    let _ = timeouts == timeouts;

    if let Ok(s) = std::str::from_utf8(data) {
        let _ = expand_url(s, &[], false, 0);
        let _ = expand_url(s, &["Email", "Mailbox"], false, 30);
        let _ = expand_url(s, &[s], true, 0);
        let _ = expand_url(s, &[s, "ContactCard", "CalendarEvent"], true, 300);
        let _ = expand_url(s, &[ALL_TYPES], false, u32::MAX);

        let _ = origin(s);
        let _ = origin_of(s);
        let _ = split_http_scheme(s);

        let headers = SharedHeaders::new(vec![("X-Fuzz".to_string(), s.to_string())]);
        let _ = format!("{headers:?}");
        headers.set(vec![(s.to_string(), "val".to_string())]);
    }
});
