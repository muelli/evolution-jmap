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
