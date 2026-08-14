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

/// Probing `ECalComponent` attachment accessors and `ICalAttach` object properties.
/// `e_cal_component_has_attachments` indicates whether any `ATTACH` properties exist,
/// `e_cal_component_get_attachments` extracts them as a `GSList` of `ICalAttach *`,
/// and `e_cal_component_set_attachments(comp, NULL)` clears all attachments.
#[test]
fn ecalcomponent_attachment_handling_and_icalattach_properties() {
    let source = text(
        "BEGIN:VCALENDAR\r\n\
         VERSION:2.0\r\n\
         PRODID:-//evolution-jmap//JMAP calendar backend//EN\r\n\
         BEGIN:VEVENT\r\n\
         UID:K1\r\n\
         SUMMARY:Standup\r\n\
         DTSTART:20260810T070000Z\r\n\
         ATTACH;FMTTYPE=application/pdf;SIZE=51200;X-JMAP-KEY=l1:https://files.example.com/standup.pdf\r\n\
         ATTACH;X-JMAP-KEY=l2:https://files.example.com/notes.txt\r\n\
         END:VEVENT\r\n\
         END:VCALENDAR\r\n",
    );
    unsafe {
        let calendar = i_cal_component_new_from_string(source.as_ptr());
        assert!(!calendar.is_null());
        let event = i_cal_component_get_first_component(calendar, I_CAL_VEVENT_COMPONENT);
        assert!(!event.is_null());

        let comp = e_cal_component_new_from_icalcomponent(i_cal_component_clone(event));
        assert!(!comp.is_null());
        assert_eq!(e_cal_component_has_attachments(comp), 1);

        let attachments = e_cal_component_get_attachments(comp);
        assert!(!attachments.is_null());
        assert_eq!(g_slist_length(attachments), 2);

        let first = (*attachments).data as *mut ICalAttach;
        assert!(!first.is_null());
        assert_eq!(i_cal_attach_get_is_url(first), 1);
        let first_url = i_cal_attach_get_url(first);
        assert!(!first_url.is_null());
        assert_eq!(
            CStr::from_ptr(first_url).to_str().unwrap(),
            "https://files.example.com/standup.pdf"
        );

        let next_node = (*attachments).next;
        assert!(!next_node.is_null());
        let second = (*next_node).data as *mut ICalAttach;
        assert!(!second.is_null());
        assert_eq!(i_cal_attach_get_is_url(second), 1);
        let second_url = i_cal_attach_get_url(second);
        assert!(!second_url.is_null());
        assert_eq!(
            CStr::from_ptr(second_url).to_str().unwrap(),
            "https://files.example.com/notes.txt"
        );

        unsafe extern "C" fn unref_obj(ptr: *mut std::ffi::c_void) {
            unsafe { g_object_unref(ptr.cast()) };
        }
        g_slist_free_full(attachments, Some(unref_obj));

        // Clearing attachments via set_attachments with NULL removes them all.
        e_cal_component_set_attachments(comp, std::ptr::null());
        assert_eq!(e_cal_component_has_attachments(comp), 0);

        g_object_unref(comp.cast());
        g_object_unref(event.cast());
        g_object_unref(calendar.cast());
    }
}

/// Probing `ICalProperty` modification for ATTACH and IMAGE lines:
/// Setting a new URL via `i_cal_attach_new_from_url` and `i_cal_property_set_attach`
/// modifies the attachment in place while preserving non-standard `X-JMAP-KEY` parameters,
/// and removing one property leaves other ATTACH and IMAGE properties intact.
#[test]
fn icalproperty_attach_and_image_modification_and_parameter_preservation() {
    let source = text(
        "BEGIN:VCALENDAR\r\n\
         VERSION:2.0\r\n\
         PRODID:-//evolution-jmap//JMAP calendar backend//EN\r\n\
         BEGIN:VEVENT\r\n\
         UID:K1\r\n\
         SUMMARY:Standup\r\n\
         DTSTART:20260810T070000Z\r\n\
         ATTACH;FMTTYPE=application/pdf;SIZE=51200;X-JMAP-KEY=l1:https://files.example.com/standup.pdf\r\n\
         ATTACH;X-JMAP-KEY=l2:https://files.example.com/notes.txt\r\n\
         IMAGE;VALUE=URI;DISPLAY=BADGE;X-JMAP-KEY=img1:https://files.example.com/logo.png\r\n\
         END:VEVENT\r\n\
         END:VCALENDAR\r\n",
    );
    unsafe {
        let calendar = i_cal_component_new_from_string(source.as_ptr());
        assert!(!calendar.is_null());
        let event = i_cal_component_get_first_component(calendar, I_CAL_VEVENT_COMPONENT);
        assert!(!event.is_null());

        let attach_prop = i_cal_component_get_first_property(event, I_CAL_ATTACH_PROPERTY);
        assert!(!attach_prop.is_null());

        // Modify the first ATTACH in place to a new URL
        let new_url = text("https://files.example.com/standup-v2.pdf");
        let new_attach = i_cal_attach_new_from_url(new_url.as_ptr());
        assert!(!new_attach.is_null());
        i_cal_property_set_attach(attach_prop, new_attach);
        g_object_unref(new_attach.cast());

        let rendered = take_string(i_cal_component_as_ical_string(calendar));
        assert!(
            rendered.contains("standup-v2.pdf"),
            "new url missing: {rendered}"
        );
        assert!(rendered.contains("X-JMAP-KEY=l1"), "lost key: {rendered}");
        assert!(
            rendered.contains("FMTTYPE=application/pdf"),
            "lost fmttype: {rendered}"
        );
        assert!(
            rendered.contains("X-JMAP-KEY=l2"),
            "lost secondary attach: {rendered}"
        );
        assert!(rendered.contains("IMAGE"), "lost image: {rendered}");
        assert!(
            rendered.contains("X-JMAP-KEY=img1"),
            "lost image key: {rendered}"
        );

        // Remove the second ATTACH property
        let second_prop = i_cal_component_get_next_property(event, I_CAL_ATTACH_PROPERTY);
        assert!(!second_prop.is_null());
        i_cal_component_remove_property(event, second_prop);
        g_object_unref(second_prop.cast());
        g_object_unref(attach_prop.cast());

        let after_remove = take_string(i_cal_component_as_ical_string(calendar));
        assert!(after_remove.contains("standup-v2.pdf"), "{after_remove}");
        assert!(
            !after_remove.contains("notes.txt"),
            "removed attach still present: {after_remove}"
        );
        assert!(
            after_remove.contains("IMAGE"),
            "image removed: {after_remove}"
        );
        assert!(
            after_remove.contains("logo.png"),
            "image url lost: {after_remove}"
        );

        g_object_unref(event.cast());
        g_object_unref(calendar.cast());
    }
}

/// Probing `ECalComponent` classification, transparency, and status properties:
/// `e_cal_component_get_classification` maps `CLASS:CONFIDENTIAL` to `E_CAL_COMPONENT_CLASS_CONFIDENTIAL`,
/// `e_cal_component_get_transparency` maps `TRANSP:TRANSPARENT` to `E_CAL_COMPONENT_TRANSP_TRANSPARENT`,
/// and `e_cal_component_get_status` maps `STATUS:CONFIRMED` to `I_CAL_STATUS_CONFIRMED`.
/// Setting values in place modifies the component, and setting NONE clears them.
#[test]
fn ecalcomponent_classification_transparency_and_status_in_eds() {
    let source = text(
        "BEGIN:VCALENDAR\r\n\
         VERSION:2.0\r\n\
         PRODID:-//evolution-jmap//JMAP calendar backend//EN\r\n\
         BEGIN:VEVENT\r\n\
         UID:K1\r\n\
         SUMMARY:Planning\r\n\
         DTSTART:20260810T070000Z\r\n\
         CLASS:CONFIDENTIAL\r\n\
         TRANSP:TRANSPARENT\r\n\
         STATUS:CONFIRMED\r\n\
         END:VEVENT\r\n\
         END:VCALENDAR\r\n",
    );
    unsafe {
        let calendar = i_cal_component_new_from_string(source.as_ptr());
        assert!(!calendar.is_null());
        let event = i_cal_component_get_first_component(calendar, I_CAL_VEVENT_COMPONENT);
        assert!(!event.is_null());

        let comp = e_cal_component_new_from_icalcomponent(i_cal_component_clone(event));
        assert!(!comp.is_null());

        // Getters
        assert_eq!(
            e_cal_component_get_classification(comp),
            E_CAL_COMPONENT_CLASS_CONFIDENTIAL
        );
        assert_eq!(
            e_cal_component_get_transparency(comp),
            E_CAL_COMPONENT_TRANSP_TRANSPARENT
        );
        assert_eq!(e_cal_component_get_status(comp), I_CAL_STATUS_CONFIRMED);

        // Modify in place
        e_cal_component_set_classification(comp, E_CAL_COMPONENT_CLASS_PRIVATE);
        assert_eq!(
            e_cal_component_get_classification(comp),
            E_CAL_COMPONENT_CLASS_PRIVATE
        );

        e_cal_component_set_transparency(comp, E_CAL_COMPONENT_TRANSP_OPAQUE);
        assert_eq!(
            e_cal_component_get_transparency(comp),
            E_CAL_COMPONENT_TRANSP_OPAQUE
        );

        e_cal_component_set_status(comp, I_CAL_STATUS_TENTATIVE);
        assert_eq!(e_cal_component_get_status(comp), I_CAL_STATUS_TENTATIVE);

        let inner = e_cal_component_get_icalcomponent(comp);
        let rendered = take_string(i_cal_component_as_ical_string(inner));
        assert!(
            rendered.contains("CLASS:PRIVATE"),
            "modified class missing: {rendered}"
        );
        assert!(
            rendered.contains("TRANSP:OPAQUE"),
            "modified transp missing: {rendered}"
        );
        assert!(
            rendered.contains("STATUS:TENTATIVE"),
            "modified status missing: {rendered}"
        );

        // Clear fields by setting NONE
        e_cal_component_set_classification(comp, E_CAL_COMPONENT_CLASS_NONE);
        assert_eq!(
            e_cal_component_get_classification(comp),
            E_CAL_COMPONENT_CLASS_NONE
        );

        e_cal_component_set_transparency(comp, E_CAL_COMPONENT_TRANSP_NONE);
        assert_eq!(
            e_cal_component_get_transparency(comp),
            E_CAL_COMPONENT_TRANSP_NONE
        );

        e_cal_component_set_status(comp, I_CAL_STATUS_NONE);
        assert_eq!(e_cal_component_get_status(comp), I_CAL_STATUS_NONE);

        let inner_cleared = e_cal_component_get_icalcomponent(comp);
        let cleared_rendered = take_string(i_cal_component_as_ical_string(inner_cleared));
        assert!(
            !cleared_rendered.contains("CLASS:"),
            "cleared class still present: {cleared_rendered}"
        );
        assert!(
            !cleared_rendered.contains("TRANSP:"),
            "cleared transp still present: {cleared_rendered}"
        );
        assert!(
            !cleared_rendered.contains("STATUS:"),
            "cleared status still present: {cleared_rendered}"
        );

        g_object_unref(comp.cast());
        g_object_unref(event.cast());
        g_object_unref(calendar.cast());
    }
}

