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
        // And the two parts that are not `BYxxx` at all: libical writes the end
        // of the series — `COUNT` or `UNTIL` — *before* `INTERVAL`, which is why
        // `jmap-ical` emits them in that order. It is the only ordering claim in
        // the mapping that concerns a part outside the `BYxxx` block.
        "FREQ=WEEKLY;COUNT=6;INTERVAL=2;BYDAY=MO",
        "FREQ=DAILY;UNTIL=20261231T000000Z;INTERVAL=2",
    ] {
        assert_eq!(reparsed_rrule(rrule).as_deref(), Some(rrule));
    }
    // The order the other way round, to show the claim is about libical's
    // preference and not about what it will accept: it takes the rule and hands it
    // back respelled, which is exactly what the save path must not be shown.
    assert_eq!(
        reparsed_rrule("FREQ=WEEKLY;INTERVAL=2;COUNT=6;BYDAY=MO").as_deref(),
        Some("FREQ=WEEKLY;COUNT=6;INTERVAL=2;BYDAY=MO"),
    );
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
    // And why `time_of_day_part` refuses anything outside RFC 5545's `hour`: unlike
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

/// The same question for the minutes and seconds a rule repeats at, the last two
/// `BYxxx` parts of RFC 8984 §4.3.3 this mapping models — and the two that go
/// ahead of even the hours.
#[test]
fn libical_answers_for_the_minutes_and_seconds() {
    // Where they go: `BYSECOND`, then `BYMINUTE`, then `BYHOUR`, then the dates —
    // libical's own order, and the one `jmap-ical`'s `named_by_parts` writes, so
    // a rule that goes out this way comes back out of EDS's cache spelled
    // identically instead of reordered and read as an edit.
    assert_eq!(
        reparsed_rrule("FREQ=WEEKLY;BYDAY=MO;BYMINUTE=30;BYSECOND=15;BYHOUR=9").as_deref(),
        Some("FREQ=WEEKLY;BYSECOND=15;BYMINUTE=30;BYHOUR=9;BYDAY=MO"),
    );
    let all = "FREQ=YEARLY;BYSECOND=0;BYMINUTE=30;BYHOUR=9;BYDAY=WE;BYMONTHDAY=15;\
               BYYEARDAY=100;BYWEEKNO=20;BYMONTH=3;BYSETPOS=2;WKST=SU";
    assert_eq!(reparsed_rrule(all).as_deref(), Some(all));
    // The values survive verbatim, in the order given and at every frequency, so
    // the mapping neither sorts nor gates them — and the sixtieth second is a
    // *legal* value, RFC 5545 §3.3.10's `seconds` running 0 to 60 to admit the
    // leap second, which is the one place the two ranges differ.
    for rrule in [
        "FREQ=HOURLY;BYMINUTE=0,30",
        "FREQ=MINUTELY;BYSECOND=30,0",
        "FREQ=DAILY;BYMINUTE=59",
        "FREQ=DAILY;BYSECOND=60",
        // And both are sets `BYSETPOS` may select out of, as the hours are.
        "FREQ=DAILY;BYMINUTE=30;BYSETPOS=-1",
        "FREQ=DAILY;BYSECOND=30;BYSETPOS=-1",
    ] {
        assert_eq!(reparsed_rrule(rrule).as_deref(), Some(rrule), "{rrule}");
    }
    // And why `time_of_day_part` refuses a value out of range: as with the hours,
    // *every* one libical cannot use costs the **whole** `RRULE` — the event
    // reaches EDS's cache as a single appointment with the user's series gone.
    for rrule in [
        "FREQ=DAILY;BYMINUTE=60",
        "FREQ=DAILY;BYSECOND=61",
        "FREQ=DAILY;BYMINUTE=-1",
        "FREQ=DAILY;BYSECOND=9,XX",
    ] {
        assert_eq!(reparsed_rrule(rrule).as_deref(), None, "{rrule}");
    }
    // The empty part is worse again, and for the reason it is worse for the
    // hours: libical answers it with the zeroth minute or second rather than
    // dropping it, quietly moving every occurrence of the series.
    for (written, back) in [
        ("FREQ=DAILY;BYMINUTE=", "FREQ=DAILY;BYMINUTE=0"),
        ("FREQ=DAILY;BYSECOND=", "FREQ=DAILY;BYSECOND=0"),
        // The spelling shape, as for the hours: the leading zero and the leading
        // plus come back canonical, which cannot bite a mapping holding numbers.
        ("FREQ=DAILY;BYMINUTE=09", "FREQ=DAILY;BYMINUTE=9"),
        ("FREQ=DAILY;BYSECOND=+9", "FREQ=DAILY;BYSECOND=9"),
    ] {
        assert_eq!(reparsed_rrule(written).as_deref(), Some(back), "{written}");
    }
}

