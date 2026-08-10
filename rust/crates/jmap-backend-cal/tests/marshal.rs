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

/// The master is the instance without a `RECURRENCE-ID`, and it leads the
/// envelope — *whatever position it holds in the list*. Taking the first node
/// instead would map a moved single occurrence as if it were the series.
///
/// The detached occurrences come with it. `jmap-ical` reads a series' overrides
/// out of them, so leaving them behind would hand the mapping a component
/// saying the edited day is like every other, and a save would patch that over
/// the server's copy.
#[test]
fn the_master_leads_the_envelope_and_the_overrides_follow_it() {
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

        let master = saved
            .icalendar
            .find("RRULE:FREQ=DAILY")
            .unwrap_or_else(|| panic!("not the master: {}", saved.icalendar));
        let detached = saved
            .icalendar
            .find("RECURRENCE-ID")
            .unwrap_or_else(|| panic!("lost the override: {}", saved.icalendar));
        assert!(master < detached, "{}", saved.icalendar);
        assert!(
            saved.icalendar.contains("SUMMARY:Standup\\, moved"),
            "lost the override's own title: {}",
            saved.icalendar
        );
        // One copy of each, rather than the master twice.
        assert_eq!(saved.icalendar.matches("BEGIN:VEVENT").count(), 2);

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

/// Only a detached occurrence, with no series to attach it to. JSCalendar says
/// "this instance differs" only relative to a series, so there is nothing
/// honest to send — and a failure the user sees beats a save that silently
/// rewrites the whole series to look like one moved day.
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

/// libical's own identifier for a builtin zone. Evolution's appointment editor
/// sets a start with the zone *object*, and the `TZID` libical then writes is
/// this — a form RFC 5545 §3.2.19 allows only because the `VTIMEZONE` that
/// defines it is meant to travel in the same object. Nothing outside libical
/// resolves it, and it is not an RFC 8984 §1.4.9 `TimeZoneId` either, so an
/// envelope that names it and defines nothing says "some zone" and no more.
const LIBICAL_TZID: &str = "/freeassociation.sourceforge.net/Europe/Berlin";

/// One instance in the zone `tzid` names, at a wall-clock time.
fn zoned(tzid: &str) -> String {
    format!(
        "BEGIN:VEVENT\r\nUID:K1\r\nSUMMARY:Standup\r\n\
         DTSTART;TZID={tzid}:20260810T090000\r\nEND:VEVENT\r\n"
    )
}

/// How many `VTIMEZONE`s the envelope defines.
fn definitions(icalendar: &str) -> usize {
    icalendar.matches("BEGIN:VTIMEZONE").count()
}

/// The zone an event is in has to reach the server, and the only thing that can
/// carry it is the definition libical holds: the identifier on the `DTSTART` is
/// libical's own, so an envelope without the `VTIMEZONE` beside it hands the
/// mapping a zone it cannot name — and `patch::diff` then leaves `timeZone`
/// alone, which is a zone change the user made and nobody else ever sees.
///
/// The last assertion is the one that matters; the two before it say *how* the
/// zone got there, so that a failure names the missing half rather than only
/// the symptom.
#[test]
fn a_zoned_instance_brings_the_definition_of_its_zone() {
    let component = instance(&zoned(LIBICAL_TZID));
    let list = instance_list(&[component]);

    unsafe {
        let saved = marshal::icalendar_from_instances(list).expect("a master");
        assert_eq!(
            definitions(&saved.icalendar),
            1,
            "the zone the event is in is not defined in the envelope: {}",
            saved.icalendar
        );
        assert!(
            saved.icalendar.contains(&format!("TZID:{LIBICAL_TZID}")),
            "the definition is not of the zone the event names: {}",
            saved.icalendar
        );
        assert_eq!(
            jmap_ical::ical_to_event(&saved.icalendar)
                .expect("the envelope is a calendar object")
                .time_zone
                .as_deref(),
            Some("Europe/Berlin"),
            "the mapping still cannot name the zone: {}",
            saved.icalendar
        );

        glib_sys::g_slist_free(list);
        g_object_unref(component.cast());
    }
}

/// A zone already spelled the way JSCalendar wants it needs no translating, but
/// it does need defining — and under the identifier the event's properties use,
/// not libical's, or the envelope refers to one zone and defines another.
#[test]
fn a_zone_named_plainly_is_defined_under_the_name_the_event_uses() {
    let component = instance(&zoned("Europe/Zurich"));
    let list = instance_list(&[component]);

    unsafe {
        let saved = marshal::icalendar_from_instances(list).expect("a master");
        assert_eq!(definitions(&saved.icalendar), 1, "{}", saved.icalendar);
        assert!(
            saved.icalendar.contains("TZID:Europe/Zurich"),
            "the definition does not carry the event's own identifier: {}",
            saved.icalendar
        );
        assert!(
            !saved.icalendar.contains("TZID:/"),
            "the envelope defines libical's identifier instead of the event's: {}",
            saved.icalendar
        );
        assert_eq!(
            jmap_ical::ical_to_event(&saved.icalendar)
                .expect("the envelope is a calendar object")
                .time_zone
                .as_deref(),
            Some("Europe/Zurich"),
            "{}",
            saved.icalendar
        );

        glib_sys::g_slist_free(list);
        g_object_unref(component.cast());
    }
}

/// Every instance is asked which zone it means, and every property of it — a
/// detached occurrence states the instant it replaces in the zone of the series,
/// and may have been moved into another. One definition per zone, however many
/// properties refer to it: a second copy of the same `VTIMEZONE` is a duplicate
/// `TZID` in one object, which is not a calendar object any more.
#[test]
fn every_instance_and_every_property_is_asked_which_zone_it_means() {
    let master = format!(
        "BEGIN:VEVENT\r\nUID:K1\r\nSUMMARY:Standup\r\n\
         DTSTART;TZID={LIBICAL_TZID}:20260810T090000\r\n\
         RRULE:FREQ=DAILY\r\nEND:VEVENT\r\n"
    );
    let moved = format!(
        "BEGIN:VEVENT\r\nUID:K1\r\nSUMMARY:Standup\\, moved\r\n\
         RECURRENCE-ID;TZID={LIBICAL_TZID}:20260812T090000\r\n\
         DTSTART;TZID=Europe/Zurich:20260812T100000\r\nEND:VEVENT\r\n"
    );
    let components = [instance(&moved), instance(&master)];
    let list = instance_list(&components);

    unsafe {
        let saved = marshal::icalendar_from_instances(list).expect("a master");
        assert_eq!(
            definitions(&saved.icalendar),
            2,
            "not one definition per zone: {}",
            saved.icalendar
        );
        assert!(
            saved.icalendar.contains(&format!("TZID:{LIBICAL_TZID}")),
            "{}",
            saved.icalendar
        );
        assert!(
            saved.icalendar.contains("TZID:Europe/Zurich"),
            "the zone the occurrence was moved into is undefined: {}",
            saved.icalendar
        );

        glib_sys::g_slist_free(list);
        for component in components {
            g_object_unref(component.cast());
        }
    }
}

/// A `TZID` no zone database knows — Windows' own spelling is the one that
/// reaches EDS from an Exchange invitation — is left undefined rather than
/// guessed at. The event still goes, and the mapping refuses the zone
/// downstream, which leaves the server's own value standing.
#[test]
fn a_zone_no_database_knows_is_left_undefined() {
    let component = instance(&zoned("W. Europe Standard Time"));
    let list = instance_list(&[component]);

    unsafe {
        let saved = marshal::icalendar_from_instances(list).expect("a master");
        assert_eq!(
            definitions(&saved.icalendar),
            0,
            "a zone was invented for an identifier nothing resolves: {}",
            saved.icalendar
        );
        assert!(
            saved.icalendar.contains("SUMMARY:Standup"),
            "the event itself did not survive: {}",
            saved.icalendar
        );

        glib_sys::g_slist_free(list);
        g_object_unref(component.cast());
    }
}

/// UTC is the zone libical resolves and has no `VTIMEZONE` for — it is not a
/// zone with rules, it is the absence of them. Asking it for a definition
/// returns NULL, and NULL is not something to put in an envelope.
#[test]
fn utc_is_a_zone_with_nothing_to_define() {
    let component = instance(&zoned("UTC"));
    let list = instance_list(&[component]);

    unsafe {
        let saved = marshal::icalendar_from_instances(list).expect("a master");
        assert_eq!(definitions(&saved.icalendar), 0, "{}", saved.icalendar);
        assert!(
            saved.icalendar.contains("SUMMARY:Standup"),
            "the event itself did not survive: {}",
            saved.icalendar
        );

        glib_sys::g_slist_free(list);
        g_object_unref(component.cast());
    }
}

/// The recurrence parts `jmap-ical` writes, run through the parser that
/// actually reads them.
///
/// `jmap-ical` emits `BYDAY` before `BYMONTHDAY` before `BYMONTH` because that
/// is the order libical writes them in, and a rule that comes back out of EDS's
/// own cache spelled differently than it went in compares unequal to itself —
/// which the save path reads as an edit. Nothing in `jmap-ical` can check that
/// claim: calcard is what parses there. This is where libical is available to be
/// asked.
#[test]
fn libical_keeps_the_recurrence_parts_this_mapping_writes() {
    for rrule in [
        "FREQ=MONTHLY;COUNT=6;BYDAY=WE;BYMONTHDAY=15,-1",
        "FREQ=MONTHLY;BYMONTHDAY=-1",
        "FREQ=MONTHLY;INTERVAL=2;BYMONTHDAY=1,15",
        "FREQ=YEARLY;COUNT=4;BYMONTH=3,9",
        "FREQ=YEARLY;BYDAY=WE;BYMONTHDAY=15;BYMONTH=3",
        "FREQ=YEARLY;COUNT=4;BYYEARDAY=1,-1",
        "FREQ=YEARLY;BYYEARDAY=366",
        // Every modeled part at once, which is where the claim about the order
        // `BYYEARDAY` is written in gets tested: between the days of the month
        // and the months.
        "FREQ=YEARLY;BYDAY=WE;BYMONTHDAY=15;BYYEARDAY=100;BYMONTH=3",
        "FREQ=YEARLY;COUNT=4;BYWEEKNO=1,-1;WKST=SU",
        "FREQ=YEARLY;BYWEEKNO=53",
        // And where `BYWEEKNO` is written: between the days of the year and the
        // months, before the day the week is counted from.
        "FREQ=YEARLY;BYDAY=WE;BYMONTHDAY=15;BYYEARDAY=100;BYWEEKNO=20;BYMONTH=3;WKST=SU",
        "FREQ=MONTHLY;COUNT=4;BYDAY=FR;BYSETPOS=-1",
        "FREQ=MONTHLY;BYDAY=MO,TU,WE,TH,FR;BYSETPOS=1,-1",
        // And where `BYSETPOS` is written: after the months, before the day the
        // week is counted from — the seventh part, and the one that decides
        // where a rule naming everything folds.
        "FREQ=YEARLY;BYDAY=WE;BYMONTHDAY=15;BYYEARDAY=100;BYWEEKNO=20;BYMONTH=3;\
         BYSETPOS=2;WKST=SU",
        // RFC 5545 §3.3.10 defines BYYEARDAY beside a period shorter than a day
        // as well, limiting what those expand to.
        "FREQ=HOURLY;BYYEARDAY=100",
        // RFC 5545 §3.3.10 defines BYMONTH at every frequency, so a weekly rule
        // may carry one and `jmap-ical` does not gate it as it gates BYMONTHDAY.
        "FREQ=WEEKLY;BYDAY=MO;BYMONTH=12",
    ] {
        assert_eq!(reparsed_rrule(rrule).as_deref(), Some(rrule));
    }
}

/// And why a day of the month outside RFC 5545's `ordmoday` is refused whole
/// rather than written: libical answers such a rule by dropping the **entire**
/// `RRULE`, so an event written that way reaches EDS's cache as a single
/// appointment and the user's series is gone. `jmap-ical`'s `month_day_token`
/// leaving the part off is what keeps that from ever being written.
#[test]
fn a_day_of_the_month_no_month_has_would_cost_libical_the_whole_rule() {
    for rrule in ["FREQ=MONTHLY;BYMONTHDAY=32", "FREQ=MONTHLY;BYMONTHDAY=0"] {
        assert_eq!(reparsed_rrule(rrule).as_deref(), None, "{rrule}");
    }
}

/// The same question for the months of the year, whose answers are three
/// different shapes — which together are why `jmap-ical`'s `month_token` accepts
/// only the canonical decimal spelling of a month 1 to 12, and measured here
/// rather than assumed.
#[test]
fn libical_answers_for_a_month_this_mapping_refuses() {
    // Some values cost the whole `RRULE`, as a day of the month out of range
    // does: an event written that way would reach EDS's cache as a single
    // appointment with the user's series gone.
    for rrule in [
        "FREQ=YEARLY;BYMONTH=0",
        "FREQ=YEARLY;BYMONTH=-1",
        "FREQ=YEARLY;BYMONTH=99",
        "FREQ=YEARLY;BYMONTH=3,XX",
    ] {
        assert_eq!(reparsed_rrule(rrule).as_deref(), None, "{rrule}");
    }
    // A thirteenth month and a leap month, though, libical keeps verbatim — so
    // the reason to refuse them is not that the parser objects. It is that a
    // Gregorian series has neither, so the rule would sit in the cache as one
    // that never occurs; `5L` names a month at all only under RFC 7529's
    // `RSCALE`, which nothing here writes and whose support in libical is a build
    // option in the first place.
    for rrule in ["FREQ=YEARLY;BYMONTH=13", "FREQ=YEARLY;BYMONTH=5L"] {
        assert_eq!(reparsed_rrule(rrule).as_deref(), Some(rrule), "{rrule}");
    }
    // And the third shape: a spelling that is kept but *changed*. RFC 5545's
    // `monthnum` admits the leading zero, and a rule written `03` comes back `3`
    // — a difference the save path reads as an edit the user never made.
    assert_eq!(
        reparsed_rrule("FREQ=YEARLY;BYMONTH=03").as_deref(),
        Some("FREQ=YEARLY;BYMONTH=3"),
    );
}

/// The same question for the days of the year, whose answers split the same
/// three ways the months' do — and not along the boundary RFC 5545 draws, which
/// is why `jmap-ical`'s `year_day_token` and its frequency gate are measured here
/// rather than assumed.
#[test]
fn libical_answers_for_a_day_of_the_year_this_mapping_refuses() {
    // Some values cost the whole `RRULE`, as a day of the month out of range
    // does: an event written that way would reach EDS's cache as a single
    // appointment with the user's series gone.
    for rrule in [
        "FREQ=YEARLY;BYYEARDAY=0",
        "FREQ=YEARLY;BYYEARDAY=999",
        "FREQ=YEARLY;BYYEARDAY=100,XX",
    ] {
        assert_eq!(reparsed_rrule(rrule).as_deref(), None, "{rrule}");
    }
    // But not every day outside RFC 5545's `yeardaynum`: 367 and -367 libical
    // keeps verbatim, where 999 costs the rule. So the reason to refuse a day just
    // past the end of a leap year is not that the parser objects — it is that no
    // year has one, and the rule would sit in EDS's cache as a series that never
    // occurs.
    for rrule in ["FREQ=YEARLY;BYYEARDAY=367", "FREQ=YEARLY;BYYEARDAY=-367"] {
        assert_eq!(reparsed_rrule(rrule).as_deref(), Some(rrule), "{rrule}");
    }
    // The frequency gate libical does not enforce either: it keeps
    // `BYYEARDAY=100` beside all three frequencies RFC 5545 §3.3.10 forbids it
    // next to. `jmap-ical` leaves the part off anyway, because a month is not a
    // period a day of the year sits inside and every other reader of the rule is
    // free to make of that what it likes.
    for rrule in [
        "FREQ=MONTHLY;BYYEARDAY=100",
        "FREQ=WEEKLY;BYYEARDAY=100",
        "FREQ=DAILY;BYYEARDAY=100",
    ] {
        assert_eq!(reparsed_rrule(rrule).as_deref(), Some(rrule), "{rrule}");
    }
    // And the third shape, a spelling kept but *changed*: RFC 5545's `yeardaynum`
    // admits the leading zero and the leading plus, and both come back canonical.
    // Unlike `BYMONTH=03`, that cannot bite the save path here — `byYearDay` holds
    // a number, so the only spelling this mapping can write is the canonical one.
    for (written, back) in [
        ("FREQ=YEARLY;BYYEARDAY=010", "FREQ=YEARLY;BYYEARDAY=10"),
        ("FREQ=YEARLY;BYYEARDAY=+100", "FREQ=YEARLY;BYYEARDAY=100"),
    ] {
        assert_eq!(reparsed_rrule(written).as_deref(), Some(back), "{written}");
    }
}

/// The same question for the weeks of the year, whose answers split the same
/// three ways the days of the year's do — and, as there, not along the boundary
/// RFC 5545 draws, which is why `jmap-ical`'s `week_no_token` and its frequency
/// gate are measured here rather than assumed.
#[test]
fn libical_answers_for_a_week_of_the_year_this_mapping_refuses() {
    // Some values cost the whole `RRULE`: an event written that way would reach
    // EDS's cache as a single appointment with the user's series gone.
    for rrule in [
        "FREQ=YEARLY;BYWEEKNO=0",
        "FREQ=YEARLY;BYWEEKNO=999",
        "FREQ=YEARLY;BYWEEKNO=20,XX",
    ] {
        assert_eq!(reparsed_rrule(rrule).as_deref(), None, "{rrule}");
    }
    // But not every week outside RFC 5545's `ordwk`: 54 and -54 libical keeps
    // verbatim, where 999 costs the rule. So the reason to refuse a week just past
    // the end of the longest year is not that the parser objects — it is that no
    // year has one, and the rule would sit in EDS's cache as a series that never
    // occurs.
    for rrule in ["FREQ=YEARLY;BYWEEKNO=54", "FREQ=YEARLY;BYWEEKNO=-54"] {
        assert_eq!(reparsed_rrule(rrule).as_deref(), Some(rrule), "{rrule}");
    }
    // The frequency gate libical does not enforce either: it keeps `BYWEEKNO=20`
    // beside every frequency RFC 5545 §3.3.10 forbids it next to — which here is
    // every frequency but `YEARLY`. `jmap-ical` leaves the part off anyway, because
    // no other reader of the rule is obliged to be as forgiving.
    for rrule in [
        "FREQ=MONTHLY;BYWEEKNO=20",
        "FREQ=WEEKLY;BYWEEKNO=20",
        "FREQ=DAILY;BYWEEKNO=20",
        "FREQ=HOURLY;BYWEEKNO=20",
    ] {
        assert_eq!(reparsed_rrule(rrule).as_deref(), Some(rrule), "{rrule}");
    }
    // And the third shape, a spelling kept but *changed*: `weeknum` admits the
    // leading zero and the leading plus, and both come back canonical. As with
    // `BYYEARDAY`, that cannot bite the save path — `byWeekNo` holds a number, so
    // the only spelling this mapping can write is the canonical one.
    for (written, back) in [
        ("FREQ=YEARLY;BYWEEKNO=020", "FREQ=YEARLY;BYWEEKNO=20"),
        ("FREQ=YEARLY;BYWEEKNO=+20", "FREQ=YEARLY;BYWEEKNO=20"),
    ] {
        assert_eq!(reparsed_rrule(written).as_deref(), Some(back), "{written}");
    }
}

/// The same question for the occurrence a rule selects out of its set, whose
/// answers split the same three ways the weeks of the year's do — and where the
/// gate that matters most, RFC 5545 §3.3.10's "only in conjunction with another
/// BYxxx rule part", libical does not enforce at all.
#[test]
fn libical_answers_for_a_position_in_the_set_this_mapping_refuses() {
    // Some values cost the whole `RRULE`: an event written that way would reach
    // EDS's cache as a single appointment with the user's series gone.
    for rrule in [
        "FREQ=MONTHLY;BYDAY=MO;BYSETPOS=0",
        "FREQ=MONTHLY;BYDAY=MO;BYSETPOS=999",
        "FREQ=MONTHLY;BYDAY=MO;BYSETPOS=1,XX",
    ] {
        assert_eq!(reparsed_rrule(rrule).as_deref(), None, "{rrule}");
    }
    // But not every position outside RFC 5545's `setposday`: 367 and -367
    // libical keeps verbatim, where 999 costs the rule. So the reason
    // `jmap-ical`'s `set_position_token` refuses a position just past the end of
    // a leap year is not that the parser objects — it is that no interval
    // expands to that many occurrences, and the rule would sit in EDS's cache
    // selecting an occurrence that is never there.
    for rrule in [
        "FREQ=MONTHLY;BYDAY=MO;BYSETPOS=367",
        "FREQ=MONTHLY;BYDAY=MO;BYSETPOS=-367",
    ] {
        assert_eq!(reparsed_rrule(rrule).as_deref(), Some(rrule), "{rrule}");
    }
    // And the gate `jmap-ical` carries that none of the other parts needs:
    // libical keeps a `BYSETPOS` with no other `BYxxx` beside it, at every
    // frequency, though §3.3.10 admits none of these. Such a rule selects out of
    // the single occurrence the frequency already names, so `BYSETPOS=2` is a
    // series that happens once and never again — which is why the mapping leaves
    // the part off rather than trusting the parser to object.
    for rrule in [
        "FREQ=DAILY;BYSETPOS=1",
        "FREQ=MONTHLY;BYSETPOS=-1",
        "FREQ=YEARLY;BYSETPOS=2",
    ] {
        assert_eq!(reparsed_rrule(rrule).as_deref(), Some(rrule), "{rrule}");
    }
    // And the third shape, a spelling kept but *changed*: `setposday` admits the
    // leading zero and the leading plus, and both come back canonical. As with
    // `BYYEARDAY`, that cannot bite the save path — `bySetPosition` holds a
    // number, so the only spelling this mapping can write is the canonical one.
    for (written, back) in [
        (
            "FREQ=MONTHLY;BYDAY=MO;BYSETPOS=01",
            "FREQ=MONTHLY;BYDAY=MO;BYSETPOS=1",
        ),
        (
            "FREQ=MONTHLY;BYDAY=MO;BYSETPOS=+1",
            "FREQ=MONTHLY;BYDAY=MO;BYSETPOS=1",
        ),
    ] {
        assert_eq!(reparsed_rrule(written).as_deref(), Some(back), "{written}");
    }
}

/// The same question for the hours of the day a rule repeats at, which is the
/// part whose answers most needed measuring: the position it is *written* in is
/// not the one every part before it took, and every value out of range costs the
/// whole rule.
#[test]
fn libical_answers_for_the_hours_of_the_day() {
    // Where `BYHOUR` goes: **first**, ahead of the days — not last, where each
    // part added before it went. `jmap-ical`'s `named_by_parts` puts it there
    // because of this measurement; a rule emitted in any other order comes back
    // out of EDS's own cache reordered and compares unequal to itself, which the
    // save path reads as an edit the user never made.
    assert_eq!(
        reparsed_rrule("FREQ=WEEKLY;BYDAY=MO;BYHOUR=9").as_deref(),
        Some("FREQ=WEEKLY;BYHOUR=9;BYDAY=MO"),
    );
    assert_eq!(
        reparsed_rrule(
            "FREQ=YEARLY;BYHOUR=9;BYDAY=WE;BYMONTHDAY=15;BYYEARDAY=100;BYWEEKNO=20;\
             BYMONTH=3;BYSETPOS=2;WKST=SU"
        )
        .as_deref(),
        Some(
            "FREQ=YEARLY;BYHOUR=9;BYDAY=WE;BYMONTHDAY=15;BYYEARDAY=100;BYWEEKNO=20;\
             BYMONTH=3;BYSETPOS=2;WKST=SU"
        ),
    );
    // The hours themselves survive verbatim, in the order given and at every
    // frequency — §3.3.10 defines the part beside all of them — so the mapping
    // neither sorts them nor gates them on the frequency.
    for rrule in [
        "FREQ=DAILY;BYHOUR=0",
        "FREQ=DAILY;BYHOUR=23,9,0",
        "FREQ=HOURLY;BYHOUR=9",
        "FREQ=MINUTELY;BYHOUR=9",
        "FREQ=YEARLY;BYHOUR=9",
    ] {
        assert_eq!(reparsed_rrule(rrule).as_deref(), Some(rrule), "{rrule}");
    }
    // And why `hour_token` refuses anything outside RFC 5545's `hour`: unlike the
    // days of the year and the positions in the set, where libical keeps an
    // out-of-range value verbatim, *every* hour it cannot use costs the *whole*
    // `RRULE` — the event reaches EDS's cache as a single appointment with the
    // user's series gone.
    for rrule in [
        "FREQ=DAILY;BYHOUR=24",
        "FREQ=DAILY;BYHOUR=99",
        "FREQ=DAILY;BYHOUR=-1",
        "FREQ=DAILY;BYHOUR=9,XX",
    ] {
        assert_eq!(reparsed_rrule(rrule).as_deref(), None, "{rrule}");
    }
    // The empty part is worse than any of those, and is why `by_hour_part`
    // refuses an empty set rather than writing `BYHOUR=`: libical reads it as
    // midnight, quietly moving the whole series to 00:00.
    assert_eq!(
        reparsed_rrule("FREQ=DAILY;BYHOUR=").as_deref(),
        Some("FREQ=DAILY;BYHOUR=0"),
    );
    // The spelling shape, as for the position in the set: the leading zero and
    // the leading plus come back canonical, which cannot bite a mapping whose
    // `byHour` holds numbers.
    for written in ["FREQ=DAILY;BYHOUR=09", "FREQ=DAILY;BYHOUR=+9"] {
        assert_eq!(
            reparsed_rrule(written).as_deref(),
            Some("FREQ=DAILY;BYHOUR=9"),
            "{written}"
        );
    }
}

/// And the gate that is not libical's to enforce: RFC 5545 §3.3.10 forbids
/// `BYHOUR` beside a `DTSTART` of value type DATE, since an hour of the day means
/// nothing on a day with no clock. libical keeps such a component whole, so
/// `jmap-ical` is the only place the contradiction is resolved — by drawing an
/// all-day event whose rule names hours as a timed one instead.
#[test]
fn libical_keeps_the_hours_beside_an_all_day_start_that_forbids_them() {
    let vevent = concat!(
        "BEGIN:VEVENT\r\nUID:K1\r\nSUMMARY:S\r\nDTSTART;VALUE=DATE:20260810\r\n",
        "RRULE:FREQ=DAILY;BYHOUR=9\r\nEND:VEVENT\r\n",
    );
    let component = instance(vevent);
    let list = instance_list(&[component]);
    let saved = unsafe { marshal::icalendar_from_instances(list) }.expect("a master");
    unsafe {
        glib_sys::g_slist_free(list);
        g_object_unref(component.cast());
    }

    assert!(
        saved.icalendar.contains("RRULE:FREQ=DAILY;BYHOUR=9"),
        "the rule libical is expected to keep as written: {}",
        saved.icalendar
    );
    assert!(
        saved.icalendar.contains("DTSTART;VALUE=DATE:20260810"),
        "beside the DATE start §3.3.10 says it must not have: {}",
        saved.icalendar
    );
}

/// The same question for the day a rule's weeks start on, which decides the one
/// asymmetry in `jmap-ical`'s `first_day_of_week_part`: the default is *dropped*
/// on the way out rather than written, and this is the measurement that says it
/// has to be.
#[test]
fn libical_answers_for_the_day_a_week_starts_on() {
    // The reason the default is left off: libical drops `WKST=MO` from a rule it
    // reads, because RFC 5545 §3.3.10 makes Monday the default. A rule written
    // with it would come back out of EDS's cache without it — which the save path
    // would read as the user removing `firstDayOfWeek`.
    assert_eq!(
        reparsed_rrule("FREQ=WEEKLY;WKST=MO").as_deref(),
        Some("FREQ=WEEKLY"),
    );
    // Every other day survives verbatim, at every frequency — including the ones
    // where §3.3.10 calls the part insignificant. So there is no frequency gate in
    // the mapping: dropping the day where libical keeps it would be the mapping
    // inventing a narrowing of its own.
    for rrule in [
        "FREQ=WEEKLY;WKST=SU",
        "FREQ=WEEKLY;INTERVAL=2;BYDAY=TU;WKST=SU",
        "FREQ=DAILY;WKST=SU",
        "FREQ=MONTHLY;WKST=SA",
        "FREQ=YEARLY;WKST=SU",
    ] {
        assert_eq!(reparsed_rrule(rrule).as_deref(), Some(rrule), "{rrule}");
    }
    // A day outside RFC 5545's `weekday` costs the **whole** `RRULE`, as an
    // out-of-range day of the month does: the event would reach EDS's cache as a
    // single appointment with the user's series gone. That is what
    // `jmap-ical`'s refusal of an unknown `firstDayOfWeek` prevents.
    for rrule in ["FREQ=WEEKLY;WKST=XX", "FREQ=WEEKLY;WKST="] {
        assert_eq!(reparsed_rrule(rrule).as_deref(), None, "{rrule}");
    }
    // And the third shape, a spelling kept but *changed*: iCalendar's weekday is
    // upper case, so a lower-case one comes back respelled. The mapping writes
    // upper case, so this cannot bite it.
    assert_eq!(
        reparsed_rrule("FREQ=YEARLY;WKST=su").as_deref(),
        Some("FREQ=YEARLY;WKST=SU"),
    );
    // Written last, after `BYMONTH` — which is the order `jmap-ical` emits the
    // parts in, so that a rule that went out this way and came back through EDS's
    // own cache compares equal to itself.
    assert_eq!(
        reparsed_rrule("FREQ=WEEKLY;BYDAY=MO;WKST=SU;BYMONTH=3").as_deref(),
        Some("FREQ=WEEKLY;BYDAY=MO;BYMONTH=3;WKST=SU"),
    );
}

/// The `RRULE` value libical hands back after reading a component carrying
/// `value`, or `None` if it kept no `RRULE` at all.
///
/// The folds are undone first: RFC 5545 §3.1 splits a content line past 75
/// octets across several physical ones, which a rule naming six parts reaches,
/// and reading only the first fragment would compare half a rule against a whole
/// one.
fn reparsed_rrule(value: &str) -> Option<String> {
    let vevent = format!(
        "BEGIN:VEVENT\r\nUID:K1\r\nSUMMARY:S\r\nDTSTART:20260810T070000Z\r\n\
         RRULE:{value}\r\nEND:VEVENT\r\n"
    );
    let component = instance(&vevent);
    let list = instance_list(&[component]);
    // SAFETY: `list` holds one live component, freed below with the instance.
    let saved = unsafe { marshal::icalendar_from_instances(list) }.expect("a master");
    unsafe {
        glib_sys::g_slist_free(list);
        g_object_unref(component.cast());
    }
    let unfolded = saved
        .icalendar
        .replace("\r\n ", "")
        .replace("\r\n\t", "")
        .replace("\n ", "")
        .replace("\n\t", "");
    unfolded
        .lines()
        .find_map(|line| line.strip_prefix("RRULE:"))
        .map(str::to_owned)
}
