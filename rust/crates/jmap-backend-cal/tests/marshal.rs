// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Tests for calendar component marshalling and conversion across C boundaries.

use std::ffi::{CStr, CString};
use std::ptr;
use std::sync::atomic::{AtomicU32, Ordering};

use eds_sys::{
    E_SOURCE_EXTENSION_CALENDAR, ECalComponent, ECalMetaBackendInfo, ESourceSelectable,
    ETimezoneCache, I_CAL_VCALENDAR_COMPONENT, e_cal_cache_new, e_cal_component_new_from_string,
    e_cal_meta_backend_info_free, e_source_get_extension, e_source_selectable_set_color,
    e_timezone_cache_add_timezone, i_cal_component_get_timezone, i_cal_component_isa,
    i_cal_component_new_from_string, i_cal_timezone_get_component,
};
use glib_sys::{GError, GSList, g_slist_length, g_slist_nth_data};
use gobject_sys::g_object_unref;
use jmap_backend_cal::marshal;
use jmap_backend_core::source::OwnedSource;
use jmap_cal_sync::ComponentInfo;

/// Event fixture in a `VCALENDAR` envelope.
const OBJECT: &str = "BEGIN:VCALENDAR\r\n\
                      VERSION:2.0\r\n\
                      PRODID:-//evolution-jmap//JMAP calendar backend//EN\r\n\
                      BEGIN:VEVENT\r\n\
                      UID:K1\r\n\
                      SUMMARY:Standup\r\n\
                      DTSTART;TZID=Europe/Zurich:20260810T090000\r\n\
                      END:VEVENT\r\n\
                      END:VCALENDAR\r\n";

/// Event fixture with a UTC instant and no timezone definition.
///
/// Tests use this fixture when timezone injection is not under test.
const UNZONED_OBJECT: &str = "BEGIN:VCALENDAR\r\n\
                              VERSION:2.0\r\n\
                              PRODID:-//evolution-jmap//JMAP calendar backend//EN\r\n\
                              BEGIN:VEVENT\r\n\
                              UID:K1\r\n\
                              SUMMARY:Standup\r\n\
                              DTSTART:20260810T070000Z\r\n\
                              END:VEVENT\r\n\
                              END:VCALENDAR\r\n";

fn info(uid: &str, revision: &str, icalendar: &str) -> ComponentInfo {
    ComponentInfo {
        uid: uid.to_owned(),
        revision: revision.to_owned(),
        icalendar: icalendar.to_owned(),
    }
}

/// Read a `GSList` node as an `ECalMetaBackendInfo`.
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

/// Wrap an event fixture in an `ECalComponent`.
fn instance(vevent: &str) -> *mut ECalComponent {
    let text = std::ffi::CString::new(vevent).unwrap();
    // SAFETY: the text is NUL-terminated and valid for the call.
    let component = unsafe { e_cal_component_new_from_string(text.as_ptr()) };
    assert!(!component.is_null(), "the instance did not parse: {vevent}");
    component
}

/// Build a `GSList` of component instances in the given order.
///
/// Caller retains ownership of the components.
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
    let infos = [
        info("K1", "r1", UNZONED_OBJECT),
        info("K2", "r2", UNZONED_OBJECT),
    ];
    let list = marshal::info_list(&infos);

    unsafe {
        assert_eq!(g_slist_length(list), 2);
        assert_eq!(
            nth_info(list, 0),
            (
                "K1".to_owned(),
                Some("r1".to_owned()),
                Some(UNZONED_OBJECT.to_owned()),
                // EDS stores the extra as the base of a later edit.
                Some(UNZONED_OBJECT.to_owned())
            )
        );
        assert_eq!(nth_info(list, 1).0, "K2");
        glib_sys::g_slist_free_full(list, Some(e_cal_meta_backend_info_free));
    }
}

/// EDS represents an empty object list as a NULL list.
#[test]
fn an_empty_list_is_null() {
    assert!(marshal::info_list(&[]).is_null());
    assert!(marshal::removed_info_list(&[]).is_null());
}

/// Calendar deletions pass `ECalMetaBackendInfo` records with only the UID set.
///
/// Revision and object fields are null for deletions.
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

/// Verify that UID resolution accepts bare `VEVENT` and `VCALENDAR` envelopes.
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

/// Verify that an envelope without components fails to parse.
#[test]
fn an_envelope_with_no_event_in_it_is_refused() {
    assert!(
        marshal::component_from_ical("BEGIN:VCALENDAR\r\nVERSION:2.0\r\nEND:VCALENDAR\r\n")
            .is_null()
    );
}

/// Verify that missing or empty UIDs reject as invalid identifiers.
///
/// The save path uses UID presence to distinguish creates from updates.
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

/// Verify that an explicitly empty UID rejects as missing.
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

/// Verify that the master instance without `RECURRENCE-ID` leads the envelope.
///
/// Detached occurrences follow the master to preserve recurrence overrides.
#[test]
fn the_master_leads_the_envelope_and_the_overrides_follow_it() {
    let components = [instance(OVERRIDE), instance(MASTER)];
    let list = instance_list(&components);

    unsafe {
        let saved = marshal::icalendar_from_instances(list, ptr::null_mut()).expect("a master");
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
        // One copy of each instance.
        assert_eq!(saved.icalendar.matches("BEGIN:VEVENT").count(), 2);

        glib_sys::g_slist_free(list);
        for component in components {
            g_object_unref(component.cast());
        }
    }
}

/// Verify that marshalling clones instances instead of taking ownership.
///
/// EDS owns the instances; transferring ownership causes double free errors.
#[test]
fn the_instances_survive_being_marshalled() {
    let master = instance(MASTER);
    let list = instance_list(&[master]);

    unsafe {
        let first = marshal::icalendar_from_instances(list, ptr::null_mut()).expect("a master");
        let again =
            marshal::icalendar_from_instances(list, ptr::null_mut()).expect("still a master");
        assert_eq!(first.icalendar, again.icalendar);

        glib_sys::g_slist_free(list);
        g_object_unref(master.cast());
    }
}

/// Verify that saving a detached occurrence without a master series fails.
#[test]
fn a_save_of_nothing_but_an_override_is_refused() {
    let component = instance(OVERRIDE);
    let list = instance_list(&[component]);

    unsafe {
        assert!(marshal::icalendar_from_instances(list, ptr::null_mut()).is_none());
        glib_sys::g_slist_free(list);
        g_object_unref(component.cast());
    }
}