/// And the gate that is not libical's to enforce: RFC 5545 §3.3.10 forbids
/// `BYHOUR`, `BYMINUTE` and `BYSECOND` beside a `DTSTART` of value type DATE,
/// since a time of day means nothing on a day with no clock. libical keeps such a
/// component whole, so `jmap-ical` is the only place the contradiction is
/// resolved — by drawing an all-day event whose rule names a time as a timed one
/// instead.
#[test]
fn libical_keeps_a_time_of_day_beside_an_all_day_start_that_forbids_it() {
    let vevent = concat!(
        "BEGIN:VEVENT\r\nUID:K1\r\nSUMMARY:S\r\nDTSTART;VALUE=DATE:20260810\r\n",
        "RRULE:FREQ=DAILY;BYSECOND=0;BYMINUTE=30;BYHOUR=9\r\nEND:VEVENT\r\n",
    );
    let component = instance(vevent);
    let list = instance_list(&[component]);
    let saved = unsafe { marshal::icalendar_from_instances(list) }.expect("a master");
    unsafe {
        glib_sys::g_slist_free(list);
        g_object_unref(component.cast());
    }

    assert!(
        saved
            .icalendar
            .contains("RRULE:FREQ=DAILY;BYSECOND=0;BYMINUTE=30;BYHOUR=9"),
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

/// The place an event happens at, through the parser that actually reads it.
///
/// `jmap-ical` writes the key of the `locations` entry a `LOCATION` came from as
/// an `X-JMAP-KEY` parameter, so that a save patches the server's own entry
/// instead of replacing the property. Nothing in `jmap-ical` can check that
/// libical keeps a parameter it has never heard of — calcard is what parses
/// there — and if libical dropped it, the key would be gone by the time EDS
/// handed the component back. It does keep it, verbatim, escaping and all.
///
/// This is a measurement, not a requirement: the save path deliberately takes
/// the key from the event the *server* holds, because Evolution's appointment
/// editor writes the `LOCATION` afresh and need not carry the parameter through
/// an edit. What would break without libical's cooperation is only the case of a
/// component saved back untouched.
#[test]
fn libical_keeps_the_location_key_this_mapping_writes() {
    for line in [
        "LOCATION;X-JMAP-KEY=srv1:Room 42",
        // TEXT escaping (RFC 5545 §3.3.11) beside the parameter, since a room
        // name holding a comma is what the escaping is there for.
        "LOCATION;X-JMAP-KEY=srv1:Berlin\\, Room 42\\; 3rd floor",
    ] {
        assert_eq!(reparsed_lines(line), vec![line.to_owned()]);
    }
    // And what libical does *not* do: enforce RFC 5545 §3.6.1's one `LOCATION`
    // per `VEVENT`. A component may therefore arrive naming two places, which
    // `jmap-ical` reads as the first — the same narrowing as an event whose
    // `locations` map holds two, and the same refusal to write it back.
    assert_eq!(
        reparsed_lines("LOCATION:Room 42\r\nLOCATION:Cafeteria"),
        vec![
            "LOCATION:Room 42".to_owned(),
            "LOCATION:Cafeteria".to_owned()
        ]
    );
}

/// The tags an event carries, through the parser that actually reads them.
///
/// `jmap-ical` writes the whole `keywords` set as **one** `CATEGORIES` line of
/// `,`-separated TEXT values, and the save path replaces the property from what
/// comes back — so a tag libical dropped or re-spelled would be a tag the next
/// save deletes.
///
/// It does neither, but it does **not hand the line back as it was written**: it
/// splits every value onto a `CATEGORIES` line of its own, escaping intact. That
/// is why `jmap_ical::ical_to_event` reads *every* occurrence of the property
/// rather than the first — a component EDS hands back holds one line per tag, not
/// the line we wrote, so reading the first alone would see a set of one and the
/// save would delete the rest. This test is that case being reachable rather than
/// hypothetical, measured rather than assumed.
#[test]
fn libical_splits_the_categories_this_mapping_writes_and_keeps_every_tag() {
    assert_eq!(
        reparsed("CATEGORIES", "CATEGORIES:offsite,planning"),
        vec![
            "CATEGORIES:offsite".to_owned(),
            "CATEGORIES:planning".to_owned()
        ]
    );
    // TEXT escaping (RFC 5545 §3.3.11) inside one value, which is what keeps a
    // tag holding a comma from becoming two tags — including across the split.
    assert_eq!(
        reparsed(
            "CATEGORIES",
            "CATEGORIES:Berlin\\, offsite\\; 2026,planning"
        ),
        vec![
            "CATEGORIES:Berlin\\, offsite\\; 2026".to_owned(),
            "CATEGORIES:planning".to_owned()
        ]
    );

    // And the consequence, which is the part that matters: the set the mapping
    // wrote comes back whole, so a save diffs it against a baseline that agrees
    // and sends nothing.
    let object = reparsed_object("CATEGORIES:Berlin\\, offsite\\; 2026,planning");
    let read_back = jmap_ical::ical_to_event(&object).expect("parse");
    assert_eq!(
        read_back
            .keywords
            .as_ref()
            .map(|tags| tags.keys().cloned().collect::<Vec<_>>()),
        Some(vec![
            "Berlin, offsite; 2026".to_owned(),
            "planning".to_owned()
        ]),
        "a tag was lost between what this crate wrote and what libical handed back:\n{object}"
    );
}

/// The same property, on the component that stands for one occurrence.
///
/// An override may restate the tags (`jmap_ical::OVERRIDE_PROPERTIES`), and the
/// only place the set for one occurrence is stated is that instance's own
/// `CATEGORIES` — so the claim rests entirely on libical keeping the two
/// components' sets apart through the marshalling. It does: each keeps its own,
/// split per value as above, and neither acquires the other's. Which is what
/// makes the difference between them the user's edit rather than an artefact of
/// the trip through EDS's cache.
#[test]
fn libical_keeps_the_tags_of_one_occurrence_apart_from_the_series() {
    let master = instance(
        "BEGIN:VEVENT\r\nUID:K1\r\nSUMMARY:Standup\r\nDTSTART:20260810T070000Z\r\n\
         RRULE:FREQ=DAILY\r\nCATEGORIES:offsite\r\nEND:VEVENT\r\n",
    );
    let occurrence = instance(
        "BEGIN:VEVENT\r\nUID:K1\r\nRECURRENCE-ID:20260812T070000Z\r\n\
         DTSTART:20260812T070000Z\r\nSUMMARY:Standup\r\n\
         CATEGORIES:cancelled,offsite\r\nEND:VEVENT\r\n",
    );
    let list = instance_list(&[master, occurrence]);
    // SAFETY: `list` holds two live components, freed below.
    let saved = unsafe { marshal::icalendar_from_instances(list) }.expect("a master");
    unsafe {
        glib_sys::g_slist_free(list);
        for component in [master, occurrence] {
            g_object_unref(component.cast());
        }
    }
    let object = saved.icalendar;

    let event = jmap_ical::ical_to_event(&object).expect("parse");
    assert_eq!(
        event
            .keywords
            .map(|tags| tags.keys().cloned().collect::<Vec<_>>()),
        Some(vec!["offsite".to_owned()]),
        "the series' own set is not what it was written as:\n{object}"
    );

    let overrides = event.recurrence_overrides.unwrap_or_default();
    assert_eq!(overrides.len(), 1, "one occurrence differs:\n{object}");
    let patch = overrides
        .get("2026-08-12T07:00:00")
        .unwrap_or_else(|| panic!("the occurrence is not the one that was written:\n{object}"));
    let tags = patch
        .get("keywords")
        .and_then(|tags| tags.as_object())
        .unwrap_or_else(|| panic!("the occurrence's own tags did not come back:\n{object}"));
    assert_eq!(
        tags.keys().cloned().collect::<Vec<_>>(),
        vec!["cancelled".to_owned(), "offsite".to_owned()],
        "a tag was lost between the instance and libical's answer:\n{object}"
    );
    assert_eq!(
        patch.as_object().map(|patch| patch.len()),
        Some(1),
        "the marshalling introduced a difference nobody wrote:\n{object}"
    );
}

/// The reminders of one occurrence, on the component that stands for it.
///
/// The same claim as the tags above, one nesting level deeper: an override may
/// restate `alerts` (`jmap_ical::OVERRIDE_PROPERTIES`) and the only place the
/// reminders of one occurrence are stated is that instance's own `VALARM`s — which
/// are a child component *of a child component* once the two `VEVENT`s are
/// marshalled into one `VCALENDAR`. So the claim rests on libical keeping each
/// instance's alarms with the instance, rather than hoisting them onto the master
/// or merging the two sets. It does, keys and signs intact — which is what makes
/// the difference between the two the user's edit rather than an artefact of the
/// trip through EDS's cache.
#[test]
fn libical_keeps_the_reminder_of_one_occurrence_apart_from_the_series() {
    let master = instance(
        "BEGIN:VEVENT\r\nUID:K1\r\nSUMMARY:Standup\r\nDTSTART:20260810T070000Z\r\n\
         RRULE:FREQ=DAILY\r\nBEGIN:VALARM\r\nUID:k1\r\nACTION:DISPLAY\r\n\
         DESCRIPTION:Standup\r\nTRIGGER:-PT15M\r\nEND:VALARM\r\nEND:VEVENT\r\n",
    );
    let occurrence = instance(
        "BEGIN:VEVENT\r\nUID:K1\r\nRECURRENCE-ID:20260812T070000Z\r\n\
         DTSTART:20260812T070000Z\r\nSUMMARY:Standup\r\nBEGIN:VALARM\r\nUID:k1\r\n\
         ACTION:DISPLAY\r\nDESCRIPTION:Standup\r\nTRIGGER:-PT1H\r\nEND:VALARM\r\n\
         END:VEVENT\r\n",
    );
    let list = instance_list(&[master, occurrence]);
    // SAFETY: `list` holds two live components, freed below.
    let saved = unsafe { marshal::icalendar_from_instances(list) }.expect("a master");
    unsafe {
        glib_sys::g_slist_free(list);
        for component in [master, occurrence] {
            g_object_unref(component.cast());
        }
    }
    let object = saved.icalendar;

    let event = jmap_ical::ical_to_event(&object).expect("parse");
    let series = event
        .alerts
        .unwrap_or_else(|| panic!("the series' reminder did not come back:\n{object}"));
    assert_eq!(
        series.keys().collect::<Vec<_>>(),
        ["k1"],
        "the key the series' entry rides on was not kept:\n{object}"
    );
    assert_eq!(
        series["k1"]["trigger"]["offset"].as_str(),
        Some("-PT15M"),
        "the series' own reminder is not what it was written as:\n{object}"
    );

    let overrides = event.recurrence_overrides.unwrap_or_default();
    assert_eq!(overrides.len(), 1, "one occurrence differs:\n{object}");
    let patch = overrides
        .get("2026-08-12T07:00:00")
        .unwrap_or_else(|| panic!("the occurrence is not the one that was written:\n{object}"));
    let alerts = patch
        .get("alerts")
        .unwrap_or_else(|| panic!("the occurrence's own reminder did not come back:\n{object}"));
    assert_eq!(
        alerts["k1"]["trigger"]["offset"].as_str(),
        Some("-PT1H"),
        "the reminder on the occurrence is not the one written there:\n{object}"
    );
    assert_eq!(
        patch.as_object().map(|patch| patch.len()),
        Some(1),
        "the marshalling introduced a difference nobody wrote:\n{object}"
    );
}

/// Whether an event blocks time, through the parser that actually reads it.
///
/// `jmap-ical` reads a missing `TRANSP` as *nothing said* rather than as RFC 5545
/// §3.8.2.7's OPAQUE default, so that a save can tell an event the server never
/// gave a transparency from one the user just set back to busy. That reading is
/// only sound if the component EDS hands back does not acquire a line the
/// mapping never wrote — libical filling the default in would turn every save of
/// such an event into a patch stating `busy`.
///
/// It does not: a `VEVENT` with no `TRANSP` comes back with none, and one that
/// has it comes back with the same value, unrespelled. What this cannot say is
/// what Evolution's *appointment editor* writes — it may well set the property
/// explicitly from its "Show Time as" combo, and a patch stating the default
/// would then be sent once. That is a redundant write, not a wrong one: RFC 8984
/// §4.4.2 makes `busy` and no value the same state.
#[test]
fn libical_neither_invents_a_transparency_nor_respells_one() {
    assert_eq!(
        reparsed("TRANSP", "DESCRIPTION:the quarter"),
        Vec::<String>::new(),
        "libical filled in the OPAQUE default"
    );
    for line in ["TRANSP:TRANSPARENT", "TRANSP:OPAQUE"] {
        assert_eq!(reparsed("TRANSP", line), vec![line.to_owned()]);
        let object = reparsed_object(line);
        let read_back = jmap_ical::ical_to_event(&object).expect("parse");
        assert_eq!(
            read_back.free_busy_status.as_deref(),
            match line {
                "TRANSP:TRANSPARENT" => Some("free"),
                _ => Some("busy"),
            },
            "{object}"
        );
    }
}

/// How important an event is, through the parser that actually reads it.
///
/// Two readings rest on this. `jmap-ical` reads a missing `PRIORITY` as *nothing
/// said* rather than as RFC 5545 §3.8.1.9's undefined priority, so a save can tell
/// an event the server never gave an importance from one the user just made
/// unimportant — which is only sound if the component EDS hands back does not
/// acquire a line the mapping never wrote.
///
/// And `PRIORITY:0` is written out rather than left off, even though RFC 5545
/// §3.8.1.9 and RFC 8984 §4.4.1 both make 0 and no value the same state. That is
/// the reading with something to lose: were libical to drop the property as
/// meaning nothing, an event the server states as 0 would come back with no line,
/// read as `None`, and every save of it would carry a redundant `"priority": null`
/// — the same state, so not wrong, but a write nobody asked for.
///
/// It does neither: no line stays no line, and every value 0 to 9 comes back as
/// itself. What this cannot say is what Evolution's *appointment editor* writes —
/// but for once that question has an answer that does not need a display:
/// `e_comp_editor_property_part_priority_new` is called only from
/// `e_comp_editor_task` and `e_bulk_edit_tasks` in Evolution 3.52's
/// `libevolution-calendar.so`, never from `e_comp_editor_event`. The event editor
/// has no priority control at all, so it has nothing to write and leaves the line
/// as it found it — which is what makes this probe the whole story for events.
#[test]
fn libical_neither_invents_a_priority_nor_respells_one() {
    assert_eq!(
        reparsed("PRIORITY", "DESCRIPTION:the quarter"),
        Vec::<String>::new(),
        "libical filled in an undefined priority"
    );
    for priority in 0..=9 {
        let line = format!("PRIORITY:{priority}");
        assert_eq!(reparsed("PRIORITY", &line), vec![line.clone()]);
        let object = reparsed_object(&line);
        let read_back = jmap_ical::ical_to_event(&object).expect("parse");
        assert_eq!(read_back.priority, Some(priority), "{object}");
    }
}

/// How much of an event may be shared, through the parser that actually reads it.
///
/// `CLASS` is the one property so far whose iCalendar value libical does *not*
/// hold as text: `i_cal_property_new_class` takes an `ICalPropertyClass` enum, so
/// the parser has to recognise a value in order to keep it. That makes both
/// readings this mapping rests on worth measuring rather than assuming.
///
/// The first: `jmap-ical` reads a missing `CLASS` as *nothing said* rather than as
/// RFC 5545 §3.8.1.3's PUBLIC default, so a save can tell an event the server
/// never classified from one the user just made public — which is only sound if
/// the component EDS hands back does not acquire a line the mapping never wrote.
///
/// The second, and the one with something to lose: `CLASS:PUBLIC` is written out
/// rather than left off, even though RFC 5545 §3.8.1.3 and RFC 8984 §4.4.3 both
/// make public and no value the same state. Were libical to drop the property as
/// meaning nothing, an event the server states as public would come back with no
/// line and every save of it would carry a redundant `"privacy": "public"`.
///
/// It does neither: no line stays no line, and all three values come back as
/// themselves. And unlike `TRANSP` and `PRIORITY`, what Evolution's *appointment
/// editor* does here is not left open — it is measurable from the installed
/// binary. Evolution 3.52's `libevolution-calendar.so` has no classification
/// property *part* on the event editor (`e_comp_editor_property_part_classification_new`
/// is called only from the task, memo and bulk-task editors); the appointment
/// editor exposes the classification as an Options ▸ Classification radio menu
/// (`classify-public` / `classify-private` / `classify-confidential`) and its
/// `fill_component` reads that menu and calls `i_cal_property_set_class` — or
/// `i_cal_property_new_class` when the component has no `CLASS` yet —
/// **unconditionally**. So the editor states the classification on every save,
/// defaulting to public, which is exactly why writing `CLASS:PUBLIC` out is not
/// optional: a baseline rendered without it would differ from what EDS hands back
/// on every save of a public event rather than once. See
/// `jmap-cal-sync/tests/save.rs`'s `saving_a_public_event_back_unchanged_sends_no_patch`.
#[test]
fn libical_neither_invents_a_classification_nor_respells_one() {
    assert_eq!(
        reparsed("CLASS", "DESCRIPTION:the quarter"),
        Vec::<String>::new(),
        "libical filled in the PUBLIC default"
    );
    for (line, privacy) in [
        ("CLASS:PUBLIC", "public"),
        ("CLASS:PRIVATE", "private"),
        ("CLASS:CONFIDENTIAL", "secret"),
    ] {
        assert_eq!(reparsed("CLASS", line), vec![line.to_owned()]);
        let object = reparsed_object(line);
        let read_back = jmap_ical::ical_to_event(&object).expect("parse");
        assert_eq!(read_back.privacy.as_deref(), Some(privacy), "{object}");
    }
}

/// When an event was made and last changed, through the parser that actually
/// reads them.
///
/// `jmap-ical` draws RFC 8984's `created` and `updated` as `CREATED`, `DTSTAMP`
/// and `LAST-MODIFIED` and never reads any of the three back, so no value the
/// save path uses rests on libical here. What does is one claim `jmap-ical`
/// cannot check for itself: that a component written **without** a `DTSTAMP` —
/// which is what an event whose server states no `updated` gets, RFC 5545 §3.6.1
/// requiring the property notwithstanding — is read rather than refused.
///
/// It is, and libical closes the gap itself: it stamps a `DTSTAMP` from the
/// clock, so what EDS hands a save always carries one. That is the measurement
/// worth having, because it says exactly what such a line *is* — libical's
/// moment, not the server's — and it is why reading `updated` back off a
/// component would be reading the local clock. It invents neither of the other
/// two, and it re-renders all three verbatim when they are written.
#[test]
fn libical_stamps_a_dtstamp_of_its_own_and_keeps_the_timestamps_written() {
    let invented = reparsed("DTSTAMP", "DESCRIPTION:the quarter");
    let [stamp] = invented.as_slice() else {
        panic!("expected exactly one invented DTSTAMP, got {invented:?}");
    };
    assert!(
        stamp.starts_with("DTSTAMP:")
            && stamp.ends_with('Z')
            && stamp.len() == "DTSTAMP:".len() + 16,
        "{stamp} is not a UTC date-time"
    );
    for property in ["CREATED", "LAST-MODIFIED"] {
        assert_eq!(
            reparsed(property, "DESCRIPTION:the quarter"),
            Vec::<String>::new(),
            "libical filled in a {property} of its own"
        );
    }
    // And what it does with the ones this mapping writes: hands them back
    // verbatim, UTC designator included — the invented stamp is a default, not
    // an overwrite.
    for line in [
        "CREATED:20260102T093000Z",
        "DTSTAMP:20260115T174501Z",
        "LAST-MODIFIED:20260115T174501Z",
    ] {
        let (property, _) = line.split_once(':').expect("a content line");
        assert_eq!(reparsed(property, line), vec![line.to_owned()]);
    }
}

/// The guest list, through the parser that hands it to Evolution.
///
/// `jmap-ical` draws RFC 8984 §4.4.6's `participants` as `ATTENDEE` lines and
/// the owner among them as an `ORGANIZER`, and never reads any of them back, so
/// no value the save path uses rests on libical here either. What does rest on
/// it is what the user is *shown*: these lines exist to be read, and a parameter
/// libical dropped or a value it mangled would be a guest list Evolution shows
/// wrongly — an attendee with no name, or one whose reply went missing.
///
/// It keeps both lines with every parameter this mapping writes, in the order
/// they were written.
#[test]
fn libical_keeps_the_guest_list_it_was_handed() {
    for line in [
        "ATTENDEE;CN=Bob Example;ROLE=REQ-PARTICIPANT;PARTSTAT=ACCEPTED:\
         mailto:bob@example.com",
        "ATTENDEE;CN=Room 1;CUTYPE=ROOM;PARTSTAT=NEEDS-ACTION;RSVP=TRUE:\
         mailto:room-1@example.com",
        "ORGANIZER;CN=Alice Example:mailto:alice@example.com",
    ] {
        let (property, _) = line.split_once(';').expect("a parameterised line");
        assert_eq!(reparsed(property, line), vec![line.to_owned()]);
    }
    // And it invents neither: an event the server gave no participants for is
    // shown as one nobody was invited to, which is what it is.
    for property in ["ATTENDEE", "ORGANIZER"] {
        assert_eq!(
            reparsed(property, "DESCRIPTION:the quarter"),
            Vec::<String>::new(),
            "libical filled in an {property} of its own"
        );
    }
}

/// The `LOCATION` lines of a `VEVENT` after libical has parsed and re-rendered
/// it, unfolded.
fn reparsed_lines(lines: &str) -> Vec<String> {
    reparsed("LOCATION", lines)
}

/// The lines of one property of a `VEVENT` after libical has parsed and
/// re-rendered it, unfolded.
fn reparsed(property: &str, lines: &str) -> Vec<String> {
    reparsed_object(lines)
        .replace("\r\n ", "")
        .replace("\r\n\t", "")
        .replace("\n ", "")
        .replace("\n\t", "")
        .lines()
        .filter(|line| line.starts_with(property))
        .map(str::to_owned)
        .collect()
}

/// A `VEVENT` carrying `lines`, through libical and back out as the whole
/// `VCALENDAR` object a save is handed.
fn reparsed_object(lines: &str) -> String {
    let vevent = format!(
        "BEGIN:VEVENT\r\nUID:K1\r\nSUMMARY:S\r\nDTSTART:20260810T070000Z\r\n\
         {lines}\r\nEND:VEVENT\r\n"
    );
    let component = instance(&vevent);
    let list = instance_list(&[component]);
    // SAFETY: `list` holds one live component, freed below with the instance.
    let saved = unsafe { marshal::icalendar_from_instances(list) }.expect("a master");
    unsafe {
        glib_sys::g_slist_free(list);
        g_object_unref(component.cast());
    }
    saved.icalendar
}

/// The reminders an event carries, through the parser that actually reads them.
///
/// This is the first mapped property that is a **child component**, so more of it
/// rests on libical than a content line does: `jmap-ical` writes a `VALARM` per
/// alert, and the key of the `alerts` entry rides on the RFC 9074 §6 `UID` inside
/// it — a property RFC 5545 never gave a `VALARM`, and one nothing in `jmap-ical`
/// can check the survival of, since calcard is what parses there. If libical
/// dropped the alarm, the reminder would be gone by the time EDS handed the
/// component back and the next save would delete it server-side; if it dropped
/// just the `UID`, every save would re-key the property.
///
/// It keeps all of it: the alarm, its `UID`, the `RELATED=END` parameter and the
/// **sign** on the trigger — a reminder a quarter of an hour *before* the event
/// coming back as one a quarter of an hour after it is the failure this watches
/// for. What libical adds is an `X-EVOLUTION-ALARM-UID` of its own, which is what
/// Evolution keys alarms on and which this mapping ignores.
#[test]
fn libical_keeps_the_reminder_this_mapping_writes() {
    let object = reparsed_object(
        "BEGIN:VALARM\r\nUID:k1\r\nACTION:DISPLAY\r\nDESCRIPTION:S\r\n\
         TRIGGER;RELATED=END:-PT15M\r\nEND:VALARM",
    );

    let event = jmap_ical::ical_to_event(&object).expect("parse");
    let alerts = event
        .alerts
        .unwrap_or_else(|| panic!("the reminder did not come back at all:\n{object}"));
    assert_eq!(
        alerts.keys().collect::<Vec<_>>(),
        ["k1"],
        "the key the entry rides on was not kept:\n{object}"
    );
    let alert = &alerts["k1"];
    assert_eq!(
        alert.get("action").and_then(|action| action.as_str()),
        Some("display"),
        "{object}"
    );
    let trigger = alert
        .get("trigger")
        .unwrap_or_else(|| panic!("no trigger:\n{object}"));
    assert_eq!(
        trigger.get("offset").and_then(|offset| offset.as_str()),
        Some("-PT15M"),
        "the reminder moved to the other side of the event:\n{object}"
    );
    assert_eq!(
        trigger
            .get("relativeTo")
            .and_then(|relative_to| relative_to.as_str()),
        Some("end"),
        "{object}"
    );
}
