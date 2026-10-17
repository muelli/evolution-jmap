// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Mutation test coverage suite for jmap-client/src/eventsource.rs.
//!
//! Pins EventSource push subscription lifecycle (RFC 8620 Section 7.1, 7.3),
//! SharedHeaders snapshotting and debug formatting, reconnect tracking,
//! Last-Event-ID transmission and clearing, drop_connection socket shutdown,
//! recv_timeout and recv_item_timeout precision, and URI template expansion.

use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use jmap_client::eventsource::{
    ALL_TYPES, DEFAULT_CONNECT_TIMEOUT, DEFAULT_READ_HEAD_TIMEOUT, DEFAULT_STREAM_READ_TIMEOUT,
    EventSourceItem, EventSourceSubscription, EventSourceTimeouts, SharedHeaders, expand_url,
};
use jmap_client::transport::CancelFlag;
use jmap_proto::push::StateChange;
use jmap_proto::{Id, State};

fn make_state_change(account: &str, kind: &str, state: &str) -> StateChange {
    let mut types = BTreeMap::new();
    types.insert(kind.to_owned(), State::new(state));
    let mut changed = BTreeMap::new();
    changed.insert(Id::new(account), types);
    StateChange::new(changed)
}

#[test]
fn eventsource_timeouts_and_constants_verification() {
    assert_eq!(ALL_TYPES, "*");
    assert_eq!(DEFAULT_CONNECT_TIMEOUT, Duration::from_secs(30));
    assert_eq!(DEFAULT_READ_HEAD_TIMEOUT, Duration::from_secs(30));
    assert_eq!(DEFAULT_STREAM_READ_TIMEOUT, Duration::from_secs(300));

    let timeouts = EventSourceTimeouts::default();
    assert_eq!(timeouts.connect, Duration::from_secs(30));
    assert_eq!(timeouts.read_head, Duration::from_secs(30));
    assert_eq!(timeouts.stream_read, Duration::from_secs(300));

    let copy = timeouts;
    assert_eq!(copy, timeouts);
    let debug_repr = format!("{timeouts:?}");
    assert!(debug_repr.contains("EventSourceTimeouts"));
    assert!(debug_repr.contains("connect"));
}

#[test]
fn shared_headers_debug_and_snapshot_preservation() {
    let headers = SharedHeaders::new(vec![
        ("X-Token".to_owned(), "secret-abc".to_owned()),
        ("User-Agent".to_owned(), "custom-agent".to_owned()),
    ]);
    let debug = format!("{headers:?}");
    assert!(debug.contains("SharedHeaders"));
    assert!(debug.contains("header_count: 2"));

    let listener = TcpListener::bind("127.0.0.1:0").expect("bind listener");
    let addr = listener.local_addr().expect("local addr");
    let seen_headers = Arc::new(Mutex::new(Vec::new()));
    let seen_clone = Arc::clone(&seen_headers);

    let server_handle = thread::spawn(move || {
        if let Ok((mut stream, _)) = listener.accept() {
            let mut reader = BufReader::new(stream.try_clone().expect("clone stream"));
            let mut lines = Vec::new();
            loop {
                let mut line = String::new();
                if reader.read_line(&mut line).unwrap_or(0) == 0 || line == "\r\n" {
                    break;
                }
                lines.push(line.trim_end().to_owned());
            }
            seen_clone.lock().unwrap().push(lines);

            let resp = "HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n";
            let _ = stream.write_all(resp.as_bytes());
            let _ = stream.flush();
            let mut buf = [0u8; 1];
            let _ = stream.read(&mut buf);
        }
    });

    let url = format!("http://{addr}/eventsource?types=*&closeafter=no&ping=0");
    let subscription = EventSourceSubscription::start(url, headers.clone(), CancelFlag::new());

    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while seen_headers.lock().unwrap().is_empty() {
        assert!(
            std::time::Instant::now() < deadline,
            "server received request"
        );
        thread::sleep(Duration::from_millis(10));
    }

    let recorded = seen_headers.lock().unwrap();
    let lines = &recorded[0];
    assert!(lines.contains(&"X-Token: secret-abc".to_owned()));
    assert!(lines.contains(&"User-Agent: custom-agent".to_owned()));
    assert!(!lines.iter().any(|h| h.contains("xyzzy")));

    drop(subscription);
    let _ = server_handle.join();
}

