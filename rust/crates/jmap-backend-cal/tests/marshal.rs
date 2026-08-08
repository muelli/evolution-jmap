// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! The C boundary of the calendar backend, exercised the way EDS will: every
//! list is walked as a `GSList` and freed with the function EDS would call, and
//! the instances handed to a save are real `ECalComponent`s owned by the
//! caller — so a wrong node type, a missing copy or a stolen reference shows up
//! here rather than as a crash in `evolution-calendar-factory`.

use std::ffi::CStr;
use std::ptr;

use eds_sys::{
    ECalComponent, ECalMetaBackendInfo, I_CAL_VCALENDAR_COMPONENT, e_cal_component_new_from_string,
    e_cal_meta_backend_info_free, i_cal_component_isa,
};
use glib_sys::{GSList, g_slist_length, g_slist_nth_data};
use gobject_sys::g_object_unref;
use jmap_backend_cal::marshal;
use jmap_cal_sync::ComponentInfo;

/// The rendering `jmap-ical` produces and `ECalMetaBackend` caches: one
/// `VEVENT` in a `VCALENDAR`.
const OBJECT: &str = "BEGIN:VCALENDAR\r\n\
                      VERSION:2.0\r\n\
                      PRODID:-//evolution-jmap//JMAP calendar backend//EN\r\n\
                      BEGIN:VEVENT\r\n\
                      UID:K1\r\n\
                      SUMMARY:Standup\r\n\
                      DTSTART;TZID=Europe/Zurich:20260810T090000\r\n\
                      END:VEVENT\r\n\
                      END:VCALENDAR\r\n";

fn info(uid: &str, revision: &str, icalendar: &str) -> ComponentInfo {
    ComponentInfo {
        uid: uid.to_owned(),
        revision: revision.to_owned(),
        icalendar: icalendar.to_owned(),
    }
}

/// Reads a `GSList` node as an `ECalMetaBackendInfo`, the way
/// `e_cal_meta_backend_process_changes_sync` does.
unsafe fn nth_info(
    list: *mut GSList,
    n: u32,
) -> (String, Option<String>, Option<String>, Option<String>) {
    unsafe {
        let node = g_slist_nth_data(list, n).cast::<ECalMetaBackendInfo>();
        assert!(!node.is_null(), "no node {n}");
        let text =
            |p: *mut i8| (!p.is_null()).then(|| CStr::from_ptr(p).to_string_lossy().into_owned());
        (
            text((*node).uid).expect("a removal without a uid identifies nothing"),
            text((*node).revision),
            text((*node).object),
            text((*node).extra),
        )
    }
}

/// One instance of an event, as `save_component_sync` receives them: an
/// `ECalComponent` around a single `VEVENT`.
fn instance(vevent: &str) -> *mut ECalComponent {
    let text = std::ffi::CString::new(vevent).unwrap();
    // SAFETY: the text is NUL-terminated and valid for the call.
    let component = unsafe { e_cal_component_new_from_string(text.as_ptr()) };
    assert!(!component.is_null(), "the instance did not parse: {vevent}");
    component
}

/// Builds the `GSList` of instances EDS passes, in the given order. The
/// components stay owned by the caller, which is the ownership the vfunc has.
fn instance_list(components: &[*mut ECalComponent]) -> *mut GSList {
    let mut list = ptr::null_mut();
    for component in components.iter().rev() {
        // SAFETY: `list` is a valid GSList and the payload outlives it.
        list = unsafe { glib_sys::g_slist_prepend(list, component.cast()) };
    }
    list
}

const MASTER: &str = "BEGIN:VEVENT\r\nUID:K1\r\nSUMMARY:Standup\r\nDTSTART:20260810T070000Z\r\n\
                      RRULE:FREQ=DAILY\r\nEND:VEVENT\r\n";
const OVERRIDE: &str = "BEGIN:VEVENT\r\nUID:K1\r\nRECURRENCE-ID:20260812T070000Z\r\n\
                        SUMMARY:Standup, moved\r\nDTSTART:20260812T080000Z\r\nEND:VEVENT\r\n";