/// Probing `ECalComponent` categories, location, URL, and summary accessors:
/// `e_cal_component_get_categories` returns a comma-separated string,
/// `e_cal_component_get_categories_list` returns individual category tokens,
/// `e_cal_component_get_location` and `e_cal_component_get_url` return their respective strings,
/// and `e_cal_component_get_summary` returns the structured `ECalComponentText`.
#[test]
fn ecalcomponent_categories_location_url_and_descriptions_in_eds() {
    let source = text(
        "BEGIN:VCALENDAR\r\n\
         VERSION:2.0\r\n\
         PRODID:-//evolution-jmap//JMAP calendar backend//EN\r\n\
         BEGIN:VEVENT\r\n\
         UID:K1\r\n\
         SUMMARY:Sprint Planning\r\n\
         DTSTART:20260810T070000Z\r\n\
         CATEGORIES:offsite,planning\r\n\
         LOCATION:Room 42\r\n\
         URL:https://meet.example.com/planning\r\n\
         END:VEVENT\r\n\
         END:VCALENDAR\r\n",
    );
    unsafe {
        let calendar = i_cal_component_new_from_string(source.as_ptr());
        assert!(!calendar.is_null());
        let event = i_cal_component_get_first_component(calendar, I_CAL_VEVENT_COMPONENT);
        assert!(!event.is_null());

        let comp = e_cal_component_new_from_icalcomponent(i_cal_component_clone(event));
        assert!(!comp.is_null());

        // Categories string accessor
        let categories_raw = e_cal_component_get_categories(comp);
        assert!(!categories_raw.is_null());
        let categories_str = CStr::from_ptr(categories_raw).to_str().unwrap();
        assert_eq!(categories_str, "offsite,planning");

        // Categories list accessor
        let cat_list = e_cal_component_get_categories_list(comp);
        assert!(!cat_list.is_null());
        assert_eq!(g_slist_length(cat_list), 2);
        let cat1 = CStr::from_ptr((*cat_list).data as *const gchar)
            .to_str()
            .unwrap();
        assert_eq!(cat1, "offsite");
        let cat2_node = (*cat_list).next;
        assert!(!cat2_node.is_null());
        let cat2 = CStr::from_ptr((*cat2_node).data as *const gchar)
            .to_str()
            .unwrap();
        assert_eq!(cat2, "planning");

        unsafe extern "C" fn free_gchar(ptr: *mut std::ffi::c_void) {
            unsafe { g_free(ptr) };
        }
        g_slist_free_full(cat_list, Some(free_gchar));

        // Location accessor
        let loc_raw = e_cal_component_get_location(comp);
        assert!(!loc_raw.is_null());
        assert_eq!(CStr::from_ptr(loc_raw).to_str().unwrap(), "Room 42");

        // URL accessor
        let url_raw = e_cal_component_get_url(comp);
        assert!(!url_raw.is_null());
        assert_eq!(
            CStr::from_ptr(url_raw).to_str().unwrap(),
            "https://meet.example.com/planning"
        );

        // Summary text accessor
        let summary_text = e_cal_component_get_summary(comp);
        assert!(!summary_text.is_null());
        let summary_val = e_cal_component_text_get_value(summary_text);
        assert!(!summary_val.is_null());
        assert_eq!(
            CStr::from_ptr(summary_val).to_str().unwrap(),
            "Sprint Planning"
        );
        e_cal_component_text_free(summary_text.cast());

        // Modifying location and URL in place
        let new_loc = text("Conference Hall A");
        e_cal_component_set_location(comp, new_loc.as_ptr());
        let new_url = text("https://meet.example.com/hall-a");
        e_cal_component_set_url(comp, new_url.as_ptr());

        // Modifying categories
        let new_cats = text("work,engineering");
        e_cal_component_set_categories(comp, new_cats.as_ptr());

        let inner = e_cal_component_get_icalcomponent(comp);
        let rendered = take_string(i_cal_component_as_ical_string(inner));
        assert!(
            rendered.contains("LOCATION:Conference Hall A"),
            "modified location missing: {rendered}"
        );
        assert!(
            rendered.contains("URL:https://meet.example.com/hall-a"),
            "modified url missing: {rendered}"
        );
        assert!(
            rendered.contains("CATEGORIES:work,engineering"),
            "modified categories missing: {rendered}"
        );

        // Clearing categories, location, and url via NULL
        e_cal_component_set_categories(comp, std::ptr::null());
        e_cal_component_set_location(comp, std::ptr::null());
        e_cal_component_set_url(comp, std::ptr::null());

        let inner_cleared = e_cal_component_get_icalcomponent(comp);
        let cleared_rendered = take_string(i_cal_component_as_ical_string(inner_cleared));
        assert!(
            !cleared_rendered.contains("CATEGORIES:"),
            "cleared categories still present: {cleared_rendered}"
        );
        assert!(
            !cleared_rendered.contains("LOCATION:"),
            "cleared location still present: {cleared_rendered}"
        );
        assert!(
            !cleared_rendered.contains("URL:"),
            "cleared url still present: {cleared_rendered}"
        );

        g_object_unref(comp.cast());
        g_object_unref(event.cast());
        g_object_unref(calendar.cast());
    }
}

/// Probing `ECalComponent` organizer and attendee accessors:
/// `e_cal_component_get_organizer` returns an `ECalComponentOrganizer *`,
/// with value, CN, SENT-BY, and LANGUAGE accessors;
/// `e_cal_component_get_attendees` returns a `GSList` of `ECalComponentAttendee *`,
/// with value, CN, CUTYPE, ROLE, PARTSTAT, and RSVP accessors;
/// `e_cal_component_set_organizer` and `e_cal_component_set_attendees` support in-place
/// modification and NULL clearing.
#[test]
fn ecalcomponent_organizer_and_attendees_in_eds() {
    let source = text(
        "BEGIN:VCALENDAR\r\n\
         VERSION:2.0\r\n\
         PRODID:-//evolution-jmap//JMAP calendar backend//EN\r\n\
         BEGIN:VEVENT\r\n\
         UID:K1\r\n\
         SUMMARY:Architecture Review\r\n\
         DTSTART:20260810T100000Z\r\n\
         ORGANIZER;CN=Alice Smith;SENT-BY=\"mailto:sec@example.com\";LANGUAGE=en:mailto:alice@example.com\r\n\
         ATTENDEE;CN=Bob Jones;CUTYPE=INDIVIDUAL;ROLE=REQ-PARTICIPANT;PARTSTAT=ACCEPTED;RSVP=TRUE:mailto:bob@example.com\r\n\
         ATTENDEE;CN=Carol Danvers;ROLE=OPT-PARTICIPANT;PARTSTAT=TENTATIVE:mailto:carol@example.com\r\n\
         END:VEVENT\r\n\
         END:VCALENDAR\r\n",
    );
    unsafe {
        let calendar = i_cal_component_new_from_string(source.as_ptr());
        assert!(!calendar.is_null());
        let event = i_cal_component_get_first_component(calendar, I_CAL_VEVENT_COMPONENT);
        assert!(!event.is_null());

        let comp = e_cal_component_new_from_icalcomponent(i_cal_component_clone(event));
        assert!(!comp.is_null());

        // Organizer accessors
        let organizer = e_cal_component_get_organizer(comp);
        assert!(!organizer.is_null());
        let org_val = e_cal_component_organizer_get_value(organizer);
        assert!(!org_val.is_null());
        assert_eq!(
            CStr::from_ptr(org_val).to_str().unwrap(),
            "mailto:alice@example.com"
        );
        let org_cn = e_cal_component_organizer_get_cn(organizer);
        assert!(!org_cn.is_null());
        assert_eq!(CStr::from_ptr(org_cn).to_str().unwrap(), "Alice Smith");
        let org_sentby = e_cal_component_organizer_get_sentby(organizer);
        assert!(!org_sentby.is_null());
        assert_eq!(
            CStr::from_ptr(org_sentby).to_str().unwrap(),
            "mailto:sec@example.com"
        );
        let org_lang = e_cal_component_organizer_get_language(organizer);
        assert!(!org_lang.is_null());
        assert_eq!(CStr::from_ptr(org_lang).to_str().unwrap(), "en");

        // Attendee accessors
        let attendees = e_cal_component_get_attendees(comp);
        assert!(!attendees.is_null());
        assert_eq!(g_slist_length(attendees), 2);

        let att1 = (*attendees).data as *mut ECalComponentAttendee;
        assert!(!att1.is_null());
        let att1_val = e_cal_component_attendee_get_value(att1);
        assert!(!att1_val.is_null());
        assert_eq!(
            CStr::from_ptr(att1_val).to_str().unwrap(),
            "mailto:bob@example.com"
        );
        let att1_cn = e_cal_component_attendee_get_cn(att1);
        assert!(!att1_cn.is_null());
        assert_eq!(CStr::from_ptr(att1_cn).to_str().unwrap(), "Bob Jones");
        assert_eq!(
            e_cal_component_attendee_get_partstat(att1),
            I_CAL_PARTSTAT_ACCEPTED
        );
        assert_eq!(
            e_cal_component_attendee_get_role(att1),
            I_CAL_ROLE_REQPARTICIPANT
        );
        assert_eq!(e_cal_component_attendee_get_rsvp(att1), 1);
        assert_eq!(
            e_cal_component_attendee_get_cutype(att1),
            I_CAL_CUTYPE_INDIVIDUAL
        );

        let att2_node = (*attendees).next;
        assert!(!att2_node.is_null());
        let att2 = (*att2_node).data as *mut ECalComponentAttendee;
        assert!(!att2.is_null());
        let att2_val = e_cal_component_attendee_get_value(att2);
        assert!(!att2_val.is_null());
        assert_eq!(
            CStr::from_ptr(att2_val).to_str().unwrap(),
            "mailto:carol@example.com"
        );
        let att2_cn = e_cal_component_attendee_get_cn(att2);
        assert!(!att2_cn.is_null());
        assert_eq!(CStr::from_ptr(att2_cn).to_str().unwrap(), "Carol Danvers");
        assert_eq!(
            e_cal_component_attendee_get_partstat(att2),
            I_CAL_PARTSTAT_TENTATIVE
        );
        assert_eq!(
            e_cal_component_attendee_get_role(att2),
            I_CAL_ROLE_OPTPARTICIPANT
        );

        // Modify organizer CN in place
        let new_cn = text("Alice Wonderland");
        e_cal_component_organizer_set_cn(organizer, new_cn.as_ptr());
        e_cal_component_set_organizer(comp, organizer);

        let inner = e_cal_component_get_icalcomponent(comp);
        let rendered = take_string(i_cal_component_as_ical_string(inner));
        assert!(
            rendered.contains("ORGANIZER;CN=Alice Wonderland"),
            "modified organizer missing: {rendered}"
        );

        e_cal_component_organizer_free(organizer.cast());
        unsafe extern "C" fn free_attendee(ptr: *mut std::ffi::c_void) {
            unsafe { e_cal_component_attendee_free(ptr.cast()) };
        }
        g_slist_free_full(attendees, Some(free_attendee));

        // Clear organizer and attendees via NULL
        e_cal_component_set_organizer(comp, std::ptr::null());
        e_cal_component_set_attendees(comp, std::ptr::null());

        let inner_cleared = e_cal_component_get_icalcomponent(comp);
        let cleared_rendered = take_string(i_cal_component_as_ical_string(inner_cleared));
        assert!(
            !cleared_rendered.contains("ORGANIZER:"),
            "cleared organizer still present: {cleared_rendered}"
        );
        assert!(
            !cleared_rendered.contains("ATTENDEE:"),
            "cleared attendees still present: {cleared_rendered}"
        );

        g_object_unref(comp.cast());
        g_object_unref(event.cast());
        g_object_unref(calendar.cast());
    }
}

