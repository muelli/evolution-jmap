// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! `GET /eventsource` actually serving `text/event-stream` and pushing a
//! `StateChange` on the `MockServer::push_state_change` test hook (RFC 8620
//! §7.1/§7.3, `docs/ROADMAP.md` item 28's mock slice).
//!
//! Asked over a raw `TcpStream` like `jmap-mock/tests/upload.rs`: this is a
//! long-lived connection with no `Content-Length`, which is exactly the
//! shape no client dependency of this crate speaks yet.

use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::time::Duration;

use jmap_mock::MockServer;
use jmap_proto::push::StateChange;
use jmap_proto::{Id, State};

/// Open `GET /eventsource?{query}` and send the request; the caller reads
/// the response at its own pace.
fn open_eventsource(server: &MockServer, query: &str) -> TcpStream {
    let address = server
        .origin()
        .strip_prefix("http://")
        .expect("the mock serves plain HTTP")
        .to_owned();
    let mut stream = TcpStream::connect(&address).expect("connect to the mock");
    let request = format!(
        "GET /eventsource?{query} HTTP/1.1\r\n\
         Host: {address}\r\n\
         Connection: close\r\n\
         \r\n"
    );
    stream.write_all(request.as_bytes()).expect("write request");
    stream
}

#[test]
fn a_pushed_state_change_arrives_as_an_sse_state_event() {
    let server = MockServer::builder().start();
    let mut stream = open_eventsource(&server, "types=*&closeafter=state&ping=0");

    // `closeafter=state` (RFC 8620 §7.3) ends the response right after the
    // pushed event, so reading to EOF terminates instead of hanging forever.
    let reader = std::thread::spawn(move || {
        let mut response = String::new();
        stream
            .read_to_string(&mut response)
            .expect("read the response");
        response
    });

    server.wait_for_event_source_subscriber(Duration::from_secs(5));

    let mut types = BTreeMap::new();
    types.insert("Email".to_owned(), State::new("d35ecb040aab"));
    let mut changed = BTreeMap::new();
    changed.insert(Id::new("a3123"), types);
    server.push_state_change(&StateChange::new(changed));

    let response = reader.join().expect("the reader thread finished");
    assert!(
        response.contains("Content-Type: text/event-stream"),
        "{response}"
    );
    assert!(response.contains("event: state"), "{response}");
    assert!(response.contains("\"@type\":\"StateChange\""), "{response}");
    assert!(response.contains("\"a3123\""), "{response}");
    assert!(response.contains("d35ecb040aab"), "{response}");
}

#[test]
fn a_subscriber_filtered_by_types_only_receives_matching_pushes() {
    let server = MockServer::builder().start();
    let stream = open_eventsource(&server, "types=ContactCard&closeafter=no&ping=0");
    let mut reader = BufReader::new(stream);

    server.wait_for_event_source_subscriber(Duration::from_secs(5));

    // A `Mailbox` change does not match this subscriber's `types=ContactCard`
    // filter, so it must be sent nothing at all — not an empty `StateChange`,
    // which nothing distinguishes from a match with no surviving types.
    server.push_state_change(&StateChange::new(BTreeMap::from([(
        Id::new("a1"),
        BTreeMap::from([("Mailbox".to_owned(), State::new("1"))]),
    )])));

    // A `ContactCard` change does match, and must still arrive.
    let mut changed = BTreeMap::new();
    changed.insert(
        Id::new("a1"),
        BTreeMap::from([
            ("Mailbox".to_owned(), State::new("2")),
            ("ContactCard".to_owned(), State::new("1")),
        ]),
    );
    server.push_state_change(&StateChange::new(changed));

    let mut line = String::new();
    loop {
        line.clear();
        let n = reader.read_line(&mut line).expect("read a line");
        assert!(n > 0, "connection closed before the matching push arrived");
        if line.trim_end() == "event: state" {
            break;
        }
    }
    let mut data = String::new();
    reader.read_line(&mut data).expect("read the data line");
    assert!(data.contains("ContactCard"), "{data}");
    assert!(
        !data.contains("\"Mailbox\""),
        "the unmatched type leaked through: {data}"
    );
}

#[test]
fn a_second_subscriber_also_receives_the_push() {
    let server = MockServer::builder().start();
    let mut first = open_eventsource(&server, "types=*&closeafter=state&ping=0");
    let mut second = open_eventsource(&server, "types=*&closeafter=state&ping=0");

    let first_reader = std::thread::spawn(move || {
        let mut response = String::new();
        first.read_to_string(&mut response).expect("read first");
        response
    });
    let second_reader = std::thread::spawn(move || {
        let mut response = String::new();
        second.read_to_string(&mut response).expect("read second");
        response
    });

    // Both connections must have registered before the push, or one would
    // miss it — not a "some subscriber" but "the count this test opened".
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    loop {
        let count = {
            let state = server.state();
            let state = state.lock().unwrap();
            state.event_source.subscriber_count()
        };
        if count >= 2 {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "only {count} of 2 subscribers connected in time"
        );
        std::thread::sleep(Duration::from_millis(5));
    }

    server.push_state_change(&StateChange::new(BTreeMap::from([(
        Id::new("a1"),
        BTreeMap::from([("Mailbox".to_owned(), State::new("1"))]),
    )])));

    let first_response = first_reader.join().expect("first reader finished");
    let second_response = second_reader.join().expect("second reader finished");
    assert!(first_response.contains("event: state"), "{first_response}");
    assert!(
        second_response.contains("event: state"),
        "{second_response}"
    );
}

#[test]
fn a_configured_ping_interval_pings_when_nothing_is_pushed() {
    let server = MockServer::builder().start();
    let stream = open_eventsource(&server, "types=*&closeafter=no&ping=1");
    let mut reader = BufReader::new(stream);

    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    loop {
        let mut line = String::new();
        let n = reader.read_line(&mut line).expect("read a line");
        assert!(n > 0, "connection closed before a ping event arrived");
        if line.trim_end() == "event: ping" {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "no ping event within 5s"
        );
    }
}

#[test]
fn closeafter_no_keeps_the_connection_open_past_one_push() {
    let server = MockServer::builder().start();
    let stream = open_eventsource(&server, "types=*&closeafter=no&ping=0");
    let mut reader = BufReader::new(stream);

    server.wait_for_event_source_subscriber(Duration::from_secs(5));
    server.push_state_change(&StateChange::new(BTreeMap::from([(
        Id::new("a1"),
        BTreeMap::from([("Mailbox".to_owned(), State::new("1"))]),
    )])));

    // First push arrives, and the connection is still alive to carry a
    // second one — the opposite of `closeafter=state`.
    let mut line = String::new();
    loop {
        line.clear();
        let n = reader.read_line(&mut line).expect("read a line");
        assert!(n > 0, "connection closed after only one push");
        if line.trim_end() == "event: state" {
            break;
        }
    }

    server.push_state_change(&StateChange::new(BTreeMap::from([(
        Id::new("a1"),
        BTreeMap::from([("Mailbox".to_owned(), State::new("2"))]),
    )])));
    loop {
        line.clear();
        let n = reader.read_line(&mut line).expect("read a line");
        assert!(n > 0, "connection closed before the second push");
        if line.trim_end() == "event: state" {
            break;
        }
    }
}
