// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

#![no_main]

use libfuzzer_sys::fuzz_target;

use jmap_client::eventsource::{
    ALL_TYPES, EventSourceItem, EventSourceTimeouts, SharedHeaders, expand_url,
    parse_chunked_stream, parse_chunked_stream_with_last_id, parse_response_head, parse_stream,
    parse_stream_with_last_id, parse_url, safe_header_name, safe_header_value,
};
use jmap_client::transport::CancelFlag;
use jmap_client::url::{origin, origin_of, same_origin, split_http_scheme};

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
            for (account_id, type_states) in &state_change.changed {
                let _ = account_id.as_str();
                for (type_name, state) in type_states {
                    let _ = type_name.as_str();
                    let _ = state.as_str();
                }
            }
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
            for (account_id, type_states) in &state_change.changed {
                let _ = account_id.as_str();
                for (type_name, state) in type_states {
                    let _ = type_name.as_str();
                    let _ = state.as_str();
                }
            }
        }
    }

    let last_id_slot = std::sync::Arc::new(std::sync::Mutex::new(None));
    let (tx_id, rx_id) = std::sync::mpsc::channel();
    let _ = parse_stream_with_last_id(
        std::io::BufReader::new(data),
        &cancel,
        &tx_id,
        &last_id_slot,
    );
    while rx_id.try_recv().is_ok() {}
    let _ = format!("{:?}", *last_id_slot.lock().unwrap());

    let last_id_chunk = std::sync::Arc::new(std::sync::Mutex::new(Some("seed-id".to_string())));
    let (tx_cid, rx_cid) = std::sync::mpsc::channel();
    let _ = parse_chunked_stream_with_last_id(
        std::io::BufReader::new(data),
        &cancel,
        &tx_cid,
        &last_id_chunk,
    );
    while rx_cid.try_recv().is_ok() {}
    let _ = format!("{:?}", *last_id_chunk.lock().unwrap());

    let (tx_small, rx_small) = std::sync::mpsc::channel();
    let _ = parse_stream(
        std::io::BufReader::with_capacity(1, data),
        &cancel,
        &tx_small,
    );
    let _ = parse_chunked_stream(
        std::io::BufReader::with_capacity(1, data),
        &cancel,
        &tx_small,
    );
    let _ = parse_stream(
        std::io::BufReader::with_capacity(7, data),
        &cancel,
        &tx_small,
    );
    let _ = parse_chunked_stream(
        std::io::BufReader::with_capacity(7, data),
        &cancel,
        &tx_small,
    );
    while rx_small.try_recv().is_ok() {}

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
    let _ = parse_response_head(std::io::BufReader::with_capacity(1, data));
    let _ = parse_response_head(std::io::BufReader::with_capacity(7, data));

    let timeouts = EventSourceTimeouts::default();
    let _ = format!("{timeouts:?}");
    let cloned = timeouts;
    let _ = timeouts == cloned;

    let custom_timeouts = EventSourceTimeouts {
        connect: std::time::Duration::from_millis(data.len() as u64),
        read_head: std::time::Duration::from_secs(1),
        stream_read: std::time::Duration::from_secs(2),
    };
    let _ = format!("{custom_timeouts:?}");
    let _ = timeouts == custom_timeouts;

    let empty_headers = SharedHeaders::new(vec![]);
    let _ = format!("{empty_headers:?}");
    let cloned_empty = empty_headers.clone();
    let _ = format!("{cloned_empty:?}");
    empty_headers.set(vec![]);

    if let Ok(s) = std::str::from_utf8(data) {
        let _ = expand_url(s, &[], false, 0);
        let _ = expand_url(s, &["Email", "Mailbox"], false, 30);
        let _ = expand_url(s, &[s], true, 0);
        let _ = expand_url(s, &[s, "ContactCard", "CalendarEvent"], true, 300);
        let _ = expand_url(s, &[ALL_TYPES], false, u32::MAX);
        let _ = expand_url(s, &["*"], true, 60);
        let _ = expand_url(s, &[s, "custom/type", "a,b"], false, 15);
        let _ = expand_url(
            "https://example.com/eventsource/{types}/{closeafter}/{ping}",
            &[s],
            false,
            10,
        );
        let _ = expand_url(
            "https://example.com/eventsource?types={types}&ping={ping}",
            &[s, "Thread"],
            true,
            120,
        );
        let _ = expand_url("https://example.com/eventsource?custom=1", &[s], true, 0);
        let _ = expand_url(
            "https://example.com/eventsource/{types}",
            &[s, "Email,Mailbox", "*", "a/b", "c?d#e"],
            true,
            60,
        );

        let _ = origin(s);
        let _ = origin_of(s);
        let _ = split_http_scheme(s);
        let _ = same_origin(s, s);
        let _ = same_origin(s, "https://example.com");
        let _ = same_origin("https://example.com", s);
        let _ = same_origin(s, "http://example.com:80");
        let _ = same_origin(s, "https://example.com:443");
        let _ = same_origin(s, "http://localhost");
        let _ = same_origin(s, "https://[::1]:8443");
        let _ = same_origin(s, "http://[::1]:80/");
        let _ = same_origin(s, "https://example.org:443/path?query#frag");

        let headers = SharedHeaders::new(vec![("X-Fuzz".to_string(), s.to_string())]);
        let _ = format!("{headers:?}");
        let cloned_headers = headers.clone();
        let _ = format!("{cloned_headers:?}");
        headers.set(vec![(s.to_string(), "val".to_string())]);
        headers.set(vec![("Last-Event-ID".to_string(), s.to_string())]);

        let multi_headers = SharedHeaders::new(vec![
            ("Header-1".to_string(), s.to_string()),
            ("Header-2".to_string(), "val".to_string()),
        ]);
        let _ = format!("{multi_headers:?}");
        let cloned_multi = multi_headers.clone();
        let _ = format!("{cloned_multi:?}");
        multi_headers.set(vec![(s.to_string(), s.to_string())]);
        multi_headers.set(vec![
            ("Authorization".to_string(), format!("Bearer {s}")),
            ("Last-Event-ID".to_string(), s.to_string()),
        ]);

        let reconnected = EventSourceItem::Reconnected {
            last_event_id: Some(s.to_string()),
            reconnect_count: data.len() as u64,
        };
        let _ = format!("{reconnected:?}");
        let cloned_recon = reconnected.clone();
        let _ = reconnected == cloned_recon;

        let reconnected_none = EventSourceItem::Reconnected {
            last_event_id: None,
            reconnect_count: 0,
        };
        let _ = reconnected == reconnected_none;

        let _ = safe_header_name(s);
        let _ = safe_header_value(s);
        if let Some(parts) = parse_url(s) {
            let _ = format!("{parts:?}");
            let cloned_parts = parts.clone();
            let _ = parts == cloned_parts;
            let _ = parts.tls;
            let _ = parts.authority.as_str();
            let _ = parts.dial.as_str();
            let _ = parts.host.as_str();
            let _ = parts.path_and_query.as_str();
        }
        for url_tmpl in [
            format!("http://{s}/eventsource"),
            format!("https://{s}/eventsource"),
            format!("http://example.com:80/{s}"),
            format!("https://[::1]:8443/{s}"),
        ] {
            let _ = parse_url(&url_tmpl);
        }
    }

    let lossy = String::from_utf8_lossy(data);
    let _ = safe_header_name(&lossy);
    let _ = safe_header_value(&lossy);
    let _ = parse_url(&lossy);
    let _ = origin(&lossy);
    let _ = origin_of(&lossy);
    let _ = split_http_scheme(&lossy);
    let _ = same_origin(&lossy, &lossy);
    let _ = same_origin(&lossy, "https://example.com");
    let _ = same_origin("https://example.com", &lossy);
    let _ = expand_url(&lossy, &[], false, 0);
    let _ = expand_url(&lossy, &[&lossy], true, 60);

    let builder = jmap_client::Client::builder();
    let _ = format!("{builder:?}");
    let _ = builder.rebase_urls_to_origin(data.len() % 2 == 0);
});
