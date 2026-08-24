// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! The server-controlled half of the calendar boundary, checked against the C
//! that consumes it.
//!
//! `jmap-ical`'s own `tests/hostile.rs` pins the text this crate renders. This
//! file is the other half of the same finding: what *libical* — the parser that
//! decides what lands in the user's calendar — makes of that text. A property
//! the server managed to inject would be invisible in a string comparison and
//! perfectly visible here.
//!
//! See `docs/AUDIT-FFI.md`, findings F2, F4 and F7.

use std::ffi::CStr;
use std::sync::RwLock;

use eds_sys::{
    I_CAL_SUMMARY_PROPERTY, I_CAL_VEVENT_COMPONENT, ICalComponent,
    i_cal_component_get_first_component, i_cal_component_get_first_property,
    i_cal_component_get_summary, i_cal_component_get_uid,
};
use jmap_backend_cal::marshal;
use jmap_backend_core::instance::Slot;
use jmap_cal_sync::CalSync;
use jmap_proto::calendars::{CalendarEvent, RecurrenceRule};

/// The `VEVENT` inside a rendered `VCALENDAR`, borrowed from it.
///
/// # Safety
///
/// `calendar` must be a valid `ICalComponent`.
unsafe fn vevent(calendar: *mut ICalComponent) -> *mut ICalComponent {
    // SAFETY: the caller guarantees a valid component; the returned wrapper is
    // ours and is leaked for the length of the test, which is the shorter life.
    unsafe { i_cal_component_get_first_component(calendar, I_CAL_VEVENT_COMPONENT) }
}

/// The event a hostile server sends: `duration`, `timeZone` and a recurrence
/// rule's `frequency` all carry a line break and a property of the attacker's
/// choosing. None of the three passes through `escape` on the way out.
fn hostile_event() -> CalendarEvent {
    CalendarEvent {
        id: Some("E1".into()),
        title: Some("Standup".to_owned()),
        start: Some("2026-01-15T13:00:00".to_owned()),
        duration: Some("PT1H\r\nSUMMARY:Cancelled".to_owned()),
        time_zone: Some("Europe/Berlin\r\nDESCRIPTION:See attachment".to_owned()),
        recurrence_rule: Some(RecurrenceRule {
            frequency: "daily\r\nLOCATION:Elsewhere".to_owned(),
            ..RecurrenceRule::default()
        }),
        ..CalendarEvent::default()
    }
}

/// The exploit, run all the way into libical: the summary Evolution shows must
/// be the one `title` says, and the injected properties must not exist at all.
#[test]
fn no_unescaped_value_can_add_a_property_to_the_component() {
    let ics = jmap_ical::event_to_ical(&hostile_event());
    let calendar = marshal::component_from_ical(&ics);
    assert!(
        !calendar.is_null(),
        "the sanitised object did not parse:\n{ics}"
    );

    // SAFETY: `calendar` is the reference `component_from_ical` handed over, and
    // the getters below borrow from it.
    unsafe {
        let event = vevent(calendar);
        assert!(!event.is_null(), "no VEVENT in:\n{ics}");

        let uid = i_cal_component_get_uid(event);
        assert_eq!(CStr::from_ptr(uid).to_string_lossy(), "E1");

        let summary = i_cal_component_get_summary(event);
        assert_eq!(
            CStr::from_ptr(summary).to_string_lossy(),
            "Standup",
            "the duration's injected SUMMARY reached libical"
        );

        // Exactly one SUMMARY, so the injection did not merely lose the race.
        let first = i_cal_component_get_first_property(event, I_CAL_SUMMARY_PROPERTY);
        assert!(!first.is_null());
        let second = eds_sys::i_cal_component_get_next_property(event, I_CAL_SUMMARY_PROPERTY);
        assert!(
            second.is_null(),
            "a second SUMMARY property reached libical"
        );
        gobject_sys::g_object_unref(first.cast());

        for (kind, name) in [
            (eds_sys::I_CAL_DESCRIPTION_PROPERTY, "DESCRIPTION"),
            (eds_sys::I_CAL_LOCATION_PROPERTY, "LOCATION"),
        ] {
            let injected = i_cal_component_get_first_property(event, kind);
            assert!(
                injected.is_null(),
                "an injected {name} property reached libical:\n{ics}"
            );
        }

        gobject_sys::g_object_unref(event.cast());
        marshal::component_unref(calendar);
    }
}

/// The other thing a CRLF buys: `END:VEVENT` followed by a second
/// `BEGIN:VEVENT`, which would make one server-side event two appointments.
#[test]
fn no_unescaped_value_can_close_the_event_and_open_another() {
    let ics = jmap_ical::event_to_ical(&CalendarEvent {
        duration: Some(
            "PT1H\r\nEND:VEVENT\r\nBEGIN:VEVENT\r\nUID:E2\r\nSUMMARY:Mallory\r\nEND:VEVENT\r\nX"
                .to_owned(),
        ),
        ..hostile_event()
    });

    let lines = |name: &str| {
        ics.split("\r\n")
            .filter(|line| line.eq_ignore_ascii_case(name))
            .count()
    };
    assert_eq!(lines("BEGIN:VEVENT"), 1, "a second event appeared:\n{ics}");
    assert_eq!(lines("END:VEVENT"), 1, "the event was ended twice:\n{ics}");

    let calendar = marshal::component_from_ical(&ics);
    assert!(!calendar.is_null());
    // SAFETY: as above.
    unsafe {
        let event = vevent(calendar);
        assert!(!event.is_null());
        let next = eds_sys::i_cal_component_get_next_component(calendar, I_CAL_VEVENT_COMPONENT);
        assert!(next.is_null(), "libical read two VEVENTs out of:\n{ics}");
        gobject_sys::g_object_unref(event.cast());
        marshal::component_unref(calendar);
    }
}

/// F4, at the C boundary: a document nested past the parser's cap is refused as
/// an error rather than parsed into a tree whose recursive drop aborts the
/// process. libical is asked nothing here — `ical_to_event` never gets that far.
#[test]
fn a_pathologically_nested_object_is_an_error_not_an_abort() {
    let mut ics = String::from("BEGIN:VCALENDAR\r\n");
    for _ in 0..100_000 {
        ics.push_str("BEGIN:VALARM\r\n");
    }
    for _ in 0..100_000 {
        ics.push_str("END:VALARM\r\n");
    }
    ics.push_str("END:VCALENDAR\r\n");

    assert!(matches!(
        jmap_ical::ical_to_event(&ics),
        Err(jmap_ical::ICalError::TooDeep(_))
    ));
}

/// F7: the threading claim the instance struct rests on, made a compile error
/// rather than a comment. See `jmap-backend-book`'s counterpart.
#[test]
fn the_connection_an_instance_holds_is_shareable_across_threads() {
    fn assert_send_sync<T: Send + Sync>() {}

    assert_send_sync::<CalSync>();
    assert_send_sync::<Slot<RwLock<Option<CalSync>>>>();
}