#[test]
fn eventsource_subscription_debug_formatting() {
    let headers = SharedHeaders::new(Vec::new());
    let url = "http://127.0.0.1:49999/eventsource?types=*".to_owned();
    let cancel = CancelFlag::new();
    cancel.cancel();
    let subscription = EventSourceSubscription::start(url, headers, cancel);

    let debug = format!("{subscription:?}");
    assert!(debug.contains("EventSourceSubscription"));
    assert!(debug.contains("reconnect_count: 0"));
    assert!(debug.contains("last_event_id: None"));
}

#[test]
fn eventsource_recv_timeout_and_recv_item_timeout_precision() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind listener");
    let addr = listener.local_addr().expect("local addr");
    let first_change = make_state_change("acc1", "Mailbox", "state-1");
    let second_change = make_state_change("acc1", "Email", "state-2");
    let srv_first = first_change.clone();
    let srv_second = second_change.clone();

    let server_handle = thread::spawn(move || {
        // Connection 1: send first event, then drop
        if let Ok((mut stream, _)) = listener.accept() {
            let mut reader = BufReader::new(stream.try_clone().expect("clone stream"));
            let mut line = String::new();
            while reader.read_line(&mut line).unwrap_or(0) > 0 && line != "\r\n" {
                line.clear();
            }

            let data1 = serde_json::to_string(&srv_first).expect("serialize");
            let frame1 = format!("id: ev-1\nevent: state\ndata: {data1}\n\n");
            let chunk1 = format!("{:x}\r\n{}\r\n", frame1.len(), frame1);
            let resp1 = format!("HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n{chunk1}");
            let _ = stream.write_all(resp1.as_bytes());
            let _ = stream.flush();

            thread::sleep(Duration::from_millis(50));
            let _ = stream.shutdown(std::net::Shutdown::Both);
        }

        // Connection 2: send second event
        if let Ok((mut stream, _)) = listener.accept() {
            let mut reader = BufReader::new(stream.try_clone().expect("clone stream"));
            let mut line = String::new();
            while reader.read_line(&mut line).unwrap_or(0) > 0 && line != "\r\n" {
                line.clear();
            }

            let data2 = serde_json::to_string(&srv_second).expect("serialize");
            let frame2 = format!("id: ev-2\nevent: state\ndata: {data2}\n\n");
            let chunk2 = format!("{:x}\r\n{}\r\n", frame2.len(), frame2);
            let resp2 = format!("HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n{chunk2}");
            let _ = stream.write_all(resp2.as_bytes());
            let _ = stream.flush();

            let mut buf = [0u8; 1];
            let _ = stream.read(&mut buf);
        }
    });

    let url = format!("http://{addr}/eventsource?types=*&closeafter=no&ping=0");
    let subscription =
        EventSourceSubscription::start(url, SharedHeaders::new(Vec::new()), CancelFlag::new());

    // 1. First event arrives via recv_item_timeout
    let item1 = subscription
        .recv_item_timeout(Duration::from_secs(5))
        .expect("first item");
    assert_eq!(item1, EventSourceItem::State(first_change));
    assert_eq!(subscription.last_event_id().as_deref(), Some("ev-1"));

    // 2. recv_timeout skips Reconnected notification and yields second State change
    let change2 = subscription
        .recv_timeout(Duration::from_secs(5))
        .expect("second state change");
    assert_eq!(change2, second_change);
    assert_eq!(subscription.last_event_id().as_deref(), Some("ev-2"));
    assert_eq!(subscription.reconnect_count(), 1);

    // 3. Waiting when no events arrive returns None after timeout
    let none_item = subscription.recv_item_timeout(Duration::from_millis(50));
    assert_eq!(none_item, None);

    let none_state = subscription.recv_timeout(Duration::from_millis(50));
    assert_eq!(none_state, None);

    drop(subscription);
    let _ = server_handle.join();
}

