// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! `CalSync::set_color` against the mock server: the whole of what a
//! calendar-colour write-back means, minus the vfunc/diff bookkeeping.

mod common;

use common::Fixture;
use jmap_proto::Id;
use jmap_tracing_capture::capture;
use tracing::Level;

#[test]
fn set_color_reaches_the_server() {
    let fixture = Fixture::start();
    let sync = fixture.sync();

    let _ = capture(|| sync.set_color(Some("#00ff00")).unwrap());

    let calendars = fixture.client().calendars(&fixture.account_id).unwrap();
    let ours = calendars
        .into_iter()
        .find(|c| c.id.as_ref() == Some(&fixture.ours))
        .unwrap();
    assert_eq!(ours.color.as_deref(), Some("#00ff00"));
}

#[test]
fn set_color_of_none_clears_it() {
    let fixture = Fixture::start();
    let sync = fixture.sync();
    let _ = capture(|| sync.set_color(Some("#00ff00")).unwrap());

    let _ = capture(|| sync.set_color(None).unwrap());

    let calendars = fixture.client().calendars(&fixture.account_id).unwrap();
    let ours = calendars
        .into_iter()
        .find(|c| c.id.as_ref() == Some(&fixture.ours))
        .unwrap();
    assert_eq!(ours.color, None);
}

#[test]
fn set_color_traces_the_account_and_calendar_on_success() {
    let fixture = Fixture::start();
    let sync = fixture.sync();

    let (_, captured) = capture(|| {
        sync.set_color(Some("#00ff00")).unwrap();
    });

    assert!(
        captured
            .iter()
            .any(|(level, name, value)| *level == Level::DEBUG
                && name == "account_id"
                && *value == fixture.account_id.to_string()),
        "expected a DEBUG account_id field, got {captured:?}"
    );
    assert!(
        captured
            .iter()
            .any(|(level, name, value)| *level == Level::DEBUG
                && name == "calendar_id"
                && *value == fixture.ours.to_string()),
        "expected a DEBUG calendar_id field, got {captured:?}"
    );
    assert!(
        captured.iter().all(|(_, name, _)| name != "error"),
        "a successful push should not log an error field, got {captured:?}"
    );
}

#[test]
fn set_color_traces_the_failure_when_the_calendar_is_gone() {
    let fixture = Fixture::start();
    let sync = jmap_cal_sync::CalSync::new(
        fixture.client(),
        fixture.account_id.clone(),
        Id::new("nonexistent"),
    );

    let (_, captured) = capture(|| {
        let _ = sync.set_color(Some("#00ff00"));
    });

    assert!(
        captured
            .iter()
            .any(|(level, name, _)| *level == Level::WARN && name == "error"),
        "expected a WARN error field, got {captured:?}"
    );
}