#[test]
fn an_info_list_carries_one_node_per_event_in_order() {
    let infos = [info("K1", "r1", OBJECT), info("K2", "r2", OBJECT)];
    let list = marshal::info_list(&infos);

    unsafe {
        assert_eq!(g_slist_length(list), 2);
        assert_eq!(
            nth_info(list, 0),
            (
                "K1".to_owned(),
                Some("r1".to_owned()),
                Some(OBJECT.to_owned()),
                None
            )
        );
        assert_eq!(nth_info(list, 1).0, "K2");
        glib_sys::g_slist_free_full(list, Some(e_cal_meta_backend_info_free));
    }
}

/// EDS reads "no objects" as a NULL list, not as an empty allocation.
#[test]
fn an_empty_list_is_null() {
    assert!(marshal::info_list(&[]).is_null());
    assert!(marshal::removed_info_list(&[]).is_null());
}

/// The calendar's removals are `ECalMetaBackendInfo`s, not bare strings the way
/// the address book's are — a `GSList` of `gchar *` here would be read as
/// structs and dereference the first characters of a uid as pointers. Only the
/// uid can be filled in: a component that is gone has no revision and no
/// object to report, and `e_cal_meta_backend_info_new` documents both as
/// nullable while the uid is not.
#[test]
fn a_removal_is_an_info_carrying_only_the_uid() {
    let list = marshal::removed_info_list(&["K1".to_owned(), "K2".to_owned()]);

    unsafe {
        assert_eq!(g_slist_length(list), 2);
        assert_eq!(nth_info(list, 0), ("K1".to_owned(), None, None, None));
        assert_eq!(nth_info(list, 1), ("K2".to_owned(), None, None, None));
        glib_sys::g_slist_free_full(list, Some(e_cal_meta_backend_info_free));
    }
}

#[test]
fn a_calendar_object_round_trips_through_icalcomponent() {
    let component = marshal::component_from_ical(OBJECT);
    assert!(!component.is_null());

    unsafe {
        assert_eq!(i_cal_component_isa(component), I_CAL_VCALENDAR_COMPONENT);
        let back = marshal::ical_from_component(component).expect("rendered");
        assert!(back.contains("SUMMARY:Standup"), "lost the summary: {back}");
        assert!(back.contains("TZID=Europe/Zurich"), "lost the zone: {back}");
        assert_eq!(marshal::component_uid(component).as_deref(), Some("K1"));
        marshal::component_unref(component);
    }
}

/// `ECalMetaBackend` accepts a bare `VEVENT` from `load_component_sync` as
/// readily as an envelope, and the cache may hand one back on a save, so the
/// uid has to be readable either way round.
#[test]
fn a_bare_vevent_is_a_component_too_and_reports_its_uid() {
    let component = marshal::component_from_ical(MASTER);
    assert!(!component.is_null());
    unsafe {
        assert_eq!(marshal::component_uid(component).as_deref(), Some("K1"));
        marshal::component_unref(component);
    }
}

#[test]
fn text_that_is_not_a_calendar_object_is_refused() {
    assert!(marshal::component_from_ical("not a calendar object at all").is_null());
}

/// libical parses an envelope with nothing in it happily, and EDS would take
/// the result — an appointment that exists and has no properties. It has to be
/// a failure instead, for the same reason a malformed vCard is on the address
/// book side.
#[test]
fn an_envelope_with_no_event_in_it_is_refused() {
    assert!(
        marshal::component_from_ical("BEGIN:VCALENDAR\r\nVERSION:2.0\r\nEND:VCALENDAR\r\n")
            .is_null()
    );
}

