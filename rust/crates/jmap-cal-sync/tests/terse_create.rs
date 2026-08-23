// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! `CalSync::save_component`'s create path must not trust `CalendarEvent/set`'s
//! `created` object to carry the properties the client itself just sent.
//!
//! RFC 8620 §5.3 only requires the server to report properties it set
//! itself; a real deployment (Stalwart, found via `jmap-cal-sync/tests/
//! live_server_save.rs` against the live test server) takes this literally
//! and answers a `CalendarEvent/set` create with `{"id": "..."}` alone.
//! Before this fix, `save_component`'s create branch rendered its return
//! value straight from that terse object, so the iCalendar object
//! `save_component_sync` hands back to EDS — the record EDS caches
//! immediately, before any later sync — was missing the summary, start time,
//! and every other property the caller just wrote. Mirrors
//! `jmap-book-sync/tests/terse_create.rs`'s identical finding for contacts.

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

    // The revision must match a normal load of the same event — otherwise
    // the very next `get_changes` looks like an external edit happened.
    let reloaded = sync.load_component(&saved.uid).unwrap();
    assert_eq!(saved.revision, reloaded.revision);
}