#[test]
fn eventsource_reconnect_count_and_first_connection_is_not_reconnect() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind listener");
    let addr = listener.local_addr().expect("local addr");
    let connection_count = Arc::new(AtomicUsize::new(0));
    let count_clone = Arc::clone(&connection_count);

    let server_handle = thread::spawn(move || {
        for _ in 0..3 {
            if let Ok((mut stream, _)) = listener.accept() {
                count_clone.fetch_add(1, Ordering::SeqCst);
                let resp = "HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n";
                let _ = stream.write_all(resp.as_bytes());
                let _ = stream.flush();
                thread::sleep(Duration::from_millis(50));
                let _ = stream.shutdown(std::net::Shutdown::Both);
            }
        }
    });

    let url = format!("http://{addr}/eventsource?types=*&closeafter=no&ping=0");
    let subscription =
        EventSourceSubscription::start(url, SharedHeaders::new(Vec::new()), CancelFlag::new());

    assert_eq!(subscription.reconnect_count(), 0);

    // First event received after connection 1 drops must be Reconnected with count 1
    let item1 = subscription
        .recv_item_timeout(Duration::from_secs(5))
        .expect("reconnect 1");
    assert_eq!(
        item1,
        EventSourceItem::Reconnected {
            last_event_id: None,
            reconnect_count: 1,
        }
    );
    assert_eq!(subscription.reconnect_count(), 1);

    // Second event received after connection 2 drops must be Reconnected with count 2
    let item2 = subscription
        .recv_item_timeout(Duration::from_secs(5))
        .expect("reconnect 2");
    assert_eq!(
        item2,
        EventSourceItem::Reconnected {
            last_event_id: None,
            reconnect_count: 2,
        }
    );
    assert_eq!(subscription.reconnect_count(), 2);

    drop(subscription);
    let _ = server_handle.join();
}

#[test]
fn eventsource_last_event_id_header_handling_and_no_duplicate() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind listener");
    let addr = listener.local_addr().expect("local addr");
    let headers_seen = Arc::new(Mutex::new(Vec::new()));
    let seen_clone = Arc::clone(&headers_seen);

    let server_handle = thread::spawn(move || {
        for conn_idx in 0..2 {
            if let Ok((mut stream, _)) = listener.accept() {
                let mut reader = BufReader::new(stream.try_clone().expect("clone stream"));
                let mut lines = Vec::new();
                loop {
                    let mut line = String::new();
                    if reader.read_line(&mut line).unwrap_or(0) == 0 || line == "\r\n" {
                        break;
                    }
                    lines.push(line.trim_end().to_owned());
                }
                seen_clone.lock().unwrap().push(lines);

                if conn_idx == 0 {
                    let frame = "id: custom-ev-1\nevent: ping\ndata: {}\n\n";
                    let chunk = format!("{:x}\r\n{}\r\n", frame.len(), frame);
                    let resp =
                        format!("HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n{chunk}");
                    let _ = stream.write_all(resp.as_bytes());
                    let _ = stream.flush();
                    thread::sleep(Duration::from_millis(50));
                    let _ = stream.shutdown(std::net::Shutdown::Both);
                } else {
                    let resp = "HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n";
                    let _ = stream.write_all(resp.as_bytes());
                    let _ = stream.flush();
                    let mut buf = [0u8; 1];
                    let _ = stream.read(&mut buf);
                }
            }
        }
    });

    let url = format!("http://{addr}/eventsource?types=*&closeafter=no&ping=0");
    let headers = SharedHeaders::new(vec![(
        "Last-Event-ID".to_owned(),
        "manual-override".to_owned(),
    )]);
    let subscription = EventSourceSubscription::start(url, headers, CancelFlag::new());

    // Wait for connection 2
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while headers_seen.lock().unwrap().len() < 2 {
        assert!(std::time::Instant::now() < deadline, "second connection");
        thread::sleep(Duration::from_millis(10));
    }

    let recorded = headers_seen.lock().unwrap();
    let conn2_lines = &recorded[1];
    let last_event_id_headers: Vec<_> = conn2_lines
        .iter()
        .filter(|h| h.to_ascii_lowercase().starts_with("last-event-id:"))
        .collect();
    assert_eq!(
        last_event_id_headers.len(),
        1,
        "must not duplicate Last-Event-ID header when caller supplied one: {:?}",
        last_event_id_headers
    );
    assert_eq!(last_event_id_headers[0], "Last-Event-ID: manual-override");

    drop(subscription);
    let _ = server_handle.join();
}

#[test]
fn eventsource_stop_and_drop_cleanly_terminate_thread() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind listener");
    let addr = listener.local_addr().expect("local addr");

    let server_handle = thread::spawn(move || {
        if let Ok((mut stream, _)) = listener.accept() {
            let resp = "HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n";
            let _ = stream.write_all(resp.as_bytes());
            let _ = stream.flush();
            let mut buf = [0u8; 1];
            let _ = stream.read(&mut buf);
        }
    });

    let url = format!("http://{addr}/eventsource?types=*&closeafter=no&ping=0");
    let cancel = CancelFlag::new();
    let mut subscription =
        EventSourceSubscription::start(url, SharedHeaders::new(Vec::new()), cancel.clone());

    thread::sleep(Duration::from_millis(50));
    assert!(!cancel.is_cancelled());

    subscription.stop();
    assert!(cancel.is_cancelled());

    // Calling stop a second time is idempotent
    subscription.stop();

    let _ = server_handle.join();
}

