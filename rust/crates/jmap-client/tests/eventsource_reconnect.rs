// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Verification of EventSource reconnection, Last-Event-ID resume (RFC 8620 §7.3),
//! and gap notification when reconnecting.

use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use jmap_client::eventsource::{EventSourceItem, EventSourceSubscription, SharedHeaders};
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
fn eventsource_resumes_with_last_event_id_and_delivers_missed_changes() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind listener");
    let addr = listener.local_addr().expect("local addr");
    let request_headers_seen = Arc::new(Mutex::new(Vec::new()));
    let headers_clone = Arc::clone(&request_headers_seen);
    let connection_count = Arc::new(AtomicUsize::new(0));
    let conn_count_clone = Arc::clone(&connection_count);

    let first_change = make_state_change("a1", "Mailbox", "st-1");
    let second_change = make_state_change("a1", "Mailbox", "st-2");
    let srv_first = first_change.clone();
    let srv_second = second_change.clone();

    let server_handle = thread::spawn(move || {
        // Connection 1: send an event with id: ev-1, then abruptly close the socket.
        if let Ok((mut stream, _)) = listener.accept() {
            conn_count_clone.fetch_add(1, Ordering::SeqCst);
            let mut reader = BufReader::new(stream.try_clone().expect("clone stream"));
            let mut lines = Vec::new();
            loop {
                let mut line = String::new();
                if reader.read_line(&mut line).unwrap_or(0) == 0 || line == "\r\n" {
                    break;
                }
                lines.push(line.trim_end().to_owned());
            }
            headers_clone.lock().unwrap().push(lines);

            let data1 = serde_json::to_string(&srv_first).expect("serialize");
            let frame1 = format!("id: ev-1\nevent: state\ndata: {data1}\n\n");
            let body1 = format!("{:x}\r\n{}\r\n", frame1.len(), frame1);
            let resp1 = format!("HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n{body1}");
            let _ = stream.write_all(resp1.as_bytes());
            let _ = stream.flush();

            // Give client time to process frame 1 before dropping
            thread::sleep(Duration::from_millis(50));
            let _ = stream.shutdown(std::net::Shutdown::Both);
        }

        // Connection 2 (reconnect): read request headers, verify Last-Event-ID, send missed event.
        if let Ok((mut stream, _)) = listener.accept() {
            conn_count_clone.fetch_add(1, Ordering::SeqCst);
            let mut reader = BufReader::new(stream.try_clone().expect("clone stream"));
            let mut lines = Vec::new();
            loop {
                let mut line = String::new();
                if reader.read_line(&mut line).unwrap_or(0) == 0 || line == "\r\n" {
                    break;
                }
                lines.push(line.trim_end().to_owned());
            }
            headers_clone.lock().unwrap().push(lines);

            let data2 = serde_json::to_string(&srv_second).expect("serialize");
            let frame2 = format!("id: ev-2\nevent: state\ndata: {data2}\n\n");
            let body2 = format!("{:x}\r\n{}\r\n", frame2.len(), frame2);
            let resp2 = format!("HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n{body2}");
            let _ = stream.write_all(resp2.as_bytes());
            let _ = stream.flush();

            // Keep connection open until test finishes
            let mut buf = [0u8; 1];
            let _ = stream.read(&mut buf);
        }
    });

    let url = format!("http://{addr}/eventsource?types=*&closeafter=no&ping=0");
    let subscription =
        EventSourceSubscription::start(url, SharedHeaders::new(Vec::new()), CancelFlag::new());

    // 1. First event arrives
    let first = subscription
        .recv_item_timeout(Duration::from_secs(5))
        .expect("first event arrives");
    assert_eq!(first, EventSourceItem::State(first_change.clone()));
    assert_eq!(subscription.last_event_id().as_deref(), Some("ev-1"));

    // 2. Connection drops and reconnects: observer sees Reconnected item
    let reconnected = subscription
        .recv_item_timeout(Duration::from_secs(5))
        .expect("reconnected notification arrives");
    assert_eq!(
        reconnected,
        EventSourceItem::Reconnected {
            last_event_id: Some("ev-1".to_owned()),
            reconnect_count: 1,
        }
    );
    assert_eq!(subscription.reconnect_count(), 1);

    // 3. Second event (replayed on reconnect) arrives
    let second = subscription
        .recv_item_timeout(Duration::from_secs(5))
        .expect("second event arrives");
    assert_eq!(second, EventSourceItem::State(second_change));
    assert_eq!(subscription.last_event_id().as_deref(), Some("ev-2"));

    drop(subscription);
    let _ = server_handle.join();

    // Verify Last-Event-ID header was sent on second connection
    let headers = request_headers_seen.lock().unwrap();
    assert_eq!(headers.len(), 2, "must have seen exactly 2 connections");
    let conn2_headers = &headers[1];
    let has_last_event_id = conn2_headers
        .iter()
        .any(|h| h.eq_ignore_ascii_case("Last-Event-ID: ev-1"));
    assert!(
        has_last_event_id,
        "second connection must send Last-Event-ID: ev-1, got: {:?}",
        conn2_headers
    );
}

#[test]
fn drop_connection_triggers_reconnect_and_increments_reconnect_count() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind listener");
    let addr = listener.local_addr().expect("local addr");
    let connection_count = Arc::new(AtomicUsize::new(0));
    let count_clone = Arc::clone(&connection_count);

    let server_handle = thread::spawn(move || {
        for _ in 0..2 {
            if let Ok((mut stream, _)) = listener.accept() {
                count_clone.fetch_add(1, Ordering::SeqCst);
                let resp = "HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n";
                let _ = stream.write_all(resp.as_bytes());
                let _ = stream.flush();
                let mut buf = [0u8; 1];
                let _ = stream.read(&mut buf);
            }
        }
    });

    let url = format!("http://{addr}/eventsource?types=*&closeafter=no&ping=0");
    let subscription =
        EventSourceSubscription::start(url, SharedHeaders::new(Vec::new()), CancelFlag::new());

    // Wait for first connection
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while connection_count.load(Ordering::SeqCst) < 1 {
        assert!(std::time::Instant::now() < deadline, "connection 1 timeout");
        thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(subscription.reconnect_count(), 0);

    // Drop connection from client side
    subscription.drop_connection();

    // Wait for reconnect notification
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
