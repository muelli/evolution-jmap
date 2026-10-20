// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! The create path of `CalSync::save_component` must not expect the `created`
//! object of `CalendarEvent/set` to hold the properties that the client sent.
//!
//! RFC 8620 §5.3 requires the server to report only the properties that it set.
//! Stalwart (found with `jmap-cal-sync/tests/live_server_save.rs` against the live
//! test server) answers a create with only `{"id": "..."}`. The iCalendar object
//! that `save_component_sync` gives to EDS is the record that EDS caches
//! immediately, so it must not be built from that short object. If it is, the
//! summary, the start and all other properties that the caller wrote are missing.
//! `jmap-book-sync/tests/terse_create.rs` tests the same problem for contacts.

use jmap_cal_sync::CalSync;
use jmap_client::{Client, Credentials};
use jmap_mock::MockServer;

const NEW_EVENT: &str = "BEGIN:VCALENDAR\r\n\
VERSION:2.0\r\n\
BEGIN:VEVENT\r\n\
UID:20260808T101500Z-4711-1000-1-0@localhost\r\n\
SUMMARY:Planning\r\n\
DTSTART;TZID=Europe/Berlin:20260115T130000\r\n\
DURATION:PT90M\r\n\
END:VEVENT\r\n\
END:VCALENDAR\r\n";

#[test]
fn saving_a_new_event_against_a_terse_server_still_renders_what_was_sent() {
    let server = MockServer::builder().terse_calendar_event_create().start();
    let account_id = server.account_id();
    let calendar_id = {
        let state = server.state();
        let mut state = state.lock().unwrap();
        state
            .account_mut(&account_id)
            .unwrap()
            .seed_calendar("Personal", true)
    };
    let client = Client::connect(server.origin(), Credentials::none()).unwrap();
    let sync = CalSync::new(client, account_id, calendar_id);

    let saved = sync.save_component(NEW_EVENT, None).unwrap();

    assert!(
        saved.icalendar.contains("SUMMARY:Planning"),
        "a terse create response must not lose the summary the client just sent: {}",
        saved.icalendar
    );
    assert!(
        saved.icalendar.contains("DTSTART"),
        "a terse create response must not lose the start time the client just sent: {}",
        saved.icalendar
    );

    // The revision must match a normal load of the same event. If not, the next
    // `get_changes` reports an external edit.
    let reloaded = sync.load_component(&saved.uid).unwrap();
    assert_eq!(saved.revision, reloaded.revision);
}
