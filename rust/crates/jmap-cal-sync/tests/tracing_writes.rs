// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! `CalSync::save_component`/`remove_component` trace their writes with
//! `account_id`/`calendar_id`.

mod common;

use common::Fixture;
use jmap_proto::Id;
use jmap_tracing_capture::capture;
use tracing::Level;

const NEW_EVENT: &str = "BEGIN:VCALENDAR\r\n\
VERSION:2.0\r\n\
BEGIN:VEVENT\r\n\
UID:20260808T101500Z-4711-1000-1-0@localhost\r\n\
SUMMARY:Planning\r\n\
DTSTART;TZID=Europe/Berlin:20260115T130000\r\n\
DURATION:PT90M\r\n\
END:VEVENT\r\n\
END:VCALENDAR\r\n";

fn has(captured: &[(Level, String, String)], level: Level, name: &str, value: &str) -> bool {
    captured
        .iter()
        .any(|(l, n, v)| *l == level && n == name && v == value)
}

#[test]
fn creating_an_event_traces_the_account_and_calendar_on_success() {
    let fixture = Fixture::start();
    let sync = fixture.sync();

    let (_, captured) = capture(|| {
        sync.save_component(NEW_EVENT, None).unwrap();
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
            "calendar_id",
            fixture.ours.as_ref()
        ),
        "expected a DEBUG calendar_id field, got {captured:?}"
    );
    assert!(
        captured.iter().all(|(_, name, _)| name != "error"),
        "a successful create should not log an error field, got {captured:?}"
    );
}

#[test]
fn creating_an_event_in_a_gone_calendar_traces_the_failure() {
    let fixture = Fixture::start();
    let sync = jmap_cal_sync::CalSync::new(
        fixture.client(),
        fixture.account_id.clone(),
        Id::new("nonexistent"),
    );

    let (_, captured) = capture(|| {
        let _ = sync.save_component(NEW_EVENT, None);
    });

    assert!(
        captured
            .iter()
            .any(|(level, name, _)| *level == Level::WARN && name == "error"),
        "expected a WARN error field, got {captured:?}"
    );
}

#[test]
fn updating_an_event_traces_the_account_calendar_and_uid_on_success() {
    let fixture = Fixture::start();
    let sync = fixture.sync();
    let (saved, _) = capture(|| sync.save_component(NEW_EVENT, None).unwrap());
    let edited = saved
        .icalendar
        .replace("SUMMARY:Planning", "SUMMARY:Planning (moved)");

    let (_, captured) = capture(|| {
        sync.save_component(&edited, Some(&saved.uid)).unwrap();
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
            "calendar_id",
            fixture.ours.as_ref()
        ),
        "expected a DEBUG calendar_id field, got {captured:?}"
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
fn removing_an_event_traces_the_account_calendar_and_uid_on_success() {
    let fixture = Fixture::start();
    let sync = fixture.sync();
    let (saved, _) = capture(|| sync.save_component(NEW_EVENT, None).unwrap());

    let (_, captured) = capture(|| {
        sync.remove_component(&saved.uid).unwrap();
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
            "calendar_id",
            fixture.ours.as_ref()
        ),
        "expected a DEBUG calendar_id field, got {captured:?}"
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
fn removing_a_nonexistent_event_traces_the_failure() {
    let fixture = Fixture::start();
    let sync = fixture.sync();

    let (_, captured) = capture(|| {
        let _ = sync.remove_component("nonexistent");
    });

    assert!(
        captured
            .iter()
            .any(|(level, name, _)| *level == Level::WARN && name == "error"),
        "expected a WARN error field, got {captured:?}"
    );
}
