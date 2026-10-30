// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Verify expanded `jmap-mockd` CLI surface for builder modes and seeding.

use std::fs;
use std::io::{BufRead, BufReader};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};

mod common;

use common::Mockd;

fn api(mockd: &Mockd, body: Value) -> Value {
    let payload = serde_json::to_vec(&body).expect("serialize API request");
    let (status, _, body) = mockd.request("POST", "/jmap", &payload);
    assert!(status.contains("200"), "{status}");
    serde_json::from_str(&body).expect("a response JSON body")
}

#[test]
fn help_lists_the_expanded_builder_mode_flags() {
    let output = Command::new(env!("CARGO_BIN_EXE_jmap-mockd"))
        .arg("--help")
        .output()
        .expect("run jmap-mockd --help");
    let stderr = String::from_utf8(output.stderr).expect("utf8 help output");

    for flag in [
        "--bearer-as",
        "--without-capability",
        "--no-sieve",
        "--without-primary-accounts",
        "--calls-in-request",
        "--no-calls-in-request",
        "--stalwart-shaped-unrecognized-changes-state",
        "--objects-in-get",
        "--query-hides-limit",
        "--size-request",
        "--no-size-request",
        "--size-upload",
        "--no-size-upload",
        "--sieve-extensions",
        "--contact-photo-data-uri-as-blob-id",
        "--session-via-redirect",
        "--download-via-redirect-to",
        "--reject-download-accept-json",
        "--terse-submission-create",
        "--terse-contact-create",
        "--max-contact-size",
        "--new-collections-default-unsubscribed",
        "--terse-calendar-event-create",
        "--without-scheduling",
        "--terse-collection-create",
        "--id-omitting-collection-create",
        "--reject-collection-create",
        "--max-delayed-send",
        "--snooze-extension",
        "--sieve-vacation-script",
        "--share",
        "--seed",
    ] {
        assert!(
            stderr.contains(flag),
            "missing {flag} in help output:\n{stderr}"
        );
    }
}

#[test]
fn startup_emits_a_machine_readable_listening_line() {
    let mut child = Command::new(env!("CARGO_BIN_EXE_jmap-mockd"))
        .arg("--port")
        .arg("0")
        .stdout(Stdio::piped())
        .spawn()
        .expect("spawn jmap-mockd");

    let stdout = child.stdout.take().expect("piped stdout");
    let (sender, receiver) = mpsc::channel();
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines() {
            match line {
                Ok(line) => {
                    if sender.send(line).is_err() {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
    });

    let mut listening = None;

    for _ in 0..4 {
        let Ok(line) = receiver.recv_timeout(Duration::from_secs(2)) else {
            break;
        };
        if let Some(json) = line.strip_prefix("listening: ") {
            listening = Some(json.to_owned());
            break;
        }
    }

    let listening = listening.expect("machine readable startup line");
    let payload: Value = serde_json::from_str(&listening).expect("startup line JSON");
    let origin = payload["origin"].as_str().expect("origin string");
    let session = payload["session"].as_str().expect("session string");
    assert!(origin.starts_with("http://127.0.0.1:"), "{origin}");
    assert_eq!(session, format!("{origin}/.well-known/jmap"));

    let _ = child.kill();
    let _ = child.wait();
}

#[test]
fn seed_file_populates_accounts_mailboxes_emails_cards_and_events() {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system time")
        .as_nanos();
    let seed = json!({
        "accounts": [
            {
                "id": "A1",
                "name": "seeded@example.test",
                "mailboxes": [
                    {"id": "M100", "name": "Inbox", "role": "inbox"}
                ],
                "emails": [
                    {
                        "mailboxId": "M100",
                        "from": {"name": "Sender", "email": "sender@example.test"},
                        "subject": "Seeded message",
                        "textBody": "hello from seed",
                        "receivedAt": "2026-01-02T03:04:05Z"
                    }
                ],
                "cards": [
                    {"id": "C100", "firstName": "Seed", "lastName": "Contact"}
                ],
                "events": [
                    {"id": "CE100", "title": "Seeded event"}
                ]
            }
        ]
    });
    let path = std::env::temp_dir().join(format!(
        "jmap-mockd-seed-{}-{}.json",
        std::process::id(),
        nonce
    ));
    fs::write(
        &path,
        serde_json::to_vec(&seed).expect("serialize seed document"),
    )
    .expect("write seed file");

    let path_arg = path.to_string_lossy().into_owned();
    let mockd = Mockd::spawn(&["--seed", &path_arg]);

    let (status, _, body) = mockd.request("GET", "/.well-known/jmap", b"");
    assert!(status.contains("200"), "{status}");
    let session: Value = serde_json::from_str(&body).expect("a session JSON body");
    assert_eq!(session["username"], json!("seeded@example.test"));

    let mailbox = api(
        &mockd,
        json!({
            "using": ["urn:ietf:params:jmap:core", "urn:ietf:params:jmap:mail"],
            "methodCalls": [["Mailbox/get", {"accountId": "A1"}, "m1"]],
        }),
    );
    assert_eq!(mailbox["methodResponses"][0][1]["list"][0]["id"], "M100");

    let email = api(
        &mockd,
        json!({
            "using": ["urn:ietf:params:jmap:core", "urn:ietf:params:jmap:mail"],
            "methodCalls": [["Email/query", {"accountId": "A1"}, "e1"]],
        }),
    );
    assert_eq!(
        email["methodResponses"][0][1]["ids"]
            .as_array()
            .map(Vec::len),
        Some(1)
    );

    let cards = api(
        &mockd,
        json!({
            "using": ["urn:ietf:params:jmap:core", "urn:ietf:params:jmap:contacts"],
            "methodCalls": [["ContactCard/query", {"accountId": "A1"}, "c1"]],
        }),
    );
    assert_eq!(
        cards["methodResponses"][0][1]["ids"]
            .as_array()
            .map(Vec::len),
        Some(1)
    );

    let events = api(
        &mockd,
        json!({
            "using": ["urn:ietf:params:jmap:core", "urn:ietf:params:jmap:calendars"],
            "methodCalls": [["CalendarEvent/query", {"accountId": "A1"}, "ce1"]],
        }),
    );
    assert_eq!(
        events["methodResponses"][0][1]["ids"]
            .as_array()
            .map(Vec::len),
        Some(1)
    );

    let _ = fs::remove_file(path);
}
