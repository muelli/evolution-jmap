// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Tests [`jmap_cal_sync::is_import`], which tells a plain import or backup
//! restore from a real invitation, reply or cancellation by the top-level
//! `METHOD` (RFC 5546 1.4).

use jmap_cal_sync::is_import;

const VEVENT: &str = "BEGIN:VEVENT\r\nUID:planning@localhost\r\nEND:VEVENT\r\n";

fn calendar(method: Option<&str>) -> String {
    let method = method
        .map(|method| format!("METHOD:{method}\r\n"))
        .unwrap_or_default();
    format!("BEGIN:VCALENDAR\r\nVERSION:2.0\r\n{method}{VEVENT}END:VCALENDAR\r\n")
}

#[test]
fn no_method_at_all_is_an_import() {
    assert!(is_import(&calendar(None)));
}

#[test]
fn method_publish_is_an_import() {
    assert!(is_import(&calendar(Some("PUBLISH"))));
}

#[test]
fn method_publish_is_an_import_regardless_of_case() {
    assert!(is_import(&calendar(Some("publish"))));
}

#[test]
fn method_request_is_not_an_import() {
    assert!(!is_import(&calendar(Some("REQUEST"))));
}

#[test]
fn method_reply_is_not_an_import() {
    assert!(!is_import(&calendar(Some("REPLY"))));
}

#[test]
fn method_cancel_is_not_an_import() {
    assert!(!is_import(&calendar(Some("CANCEL"))));
}

/// The safer default when the document cannot even be parsed is to treat it
/// as an import: never schedule a reply the user did not ask for.
#[test]
fn an_unparseable_document_is_treated_as_an_import() {
    assert!(is_import("this is not iCalendar"));
}