/// A component Evolution has just created carries the `UID` it invented, but a
/// missing one must not come back as a usable identifier: the save path tells an
/// edit from a create by exactly this.
///
/// Both spellings are checked although libical folds them together — a `UID:`
/// line with no value reads back as absent, not as `""` — because that is a
/// libical behaviour rather than a promise, and it is the one thing standing
/// between an empty uid and a `CalendarEvent/set` update naming it.
#[test]
fn a_component_without_a_uid_reports_none() {
    for text in [
        "BEGIN:VEVENT\r\nSUMMARY:Nameless\r\nEND:VEVENT\r\n",
        "BEGIN:VEVENT\r\nUID:\r\nSUMMARY:Nameless\r\nEND:VEVENT\r\n",
    ] {
        let component = marshal::component_from_ical(text);
        assert!(!component.is_null(), "did not parse: {text:?}");
        unsafe {
            assert_eq!(marshal::component_uid(component), None, "for {text:?}");
            marshal::component_unref(component);
        }
    }
}

/// ...and an empty uid that *is* there — the shape libical does keep, when it
/// is set rather than parsed — is absent too, for the same reason: "" would be
/// sent to the server as the identifier of an event to patch.
#[test]
fn a_component_whose_uid_was_emptied_reports_none() {
    let component = marshal::component_from_ical(MASTER);
    let empty = std::ffi::CString::new("").unwrap();
    unsafe {
        eds_sys::i_cal_component_set_uid(component, empty.as_ptr());
        assert_eq!(marshal::component_uid(component), None);
        marshal::component_unref(component);
    }
}

/// The master is the instance without a `RECURRENCE-ID`, and it is what the
/// mapping reads — *whatever position it holds in the list*. Taking the first
/// node instead would map a moved single occurrence as if it were the series.
#[test]
fn the_master_is_found_among_the_instances_whatever_the_order() {
    let components = [instance(OVERRIDE), instance(MASTER)];
    let list = instance_list(&components);

    unsafe {
        let saved = marshal::icalendar_from_instances(list).expect("a master");
        assert_eq!(saved.uid.as_deref(), Some("K1"));
        assert!(
            saved.icalendar.starts_with("BEGIN:VCALENDAR"),
            "not an envelope: {}",
            saved.icalendar
        );
        assert!(
            saved.icalendar.contains("RRULE:FREQ=DAILY"),
            "not the master: {}",
            saved.icalendar
        );
        assert!(
            !saved.icalendar.contains("RECURRENCE-ID"),
            "carried an override: {}",
            saved.icalendar
        );

        glib_sys::g_slist_free(list);
        for component in components {
            g_object_unref(component.cast());
        }
    }
}

/// The instances belong to EDS. Cloning rather than taking is not an optimisation
/// question here: `i_cal_component_take_component` takes ownership, and the
/// component an `ECalComponent` lends out is not ours to give away — doing so
/// aborts the process on a double free once EDS drops its own reference, which
/// this test's teardown is.
#[test]
fn the_instances_survive_being_marshalled() {
    let master = instance(MASTER);
    let list = instance_list(&[master]);

    unsafe {
        let first = marshal::icalendar_from_instances(list).expect("a master");
        let again = marshal::icalendar_from_instances(list).expect("still a master");
        assert_eq!(first.icalendar, again.icalendar);

        glib_sys::g_slist_free(list);
        g_object_unref(master.cast());
    }
}

/// Only a detached occurrence, with no series to attach it to. JSCalendar keeps
/// overrides in `recurrenceOverrides`, which this mapping does not cover, so
/// there is nothing honest to send — and a failure the user sees beats a save
/// that silently rewrites the whole series to look like one moved day.
#[test]
fn a_save_of_nothing_but_an_override_is_refused() {
    let component = instance(OVERRIDE);
    let list = instance_list(&[component]);

    unsafe {
        assert!(marshal::icalendar_from_instances(list).is_none());
        glib_sys::g_slist_free(list);
        g_object_unref(component.cast());
    }
}

#[test]
fn a_save_with_no_instances_at_all_is_refused() {
    unsafe {
        assert!(marshal::icalendar_from_instances(ptr::null()).is_none());
    }
}
