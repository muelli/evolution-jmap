// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later
//
// The calendar vfuncs do not traffic in strings the way the address book ones
// do: `load_component_sync` hands back an `ICalComponent *` and
// `save_component_sync` is given a list of `ECalComponent *`. Both are types
// from libraries the address book never touched — libical-glib and libecal —
// so this is the first test that the generated bindings reach them at all, and
// that the ownership rules the marshalling will rely on are the ones the
// headers document: `i_cal_component_as_ical_string` transfers the string,
// `e_cal_component_get_icalcomponent` does not transfer the component.

use eds_sys::*;
use std::ffi::{CStr, CString};

/// A minimal calendar object of the shape `jmap-ical` emits and reads: one
/// `VEVENT` inside a `VCALENDAR`.
const OBJECT: &str = "BEGIN:VCALENDAR\r\n\
                      VERSION:2.0\r\n\
                      PRODID:-//evolution-jmap//JMAP calendar backend//EN\r\n\
                      BEGIN:VEVENT\r\n\
                      UID:K1\r\n\
                      SUMMARY:Standup\r\n\
                      DTSTART;TZID=Europe/Zurich:20260810T090000\r\n\
                      END:VEVENT\r\n\
                      END:VCALENDAR\r\n";

fn text(value: &str) -> CString {
    CString::new(value).expect("no interior NUL")
}

/// Reads an owned `gchar *` and frees it, which is what `as_ical_string`
/// returns.
unsafe fn take_string(raw: *mut gchar) -> String {
    assert!(!raw.is_null(), "the call returned NULL");
    unsafe {
        let value = CStr::from_ptr(raw).to_string_lossy().into_owned();
        g_free(raw.cast());
        value
    }
}

#[test]
fn an_icalendar_object_parses_and_renders_back() {
    let source = text(OBJECT);
    unsafe {
        let calendar = i_cal_component_new_from_string(source.as_ptr());
        assert!(!calendar.is_null(), "the object did not parse");
        assert_eq!(i_cal_component_isa(calendar), I_CAL_VCALENDAR_COMPONENT);

        let rendered = take_string(i_cal_component_as_ical_string(calendar));
        assert!(rendered.contains("SUMMARY:Standup"), "lost: {rendered}");
        assert!(
            rendered.contains("TZID=Europe/Zurich"),
            "lost the zone: {rendered}"
        );

        g_object_unref(calendar.cast());
    }
}

/// The `VEVENT` inside the envelope is what the mapping reads, and its `UID`
/// is the identifier EDS keys its cache on — so both the descent into the
/// child and the borrowed (not transferred) `get_uid` string have to work.
#[test]
fn the_vevent_inside_a_vcalendar_is_reachable_with_its_uid() {
    let source = text(OBJECT);
    unsafe {
        let calendar = i_cal_component_new_from_string(source.as_ptr());
        let event = i_cal_component_get_first_component(calendar, I_CAL_VEVENT_COMPONENT);
        assert!(!event.is_null(), "no VEVENT in the object");
        assert_eq!(i_cal_component_isa(event), I_CAL_VEVENT_COMPONENT);

        let uid = i_cal_component_get_uid(event);
        assert!(!uid.is_null(), "the event has no UID");
        assert_eq!(CStr::from_ptr(uid).to_str().unwrap(), "K1");

        g_object_unref(event.cast());
        g_object_unref(calendar.cast());
    }
}

/// `load_component_sync` may only hand EDS a component or a failure, never an
/// empty one, so text that is not a calendar object has to be distinguishable
/// from text that is. Unlike `EVCard`, libical says so by returning NULL.
#[test]
fn text_that_is_not_a_calendar_object_parses_to_null() {
    let source = text("not a calendar object at all");
    unsafe {
        assert!(i_cal_component_new_from_string(source.as_ptr()).is_null());
    }
}

/// An empty `VCALENDAR` is the envelope with nothing in it, which parses fine
/// and is not an event. The marshalling has to notice, or a save would send
/// the server a patch derived from nothing.
#[test]
fn an_empty_vcalendar_parses_but_holds_no_event() {
    let source = text("BEGIN:VCALENDAR\r\nVERSION:2.0\r\nEND:VCALENDAR\r\n");
    unsafe {
        let calendar = i_cal_component_new_from_string(source.as_ptr());
        assert!(!calendar.is_null());
        assert!(
            i_cal_component_get_first_component(calendar, I_CAL_VEVENT_COMPONENT).is_null(),
            "an empty VCALENDAR reported a VEVENT"
        );
        g_object_unref(calendar.cast());
    }
}

/// A fresh `VCALENDAR` built in Rust is how the marshalling will re-wrap the
/// instances `save_component_sync` is handed, so building an envelope and
/// filling it has to work in that direction too.
///
/// The component put in it is a **clone**. `i_cal_component_take_component`
/// takes ownership, and a component reached through its parent — which is
/// every component the vfuncs hand us, since an `ECalComponent` only lends its
/// own out — is already owned by that parent. Giving it away instead of a copy
/// aborts the process on a double free; this test is the reason the
/// marshalling clones, and it fails without the clone.
#[test]
fn a_vcalendar_can_be_built_and_given_a_clone_to_own() {
    let source = text(OBJECT);
    unsafe {
        let parsed = i_cal_component_new_from_string(source.as_ptr());
        let event = i_cal_component_get_first_component(parsed, I_CAL_VEVENT_COMPONENT);

        let calendar = i_cal_component_new_vcalendar();
        assert!(!calendar.is_null());
        i_cal_component_take_component(calendar, i_cal_component_clone(event));

        let rendered = take_string(i_cal_component_as_ical_string(calendar));
        assert!(rendered.starts_with("BEGIN:VCALENDAR"), "{rendered}");
        assert!(rendered.contains("UID:K1"), "lost the event: {rendered}");
        // The original is untouched: it still belongs to `parsed`.
        assert_eq!(
            CStr::from_ptr(i_cal_component_get_uid(event))
                .to_str()
                .unwrap(),
            "K1"
        );

        g_object_unref(calendar.cast());
        g_object_unref(event.cast());
        g_object_unref(parsed.cast());
    }
}

/// `save_component_sync` is handed `ECalComponent`s, not `ICalComponent`s, and
/// the component it carries is borrowed — a marshalling that unref'd it would
/// free memory EDS still owns.
///
/// The string is a bare `VEVENT`, not the `VCALENDAR` above: an
/// `ECalComponent` wraps one component of a kind it recognises, so handing it
/// the envelope yields nothing at all.
#[test]
fn an_ecalcomponent_lends_out_the_icalcomponent_it_carries() {
    let source = text(
        "BEGIN:VEVENT\r\nUID:K1\r\nSUMMARY:Standup\r\nDTSTART:20260810T070000Z\r\nEND:VEVENT\r\n",
    );
    unsafe {
        let component = e_cal_component_new_from_string(source.as_ptr());
        assert!(
            !component.is_null(),
            "the event did not parse as an ECalComponent"
        );

        let inner = e_cal_component_get_icalcomponent(component);
        assert!(!inner.is_null(), "the ECalComponent carries no component");
        let uid = i_cal_component_get_uid(inner);
        assert_eq!(CStr::from_ptr(uid).to_str().unwrap(), "K1");

        // Only the ECalComponent is ours; the component above was borrowed,
        // which is why nothing unrefs it.
        g_object_unref(component.cast());
    }
}