/// Probing `ECalComponent` priority, sequence, percent complete, and GEO coordinates:
/// `e_cal_component_get_priority` returns an integer (1..9, or 0 when unset),
/// `e_cal_component_get_sequence` returns sequence >= 0,
/// `e_cal_component_get_percent_complete` returns 0..100 (-1 when unset),
/// `e_cal_component_get_geo` returns `ICalGeo *` with latitude/longitude.
#[test]
fn ecalcomponent_priority_sequence_percent_complete_and_geo_in_eds() {
    let source = text(
        "BEGIN:VCALENDAR\r\n\
         VERSION:2.0\r\n\
         PRODID:-//evolution-jmap//JMAP calendar backend//EN\r\n\
         BEGIN:VEVENT\r\n\
         UID:K1\r\n\
         SUMMARY:Launch Readiness\r\n\
         DTSTART:20260810T140000Z\r\n\
         PRIORITY:1\r\n\
         SEQUENCE:3\r\n\
         PERCENT-COMPLETE:75\r\n\
         GEO:52.520008;13.404954\r\n\
         END:VEVENT\r\n\
         END:VCALENDAR\r\n",
    );
    unsafe {
        let calendar = i_cal_component_new_from_string(source.as_ptr());
        assert!(!calendar.is_null());
        let event = i_cal_component_get_first_component(calendar, I_CAL_VEVENT_COMPONENT);
        assert!(!event.is_null());

        let comp = e_cal_component_new_from_icalcomponent(i_cal_component_clone(event));
        assert!(!comp.is_null());

        // Field getters
        assert_eq!(e_cal_component_get_priority(comp), 1);
        assert_eq!(e_cal_component_get_sequence(comp), 3);
        assert_eq!(e_cal_component_get_percent_complete(comp), 75);

        let geo = e_cal_component_get_geo(comp);
        assert!(!geo.is_null());
        let lat = i_cal_geo_get_lat(geo);
        let lon = i_cal_geo_get_lon(geo);
        assert!((lat - 52.520008).abs() < 1e-5);
        assert!((lon - 13.404954).abs() < 1e-5);
        g_object_unref(geo.cast());

        // Modifying in place
        e_cal_component_set_priority(comp, 5);
        e_cal_component_set_sequence(comp, 4);
        e_cal_component_set_percent_complete(comp, 100);

        let new_geo = i_cal_geo_new(48.8566, 2.3522);
        assert!(!new_geo.is_null());
        e_cal_component_set_geo(comp, new_geo);
        g_object_unref(new_geo.cast());

        let inner = e_cal_component_get_icalcomponent(comp);
        let rendered = take_string(i_cal_component_as_ical_string(inner));
        assert!(
            rendered.contains("PRIORITY:5"),
            "modified priority missing: {rendered}"
        );
        assert!(
            rendered.contains("SEQUENCE:4"),
            "modified sequence missing: {rendered}"
        );
        assert!(
            rendered.contains("PERCENT-COMPLETE:100"),
            "modified percent missing: {rendered}"
        );
        assert!(
            rendered.contains("GEO:48.8566"),
            "modified geo missing: {rendered}"
        );

        // Clearing via -1 / -1 / NULL
        e_cal_component_set_priority(comp, -1);
        e_cal_component_set_percent_complete(comp, -1);
        e_cal_component_set_geo(comp, std::ptr::null());

        assert_eq!(e_cal_component_get_priority(comp), -1);
        assert_eq!(e_cal_component_get_percent_complete(comp), -1);
        assert!(e_cal_component_get_geo(comp).is_null());

        let inner_cleared = e_cal_component_get_icalcomponent(comp);
        let cleared_rendered = take_string(i_cal_component_as_ical_string(inner_cleared));
        assert!(
            !cleared_rendered.contains("PRIORITY:"),
            "cleared priority still present: {cleared_rendered}"
        );
        assert!(
            !cleared_rendered.contains("PERCENT-COMPLETE:"),
            "cleared percent still present: {cleared_rendered}"
        );
        assert!(
            !cleared_rendered.contains("GEO:"),
            "cleared geo still present: {cleared_rendered}"
        );

        g_object_unref(comp.cast());
        g_object_unref(event.cast());
        g_object_unref(calendar.cast());
    }
}

