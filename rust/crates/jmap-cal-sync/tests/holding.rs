// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! `CalSync::save_component`'s create path matches a re-sent event to the one
//! it already holds by its own UID, not by whichever event in the calendar a
//! query without that filter would return first.

mod common;

use common::Fixture;

const APPOINTMENT_A: &str = "BEGIN:VCALENDAR\r\n\
VERSION:2.0\r\n\
BEGIN:VEVENT\r\n\
UID:appointment-aaaa@localhost\r\n\
SUMMARY:Alpha\r\n\
DTSTART;TZID=Europe/Berlin:20260115T130000\r\n\
DURATION:PT30M\r\n\
END:VEVENT\r\n\
END:VCALENDAR\r\n";

const APPOINTMENT_B: &str = "BEGIN:VCALENDAR\r\n\
VERSION:2.0\r\n\
BEGIN:VEVENT\r\n\
UID:appointment-bbbb@localhost\r\n\
SUMMARY:Beta\r\n\
DTSTART;TZID=Europe/Berlin:20260115T140000\r\n\
DURATION:PT30M\r\n\
END:VEVENT\r\n\
END:VCALENDAR\r\n";

const APPOINTMENT_B_RENAMED: &str = "BEGIN:VCALENDAR\r\n\
VERSION:2.0\r\n\
BEGIN:VEVENT\r\n\
UID:appointment-bbbb@localhost\r\n\
SUMMARY:Beta Renamed\r\n\
DTSTART;TZID=Europe/Berlin:20260115T140000\r\n\
DURATION:PT30M\r\n\
END:VEVENT\r\n\
END:VCALENDAR\r\n";

#[test]
fn a_resend_edits_the_event_holding_its_own_uid_not_whichever_is_first() {
    let fixture = Fixture::start();
    let sync = fixture.sync();

    let alpha = sync.save_component(APPOINTMENT_A, None).unwrap();
    let beta = sync.save_component(APPOINTMENT_B, None).unwrap();
    assert_ne!(alpha.uid, beta.uid);

    let resent = sync.save_component(APPOINTMENT_B_RENAMED, None).unwrap();

    assert_eq!(
        resent.uid, beta.uid,
        "a resend naming UID appointment-bbbb must edit Beta, not Alpha"
    );
    assert!(resent.icalendar.contains("SUMMARY:Beta Renamed"));

    let alpha_reloaded = sync.load_component(&alpha.uid).unwrap();
    assert!(
        alpha_reloaded.icalendar.contains("SUMMARY:Alpha"),
        "Alpha must be untouched by a resend naming Beta's UID: {}",
        alpha_reloaded.icalendar
    );
}