#[test]
fn a_save_with_no_instances_at_all_is_refused() {
    unsafe {
        assert!(marshal::icalendar_from_instances(ptr::null(), ptr::null_mut()).is_none());
    }
}

/// Return libical builtin timezone identifier.
const LIBICAL_TZID: &str = "/freeassociation.sourceforge.net/Europe/Berlin";

/// Create an event instance in `tzid` at a wall-clock time.
fn zoned(tzid: &str) -> String {
    format!(
        "BEGIN:VEVENT\r\nUID:K1\r\nSUMMARY:Standup\r\n\
         DTSTART;TZID={tzid}:20260810T090000\r\nEND:VEVENT\r\n"
    )
}

/// Count `VTIMEZONE` definitions in an envelope.
fn definitions(icalendar: &str) -> usize {
    icalendar.matches("BEGIN:VTIMEZONE").count()
}

/// Verify that a zoned instance injects its `VTIMEZONE` definition into the envelope.
#[test]
fn a_zoned_instance_brings_the_definition_of_its_zone() {
    let component = instance(&zoned(LIBICAL_TZID));
    let list = instance_list(&[component]);

    unsafe {
        let saved = marshal::icalendar_from_instances(list, ptr::null_mut()).expect("a master");
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

/// Verify that recurrence UNTIL times map correctly in the event timezone.
#[test]
fn a_recurrence_ending_at_a_utc_instant_converts_through_the_definition_libical_holds() {
    let component = instance(&format!(
        "BEGIN:VEVENT\r\nUID:K1\r\nSUMMARY:Standup\r\n\
         DTSTART;TZID={LIBICAL_TZID}:20260810T090000\r\n\
         RRULE:FREQ=WEEKLY;UNTIL=20260901T070000Z\r\n\
         END:VEVENT\r\n"
    ));
    let list = instance_list(&[component]);

    unsafe {
        let saved = marshal::icalendar_from_instances(list, ptr::null_mut()).expect("a master");
        let rules = jmap_ical::ical_to_event(&saved.icalendar)
            .expect("the envelope is a calendar object")
            .recurrence_rule
            .expect("the rule survived the mapping");

        // 07:00 UTC corresponds to 09:00 local time in Berlin.
        assert_eq!(
            rules.until.as_deref(),
            Some("2026-09-01T09:00:00"),
            "the end of the series did not convert: {}",
            saved.icalendar
        );
        assert!(
            jmap_ical::maps_recurrence_rule(&rules),
            "the rule cannot be sent, so a create carrying it is refused: {}",
            saved.icalendar
        );

        glib_sys::g_slist_free(list);
        g_object_unref(component.cast());
    }
}

/// Verify that IANA timezone names retain their identifiers and add definitions.
#[test]
fn a_zone_named_plainly_is_defined_under_the_name_the_event_uses() {
    let component = instance(&zoned("Europe/Zurich"));
    let list = instance_list(&[component]);

    unsafe {
        let saved = marshal::icalendar_from_instances(list, ptr::null_mut()).expect("a master");
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

/// Verify that multiple zones across master and overrides define each zone once.
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
        let saved = marshal::icalendar_from_instances(list, ptr::null_mut()).expect("a master");
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

/// Verify that unknown timezone identifiers remain undefined in the envelope.
#[test]
fn a_zone_no_database_knows_is_left_undefined() {
    let component = instance(&zoned("W. Europe Standard Time"));
    let list = instance_list(&[component]);

    unsafe {
        let saved = marshal::icalendar_from_instances(list, ptr::null_mut()).expect("a master");
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

/// Verify that UTC events do not generate `VTIMEZONE` definitions.
#[test]
fn utc_is_a_zone_with_nothing_to_define() {
    let component = instance(&zoned("UTC"));
    let list = instance_list(&[component]);

    unsafe {
        let saved = marshal::icalendar_from_instances(list, ptr::null_mut()).expect("a master");
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

/// Custom timezone identifier fixture per RFC 8984 1.4.8.
const CUSTOM_TZID: &str = "/Example Corp/Reindeer Standard Time";

/// Calendar object containing a custom timezone definition without `X-LIC-LOCATION`.
const CUSTOM_ZONE_OBJECT: &str = "BEGIN:VCALENDAR\r\n\
     VERSION:2.0\r\n\
     PRODID:-//Example Corp//Invitation//EN\r\n\
     BEGIN:VTIMEZONE\r\n\
     TZID:/Example Corp/Reindeer Standard Time\r\n\
     BEGIN:STANDARD\r\n\
     DTSTART:19701025T030000\r\n\
     RRULE:FREQ=YEARLY;BYMONTH=10;BYDAY=-1SU\r\n\
     TZOFFSETFROM:+0200\r\n\
     TZOFFSETTO:+0100\r\n\
     TZNAME:RST\r\n\
     END:STANDARD\r\n\
     BEGIN:DAYLIGHT\r\n\
     DTSTART:19700329T020000\r\n\
     RRULE:FREQ=YEARLY;BYMONTH=3;BYDAY=-1SU\r\n\
     TZOFFSETFROM:+0100\r\n\
     TZOFFSETTO:+0200\r\n\
     TZNAME:RDT\r\n\
     END:DAYLIGHT\r\n\
     END:VTIMEZONE\r\n\
     END:VCALENDAR\r\n";

/// Atomic counter to distinguish test cache paths.
static CACHES: AtomicU32 = AtomicU32::new(0);

/// Create an `ETimezoneCache` populated with timezones from `objects`.
///
/// Backed by a real `ECalCache` instance matching backend runtime behavior.
fn zone_cache(objects: &[(&str, &str)]) -> *mut ETimezoneCache {
    let path = std::env::temp_dir().join(format!(
        "jmap-cal-zones-{}-{}.db",
        std::process::id(),
        CACHES.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = std::fs::remove_file(&path);
    let filename = std::ffi::CString::new(path.to_str().expect("a UTF-8 temporary path")).unwrap();

    // SAFETY: every string is NUL-terminated and valid for its call. The cache
    // and each parsed object are fresh references this owns; the zone comes back
    // transfer full from the object and is dropped once the cache has taken its
    // own reference to it.
    unsafe {
        let mut error: *mut GError = ptr::null_mut();
        let cache = e_cal_cache_new(filename.as_ptr(), ptr::null_mut(), &mut error);
        assert!(!cache.is_null(), "no calendar cache at {}", path.display());
        for (icalendar, tzid) in objects {
            let text = std::ffi::CString::new(*icalendar).unwrap();
            let name = std::ffi::CString::new(*tzid).unwrap();
            let component = i_cal_component_new_from_string(text.as_ptr());
            assert!(!component.is_null(), "not a calendar object: {icalendar}");
            let zone = i_cal_component_get_timezone(component, name.as_ptr());
            assert!(!zone.is_null(), "the object defines no {tzid}");
            e_timezone_cache_add_timezone(cache.cast(), zone);
            g_object_unref(zone.cast());
            g_object_unref(component.cast());
        }
        let _ = std::fs::remove_file(&path);
        cache.cast()
    }
}

/// Verify that custom zones in the calendar cache are defined in the envelope.
#[test]
fn a_custom_zone_the_calendar_holds_is_defined_out_of_it() {
    let component = instance(&zoned(CUSTOM_TZID));
    let list = instance_list(&[component]);
    let zones = zone_cache(&[(CUSTOM_ZONE_OBJECT, CUSTOM_TZID)]);

    unsafe {
        let saved = marshal::icalendar_from_instances(list, zones).expect("a master");
        assert_eq!(
            definitions(&saved.icalendar),
            1,
            "the zone the calendar holds did not reach the envelope: {}",
            saved.icalendar
        );
        assert!(
            saved.icalendar.contains(&format!("TZID:{CUSTOM_TZID}")),
            "the definition is not of the zone the event names: {}",
            saved.icalendar
        );
        // The mapping can now state the timezone.
        let event = jmap_ical::ical_to_event(&saved.icalendar).expect("a calendar object");
        assert_eq!(
            event.time_zone.as_deref(),
            Some(CUSTOM_TZID),
            "the mapping cannot name the zone: {}",
            saved.icalendar
        );
        assert!(
            jmap_ical::maps_time_zone(&event),
            "the zone is named but not defined, which the save refuses: {}",
            saved.icalendar
        );

        glib_sys::g_slist_free(list);
        g_object_unref(component.cast());
        g_object_unref(zones.cast());
    }
}

/// Verify that transition rules with UTC UNTIL instants survive cache round trips.
#[test]
fn an_observance_that_stopped_repeating_keeps_its_end_through_the_calendar() {
    let object = CUSTOM_ZONE_OBJECT.replace(
        "RRULE:FREQ=YEARLY;BYMONTH=3;BYDAY=-1SU\r\n",
        "RRULE:FREQ=YEARLY;BYMONTH=3;BYDAY=-1SU;UNTIL=19730325T010000Z\r\n",
    );
    let component = instance(&zoned(CUSTOM_TZID));
    let list = instance_list(&[component]);
    let zones = zone_cache(&[(&object, CUSTOM_TZID)]);

    unsafe {
        let saved = marshal::icalendar_from_instances(list, zones).expect("a master");
        let event = jmap_ical::ical_to_event(&saved.icalendar).expect("a calendar object");
        let definition = serde_json::to_value(&event.time_zones).expect("a map of time zones")
            [CUSTOM_TZID]["daylight"][0]["recurrenceRules"][0]["until"]
            .clone();

        // 01:00 UTC corresponds to 02:00 in +0100 standard time.
        assert_eq!(
            definition,
            serde_json::json!("1973-03-25T02:00:00"),
            "the end of the transition rule did not survive the trip: {}",
            saved.icalendar
        );

        glib_sys::g_slist_free(list);
        g_object_unref(component.cast());
        g_object_unref(zones.cast());
    }
}

/// Verify that unknown zones not in the calendar cache remain undefined.
#[test]
fn a_zone_the_calendar_does_not_hold_either_is_still_left_undefined() {
    let component = instance(&zoned("W. Europe Standard Time"));
    let list = instance_list(&[component]);
    let zones = zone_cache(&[(CUSTOM_ZONE_OBJECT, CUSTOM_TZID)]);

    unsafe {
        let saved = marshal::icalendar_from_instances(list, zones).expect("a master");
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
        g_object_unref(zones.cast());
    }
}

/// Verify that passing a null calendar leaves custom zones undefined.
#[test]
fn no_calendar_means_no_second_place_to_look() {
    let component = instance(&zoned(CUSTOM_TZID));
    let list = instance_list(&[component]);

    unsafe {
        let saved = marshal::icalendar_from_instances(list, ptr::null_mut()).expect("a master");
        assert_eq!(definitions(&saved.icalendar), 0, "{}", saved.icalendar);

        glib_sys::g_slist_free(list);
        g_object_unref(component.cast());
    }
}

/// Render a `VCALENDAR` envelope containing `vevents`.
fn object(vevents: &str) -> String {
    format!(
        "BEGIN:VCALENDAR\r\nVERSION:2.0\r\n\
         PRODID:-//evolution-jmap//JMAP calendar backend//EN\r\n\
         {vevents}END:VCALENDAR\r\n"
    )
}

/// Check whether `tzid` resolves from `VTIMEZONE` components in `icalendar`.
fn resolves(icalendar: &str, tzid: &str) -> bool {
    resolved_definition(icalendar, tzid).is_some()
}

/// Return the rendered `VTIMEZONE` definition for `tzid` in `icalendar`.
fn resolved_definition(icalendar: &str, tzid: &str) -> Option<String> {
    let text = std::ffi::CString::new(icalendar).unwrap();
    let name = std::ffi::CString::new(tzid).unwrap();
    // SAFETY: both strings are NUL-terminated and valid for the calls.
    // Component, zone, and definition references are released.
    unsafe {
        let component = i_cal_component_new_from_string(text.as_ptr());
        assert!(!component.is_null(), "not a calendar object: {icalendar}");
        let zone = i_cal_component_get_timezone(component, name.as_ptr());
        let definition = match zone.is_null() {
            true => None,
            false => {
                let definition = i_cal_timezone_get_component(zone);
                let rendered = marshal::ical_from_component(definition);
                if !definition.is_null() {
                    g_object_unref(definition.cast());
                }
                g_object_unref(zone.cast());
                rendered
            }
        };
        g_object_unref(component.cast());
        definition
    }
}

/// Verify that outgoing objects define the zones they reference.
#[test]
fn an_object_handed_to_eds_defines_the_zone_it_refers_to() {
    let infos = [info("K1", "r1", &object(&zoned("Europe/Zurich")))];
    let list = marshal::info_list(&infos);

    unsafe {
        let object = nth_info(list, 0).2.expect("an object");
        // Verify timezone resolution and property preservation.
        assert!(
            resolves(&object, "Europe/Zurich"),
            "a reader cannot resolve the zone out of the object: {object}"
        );
        assert_eq!(
            definitions(&object),
            1,
            "the zone the event is in is not defined in the object: {object}"
        );
        assert!(
            !object.contains("freeassociation"),
            "the definition is under libical's identifier, not the event's: {object}"
        );
        assert!(
            object.contains("DTSTART;TZID=Europe/Zurich:20260810T090000"),
            "the event itself did not survive: {object}"
        );
        glib_sys::g_slist_free_full(list, Some(e_cal_meta_backend_info_free));
    }
}

/// Verify that components returned by `load_component_sync` define referenced zones.
#[test]
fn a_component_handed_back_defines_the_zone_it_refers_to() {
    let component = marshal::component_from_ical(&object(&zoned("Europe/Zurich")));
    assert!(!component.is_null());

    unsafe {
        let rendered = marshal::ical_from_component(component).expect("a rendering");
        marshal::component_unref(component);
        assert_eq!(definitions(&rendered), 1, "{rendered}");
        assert!(
            resolves(&rendered, "Europe/Zurich"),
            "a reader cannot resolve the zone out of the component: {rendered}"
        );
    }
}

/// Verify that objects without timezones are passed through unchanged.
#[test]
fn an_object_that_names_no_zone_is_handed_over_as_it_stands() {
    let infos = [info("K1", "r1", UNZONED_OBJECT)];
    let list = marshal::info_list(&infos);

    unsafe {
        assert_eq!(nth_info(list, 0).2.as_deref(), Some(UNZONED_OBJECT));
        glib_sys::g_slist_free_full(list, Some(e_cal_meta_backend_info_free));
    }
}

/// Verify that unknown timezone names remain undefined on export.
#[test]
fn a_zone_no_database_knows_is_left_undefined_on_the_way_out() {
    let text = object(&zoned("W. Europe Standard Time"));
    let infos = [info("K1", "r1", &text)];
    let list = marshal::info_list(&infos);

    unsafe {
        let object = nth_info(list, 0).2.expect("an object");
        assert_eq!(
            definitions(&object),
            0,
            "a zone was invented for an identifier nothing resolves: {object}"
        );
        assert_eq!(object, text, "the object was rebuilt for nothing");
        glib_sys::g_slist_free_full(list, Some(e_cal_meta_backend_info_free));
    }
}

/// Verify that shared timezones across multiple events define each zone once.
#[test]
fn one_zone_named_by_two_events_is_defined_once() {
    let text = object(
        "BEGIN:VEVENT\r\nUID:K1\r\nSUMMARY:Standup\r\n\
         DTSTART;TZID=Europe/Zurich:20260810T090000\r\nRRULE:FREQ=DAILY\r\nEND:VEVENT\r\n\
         BEGIN:VEVENT\r\nUID:K1\r\nSUMMARY:Standup\\, moved\r\n\
         RECURRENCE-ID;TZID=Europe/Zurich:20260812T090000\r\n\
         DTSTART;TZID=Europe/Zurich:20260812T100000\r\nEND:VEVENT\r\n",
    );
    let infos = [info("K1", "r1", &text)];
    let list = marshal::info_list(&infos);

    unsafe {
        let object = nth_info(list, 0).2.expect("an object");
        assert_eq!(
            definitions(&object),
            1,
            "not one definition per zone: {object}"
        );
        assert!(
            resolves(&object, "Europe/Zurich"),
            "a reader cannot resolve the zone out of the object: {object}"
        );
        glib_sys::g_slist_free_full(list, Some(e_cal_meta_backend_info_free));
    }
}

/// Verify that existing timezone definitions in the object are preserved.
#[test]
fn a_zone_the_object_already_defines_is_not_defined_twice() {
    let text = object(&format!(
        "BEGIN:VTIMEZONE\r\nTZID:Europe/Zurich\r\nBEGIN:STANDARD\r\n\
         DTSTART:19701025T030000\r\nTZOFFSETFROM:+0200\r\nTZOFFSETTO:+0100\r\n\
         END:STANDARD\r\nEND:VTIMEZONE\r\n{}",
        zoned("Europe/Zurich")
    ));
    let infos = [info("K1", "r1", &text)];
    let list = marshal::info_list(&infos);

    unsafe {
        let object = nth_info(list, 0).2.expect("an object");
        assert_eq!(
            definitions(&object),
            1,
            "the zone was defined twice: {object}"
        );
        assert_eq!(object, text, "the object was rebuilt for nothing");
        glib_sys::g_slist_free_full(list, Some(e_cal_meta_backend_info_free));
    }
}

/// Calendar object fixture with a server-defined custom timezone.
fn in_a_zone_only_the_server_can_name() -> String {
    let tzid = "/example.com/Europe-Berlin";
    let event = jmap_proto::calendars::CalendarEvent {
        id: Some("K1".into()),
        title: Some("Standup".to_owned()),
        start: Some("2026-08-10T09:00:00".to_owned()),
        time_zone: Some(tzid.to_owned()),
        duration: Some("PT1H".to_owned()),
        time_zones: serde_json::from_value(serde_json::json!({tzid: {
            "@type": "TimeZone",
            "tzId": tzid,
            "standard": [{
                "@type": "TimeZoneRule",
                "start": "1970-10-25T03:00:00",
                "offsetFrom": "+0200",
                "offsetTo": "+0100",
                "recurrenceRules": [{
                    "@type": "RecurrenceRule",
                    "frequency": "yearly",
                    "byMonth": ["10"],
                    "byDay": [{"@type": "NDay", "day": "su", "nthOfPeriod": -1}],
                }],
                "names": {"CET": true},
            }],
            "daylight": [{
                "@type": "TimeZoneRule",
                "start": "1970-03-29T02:00:00",
                "offsetFrom": "+0100",
                "offsetTo": "+0200",
                "recurrenceRules": [{
                    "@type": "RecurrenceRule",
                    "frequency": "yearly",
                    "byMonth": ["3"],
                    "byDay": [{"@type": "NDay", "day": "su", "nthOfPeriod": -1}],
                }],
                "names": {"CEST": true},
            }],
        }}))
        .expect("a map of time zones"),
        ..Default::default()
    };
    jmap_ical::event_to_ical(&event)
}

/// Verify that libical constructs a valid timezone from the generated `VTIMEZONE`.
///
/// IANA names resolve through builtin tables.
/// Custom identifiers require the full server definition.
/// If libical rejects the definition, the start time floats.
/// This test verifies that libical parses the custom timezone correctly.
#[test]
fn the_zone_only_the_server_could_name_reaches_eds_and_resolves() {
    let text = in_a_zone_only_the_server_can_name();
    let infos = [info("K1", "r1", &text)];
    let list = marshal::info_list(&infos);

    unsafe {
        let object = nth_info(list, 0).2.expect("an object");
        // Verify timezone resolution.
        let zone = resolved_definition(&object, "/example.com/Europe-Berlin")
            .unwrap_or_else(|| panic!("libical resolved no zone at all: {object}"));
        // The resolved timezone must match the server transitions and offsets.
        for part in [
            "BEGIN:STANDARD",
            "DTSTART:19701025T030000",
            "TZOFFSETFROM:+0200",
            "TZOFFSETTO:+0100",
            "RRULE:FREQ=YEARLY;BYDAY=-1SU;BYMONTH=10",
            "TZNAME:CET",
            "BEGIN:DAYLIGHT",
            "DTSTART:19700329T020000",
            "TZOFFSETFROM:+0100",
            "TZOFFSETTO:+0200",
            "RRULE:FREQ=YEARLY;BYDAY=-1SU;BYMONTH=3",
            "TZNAME:CEST",
        ] {
            assert!(
                zone.lines().any(|line| line.trim_end() == part),
                "the zone libical built has no {part} line in it: {zone}"
            );
        }
        assert_eq!(
            definitions(&object),
            1,
            "the zone is defined more than once, or not at all: {object}"
        );
        // Defined zones pass through unchanged without object reconstruction.
        assert_eq!(object, text, "the object was rebuilt for nothing");
        glib_sys::g_slist_free_full(list, Some(e_cal_meta_backend_info_free));
    }
}

/// Verify that the loaded component preserves the custom timezone definition.
///
/// EDS uses this path when event info contains no cached object.
#[test]
fn the_component_handed_back_defines_the_zone_only_the_server_could_name() {
    let text = in_a_zone_only_the_server_can_name();
    let component = marshal::component_from_ical(&text);
    assert!(!component.is_null());

    unsafe {
        let rendered = marshal::ical_from_component(component).expect("a rendering");
        marshal::component_unref(component);
        assert_eq!(definitions(&rendered), 1, "{rendered}");
        assert!(
            resolves(&rendered, "/example.com/Europe-Berlin"),
            "a reader cannot resolve the zone out of the component: {rendered}"
        );
    }
}

/// Verify that libical preserves recurrence rule parts in written order.
///
/// Component caching requires identical field ordering to prevent spurious edits.
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
        // Test `BYYEARDAY` ordering between month days and months.
        "FREQ=YEARLY;BYDAY=WE;BYMONTHDAY=15;BYYEARDAY=100;BYMONTH=3",
        "FREQ=YEARLY;COUNT=4;BYWEEKNO=1,-1;WKST=SU",
        "FREQ=YEARLY;BYWEEKNO=53",
        // Test `BYWEEKNO` ordering between year days and months.
        "FREQ=YEARLY;BYDAY=WE;BYMONTHDAY=15;BYYEARDAY=100;BYWEEKNO=20;BYMONTH=3;WKST=SU",
        "FREQ=MONTHLY;COUNT=4;BYDAY=FR;BYSETPOS=-1",
        "FREQ=MONTHLY;BYDAY=MO,TU,WE,TH,FR;BYSETPOS=1,-1",
        // Test `BYSETPOS` ordering after months and before week start.
        "FREQ=YEARLY;BYDAY=WE;BYMONTHDAY=15;BYYEARDAY=100;BYWEEKNO=20;BYMONTH=3;\
         BYSETPOS=2;WKST=SU",
        // RFC 5545 section 3.3.10 allows `BYYEARDAY` with sub-daily frequencies.
        "FREQ=HOURLY;BYYEARDAY=100",
        // RFC 5545 section 3.3.10 allows `BYMONTH` at any frequency.
        "FREQ=WEEKLY;BYDAY=MO;BYMONTH=12",
        // libical writes series bounds before `INTERVAL`.
        "FREQ=WEEKLY;COUNT=6;INTERVAL=2;BYDAY=MO",
        "FREQ=DAILY;UNTIL=20261231T000000Z;INTERVAL=2",
    ] {
        assert_eq!(reparsed_rrule(rrule).as_deref(), Some(rrule));
    }
    // libical normalizes rule part ordering on output.
    assert_eq!(
        reparsed_rrule("FREQ=WEEKLY;INTERVAL=2;COUNT=6;BYDAY=MO").as_deref(),
        Some("FREQ=WEEKLY;COUNT=6;INTERVAL=2;BYDAY=MO"),
    );
}

/// Verify that invalid month days cause libical to discard the entire rule.
#[test]
fn a_day_of_the_month_no_month_has_would_cost_libical_the_whole_rule() {
    for rrule in ["FREQ=MONTHLY;BYMONTHDAY=32", "FREQ=MONTHLY;BYMONTHDAY=0"] {
        assert_eq!(reparsed_rrule(rrule).as_deref(), None, "{rrule}");
    }
}

/// Verify libical handling of rejected month values.
#[test]
fn libical_answers_for_a_month_this_mapping_refuses() {
    // Out-of-range values cause libical to drop the entire `RRULE`.
    for rrule in [
        "FREQ=YEARLY;BYMONTH=0",
        "FREQ=YEARLY;BYMONTH=-1",
        "FREQ=YEARLY;BYMONTH=99",
        "FREQ=YEARLY;BYMONTH=3,XX",
    ] {
        assert_eq!(reparsed_rrule(rrule).as_deref(), None, "{rrule}");
    }
    // libical accepts 13 and leap months, but Gregorian calendars exclude them.
    for rrule in ["FREQ=YEARLY;BYMONTH=13", "FREQ=YEARLY;BYMONTH=5L"] {
        assert_eq!(reparsed_rrule(rrule).as_deref(), Some(rrule), "{rrule}");
    }
    // libical strips leading zeros from month numbers.
    assert_eq!(
        reparsed_rrule("FREQ=YEARLY;BYMONTH=03").as_deref(),
        Some("FREQ=YEARLY;BYMONTH=3"),
    );
}

/// Verify libical handling of rejected year-day values.
#[test]
fn libical_answers_for_a_day_of_the_year_this_mapping_refuses() {
    // Out-of-range values cause libical to drop the entire `RRULE`.
    for rrule in [
        "FREQ=YEARLY;BYYEARDAY=0",
        "FREQ=YEARLY;BYYEARDAY=999",
        "FREQ=YEARLY;BYYEARDAY=100,XX",
    ] {
        assert_eq!(reparsed_rrule(rrule).as_deref(), None, "{rrule}");
    }
    // libical preserves 367 and -367, but no calendar year contains them.
    for rrule in ["FREQ=YEARLY;BYYEARDAY=367", "FREQ=YEARLY;BYYEARDAY=-367"] {
        assert_eq!(reparsed_rrule(rrule).as_deref(), Some(rrule), "{rrule}");
    }
    // libical permits `BYYEARDAY` with frequencies forbidden by RFC 5545 section 3.3.10.
    for rrule in [
        "FREQ=MONTHLY;BYYEARDAY=100",
        "FREQ=WEEKLY;BYYEARDAY=100",
        "FREQ=DAILY;BYYEARDAY=100",
    ] {
        assert_eq!(reparsed_rrule(rrule).as_deref(), Some(rrule), "{rrule}");
    }
    // libical normalizes leading zeros and plus signs on year days.
    for (written, back) in [
        ("FREQ=YEARLY;BYYEARDAY=010", "FREQ=YEARLY;BYYEARDAY=10"),
        ("FREQ=YEARLY;BYYEARDAY=+100", "FREQ=YEARLY;BYYEARDAY=100"),
    ] {
        assert_eq!(reparsed_rrule(written).as_deref(), Some(back), "{written}");
    }
}

/// Verify libical handling of rejected week-number values.
#[test]
fn libical_answers_for_a_week_of_the_year_this_mapping_refuses() {
    // Out-of-range values cause libical to drop the entire `RRULE`.
    for rrule in [
        "FREQ=YEARLY;BYWEEKNO=0",
        "FREQ=YEARLY;BYWEEKNO=999",
        "FREQ=YEARLY;BYWEEKNO=20,XX",
    ] {
        assert_eq!(reparsed_rrule(rrule).as_deref(), None, "{rrule}");
    }
    // libical preserves week 54, but no calendar year contains 54 weeks.
    for rrule in ["FREQ=YEARLY;BYWEEKNO=54", "FREQ=YEARLY;BYWEEKNO=-54"] {
        assert_eq!(reparsed_rrule(rrule).as_deref(), Some(rrule), "{rrule}");
    }
    // libical permits `BYWEEKNO` with non-yearly frequencies.
    for rrule in [
        "FREQ=MONTHLY;BYWEEKNO=20",
        "FREQ=WEEKLY;BYWEEKNO=20",
        "FREQ=DAILY;BYWEEKNO=20",
        "FREQ=HOURLY;BYWEEKNO=20",
    ] {
        assert_eq!(reparsed_rrule(rrule).as_deref(), Some(rrule), "{rrule}");
    }
    // libical normalizes leading zeros and plus signs on week numbers.
    for (written, back) in [
        ("FREQ=YEARLY;BYWEEKNO=020", "FREQ=YEARLY;BYWEEKNO=20"),
        ("FREQ=YEARLY;BYWEEKNO=+20", "FREQ=YEARLY;BYWEEKNO=20"),
    ] {
        assert_eq!(reparsed_rrule(written).as_deref(), Some(back), "{written}");
    }
}

/// Verify libical handling of rejected set positions.
#[test]
fn libical_answers_for_a_position_in_the_set_this_mapping_refuses() {
    // Out-of-range values cause libical to drop the entire `RRULE`.
    for rrule in [
        "FREQ=MONTHLY;BYDAY=MO;BYSETPOS=0",
        "FREQ=MONTHLY;BYDAY=MO;BYSETPOS=999",
        "FREQ=MONTHLY;BYDAY=MO;BYSETPOS=1,XX",
    ] {
        assert_eq!(reparsed_rrule(rrule).as_deref(), None, "{rrule}");
    }
    // libical preserves position 367, but no interval expands to 367 items.
    for rrule in [
        "FREQ=MONTHLY;BYDAY=MO;BYSETPOS=367",
        "FREQ=MONTHLY;BYDAY=MO;BYSETPOS=-367",
    ] {
        assert_eq!(reparsed_rrule(rrule).as_deref(), Some(rrule), "{rrule}");
    }
    // libical allows isolated `BYSETPOS` without accompanying `BYxxx` parts.
    for rrule in [
        "FREQ=DAILY;BYSETPOS=1",
        "FREQ=MONTHLY;BYSETPOS=-1",
        "FREQ=YEARLY;BYSETPOS=2",
    ] {
        assert_eq!(reparsed_rrule(rrule).as_deref(), Some(rrule), "{rrule}");
    }
    // libical normalizes leading zeros and plus signs on set positions.
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

/// Verify libical handling of recurrence hours.
#[test]
fn libical_answers_for_the_hours_of_the_day() {
    // libical places `BYHOUR` before day parts.
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
    // RFC 5545 section 3.3.10 allows hours at any frequency in given order.
    for rrule in [
        "FREQ=DAILY;BYHOUR=0",
        "FREQ=DAILY;BYHOUR=23,9,0",
        "FREQ=HOURLY;BYHOUR=9",
        "FREQ=MINUTELY;BYHOUR=9",
        "FREQ=YEARLY;BYHOUR=9",
    ] {
        assert_eq!(reparsed_rrule(rrule).as_deref(), Some(rrule), "{rrule}");
    }
    // Any invalid hour value causes libical to drop the entire `RRULE`.
    for rrule in [
        "FREQ=DAILY;BYHOUR=24",
        "FREQ=DAILY;BYHOUR=99",
        "FREQ=DAILY;BYHOUR=-1",
        "FREQ=DAILY;BYHOUR=9,XX",
    ] {
        assert_eq!(reparsed_rrule(rrule).as_deref(), None, "{rrule}");
    }
    // libical treats empty `BYHOUR=` as midnight.
    assert_eq!(
        reparsed_rrule("FREQ=DAILY;BYHOUR=").as_deref(),
        Some("FREQ=DAILY;BYHOUR=0"),
    );
    // libical normalizes leading zeros and plus signs on hour values.
    for written in ["FREQ=DAILY;BYHOUR=09", "FREQ=DAILY;BYHOUR=+9"] {
        assert_eq!(
            reparsed_rrule(written).as_deref(),
            Some("FREQ=DAILY;BYHOUR=9"),
            "{written}"
        );
    }
}

/// Verify libical handling of recurrence minutes and seconds.
#[test]
fn libical_answers_for_the_minutes_and_seconds() {
    // libical orders time parts as `BYSECOND`, `BYMINUTE`, then `BYHOUR`.
    assert_eq!(
        reparsed_rrule("FREQ=WEEKLY;BYDAY=MO;BYMINUTE=30;BYSECOND=15;BYHOUR=9").as_deref(),
        Some("FREQ=WEEKLY;BYSECOND=15;BYMINUTE=30;BYHOUR=9;BYDAY=MO"),
    );
    let all = "FREQ=YEARLY;BYSECOND=0;BYMINUTE=30;BYHOUR=9;BYDAY=WE;BYMONTHDAY=15;\
               BYYEARDAY=100;BYWEEKNO=20;BYMONTH=3;BYSETPOS=2;WKST=SU";
    assert_eq!(reparsed_rrule(all).as_deref(), Some(all));
    // RFC 5545 section 3.3.10 allows seconds 0 to 60 for leap seconds.
    for rrule in [
        "FREQ=HOURLY;BYMINUTE=0,30",
        "FREQ=MINUTELY;BYSECOND=30,0",
        "FREQ=DAILY;BYMINUTE=59",
        "FREQ=DAILY;BYSECOND=60",
        // `BYSETPOS` can select from minutes and seconds.
        "FREQ=DAILY;BYMINUTE=30;BYSETPOS=-1",
        "FREQ=DAILY;BYSECOND=30;BYSETPOS=-1",
    ] {
        assert_eq!(reparsed_rrule(rrule).as_deref(), Some(rrule), "{rrule}");
    }
    // Any out-of-range minute or second causes libical to drop the entire `RRULE`.
    for rrule in [
        "FREQ=DAILY;BYMINUTE=60",
        "FREQ=DAILY;BYSECOND=61",
        "FREQ=DAILY;BYMINUTE=-1",
        "FREQ=DAILY;BYSECOND=9,XX",
    ] {
        assert_eq!(reparsed_rrule(rrule).as_deref(), None, "{rrule}");
    }
    // libical interprets empty minute or second parts as zero.
    for (written, back) in [
        ("FREQ=DAILY;BYMINUTE=", "FREQ=DAILY;BYMINUTE=0"),
        ("FREQ=DAILY;BYSECOND=", "FREQ=DAILY;BYSECOND=0"),
        // libical normalizes leading zeros and plus signs on time parts.
        ("FREQ=DAILY;BYMINUTE=09", "FREQ=DAILY;BYMINUTE=9"),
        ("FREQ=DAILY;BYSECOND=+9", "FREQ=DAILY;BYSECOND=9"),
    ] {
        assert_eq!(reparsed_rrule(written).as_deref(), Some(back), "{written}");
    }
}

/// Verify that libical preserves time-of-day parts beside date start values.
#[test]
fn libical_keeps_a_time_of_day_beside_an_all_day_start_that_forbids_it() {
    let vevent = concat!(
        "BEGIN:VEVENT\r\nUID:K1\r\nSUMMARY:S\r\nDTSTART;VALUE=DATE:20260810\r\n",
        "RRULE:FREQ=DAILY;BYSECOND=0;BYMINUTE=30;BYHOUR=9\r\nEND:VEVENT\r\n",
    );
    let component = instance(vevent);
    let list = instance_list(&[component]);
    let saved =
        unsafe { marshal::icalendar_from_instances(list, ptr::null_mut()) }.expect("a master");
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

/// Verify libical handling of the recurrence week-start property.
#[test]
fn libical_answers_for_the_day_a_week_starts_on() {
    // libical removes default `WKST=MO` per RFC 5545 section 3.3.10.
    assert_eq!(
        reparsed_rrule("FREQ=WEEKLY;WKST=MO").as_deref(),
        Some("FREQ=WEEKLY"),
    );
    // Non-default week starts survive verbatim across frequencies.
    for rrule in [
        "FREQ=WEEKLY;WKST=SU",
        "FREQ=WEEKLY;INTERVAL=2;BYDAY=TU;WKST=SU",
        "FREQ=DAILY;WKST=SU",
        "FREQ=MONTHLY;WKST=SA",
        "FREQ=YEARLY;WKST=SU",
    ] {
        assert_eq!(reparsed_rrule(rrule).as_deref(), Some(rrule), "{rrule}");
    }
    // Invalid weekday tokens cause libical to drop the entire `RRULE`.
    for rrule in ["FREQ=WEEKLY;WKST=XX", "FREQ=WEEKLY;WKST="] {
        assert_eq!(reparsed_rrule(rrule).as_deref(), None, "{rrule}");
    }
    // libical converts lowercase weekday tokens to uppercase.
    assert_eq!(
        reparsed_rrule("FREQ=YEARLY;WKST=su").as_deref(),
        Some("FREQ=YEARLY;WKST=SU"),
    );
    // libical places `WKST` after `BYMONTH`.
    assert_eq!(
        reparsed_rrule("FREQ=WEEKLY;BYDAY=MO;WKST=SU;BYMONTH=3").as_deref(),
        Some("FREQ=WEEKLY;BYDAY=MO;BYMONTH=3;WKST=SU"),
    );
}

/// Return the `RRULE` string parsed by libical, unfolding line splits.
fn reparsed_rrule(value: &str) -> Option<String> {
    let vevent = format!(
        "BEGIN:VEVENT\r\nUID:K1\r\nSUMMARY:S\r\nDTSTART:20260810T070000Z\r\n\
         RRULE:{value}\r\nEND:VEVENT\r\n"
    );
    let component = instance(&vevent);
    let list = instance_list(&[component]);
    // SAFETY: `list` holds one live component, freed below with the instance.
    let saved =
        unsafe { marshal::icalendar_from_instances(list, ptr::null_mut()) }.expect("a master");
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

/// Verify that libical preserves custom location parameters and escaping.
#[test]
fn libical_keeps_the_location_key_this_mapping_writes() {
    for line in [
        "LOCATION;X-JMAP-KEY=srv1:Room 42",
        // Verify text escaping per RFC 5545 section 3.3.11.
        "LOCATION;X-JMAP-KEY=srv1:Berlin\\, Room 42\\; 3rd floor",
    ] {
        assert_eq!(reparsed_lines(line), vec![line.to_owned()]);
    }
    // libical permits multiple `LOCATION` lines despite RFC 5545 section 3.6.1 limits.
    assert_eq!(
        reparsed_lines("LOCATION:Room 42\r\nLOCATION:Cafeteria"),
        vec![
            "LOCATION:Room 42".to_owned(),
            "LOCATION:Cafeteria".to_owned()
        ]
    );
}

/// Verify that libical splits comma-separated categories into distinct lines.
///
/// libical preserves all tags and text escaping across the split.
#[test]
fn libical_splits_the_categories_this_mapping_writes_and_keeps_every_tag() {
    assert_eq!(
        reparsed("CATEGORIES", "CATEGORIES:offsite,planning"),
        vec![
            "CATEGORIES:offsite".to_owned(),
            "CATEGORIES:planning".to_owned()
        ]
    );
    // Verify text escaping per RFC 5545 section 3.3.11 across the split.
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

    // The round-tripped category set matches the original input.
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

/// Verify that occurrence category overrides remain separate from master tags.
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
    let saved =
        unsafe { marshal::icalendar_from_instances(list, ptr::null_mut()) }.expect("a master");
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

/// Verify that occurrence alarm overrides remain separate from master alarms.
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
    let saved =
        unsafe { marshal::icalendar_from_instances(list, ptr::null_mut()) }.expect("a master");
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

/// Verify that libical neither synthesizes nor alters transparency properties.
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

/// Verify that libical neither synthesizes nor alters priority properties.
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

/// Verify that libical neither synthesizes nor alters classification properties.
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

/// Verify that libical generates a default `DTSTAMP` and preserves existing timestamps.
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
    // Preserved timestamps remain verbatim without overwrite.
    for line in [
        "CREATED:20260102T093000Z",
        "DTSTAMP:20260115T174501Z",
        "LAST-MODIFIED:20260115T174501Z",
    ] {
        let (property, _) = line.split_once(':').expect("a content line");
        assert_eq!(reparsed(property, line), vec![line.to_owned()]);
    }
}

/// Verify that libical preserves attendee and organizer properties with parameters.
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
    // libical does not synthesize missing attendees or organizers.
    for property in ["ATTENDEE", "ORGANIZER"] {
        assert_eq!(
            reparsed(property, "DESCRIPTION:the quarter"),
            Vec::<String>::new(),
            "libical filled in an {property} of its own"
        );
    }
}

/// Return unfolded `LOCATION` lines after libical round-trip parsing.
fn reparsed_lines(lines: &str) -> Vec<String> {
    reparsed("LOCATION", lines)
}

/// Return unfolded property lines matching `property` after libical round-trip parsing.
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

/// Return the serialized `VCALENDAR` object from libical after parsing `lines`.
fn reparsed_object(lines: &str) -> String {
    let vevent = format!(
        "BEGIN:VEVENT\r\nUID:K1\r\nSUMMARY:S\r\nDTSTART:20260810T070000Z\r\n\
         {lines}\r\nEND:VEVENT\r\n"
    );
    let component = instance(&vevent);
    let list = instance_list(&[component]);
    // SAFETY: `list` holds one live component, freed below with the instance.
    let saved =
        unsafe { marshal::icalendar_from_instances(list, ptr::null_mut()) }.expect("a master");
    unsafe {
        glib_sys::g_slist_free(list);
        g_object_unref(component.cast());
    }
    saved.icalendar
}

/// Verify that libical preserves `VALARM` child components, UIDs, and parameters.
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

// ---------------------------------------------------------------------------
// Color tests for source changes

/// Distinct source identifier counter to prevent collisions across tests.
static NEXT_SOURCE: AtomicU32 = AtomicU32::new(0);

/// Test wrapper around an `ESource` pointer without registry dependencies.
struct TestSource(OwnedSource);

impl TestSource {
    fn new() -> Self {
        Self(OwnedSource::new(&format!(
            "jmap-cal-selectable-color-{}",
            NEXT_SOURCE.fetch_add(1, Ordering::Relaxed)
        )))
    }

    fn set_color(&self, color: &str) {
        let color = CString::new(color).unwrap();
        // SAFETY: Source pointer is valid. The extension is owned by the source. `set_color` copies the string.
        unsafe {
            let selectable: *mut ESourceSelectable =
                e_source_get_extension(self.0.as_ptr(), E_SOURCE_EXTENSION_CALENDAR.as_ptr())
                    .cast();
            e_source_selectable_set_color(selectable, color.as_ptr());
        }
    }
}

/// Verify reading custom color strings from the calendar extension.
#[test]
fn selectable_color_reads_the_calendar_extensions_color() {
    let source = TestSource::new();
    source.set_color("#ff0000");

    // SAFETY: Source pointer is valid and carries a calendar extension.
    let color = unsafe { marshal::selectable_color(source.0.as_ptr()) };
    assert_eq!(color.as_deref(), Some("#ff0000"));
}

/// Verify that an uncolored source returns the default system color.
#[test]
fn selectable_color_of_an_uncoloured_source_is_the_built_in_default_not_none() {
    let source = TestSource::new();

    // SAFETY: Source pointer is valid and carries a calendar extension.
    let color = unsafe { marshal::selectable_color(source.0.as_ptr()) };
    assert_eq!(color.as_deref(), Some("#62a0ea"));
}