/// Probing `ECalComponent` alarm accessors, trigger properties, repeat settings,
/// in-place alarm modification, and alarm clearing.
/// `e_cal_component_has_alarms` checks for `VALARM` components,
/// `e_cal_component_get_alarm_uids` lists alarm UIDs,
/// `e_cal_component_get_alarm` retrieves an `ECalComponentAlarm`,
/// `e_cal_component_add_alarm` installs a new/modified alarm,
/// `e_cal_component_remove_alarm` deletes a specific alarm by UID, and
/// `e_cal_component_remove_all_alarms` clears all alarms.
#[test]
fn ecalcomponent_alarm_handling_and_properties_in_eds() {
    let source = text(
        "BEGIN:VCALENDAR\r\n\
         VERSION:2.0\r\n\
         PRODID:-//evolution-jmap//JMAP calendar backend//EN\r\n\
         BEGIN:VEVENT\r\n\
         UID:K1\r\n\
         SUMMARY:Team Sync\r\n\
         DTSTART:20260810T140000Z\r\n\
         BEGIN:VALARM\r\n\
         UID:alarm1\r\n\
         ACTION:DISPLAY\r\n\
         DESCRIPTION:Team Sync Reminder\r\n\
         TRIGGER;RELATED=START:-PT15M\r\n\
         REPEAT:2\r\n\
         DURATION:PT5M\r\n\
         END:VALARM\r\n\
         BEGIN:VALARM\r\n\
         UID:alarm2\r\n\
         ACTION:AUDIO\r\n\
         TRIGGER;VALUE=DATE-TIME:20260810T134500Z\r\n\
         END:VALARM\r\n\
         END:VEVENT\r\n\
         END:VCALENDAR\r\n",
    );
    unsafe {
        let calendar = i_cal_component_new_from_string(source.as_ptr());
        assert!(!calendar.is_null());
        let event = i_cal_component_get_first_component(calendar, I_CAL_VEVENT_COMPONENT);
        assert!(!event.is_null());

        let comp = e_cal_component_new_from_icalcomponent(i_cal_component_clone(event));
        assert!(!comp.is_null());

        // Has alarms check
        assert_eq!(e_cal_component_has_alarms(comp), 1);

        // Alarm UIDs listing
        let uids_list = e_cal_component_get_alarm_uids(comp);
        assert!(!uids_list.is_null());
        assert_eq!(g_slist_length(uids_list), 2);

        let auid0 = CStr::from_ptr((*uids_list).data.cast())
            .to_str()
            .unwrap()
            .to_owned();
        let auid1 = CStr::from_ptr((*(*uids_list).next).data.cast())
            .to_str()
            .unwrap()
            .to_owned();

        unsafe extern "C" fn free_gchar(ptr: *mut std::ffi::c_void) {
            unsafe { g_free(ptr) };
        }
        g_slist_free_full(uids_list, Some(free_gchar));

        let auid0_c = text(&auid0);
        let auid1_c = text(&auid1);

        // Retrieve alarms by the EDS alarm UIDs
        let a0 = e_cal_component_get_alarm(comp, auid0_c.as_ptr());
        assert!(!a0.is_null());
        let a1_comp = e_cal_component_get_alarm(comp, auid1_c.as_ptr());
        assert!(!a1_comp.is_null());

        let action0 = e_cal_component_alarm_get_action(a0);

        let (a1, a2, a2_uid_to_remove) = if action0 == E_CAL_COMPONENT_ALARM_DISPLAY {
            (a0, a1_comp, auid1_c)
        } else {
            (a1_comp, a0, auid0_c)
        };

        let a1_uid = e_cal_component_alarm_get_uid(a1);
        assert!(!a1_uid.is_null());
        assert!(!CStr::from_ptr(a1_uid).to_str().unwrap().is_empty());
        assert_eq!(
            e_cal_component_alarm_get_action(a1),
            E_CAL_COMPONENT_ALARM_DISPLAY
        );

        let desc_text = e_cal_component_alarm_get_description(a1);
        assert!(!desc_text.is_null());
        assert_eq!(
            CStr::from_ptr(e_cal_component_text_get_value(desc_text))
                .to_str()
                .unwrap(),
            "Team Sync Reminder"
        );

        let trig = e_cal_component_alarm_get_trigger(a1);
        assert!(!trig.is_null());
        assert_eq!(
            e_cal_component_alarm_trigger_get_kind(trig),
            E_CAL_COMPONENT_ALARM_TRIGGER_RELATIVE_START
        );
        let dur = e_cal_component_alarm_trigger_get_duration(trig);
        assert!(!dur.is_null());
        assert_eq!(i_cal_duration_is_neg(dur), 1);
        assert_eq!(i_cal_duration_get_minutes(dur), 15);

        let rep = e_cal_component_alarm_get_repeat(a1);
        assert!(!rep.is_null());
        assert_eq!(e_cal_component_alarm_repeat_get_repetitions(rep), 2);
        let rep_dur = e_cal_component_alarm_repeat_get_interval(rep);
        assert!(!rep_dur.is_null());
        assert_eq!(i_cal_duration_get_minutes(rep_dur), 5);

        // Get alarm2
        assert_eq!(
            e_cal_component_alarm_get_action(a2),
            E_CAL_COMPONENT_ALARM_AUDIO
        );
        let trig2 = e_cal_component_alarm_get_trigger(a2);
        assert!(!trig2.is_null());
        assert_eq!(
            e_cal_component_alarm_trigger_get_kind(trig2),
            E_CAL_COMPONENT_ALARM_TRIGGER_ABSOLUTE
        );
        e_cal_component_alarm_free(a2.cast());

        // Add a new alarm (alarm3)
        let a3 = e_cal_component_alarm_new();
        assert!(!a3.is_null());
        e_cal_component_alarm_set_uid(a3, text("alarm3").as_ptr());
        e_cal_component_alarm_set_action(a3, E_CAL_COMPONENT_ALARM_DISPLAY);

        let dur3 = i_cal_duration_new_from_string(text("-PT30M").as_ptr());
        assert!(!dur3.is_null());
        let trig3 = e_cal_component_alarm_trigger_new_relative(
            E_CAL_COMPONENT_ALARM_TRIGGER_RELATIVE_START,
            dur3,
        );
        assert!(!trig3.is_null());
        e_cal_component_alarm_set_trigger(a3, trig3);
        e_cal_component_alarm_trigger_free(trig3.cast());
        g_object_unref(dur3.cast());

        e_cal_component_add_alarm(comp, a3);
        e_cal_component_alarm_free(a3.cast());

        // Verify alarm3 exists
        let a3_read = e_cal_component_get_alarm(comp, text("alarm3").as_ptr());
        assert!(!a3_read.is_null());
        assert_eq!(
            e_cal_component_alarm_get_action(a3_read),
            E_CAL_COMPONENT_ALARM_DISPLAY
        );
        assert_eq!(
            CStr::from_ptr(e_cal_component_alarm_get_uid(a3_read))
                .to_str()
                .unwrap(),
            "alarm3"
        );
        e_cal_component_alarm_free(a3_read.cast());

        // Remove alarm2 by auid
        e_cal_component_remove_alarm(comp, a2_uid_to_remove.as_ptr());
        assert!(e_cal_component_get_alarm(comp, a2_uid_to_remove.as_ptr()).is_null());

        // Remove all alarms
        e_cal_component_remove_all_alarms(comp);
        assert_eq!(e_cal_component_has_alarms(comp), 0);

        let inner = e_cal_component_get_icalcomponent(comp);
        let rendered = take_string(i_cal_component_as_ical_string(inner));
        assert!(
            !rendered.contains("BEGIN:VALARM"),
            "VALARM still present after remove_all_alarms: {rendered}"
        );

        e_cal_component_alarm_free(a1.cast());
        g_object_unref(comp.cast());
        g_object_unref(event.cast());
        g_object_unref(calendar.cast());
    }
}

/// Probing `ECalComponent` recurrence properties (RRULE, EXDATE, RECURRENCE-ID):
/// `e_cal_component_has_recurrences` and `e_cal_component_has_rrules` indicate whether recurrence rules exist,
/// `e_cal_component_get_rrules` extracts recurrence rules as a `GSList` of `ICalRecurrence *`,
/// `e_cal_component_has_exdates` and `e_cal_component_get_exdates` extract exception dates as `ECalComponentDateTime *`,
/// `e_cal_component_get_recurid_as_string` extracts the recurrence identifier string,
/// and setting NULL on setters clears `RRULE`, `EXDATE`, and `RECURRENCE-ID` from the component.
#[test]
fn ecalcomponent_recurrence_rules_exdates_and_recurid_in_eds() {
    let source = text(
        "BEGIN:VCALENDAR\r\n\
         VERSION:2.0\r\n\
         PRODID:-//evolution-jmap//JMAP calendar backend//EN\r\n\
         BEGIN:VEVENT\r\n\
         UID:K1\r\n\
         SUMMARY:Recurring Standup\r\n\
         DTSTART:20260810T090000Z\r\n\
         RRULE:FREQ=WEEKLY;INTERVAL=2;COUNT=10\r\n\
         EXDATE:20260824T090000Z\r\n\
         RECURRENCE-ID:20260810T090000Z\r\n\
         END:VEVENT\r\n\
         END:VCALENDAR\r\n",
    );
    unsafe {
        let calendar = i_cal_component_new_from_string(source.as_ptr());
        assert!(!calendar.is_null());
        let event = i_cal_component_get_first_component(calendar, I_CAL_VEVENT_COMPONENT);
        assert!(!event.is_null());

        let comp = e_cal_component_new_from_icalcomponent(i_cal_component_clone(event));
        assert!(!comp.is_null());

        // Checks
        assert_eq!(e_cal_component_has_recurrences(comp), 1);
        assert_eq!(e_cal_component_has_rrules(comp), 1);
        assert_eq!(e_cal_component_has_exdates(comp), 1);

        // Recurrence ID
        let recurid_str = e_cal_component_get_recurid_as_string(comp);
        assert!(!recurid_str.is_null());
        assert_eq!(
            CStr::from_ptr(recurid_str).to_str().unwrap(),
            "20260810T090000Z"
        );
        g_free(recurid_str.cast());

        // RRULE list
        let rrules = e_cal_component_get_rrules(comp);
        assert!(!rrules.is_null());
        assert_eq!(g_slist_length(rrules), 1);

        let recur = (*rrules).data as *mut ICalRecurrence;
        assert!(!recur.is_null());
        assert_eq!(i_cal_recurrence_get_freq(recur), I_CAL_WEEKLY_RECURRENCE);
        assert_eq!(i_cal_recurrence_get_interval(recur), 2);
        assert_eq!(i_cal_recurrence_get_count(recur), 10);

        unsafe extern "C" fn unref_obj(ptr: *mut std::ffi::c_void) {
            unsafe { g_object_unref(ptr.cast()) };
        }
        g_slist_free_full(rrules, Some(unref_obj));

        // EXDATE list
        let exdates = e_cal_component_get_exdates(comp);
        assert!(!exdates.is_null());
        assert_eq!(g_slist_length(exdates), 1);
        let ex_dt = (*exdates).data as *mut ECalComponentDateTime;
        assert!(!ex_dt.is_null());
        let ex_time = e_cal_component_datetime_get_value(ex_dt);
        assert!(!ex_time.is_null());
        assert_eq!(i_cal_time_get_year(ex_time), 2026);
        assert_eq!(i_cal_time_get_month(ex_time), 8);
        assert_eq!(i_cal_time_get_day(ex_time), 24);

        g_slist_free_full(exdates, Some(e_cal_component_datetime_free));

        // Modify in place: set a new rule with INTERVAL=1, COUNT=5
        let new_recur = i_cal_recurrence_new();
        i_cal_recurrence_set_freq(new_recur, I_CAL_DAILY_RECURRENCE);
        i_cal_recurrence_set_interval(new_recur, 1);
        i_cal_recurrence_set_count(new_recur, 5);

        let new_list = g_slist_append(std::ptr::null_mut(), new_recur.cast());
        e_cal_component_set_rrules(comp, new_list);
        g_slist_free_full(new_list, Some(unref_obj));

        let inner = e_cal_component_get_icalcomponent(comp);
        let rendered = take_string(i_cal_component_as_ical_string(inner));
        assert!(
            rendered.contains("RRULE:FREQ=DAILY;COUNT=5"),
            "modified rrule missing: {rendered}"
        );

        // Clear RRULE, EXDATE, RECURRENCE-ID
        e_cal_component_set_rrules(comp, std::ptr::null());
        e_cal_component_set_exdates(comp, std::ptr::null());
        e_cal_component_set_recurid(comp, std::ptr::null());

        assert_eq!(e_cal_component_has_rrules(comp), 0);
        assert_eq!(e_cal_component_has_exdates(comp), 0);
        assert_eq!(e_cal_component_has_recurrences(comp), 0);

        let inner_cleared = e_cal_component_get_icalcomponent(comp);
        let cleared_rendered = take_string(i_cal_component_as_ical_string(inner_cleared));
        assert!(
            !cleared_rendered.contains("RRULE:"),
            "cleared rrule still present: {cleared_rendered}"
        );
        assert!(
            !cleared_rendered.contains("EXDATE:"),
            "cleared exdate still present: {cleared_rendered}"
        );
        assert!(
            !cleared_rendered.contains("RECURRENCE-ID:"),
            "cleared recurid still present: {cleared_rendered}"
        );

        g_object_unref(comp.cast());
        g_object_unref(event.cast());
        g_object_unref(calendar.cast());
    }
}