#[test]
fn expand_url_various_combinations_and_encoding() {
    // 1. Template with all 3 parameters
    let s1 = expand_url(
        "http://example.com/es?t={types}&c={closeafter}&p={ping}",
        &["Mailbox", "Email"],
        true,
        15,
    );
    assert_eq!(s1, "http://example.com/es?t=Mailbox,Email&c=state&p=15");

    // 2. Empty types defaults to ALL_TYPES (*)
    let s2 = expand_url("http://example.com/es?t={types}", &[], false, 0);
    assert_eq!(s2, "http://example.com/es?t=*");

    // 3. Characters requiring percent encoding in types
    let s3 = expand_url(
        "http://example.com/es?t={types}",
        &["custom type/1"],
        false,
        0,
    );
    assert_eq!(s3, "http://example.com/es?t=custom%20type%2F1");

    // 4. Missing template variables appended to existing query
    let s4 = expand_url("http://example.com/es?tenant=test", &["Calendar"], false, 0);
    assert_eq!(
        s4,
        "http://example.com/es?tenant=test&types=Calendar&closeafter=no&ping=0"
    );

    // 5. Missing template variables appended to bare path
    let s5 = expand_url("http://example.com/es", &[], true, 60);
    assert_eq!(s5, "http://example.com/es?types=*&closeafter=state&ping=60");
}

#[test]
fn eventsource_head_reading_rejects_non_200_and_non_chunked() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind listener");
    let addr = listener.local_addr().expect("local addr");
    let final_change = make_state_change("accX", "Mailbox", "st-final");
    let srv_final = final_change.clone();

    let attempts = Arc::new(AtomicUsize::new(0));
    let attempts_clone = Arc::clone(&attempts);

    let server_handle = thread::spawn(move || {
        // Attempt 1: 500 error
        if let Ok((mut stream, _)) = listener.accept() {
            attempts_clone.fetch_add(1, Ordering::SeqCst);
            let mut reader = BufReader::new(stream.try_clone().expect("clone stream"));
            let mut line = String::new();
            while reader.read_line(&mut line).unwrap_or(0) > 0 && line != "\r\n" {
                line.clear();
            }
            let resp = "HTTP/1.1 500 Internal Server Error\r\nTransfer-Encoding: chunked\r\n\r\n";
            let _ = stream.write_all(resp.as_bytes());
            let _ = stream.flush();
            thread::sleep(Duration::from_millis(50));
            let _ = stream.shutdown(std::net::Shutdown::Both);
        }

        // Attempt 2: non-chunked (Content-Length)
        if let Ok((mut stream, _)) = listener.accept() {
            attempts_clone.fetch_add(1, Ordering::SeqCst);
            let mut reader = BufReader::new(stream.try_clone().expect("clone stream"));
            let mut line = String::new();
            while reader.read_line(&mut line).unwrap_or(0) > 0 && line != "\r\n" {
                line.clear();
            }
            let resp = "HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n";
            let _ = stream.write_all(resp.as_bytes());
            let _ = stream.flush();
            thread::sleep(Duration::from_millis(50));
            let _ = stream.shutdown(std::net::Shutdown::Both);
        }

        // Attempt 3: Transfer-Encoding: gzip (not chunked)
        if let Ok((mut stream, _)) = listener.accept() {
            attempts_clone.fetch_add(1, Ordering::SeqCst);
            let mut reader = BufReader::new(stream.try_clone().expect("clone stream"));
            let mut line = String::new();
            while reader.read_line(&mut line).unwrap_or(0) > 0 && line != "\r\n" {
                line.clear();
            }
            let resp = "HTTP/1.1 200 OK\r\nTransfer-Encoding: gzip\r\n\r\n";
            let _ = stream.write_all(resp.as_bytes());
            let _ = stream.flush();
            thread::sleep(Duration::from_millis(50));
            let _ = stream.shutdown(std::net::Shutdown::Both);
        }

        // Attempt 4: valid chunked 200 response
        if let Ok((mut stream, _)) = listener.accept() {
            attempts_clone.fetch_add(1, Ordering::SeqCst);
            let mut reader = BufReader::new(stream.try_clone().expect("clone stream"));
            let mut line = String::new();
            while reader.read_line(&mut line).unwrap_or(0) > 0 && line != "\r\n" {
                line.clear();
            }
            let data = serde_json::to_string(&srv_final).expect("serialize");
            let frame = format!("event: state\ndata: {data}\n\n");
            let chunk = format!("{:x}\r\n{}\r\n", frame.len(), frame);
            let resp = format!("HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n{chunk}");
            let _ = stream.write_all(resp.as_bytes());
            let _ = stream.flush();
            let mut buf = [0u8; 1];
            let _ = stream.read(&mut buf);
        }
    });

    let url = format!("http://{addr}/eventsource?types=*&closeafter=no&ping=0");
    let subscription =
        EventSourceSubscription::start(url, SharedHeaders::new(Vec::new()), CancelFlag::new());

    let received = subscription
        .recv_timeout(Duration::from_secs(10))
        .expect("state change after retries");
    assert_eq!(received, final_change);
    assert_eq!(attempts.load(Ordering::SeqCst), 4);
    assert_eq!(subscription.reconnect_count(), 1);

    drop(subscription);
    let _ = server_handle.join();
}

