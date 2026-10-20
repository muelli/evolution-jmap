// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! `BookSync::save_contact` and `remove_contact` trace their writes with
//! `account_id` and `address_book_id`.

mod common;

use common::Fixture;
use jmap_tracing_capture::capture;
use tracing::Level;

const NEW_CONTACT: &str = "BEGIN:VCARD\r\n\
VERSION:3.0\r\n\
UID:pas-id-68A2F1C400000000\r\n\
FN:Vera Oldenburg\r\n\
N:Oldenburg;Vera;;;\r\n\
EMAIL;TYPE=WORK:vera@example.com\r\n\
END:VCARD\r\n";

fn has(captured: &[(Level, String, String)], level: Level, name: &str, value: &str) -> bool {
    captured
        .iter()
        .any(|(l, n, v)| *l == level && n == name && v == value)
}

#[test]
fn creating_a_contact_traces_the_account_and_address_book_on_success() {
    let fixture = Fixture::start();
    let sync = fixture.sync();

    let (_, captured) = capture(|| {
        sync.save_contact(NEW_CONTACT, None).unwrap();
    });

    assert!(
        has(
            &captured,
            Level::DEBUG,
            "account_id",
            fixture.account_id.as_ref()
        ),
        "expected a DEBUG account_id field, got {captured:?}"
    );
    assert!(
        has(
            &captured,
            Level::DEBUG,
            "address_book_id",
            fixture.ours.as_ref()
        ),
        "expected a DEBUG address_book_id field, got {captured:?}"
    );
    assert!(
        captured.iter().all(|(_, name, _)| name != "error"),
        "a successful create should not log an error field, got {captured:?}"
    );
}

#[test]
fn creating_a_contact_in_a_gone_address_book_traces_the_failure() {
    let fixture = Fixture::start();
    let sync = jmap_book_sync::BookSync::new(
        fixture.client(),
        fixture.account_id.clone(),
        "nonexistent".into(),
    );

    let (_, captured) = capture(|| {
        let _ = sync.save_contact(NEW_CONTACT, None);
    });

    assert!(
        captured
            .iter()
            .any(|(level, name, _)| *level == Level::WARN && name == "error"),
        "expected a WARN error field, got {captured:?}"
    );
}

#[test]
fn updating_a_contact_traces_the_account_address_book_and_uid_on_success() {
    let fixture = Fixture::start();
    let sync = fixture.sync();
    let (saved, _) = capture(|| sync.save_contact(NEW_CONTACT, None).unwrap());
    let edited = saved
        .vcard
        .replace("FN:Vera Oldenburg", "FN:Vera Oldenburg-Nord");

    let (_, captured) = capture(|| {
        sync.save_contact(&edited, Some(&saved.uid)).unwrap();
    });

    assert!(
        has(
            &captured,
            Level::DEBUG,
            "account_id",
            fixture.account_id.as_ref()
        ),
        "expected a DEBUG account_id field, got {captured:?}"
    );
    assert!(
        has(
            &captured,
            Level::DEBUG,
            "address_book_id",
            fixture.ours.as_ref()
        ),
        "expected a DEBUG address_book_id field, got {captured:?}"
    );
    assert!(
        has(&captured, Level::DEBUG, "uid", &saved.uid),
        "expected a DEBUG uid field, got {captured:?}"
    );
    assert!(
        captured.iter().all(|(_, name, _)| name != "error"),
        "a successful update should not log an error field, got {captured:?}"
    );
}

#[test]
fn removing_a_contact_traces_the_account_address_book_and_uid_on_success() {
    let fixture = Fixture::start();
    let sync = fixture.sync();
    let (saved, _) = capture(|| sync.save_contact(NEW_CONTACT, None).unwrap());

    let (_, captured) = capture(|| {
        sync.remove_contact(&saved.uid).unwrap();
    });

    assert!(
        has(
            &captured,
            Level::DEBUG,
            "account_id",
            fixture.account_id.as_ref()
        ),
        "expected a DEBUG account_id field, got {captured:?}"
    );
    assert!(
        has(
            &captured,
            Level::DEBUG,
            "address_book_id",
            fixture.ours.as_ref()
        ),
        "expected a DEBUG address_book_id field, got {captured:?}"
    );
    assert!(
        has(&captured, Level::DEBUG, "uid", &saved.uid),
        "expected a DEBUG uid field, got {captured:?}"
    );
    assert!(
        captured.iter().all(|(_, name, _)| name != "error"),
        "a successful destroy should not log an error field, got {captured:?}"
    );
}

#[test]
fn removing_a_nonexistent_contact_traces_the_failure() {
    let fixture = Fixture::start();
    let sync = fixture.sync();

    let (_, captured) = capture(|| {
        let _ = sync.remove_contact("nonexistent");
    });

    assert!(
        captured
            .iter()
            .any(|(level, name, _)| *level == Level::WARN && name == "error"),
        "expected a WARN error field, got {captured:?}"
    );
}