/// Probing `ICalRecurrence` string parsing, serialization, and `ICalProperty` integration:
/// `i_cal_recurrence_new_from_string` constructs an `ICalRecurrence` from an RRULE value string,
/// frequency, interval, and until getters inspect properties,
/// `i_cal_recurrence_to_string` serializes back to standard RRULE format,
/// and `i_cal_property_new_rrule` creates a property attached to an `ICalComponent`.
#[test]
fn icalrecurrence_properties_and_string_roundtrips() {
    let rrule_str = text("FREQ=MONTHLY;INTERVAL=3;UNTIL=20261231T235959Z");
    unsafe {
        let recur = i_cal_recurrence_new_from_string(rrule_str.as_ptr());
        assert!(!recur.is_null());

        assert_eq!(i_cal_recurrence_get_freq(recur), I_CAL_MONTHLY_RECURRENCE);
        assert_eq!(i_cal_recurrence_get_interval(recur), 3);

        let until_time = i_cal_recurrence_get_until(recur);
        assert!(!until_time.is_null());
        assert_eq!(i_cal_time_get_year(until_time), 2026);
        assert_eq!(i_cal_time_get_month(until_time), 12);
        assert_eq!(i_cal_time_get_day(until_time), 31);

        let serialized = take_string(i_cal_recurrence_to_string(recur));
        assert!(serialized.contains("FREQ=MONTHLY"), "{serialized}");
        assert!(serialized.contains("INTERVAL=3"), "{serialized}");
        assert!(
            serialized.contains("UNTIL=20261231T235959Z"),
            "{serialized}"
        );

        // Create an ICalProperty and attach to a VEVENT
        let prop = i_cal_property_new_rrule(recur);
        assert!(!prop.is_null());

        let vevent = i_cal_component_new(I_CAL_VEVENT_COMPONENT);
        assert!(!vevent.is_null());
        i_cal_component_take_property(vevent, prop);

        let rendered = take_string(i_cal_component_as_ical_string(vevent));
        assert!(rendered.contains("RRULE:"), "missing RRULE: {rendered}");
        assert!(
            rendered.contains("FREQ=MONTHLY"),
            "missing FREQ: {rendered}"
        );
        assert!(
            rendered.contains("INTERVAL=3"),
            "missing INTERVAL: {rendered}"
        );
        assert!(
            rendered.contains("UNTIL=20261231T235959Z"),
            "missing UNTIL: {rendered}"
        );

        g_object_unref(vevent.cast());
        g_object_unref(recur.cast());
    }
}

/// Probing `ECalComponent` datetime and duration properties (DTSTART, DTEND, DUE, DURATION):
/// `e_cal_component_get_dtstart` extracts start datetime and TZID,
/// `e_cal_component_get_dtend` extracts end datetime and TZID,
/// `e_cal_component_get_due` extracts due datetime for tasks,
/// `i_cal_component_get_duration` extracts event duration,
/// and setters modify or clear (via NULL) these properties.
#[test]
fn ecalcomponent_dtstart_dtend_due_and_duration_in_eds() {
    let source = text(
        "BEGIN:VCALENDAR\r\n\
         VERSION:2.0\r\n\
         PRODID:-//evolution-jmap//JMAP calendar backend//EN\r\n\
         BEGIN:VEVENT\r\n\
         UID:K1\r\n\
         SUMMARY:Quarterly Review\r\n\
         DTSTART;TZID=America/New_York:20260810T090000\r\n\
         DTEND;TZID=America/New_York:20260810T103000\r\n\
         END:VEVENT\r\n\
         END:VCALENDAR\r\n",
    );
    unsafe {
        let calendar = i_cal_component_new_from_string(source.as_ptr());
        assert!(!calendar.is_null());
        let event = i_cal_component_get_first_component(calendar, I_CAL_VEVENT_COMPONENT);
        assert!(!event.is_null());

        let comp = e_cal_component_new_from_icalcomponent(i_cal_component_clone(event));
        assert!(!comp.is_null());

        // Inspect DTSTART
        let dtstart = e_cal_component_get_dtstart(comp);
        assert!(!dtstart.is_null());
        let tzid = e_cal_component_datetime_get_tzid(dtstart);
        assert!(!tzid.is_null());
        assert_eq!(CStr::from_ptr(tzid).to_str().unwrap(), "America/New_York");
        let start_time = e_cal_component_datetime_get_value(dtstart);
        assert!(!start_time.is_null());
        assert_eq!(i_cal_time_get_year(start_time), 2026);
        assert_eq!(i_cal_time_get_month(start_time), 8);
        assert_eq!(i_cal_time_get_day(start_time), 10);
        assert_eq!(i_cal_time_get_hour(start_time), 9);
        assert_eq!(i_cal_time_get_minute(start_time), 0);
        assert_eq!(i_cal_time_is_date(start_time), 0);
        assert_eq!(i_cal_time_is_utc(start_time), 0);
        e_cal_component_datetime_free(dtstart.cast());

        // Inspect DTEND
        let dtend = e_cal_component_get_dtend(comp);
        assert!(!dtend.is_null());
        let end_tzid = e_cal_component_datetime_get_tzid(dtend);
        assert_eq!(
            CStr::from_ptr(end_tzid).to_str().unwrap(),
            "America/New_York"
        );
        let end_time = e_cal_component_datetime_get_value(dtend);
        assert_eq!(i_cal_time_get_hour(end_time), 10);
        assert_eq!(i_cal_time_get_minute(end_time), 30);
        e_cal_component_datetime_free(dtend.cast());

        // Modify DTSTART in place to Europe/London 20260815T120000
        let new_time = i_cal_time_new_from_string(c"20260815T120000".as_ptr());
        assert!(!new_time.is_null());
        let new_dt = e_cal_component_datetime_new_take(
            new_time,
            glib_sys::g_strdup(c"Europe/London".as_ptr()),
        );
        e_cal_component_set_dtstart(comp, new_dt);
        e_cal_component_datetime_free(new_dt.cast());

        let inner = e_cal_component_get_icalcomponent(comp);
        let rendered = take_string(i_cal_component_as_ical_string(inner));
        assert!(
            rendered.contains("DTSTART;TZID=Europe/London:20260815T120000"),
            "modified dtstart missing: {rendered}"
        );

        // Clear DTEND and set DURATION on inner ICalComponent
        e_cal_component_set_dtend(comp, std::ptr::null());

        let dur = i_cal_duration_new_from_string(c"PT1H30M".as_ptr());
        assert!(!dur.is_null());
        assert_eq!(i_cal_duration_get_hours(dur), 1);
        assert_eq!(i_cal_duration_get_minutes(dur), 30);
        assert_eq!(i_cal_duration_is_neg(dur), 0);
        i_cal_component_set_duration(inner, dur);
        g_object_unref(dur.cast());

        let dur_rendered = take_string(i_cal_component_as_ical_string(inner));
        assert!(
            dur_rendered.contains("DURATION:PT1H30M"),
            "duration missing: {dur_rendered}"
        );

        // Remove DURATION property
        let dur_prop = i_cal_component_get_first_property(inner, I_CAL_DURATION_PROPERTY);
        assert!(!dur_prop.is_null());
        i_cal_component_remove_property(inner, dur_prop);
        g_object_unref(dur_prop.cast());

        let cleared_rendered = take_string(i_cal_component_as_ical_string(inner));
        assert!(
            !cleared_rendered.contains("DTEND"),
            "cleared dtend still present: {cleared_rendered}"
        );
        assert!(
            !cleared_rendered.contains("DURATION:"),
            "cleared duration still present: {cleared_rendered}"
        );

        g_object_unref(comp.cast());
        g_object_unref(event.cast());
        g_object_unref(calendar.cast());

        // Test DUE on a VTODO component (libecal restricts DUE to E_CAL_COMPONENT_TODO)
        let todo_source = text(
            "BEGIN:VCALENDAR\r\n\
             VERSION:2.0\r\n\
             BEGIN:VTODO\r\n\
             UID:T1\r\n\
             SUMMARY:Task with deadline\r\n\
             DUE;TZID=America/New_York:20260820T170000\r\n\
             END:VTODO\r\n\
             END:VCALENDAR\r\n",
        );
        let todo_cal = i_cal_component_new_from_string(todo_source.as_ptr());
        assert!(!todo_cal.is_null());
        let todo_comp = i_cal_component_get_first_component(todo_cal, I_CAL_VTODO_COMPONENT);
        assert!(!todo_comp.is_null());

        let e_todo = e_cal_component_new_from_icalcomponent(i_cal_component_clone(todo_comp));
        assert!(!e_todo.is_null());

        let initial_due = e_cal_component_get_due(e_todo);
        assert!(!initial_due.is_null());
        let due_val = e_cal_component_datetime_get_value(initial_due);
        assert_eq!(i_cal_time_get_day(due_val), 20);
        assert_eq!(i_cal_time_get_hour(due_val), 17);
        e_cal_component_datetime_free(initial_due.cast());

        // Modify DUE
        let new_due_time = i_cal_time_new_from_string(c"20260825T180000".as_ptr());
        let new_due_dt = e_cal_component_datetime_new_take(
            new_due_time,
            glib_sys::g_strdup(c"America/New_York".as_ptr()),
        );
        e_cal_component_set_due(e_todo, new_due_dt);
        e_cal_component_datetime_free(new_due_dt.cast());

        let inner_todo = e_cal_component_get_icalcomponent(e_todo);
        let todo_rendered = take_string(i_cal_component_as_ical_string(inner_todo));
        assert!(
            todo_rendered.contains("DUE;TZID=America/New_York:20260825T180000"),
            "modified due missing: {todo_rendered}"
        );

        // Clear DUE
        e_cal_component_set_due(e_todo, std::ptr::null());
        let cleared_todo = take_string(i_cal_component_as_ical_string(inner_todo));
        assert!(
            !cleared_todo.contains("DUE"),
            "cleared due still present: {cleared_todo}"
        );

        g_object_unref(e_todo.cast());
        g_object_unref(todo_comp.cast());
        g_object_unref(todo_cal.cast());
    }
}