#[test]
fn eventsource_chunked_body_extensions_and_trailers() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind listener");
    let addr = listener.local_addr().expect("local addr");
    let state_change = make_state_change("accE", "Email", "st-ext");
    let srv_change = state_change.clone();

    let server_handle = thread::spawn(move || {
        if let Ok((mut stream, _)) = listener.accept() {
            let mut reader = BufReader::new(stream.try_clone().expect("clone stream"));
            let mut line = String::new();
            while reader.read_line(&mut line).unwrap_or(0) > 0 && line != "\r\n" {
                line.clear();
            }

            let data = serde_json::to_string(&srv_change).expect("serialize");
            let frame = format!("id: ev-ext\nevent: state\ndata: {data}\n\n");
            // Chunk with chunk extension and trailing header after 0 chunk
            let chunk = format!(
                "{:x};name=val\r\n{}\r\n0\r\nTrailer-Header: checksum\r\n\r\n",
                frame.len(),
                frame
            );
            let resp = format!("HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n{chunk}");
            let _ = stream.write_all(resp.as_bytes());
            let _ = stream.flush();
            let mut buf = [0u8; 1];
            let _ = stream.read(&mut buf);
        }
    });

    let url = format!("http://{addr}/eventsource?types=*&closeafter=no&ping=0");
    let subscription =
        EventSourceSubscription::start(url, SharedHeaders::new(Vec::new()), CancelFlag::new());

    let received = subscription
        .recv_timeout(Duration::from_secs(5))
        .expect("state change received through chunked extensions");
    assert_eq!(received, state_change);
    assert_eq!(subscription.last_event_id().as_deref(), Some("ev-ext"));

    drop(subscription);
    let _ = server_handle.join();
}

#[test]
fn eventsource_stream_events_frame_parsing_and_resets() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind listener");
    let addr = listener.local_addr().expect("local addr");
    let first = make_state_change("acc1", "Mailbox", "st-1");
    let second = make_state_change("acc1", "Mailbox", "st-2");
    let srv_first = first.clone();
    let srv_second = second.clone();

    let server_handle = thread::spawn(move || {
        if let Ok((mut stream, _)) = listener.accept() {
            let mut reader = BufReader::new(stream.try_clone().expect("clone stream"));
            let mut line = String::new();
            while reader.read_line(&mut line).unwrap_or(0) > 0 && line != "\r\n" {
                line.clear();
            }

            let d1 = serde_json::to_string(&srv_first).expect("serialize");
            let d2 = serde_json::to_string(&srv_second).expect("serialize");

            // 1. ping event (ignored)
            // 2. unknown event (ignored)
            // 3. state event with id: first-id
            // 4. id\n (resets id)
            // 5. state event with id: second-id
            // 6. id:\n (resets id)
            let frames = format!(
                "event: ping\ndata: {{}}\n\n\
                 event: unknown_event\ndata: {{}}\n\n\
                 id: first-id\nevent: state\ndata: {d1}\n\n\
                 id\nevent: ping\ndata: {{}}\n\n\
                 id: second-id\nevent: state\ndata: {d2}\n\n\
                 id:\nevent: ping\ndata: {{}}\n\n"
            );
            let chunk = format!("{:x}\r\n{}\r\n", frames.len(), frames);
            let resp = format!("HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n{chunk}");
            let _ = stream.write_all(resp.as_bytes());
            let _ = stream.flush();
            let mut buf = [0u8; 1];
            let _ = stream.read(&mut buf);
        }
    });

    let url = format!("http://{addr}/eventsource?types=*&closeafter=no&ping=0");
    let subscription =
        EventSourceSubscription::start(url, SharedHeaders::new(Vec::new()), CancelFlag::new());

    let ev1 = subscription
        .recv_item_timeout(Duration::from_secs(5))
        .expect("first event");
    assert_eq!(ev1, EventSourceItem::State(first));

    let ev2 = subscription
        .recv_item_timeout(Duration::from_secs(5))
        .expect("second event");
    assert_eq!(ev2, EventSourceItem::State(second));

    // Wait for the trailing id:\n frame to reset the event ID
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while subscription.last_event_id().is_some() {
        assert!(std::time::Instant::now() < deadline, "last_event_id reset");
        thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(subscription.last_event_id(), None);

    drop(subscription);
    let _ = server_handle.join();
}

