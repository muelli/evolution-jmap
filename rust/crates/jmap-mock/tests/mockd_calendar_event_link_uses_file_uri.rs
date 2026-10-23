// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Verify hostile calendar-event link URI flags on standalone `jmap-mockd`.

use std::process::Command;

use serde_json::{Value, json};

mod common;

use common::Mockd;

const SAFE_LINK_URI: &str = "https://files.example.com/agenda.pdf";
const HOSTILE_LINK_URI: &str = "file:///dev/zero";

fn api(mockd: &Mockd, body: Value) -> Value {
    let payload = serde_json::to_vec(&body).expect("serialize API request");
    let (status, _, body) = mockd.request("POST", "/jmap", &payload);
    assert!(status.contains("200"), "{status}");
    serde_json::from_str(&body).expect("a response JSON body")
}

fn default_calendar_id(mockd: &Mockd) -> String {
    let response = api(
        mockd,
        json!({
            "using": ["urn:ietf:params:jmap:core", "urn:ietf:params:jmap:calendars"],
            "methodCalls": [[
                "Calendar/get",
                {"accountId": "A1", "properties": ["id"]},
                "c1"
            ]],
        }),
    );

    response["methodResponses"][0][1]["list"][0]["id"]
        .as_str()
        .expect("default calendar id")
        .to_owned()
}

fn create_event_with_link(mockd: &Mockd) -> String {
    let calendar_id = default_calendar_id(mockd);
    let mut calendar_ids = serde_json::Map::new();
    calendar_ids.insert(calendar_id, Value::Bool(true));

    let response = api(
        mockd,
        json!({
            "using": ["urn:ietf:params:jmap:core", "urn:ietf:params:jmap:calendars"],
            "methodCalls": [[
                "CalendarEvent/set",
                {
                    "accountId": "A1",
                    "create": {
                        "k1": {
                            "@type": "Event",
                            "version": "2.0",
                            "calendarIds": calendar_ids,
                            "start": "2026-01-02T12:00:00",
                            "duration": "PT1H",
                            "title": "Planning",
                            "links": {
                                "l1": {
                                    "@type": "Link",
                                    "href": SAFE_LINK_URI,
                                    "title": "Agenda"
                                }
                            }
                        }
                    }
                },
                "s1"
            ]],
        }),
    );

    response["methodResponses"][0][1]["created"]["k1"]["id"]
        .as_str()
        .expect("created event id")
        .to_owned()
}

fn event_link_uri(mockd: &Mockd, event_id: &str) -> String {
    let response = api(
        mockd,
        json!({
            "using": ["urn:ietf:params:jmap:core", "urn:ietf:params:jmap:calendars"],
            "methodCalls": [[
                "CalendarEvent/get",
                {
                    "accountId": "A1",
                    "ids": [event_id],
                    "properties": ["id", "links"]
                },
                "g1"
            ]],
        }),
    );

    response["methodResponses"][0][1]["list"][0]["links"]["l1"]["href"]
        .as_str()
        .expect("link URI as string")
        .to_owned()
}

#[test]
fn the_flag_rewrites_calendar_event_link_uri_to_file_scheme() {
    let mockd = Mockd::spawn(&["--calendar-event-link-uses-file-uri"]);
    let event_id = create_event_with_link(&mockd);

    let uri = event_link_uri(&mockd, &event_id);

    assert_eq!(uri, HOSTILE_LINK_URI);
}

#[test]
fn help_lists_the_calendar_event_link_uses_file_uri_flag() {
    let output = Command::new(env!("CARGO_BIN_EXE_jmap-mockd"))
        .arg("--help")
        .output()
        .expect("run jmap-mockd --help");
    let stderr = String::from_utf8(output.stderr).expect("utf8 help output");

    assert!(
        stderr.contains("--calendar-event-link-uses-file-uri"),
        "{stderr}"
    );
}