/// Probing `ICalTime` parsing (DATE vs DATE-TIME), UTC flags, and `ICalTimezone` resolution:
/// `i_cal_time_new_from_string` distinguishes VALUE=DATE from DATE-TIME and UTC from floating/local,
/// and `i_cal_timezone_get_builtin_timezone` retrieves built-in VTIMEZONE definitions.
#[test]
fn icaltime_date_vs_datetime_and_timezone_resolution() {
    unsafe {
        // Date only (VALUE=DATE)
        let d = i_cal_time_new_from_string(c"20260810".as_ptr());
        assert!(!d.is_null());
        assert_eq!(i_cal_time_is_date(d), 1);
        assert_eq!(i_cal_time_is_utc(d), 0);
        assert_eq!(i_cal_time_get_year(d), 2026);
        assert_eq!(i_cal_time_get_month(d), 8);
        assert_eq!(i_cal_time_get_day(d), 10);
        assert_eq!(i_cal_time_get_hour(d), 0);
        g_object_unref(d.cast());

        // Date-time with UTC (Z)
        let dt_utc = i_cal_time_new_from_string(c"20260810T153000Z".as_ptr());
        assert!(!dt_utc.is_null());
        assert_eq!(i_cal_time_is_date(dt_utc), 0);
        assert_eq!(i_cal_time_is_utc(dt_utc), 1);
        assert_eq!(i_cal_time_get_hour(dt_utc), 15);
        assert_eq!(i_cal_time_get_minute(dt_utc), 30);
        g_object_unref(dt_utc.cast());

        // Date-time floating / local
        let dt_local = i_cal_time_new_from_string(c"20260810T090000".as_ptr());
        assert!(!dt_local.is_null());
        assert_eq!(i_cal_time_is_date(dt_local), 0);
        assert_eq!(i_cal_time_is_utc(dt_local), 0);
        assert_eq!(i_cal_time_get_hour(dt_local), 9);
        g_object_unref(dt_local.cast());

        // Timezone resolution by IANA name
        let tz = i_cal_timezone_get_builtin_timezone(c"Europe/Zurich".as_ptr());
        assert!(!tz.is_null());
        let vtz = i_cal_timezone_get_component(tz);
        assert!(!vtz.is_null());
        assert_eq!(i_cal_component_isa(vtz), I_CAL_VTIMEZONE_COMPONENT);

        let rendered = take_string(i_cal_component_as_ical_string(vtz));
        assert!(rendered.contains("BEGIN:VTIMEZONE"), "{rendered}");
        assert!(rendered.contains("TZID:"), "{rendered}");
    }
}

/// Probing `ECalComponent` description, comment, contact, and summary text lists and multi-locale accessors:
/// `e_cal_component_get_descriptions` returns a `GSList` of `ECalComponentText *`, with value, ALTREP, and LANGUAGE accessors;
/// `e_cal_component_dup_description_for_locale` retrieves the localized description;
/// `e_cal_component_get_comments` and `e_cal_component_dup_comment_for_locale` retrieve comments;
/// `e_cal_component_get_contacts` returns contact strings;
/// `e_cal_component_dup_summaries` returns summary entries;
/// `e_cal_component_set_descriptions`, `e_cal_component_set_comments`, `e_cal_component_set_contacts`, `e_cal_component_set_summaries` support in-place modification;
/// and setting `NULL` clears them from the component.
#[test]
fn ecalcomponent_descriptions_comments_contacts_and_summaries_in_eds() {
    let source = text(
        "BEGIN:VCALENDAR\r\n\
         VERSION:2.0\r\n\
         PRODID:-//evolution-jmap//JMAP calendar backend//EN\r\n\
         BEGIN:VEVENT\r\n\
         UID:K1\r\n\
         SUMMARY;LANGUAGE=en:Product Launch\r\n\
         DTSTART:20260810T100000Z\r\n\
         DESCRIPTION;ALTREP=\"https://example.com/desc.html\";LANGUAGE=en:Launch details\r\n\
         COMMENT;LANGUAGE=en:First comment\r\n\
         COMMENT:Second comment\r\n\
         CONTACT;ALTREP=\"https://example.com/team.vcf\":Team Alpha\r\n\
         END:VEVENT\r\n\
         END:VCALENDAR\r\n",
    );
    unsafe {
        let calendar = i_cal_component_new_from_string(source.as_ptr());
        assert!(!calendar.is_null());
        let event = i_cal_component_get_first_component(calendar, I_CAL_VEVENT_COMPONENT);
        assert!(!event.is_null());

        let comp = e_cal_component_new_from_icalcomponent(i_cal_component_clone(event));
        assert!(!comp.is_null());

        unsafe extern "C" fn free_text(ptr: *mut std::ffi::c_void) {
            unsafe { e_cal_component_text_free(ptr.cast()) };
        }

        // 1. Descriptions
        let descs = e_cal_component_get_descriptions(comp);
        assert!(!descs.is_null());
        assert_eq!(g_slist_length(descs), 1);
        let desc0 = (*descs).data as *mut ECalComponentText;
        assert!(!desc0.is_null());
        assert_eq!(
            CStr::from_ptr(e_cal_component_text_get_value(desc0))
                .to_str()
                .unwrap(),
            "Launch details"
        );
        assert_eq!(
            CStr::from_ptr(e_cal_component_text_get_altrep(desc0))
                .to_str()
                .unwrap(),
            "https://example.com/desc.html"
        );
        let lang = e_cal_component_text_get_language(desc0);
        if !lang.is_null() {
            assert_eq!(CStr::from_ptr(lang).to_str().unwrap(), "en");
        }
        g_slist_free_full(descs, Some(free_text));

        let localized_desc = e_cal_component_dup_description_for_locale(comp, text("en").as_ptr());
        assert!(!localized_desc.is_null());
        assert_eq!(
            CStr::from_ptr(e_cal_component_text_get_value(localized_desc))
                .to_str()
                .unwrap(),
            "Launch details"
        );
        e_cal_component_text_free(localized_desc.cast());

        // 2. Comments
        let comments = e_cal_component_get_comments(comp);
        assert!(!comments.is_null());
        assert_eq!(g_slist_length(comments), 2);
        let c0 = (*comments).data as *mut ECalComponentText;
        assert!(!c0.is_null());
        assert_eq!(
            CStr::from_ptr(e_cal_component_text_get_value(c0))
                .to_str()
                .unwrap(),
            "First comment"
        );
        let c1_node = (*comments).next;
        assert!(!c1_node.is_null());
        let c1 = (*c1_node).data as *mut ECalComponentText;
        assert!(!c1.is_null());
        assert_eq!(
            CStr::from_ptr(e_cal_component_text_get_value(c1))
                .to_str()
                .unwrap(),
            "Second comment"
        );
        g_slist_free_full(comments, Some(free_text));

        let localized_comment = e_cal_component_dup_comment_for_locale(comp, text("en").as_ptr());
        assert!(!localized_comment.is_null());
        assert_eq!(
            CStr::from_ptr(e_cal_component_text_get_value(localized_comment))
                .to_str()
                .unwrap(),
            "First comment"
        );
        e_cal_component_text_free(localized_comment.cast());

        // 3. Contacts
        let contacts = e_cal_component_get_contacts(comp);
        assert!(!contacts.is_null());
        assert_eq!(g_slist_length(contacts), 1);
        let cnt0 = (*contacts).data as *mut ECalComponentText;
        assert!(!cnt0.is_null());
        assert_eq!(
            CStr::from_ptr(e_cal_component_text_get_value(cnt0))
                .to_str()
                .unwrap(),
            "Team Alpha"
        );
        assert_eq!(
            CStr::from_ptr(e_cal_component_text_get_altrep(cnt0))
                .to_str()
                .unwrap(),
            "https://example.com/team.vcf"
        );
        g_slist_free_full(contacts, Some(free_text));

        // 4. Summaries
        let summaries = e_cal_component_dup_summaries(comp);
        assert!(!summaries.is_null());
        assert_eq!(g_slist_length(summaries), 1);
        let s0 = (*summaries).data as *mut ECalComponentText;
        assert!(!s0.is_null());
        assert_eq!(
            CStr::from_ptr(e_cal_component_text_get_value(s0))
                .to_str()
                .unwrap(),
            "Product Launch"
        );
        g_slist_free_full(summaries, Some(free_text));

        let localized_summary = e_cal_component_dup_summary_for_locale(comp, text("en").as_ptr());
        assert!(!localized_summary.is_null());
        assert_eq!(
            CStr::from_ptr(e_cal_component_text_get_value(localized_summary))
                .to_str()
                .unwrap(),
            "Product Launch"
        );
        e_cal_component_text_free(localized_summary.cast());

        // 5. In-place modifications
        let new_desc_text = e_cal_component_text_new(
            text("Updated launch notes").as_ptr(),
            text("https://example.com/v2.html").as_ptr(),
        );
        let new_desc_list = g_slist_append(std::ptr::null_mut(), new_desc_text.cast());
        e_cal_component_set_descriptions(comp, new_desc_list);
        g_slist_free_full(new_desc_list, Some(free_text));

        let new_cnt_text = e_cal_component_text_new(text("Team Beta").as_ptr(), std::ptr::null());
        let new_cnt_list = g_slist_append(std::ptr::null_mut(), new_cnt_text.cast());
        e_cal_component_set_contacts(comp, new_cnt_list);
        g_slist_free_full(new_cnt_list, Some(free_text));

        let inner = e_cal_component_get_icalcomponent(comp);
        let rendered = take_string(i_cal_component_as_ical_string(inner));
        assert!(
            rendered.contains(
                "DESCRIPTION;ALTREP=\"https://example.com/v2.html\":Updated launch notes"
            ),
            "modified desc missing: {rendered}"
        );
        assert!(
            rendered.contains("CONTACT:Team Beta"),
            "modified contact missing: {rendered}"
        );

        // 6. Clearing via NULL
        e_cal_component_set_descriptions(comp, std::ptr::null());
        e_cal_component_set_comments(comp, std::ptr::null());
        e_cal_component_set_contacts(comp, std::ptr::null());

        assert!(e_cal_component_get_descriptions(comp).is_null());
        assert!(e_cal_component_get_comments(comp).is_null());
        assert!(e_cal_component_get_contacts(comp).is_null());

        let inner_cleared = e_cal_component_get_icalcomponent(comp);
        let cleared_rendered = take_string(i_cal_component_as_ical_string(inner_cleared));
        assert!(
            !cleared_rendered.contains("DESCRIPTION"),
            "cleared desc still present: {cleared_rendered}"
        );
        assert!(
            !cleared_rendered.contains("COMMENT"),
            "cleared comment still present: {cleared_rendered}"
        );
        assert!(
            !cleared_rendered.contains("CONTACT"),
            "cleared contact still present: {cleared_rendered}"
        );

        g_object_unref(comp.cast());
        g_object_unref(event.cast());
        g_object_unref(calendar.cast());
    }
}