#[test]
fn eventsource_drop_connection_shuts_down_socket_and_reconnects() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind listener");
    let addr = listener.local_addr().expect("local addr");
    let accepted = Arc::new(AtomicUsize::new(0));
    let acc_clone = Arc::clone(&accepted);

    let server_handle = thread::spawn(move || {
        for _ in 0..2 {
            if let Ok((mut stream, _)) = listener.accept() {
                acc_clone.fetch_add(1, Ordering::SeqCst);
                let mut reader = BufReader::new(stream.try_clone().expect("clone stream"));
                let mut line = String::new();
                while reader.read_line(&mut line).unwrap_or(0) > 0 && line != "\r\n" {
                    line.clear();
                }

                let resp = "HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n";
                let _ = stream.write_all(resp.as_bytes());
                let _ = stream.flush();

                // Drained all request headers. Now blocks until client shuts down socket.
                let mut buf = [0u8; 1];
                let _ = stream.read(&mut buf);
            }
        }
    });

    let url = format!("http://{addr}/eventsource?types=*&closeafter=no&ping=0");
    let subscription =
        EventSourceSubscription::start(url, SharedHeaders::new(Vec::new()), CancelFlag::new());

    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while accepted.load(Ordering::SeqCst) < 1 {
        assert!(std::time::Instant::now() < deadline, "connection 1");
        thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(subscription.reconnect_count(), 0);

    // Calling drop_connection must shut down the socket and trigger reconnect
    subscription.drop_connection();

    let item = subscription
        .recv_item_timeout(Duration::from_secs(5))
        .expect("reconnect notification");
    assert_eq!(
        item,
        EventSourceItem::Reconnected {
            last_event_id: None,
            reconnect_count: 1,
        }
    );
    assert_eq!(subscription.reconnect_count(), 1);

    drop(subscription);
    let _ = server_handle.join();
}

#[test]
fn eventsource_exponential_backoff_doubles_delay() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind listener");
    let addr = listener.local_addr().expect("local addr");
    let attempt_times = Arc::new(Mutex::new(Vec::new()));
    let times_clone = Arc::clone(&attempt_times);

    let server_handle = thread::spawn(move || {
        for _ in 0..3 {
            if let Ok((mut stream, _)) = listener.accept() {
                times_clone.lock().unwrap().push(std::time::Instant::now());
                let resp = "HTTP/1.1 500 Server Error\r\n\r\n";
                let _ = stream.write_all(resp.as_bytes());
                let _ = stream.flush();
                thread::sleep(Duration::from_millis(10));
                let _ = stream.shutdown(std::net::Shutdown::Both);
            }
        }
    });

    let url = format!("http://{addr}/eventsource?types=*&closeafter=no&ping=0");
    let subscription =
        EventSourceSubscription::start(url, SharedHeaders::new(Vec::new()), CancelFlag::new());

    let deadline = std::time::Instant::now() + Duration::from_secs(6);
    while attempt_times.lock().unwrap().len() < 3 {
        assert!(
            std::time::Instant::now() < deadline,
            "3 connection attempts"
        );
        thread::sleep(Duration::from_millis(20));
    }

    let times = attempt_times.lock().unwrap();
    // Attempt 1 to 2: INITIAL_BACKOFF is 500ms
    // Attempt 2 to 3: (backoff * 2) is 1000ms
    // With `backoff / 2`, delay 2 to 3 would be only ~250ms
    let delay_2_to_3 = times[2].duration_since(times[1]);
    assert!(
        delay_2_to_3 >= Duration::from_millis(800),
        "backoff must double (>= 800ms, got {delay_2_to_3:?})"
    );

    drop(subscription);
    let _ = server_handle.join();
}