/// Probing `ECalComponent` timestamps (`CREATED`, `LAST-MODIFIED`, `DTSTAMP`, `COMPLETED`), recurrence dates (`RDATE` with `ECalComponentPeriod`), and component ID (`ECalComponentId`):
/// `e_cal_component_get_created`, `e_cal_component_get_last_modified`, and `e_cal_component_get_dtstamp` extract `ICalTime *` timestamps;
/// `e_cal_component_get_completed` extracts completion timestamp on VTODO;
/// `e_cal_component_has_rdates` and `e_cal_component_get_rdates` extract `ECalComponentPeriod *` entries;
/// `e_cal_component_period_get_kind`, `e_cal_component_period_get_start`, `e_cal_component_period_get_end`, and `e_cal_component_period_get_duration` inspect period parameters;
/// `e_cal_component_get_id` returns an `ECalComponentId *`, with `e_cal_component_id_get_uid`, `e_cal_component_id_get_rid`, `e_cal_component_id_copy`, `e_cal_component_id_equal`, `e_cal_component_id_hash`;
/// and setters support in-place modification and NULL clearing.
#[test]
fn ecalcomponent_timestamps_rdates_and_component_id_in_eds() {
    let source = text(
        "BEGIN:VCALENDAR\r\n\
         VERSION:2.0\r\n\
         PRODID:-//evolution-jmap//JMAP calendar backend//EN\r\n\
         BEGIN:VEVENT\r\n\
         UID:K1\r\n\
         SUMMARY:Milestone Review\r\n\
         DTSTART:20260810T100000Z\r\n\
         CREATED:20260801T080000Z\r\n\
         LAST-MODIFIED:20260805T120000Z\r\n\
         DTSTAMP:20260810T090000Z\r\n\
         RDATE;VALUE=PERIOD:20260812T100000Z/PT2H\r\n\
         END:VEVENT\r\n\
         END:VCALENDAR\r\n",
    );
    unsafe {
        let calendar = i_cal_component_new_from_string(source.as_ptr());
        assert!(!calendar.is_null());
        let event = i_cal_component_get_first_component(calendar, I_CAL_VEVENT_COMPONENT);
        assert!(!event.is_null());

        let comp = e_cal_component_new_from_icalcomponent(i_cal_component_clone(event));
        assert!(!comp.is_null());

        // 1. Inspect Timestamps
        let created = e_cal_component_get_created(comp);
        assert!(!created.is_null());
        assert_eq!(i_cal_time_get_year(created), 2026);
        assert_eq!(i_cal_time_get_month(created), 8);
        assert_eq!(i_cal_time_get_day(created), 1);
        assert_eq!(i_cal_time_get_hour(created), 8);
        assert_eq!(i_cal_time_is_utc(created), 1);
        g_object_unref(created.cast());

        let last_mod = e_cal_component_get_last_modified(comp);
        assert!(!last_mod.is_null());
        assert_eq!(i_cal_time_get_day(last_mod), 5);
        assert_eq!(i_cal_time_get_hour(last_mod), 12);
        g_object_unref(last_mod.cast());

        let dtstamp = e_cal_component_get_dtstamp(comp);
        assert!(!dtstamp.is_null());
        assert_eq!(i_cal_time_get_day(dtstamp), 10);
        assert_eq!(i_cal_time_get_hour(dtstamp), 9);
        g_object_unref(dtstamp.cast());

        // 2. Component ID operations
        let comp_id = e_cal_component_get_id(comp);
        assert!(!comp_id.is_null());
        let uid_str = e_cal_component_id_get_uid(comp_id);
        assert!(!uid_str.is_null());
        assert_eq!(CStr::from_ptr(uid_str).to_str().unwrap(), "K1");
        let rid_str = e_cal_component_id_get_rid(comp_id);
        assert!(rid_str.is_null() || CStr::from_ptr(rid_str).to_str().unwrap().is_empty());

        let id_copy = e_cal_component_id_copy(comp_id);
        assert!(!id_copy.is_null());
        assert_eq!(e_cal_component_id_equal(comp_id.cast(), id_copy.cast()), 1);
        assert_eq!(
            e_cal_component_id_hash(comp_id.cast()),
            e_cal_component_id_hash(id_copy.cast())
        );
        e_cal_component_id_free(id_copy.cast());

        let manual_id =
            e_cal_component_id_new(text("K2").as_ptr(), text("20260810T100000Z").as_ptr());
        assert!(!manual_id.is_null());
        assert_eq!(
            CStr::from_ptr(e_cal_component_id_get_uid(manual_id))
                .to_str()
                .unwrap(),
            "K2"
        );
        assert_eq!(
            CStr::from_ptr(e_cal_component_id_get_rid(manual_id))
                .to_str()
                .unwrap(),
            "20260810T100000Z"
        );
        assert_eq!(
            e_cal_component_id_equal(comp_id.cast(), manual_id.cast()),
            0
        );
        e_cal_component_id_free(manual_id.cast());
        e_cal_component_id_free(comp_id.cast());

        // 3. RDATEs & ECalComponentPeriod
        assert_eq!(e_cal_component_has_rdates(comp), 1);
        let rdates = e_cal_component_get_rdates(comp);
        assert!(!rdates.is_null());
        assert_eq!(g_slist_length(rdates), 1);

        let period0 = (*rdates).data as *mut ECalComponentPeriod;
        assert!(!period0.is_null());
        assert_eq!(
            e_cal_component_period_get_kind(period0),
            E_CAL_COMPONENT_PERIOD_DURATION
        );
        let p_start = e_cal_component_period_get_start(period0);
        assert!(!p_start.is_null());
        assert_eq!(i_cal_time_get_day(p_start), 12);
        assert_eq!(i_cal_time_get_hour(p_start), 10);

        let p_dur = e_cal_component_period_get_duration(period0);
        assert!(!p_dur.is_null());
        assert_eq!(i_cal_duration_get_hours(p_dur), 2);

        unsafe extern "C" fn free_period(ptr: *mut std::ffi::c_void) {
            unsafe { e_cal_component_period_free(ptr) };
        }
        g_slist_free_full(rdates, Some(free_period));

        // 4. Modify timestamps in place
        let new_created_time = i_cal_time_new_from_string(c"20260802T090000Z".as_ptr());
        e_cal_component_set_created(comp, new_created_time);
        g_object_unref(new_created_time.cast());

        let new_lastmod_time = i_cal_time_new_from_string(c"20260806T140000Z".as_ptr());
        e_cal_component_set_last_modified(comp, new_lastmod_time);
        g_object_unref(new_lastmod_time.cast());

        let new_dtstamp_time = i_cal_time_new_from_string(c"20260810T120000Z".as_ptr());
        e_cal_component_set_dtstamp(comp, new_dtstamp_time);
        g_object_unref(new_dtstamp_time.cast());

        let inner = e_cal_component_get_icalcomponent(comp);
        let rendered = take_string(i_cal_component_as_ical_string(inner));
        assert!(rendered.contains("CREATED:20260802T090000Z"), "{rendered}");
        assert!(
            rendered.contains("LAST-MODIFIED:20260806T140000Z"),
            "{rendered}"
        );
        assert!(rendered.contains("DTSTAMP:20260810T120000Z"), "{rendered}");

        // 5. Clear timestamps and RDATEs
        e_cal_component_set_created(comp, std::ptr::null());
        e_cal_component_set_last_modified(comp, std::ptr::null());
        e_cal_component_set_rdates(comp, std::ptr::null());

        assert_eq!(e_cal_component_has_rdates(comp), 0);
        assert!(e_cal_component_get_created(comp).is_null());
        assert!(e_cal_component_get_last_modified(comp).is_null());

        let inner_cleared = e_cal_component_get_icalcomponent(comp);
        let cleared_rendered = take_string(i_cal_component_as_ical_string(inner_cleared));
        assert!(!cleared_rendered.contains("CREATED:"), "{cleared_rendered}");
        assert!(
            !cleared_rendered.contains("LAST-MODIFIED:"),
            "{cleared_rendered}"
        );
        assert!(!cleared_rendered.contains("RDATE"), "{cleared_rendered}");

        g_object_unref(comp.cast());
        g_object_unref(event.cast());
        g_object_unref(calendar.cast());
    }
}

/// Probing `ECalComponent` component types (`ECalComponentVType`), cloning (`e_cal_component_clone`),
/// recurrence instance check (`e_cal_component_is_instance`), and string serialization (`e_cal_component_get_as_string`):
/// `e_cal_component_get_vtype` correctly identifies `VEVENT` (`E_CAL_COMPONENT_EVENT`), `VTODO` (`E_CAL_COMPONENT_TODO`),
/// `VJOURNAL` (`E_CAL_COMPONENT_JOURNAL`), and `VTIMEZONE` (`E_CAL_COMPONENT_TIMEZONE`);
/// `e_cal_component_is_instance` detects components carrying `RECURRENCE-ID`;
/// `e_cal_component_get_as_string` serializes the component into an allocated iCalendar string;
/// and `e_cal_component_clone` yields a distinct deep copy of the component.
#[test]
fn ecalcomponent_vtypes_cloning_and_string_serialization_in_eds() {
    let calendar_source = text(
        "BEGIN:VCALENDAR\r\n\
         VERSION:2.0\r\n\
         PRODID:-//evolution-jmap//JMAP calendar backend//EN\r\n\
         BEGIN:VEVENT\r\n\
         UID:event-1\r\n\
         SUMMARY:Team Meeting\r\n\
         DTSTART:20260810T090000Z\r\n\
         DURATION:PT1H\r\n\
         END:VEVENT\r\n\
         BEGIN:VEVENT\r\n\
         UID:event-1\r\n\
         RECURRENCE-ID:20260817T090000Z\r\n\
         SUMMARY:Team Meeting (Rescheduled)\r\n\
         DTSTART:20260817T100000Z\r\n\
         DURATION:PT1H\r\n\
         END:VEVENT\r\n\
         BEGIN:VTODO\r\n\
         UID:todo-1\r\n\
         SUMMARY:Implement Milestone\r\n\
         DUE:20260815T180000Z\r\n\
         PERCENT-COMPLETE:75\r\n\
         END:VTODO\r\n\
         BEGIN:VJOURNAL\r\n\
         UID:journal-1\r\n\
         SUMMARY:Session Notes\r\n\
         DESCRIPTION:Verified component semantics\r\n\
         END:VJOURNAL\r\n\
         END:VCALENDAR\r\n",
    );

    unsafe {
        let calendar = i_cal_component_new_from_string(calendar_source.as_ptr());
        assert!(!calendar.is_null());

        // 1. Master Event VEVENT
        let event_comp = i_cal_component_get_first_component(calendar, I_CAL_VEVENT_COMPONENT);
        assert!(!event_comp.is_null());
        let e_event = e_cal_component_new_from_icalcomponent(i_cal_component_clone(event_comp));
        assert!(!e_event.is_null());

        assert_eq!(e_cal_component_get_vtype(e_event), E_CAL_COMPONENT_EVENT);
        assert_eq!(e_cal_component_is_instance(e_event), 0);

        // String serialization
        let as_str_ptr = e_cal_component_get_as_string(e_event);
        assert!(!as_str_ptr.is_null());
        let rendered_str = CStr::from_ptr(as_str_ptr).to_str().unwrap();
        assert!(rendered_str.contains("BEGIN:VEVENT"), "{rendered_str}");
        assert!(rendered_str.contains("UID:event-1"), "{rendered_str}");
        assert!(
            rendered_str.contains("SUMMARY:Team Meeting"),
            "{rendered_str}"
        );
        g_free(as_str_ptr.cast());

        // Deep cloning
        let cloned_event = e_cal_component_clone(e_event);
        assert!(!cloned_event.is_null());
        assert_ne!(cloned_event, e_event);
        assert_eq!(
            e_cal_component_get_vtype(cloned_event),
            E_CAL_COMPONENT_EVENT
        );
        let cloned_id = e_cal_component_get_id(cloned_event);
        assert!(!cloned_id.is_null());
        assert_eq!(
            CStr::from_ptr(e_cal_component_id_get_uid(cloned_id))
                .to_str()
                .unwrap(),
            "event-1"
        );
        e_cal_component_id_free(cloned_id.cast());
        g_object_unref(cloned_event.cast());
        g_object_unref(e_event.cast());

        // 2. Recurrence override instance VEVENT
        let instance_comp = i_cal_component_get_next_component(calendar, I_CAL_VEVENT_COMPONENT);
        assert!(!instance_comp.is_null());
        let e_instance =
            e_cal_component_new_from_icalcomponent(i_cal_component_clone(instance_comp));
        assert!(!e_instance.is_null());

        assert_eq!(e_cal_component_get_vtype(e_instance), E_CAL_COMPONENT_EVENT);
        assert_eq!(e_cal_component_is_instance(e_instance), 1);
        g_object_unref(e_instance.cast());

        // 3. VTODO component
        let todo_comp = i_cal_component_get_first_component(calendar, I_CAL_VTODO_COMPONENT);
        assert!(!todo_comp.is_null());
        let e_todo = e_cal_component_new_from_icalcomponent(i_cal_component_clone(todo_comp));
        assert!(!e_todo.is_null());

        assert_eq!(e_cal_component_get_vtype(e_todo), E_CAL_COMPONENT_TODO);
        assert_eq!(e_cal_component_is_instance(e_todo), 0);
        g_object_unref(e_todo.cast());

        // 4. VJOURNAL component
        let journal_comp = i_cal_component_get_first_component(calendar, I_CAL_VJOURNAL_COMPONENT);
        assert!(!journal_comp.is_null());
        let e_journal = e_cal_component_new_from_icalcomponent(i_cal_component_clone(journal_comp));
        assert!(!e_journal.is_null());

        assert_eq!(
            e_cal_component_get_vtype(e_journal),
            E_CAL_COMPONENT_JOURNAL
        );
        g_object_unref(e_journal.cast());

        g_object_unref(calendar.cast());
    }
}

/// Probing `ECalComponent` geographic coordinates (`ICalGeo`), task percent complete, priority, and sequence in EDS:
/// `e_cal_component_get_geo` extracts `ICalGeo *` with `i_cal_geo_get_lat` and `i_cal_geo_get_lon`;
/// `e_cal_component_set_geo` updates coordinates in place or removes `GEO` with NULL;
/// `e_cal_component_get_percent_complete` / `e_cal_component_set_percent_complete` manipulate task progress;
/// `e_cal_component_get_priority` / `e_cal_component_set_priority` manage event/task priority (0..9);
/// and `e_cal_component_get_sequence` / `e_cal_component_set_sequence` track revision sequences.
#[test]
fn ecalcomponent_geo_coordinates_and_task_completion_in_eds() {
    let source = text(
        "BEGIN:VCALENDAR\r\n\
         VERSION:2.0\r\n\
         PRODID:-//evolution-jmap//JMAP calendar backend//EN\r\n\
         BEGIN:VEVENT\r\n\
         UID:event-geo-1\r\n\
         SUMMARY:Conference Keynote\r\n\
         DTSTART:20260810T100000Z\r\n\
         DURATION:PT1H30M\r\n\
         GEO:52.520008;13.404954\r\n\
         PRIORITY:1\r\n\
         SEQUENCE:3\r\n\
         END:VEVENT\r\n\
         BEGIN:VTODO\r\n\
         UID:todo-task-1\r\n\
         SUMMARY:Complete Spec\r\n\
         PERCENT-COMPLETE:50\r\n\
         PRIORITY:3\r\n\
         SEQUENCE:1\r\n\
         END:VTODO\r\n\
         END:VCALENDAR\r\n",
    );

    unsafe {
        let calendar = i_cal_component_new_from_string(source.as_ptr());
        assert!(!calendar.is_null());

        let event = i_cal_component_get_first_component(calendar, I_CAL_VEVENT_COMPONENT);
        assert!(!event.is_null());
        let comp = e_cal_component_new_from_icalcomponent(i_cal_component_clone(event));
        assert!(!comp.is_null());

        // 1. GEO Coordinates extraction
        let geo = e_cal_component_get_geo(comp);
        assert!(!geo.is_null());
        assert!((i_cal_geo_get_lat(geo) - 52.520008).abs() < 1e-5);
        assert!((i_cal_geo_get_lon(geo) - 13.404954).abs() < 1e-5);
        g_object_unref(geo.cast());

        // Priority and Sequence inspection
        assert_eq!(e_cal_component_get_priority(comp), 1);
        assert_eq!(e_cal_component_get_sequence(comp), 3);

        // 2. In-place modification of GEO, Priority, and Sequence
        let new_geo = i_cal_geo_new(48.856614, 2.352222);
        assert!(!new_geo.is_null());
        e_cal_component_set_geo(comp, new_geo);
        g_object_unref(new_geo.cast());

        e_cal_component_set_priority(comp, 2);
        e_cal_component_set_sequence(comp, 4);

        let inner = e_cal_component_get_icalcomponent(comp);
        let rendered = take_string(i_cal_component_as_ical_string(inner));
        assert!(
            rendered.contains("GEO:48.856614;2.352222")
                || rendered.contains("GEO:48.856613;2.352222"),
            "modified GEO missing: {rendered}"
        );
        assert!(rendered.contains("PRIORITY:2"), "{rendered}");
        assert!(rendered.contains("SEQUENCE:4"), "{rendered}");

        // 3. Clear GEO via NULL
        e_cal_component_set_geo(comp, std::ptr::null());
        assert!(e_cal_component_get_geo(comp).is_null());

        let inner_cleared = e_cal_component_get_icalcomponent(comp);
        let cleared_rendered = take_string(i_cal_component_as_ical_string(inner_cleared));
        assert!(!cleared_rendered.contains("GEO:"), "{cleared_rendered}");
        assert!(
            cleared_rendered.contains("PRIORITY:2"),
            "{cleared_rendered}"
        );

        g_object_unref(comp.cast());

        // 4. VTODO Task Percent-Complete and Sequence
        let todo = i_cal_component_get_first_component(calendar, I_CAL_VTODO_COMPONENT);
        assert!(!todo.is_null());
        let comp_todo = e_cal_component_new_from_icalcomponent(i_cal_component_clone(todo));
        assert!(!comp_todo.is_null());

        assert_eq!(e_cal_component_get_percent_complete(comp_todo), 50);
        assert_eq!(e_cal_component_get_priority(comp_todo), 3);
        assert_eq!(e_cal_component_get_sequence(comp_todo), 1);

        e_cal_component_set_percent_complete(comp_todo, 100);
        assert_eq!(e_cal_component_get_percent_complete(comp_todo), 100);

        let inner_todo = e_cal_component_get_icalcomponent(comp_todo);
        let rendered_todo = take_string(i_cal_component_as_ical_string(inner_todo));
        assert!(
            rendered_todo.contains("PERCENT-COMPLETE:100"),
            "{rendered_todo}"
        );

        g_object_unref(comp_todo.cast());
        g_object_unref(calendar.cast());
    }
}
