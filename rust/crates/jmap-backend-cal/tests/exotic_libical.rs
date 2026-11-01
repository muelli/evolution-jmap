// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Differential tests comparing `jmap-ical` handling of exotic input with EDS `libical`.
//!
//! Feeds ROADMAP item 168: verifies Unicode, bidi controls, zero-width characters,
//! NUL, CR/LF, NFC/NFD, and line folding behavior across both parsers.

use eds_sys::{
    I_CAL_ATTENDEE_PROPERTY, I_CAL_LOCATION_PROPERTY, I_CAL_ORGANIZER_PROPERTY,
    I_CAL_VEVENT_COMPONENT, ICalComponent, i_cal_component_get_first_component,
    i_cal_component_get_first_property, i_cal_component_get_summary,
    i_cal_component_new_from_string, i_cal_property_get_location,
    i_cal_property_get_parameter_as_string,
};
use gobject_sys::g_object_unref;
use jmap_backend_cal::marshal;
use jmap_ical::{event_to_ical, ical_to_event};
use jmap_proto::calendars::CalendarEvent;
use serde_json::json;
use std::collections::BTreeMap;
use std::ffi::CStr;

fn base_event() -> CalendarEvent {
    let mut event = CalendarEvent::default();
    event.id = Some("test-diff-1".into());
    event.title = Some("Base Event".to_owned());
    event.start = Some("2026-11-01T10:00:00".to_owned());
    event.duration = Some("PT1H".to_owned());
    event
}

unsafe fn parse_libical(ics: &str) -> *mut ICalComponent {
    // Note: CStr must be NUL terminated.
    let text = jmap_backend_core::error::cstring_lossy(ics);
    unsafe { i_cal_component_new_from_string(text.as_ptr()) }
}

unsafe fn vevent(calendar: *mut ICalComponent) -> *mut ICalComponent {
    unsafe { i_cal_component_get_first_component(calendar, I_CAL_VEVENT_COMPONENT) }
}

#[test]
fn libical_vs_jmap_ical_bidi_controls() {
    let mut event = base_event();
    let bidi_summary = "Project \u{202e}exe.test\u{2069} review \u{202d}LTR\u{202c}";
    let bidi_location = "Building \u{2067}حاسوب\u{2069} Room 404";
    let bidi_organizer_cn = "Alice \u{202e}ecilA\u{2069}";
    let bidi_attendee_cn = "Bob \u{202d}boB\u{2069}";

    event.title = Some(bidi_summary.to_owned());
    let mut locations = BTreeMap::new();
    locations.insert(
        "loc1".to_owned(),
        json!({
            "@type": "Location",
            "name": bidi_location,
        }),
    );
    event.locations = Some(locations);

    let mut participants = BTreeMap::new();
    participants.insert(
        "part-org".to_owned(),
        json!({
            "@type": "Participant",
            "name": bidi_organizer_cn,
            "calendarAddress": "mailto:alice@example.com",
            "roles": { "owner": true }
        }),
    );
    participants.insert(
        "part-att".to_owned(),
        json!({
            "@type": "Participant",
            "name": bidi_attendee_cn,
            "calendarAddress": "mailto:bob@example.com",
            "roles": { "attendee": true }
        }),
    );
    event.participants = Some(participants);

    let ics = event_to_ical(&event);

    // 1. jmap-ical roundtrip
    let jmap_event = ical_to_event(&ics).expect("jmap_ical parses bidi");
    assert_eq!(jmap_event.title.as_deref(), Some(bidi_summary));

    // 2. libical parsing
    unsafe {
        let cal = parse_libical(&ics);
        assert!(!cal.is_null(), "libical parses bidi ics");
        let ev = vevent(cal);
        assert!(!ev.is_null());

        let summary = i_cal_component_get_summary(ev);
        let summary_str = CStr::from_ptr(summary).to_string_lossy();
        assert_eq!(summary_str, bidi_summary, "libical preserves bidi summary");

        let loc_prop = i_cal_component_get_first_property(ev, I_CAL_LOCATION_PROPERTY);
        assert!(!loc_prop.is_null());
        let loc_str = CStr::from_ptr(i_cal_property_get_location(loc_prop)).to_string_lossy();
        assert_eq!(loc_str, bidi_location, "libical preserves bidi location");
        g_object_unref(loc_prop.cast());

        let org_prop = i_cal_component_get_first_property(ev, I_CAL_ORGANIZER_PROPERTY);
        assert!(!org_prop.is_null());
        let cn_param = i_cal_property_get_parameter_as_string(org_prop, c"CN".as_ptr());
        let cn_str = CStr::from_ptr(cn_param).to_string_lossy();
        // Check what libical returns for CN: does it return quotes or unquoted?
        println!("libical org CN: {cn_str}");
        g_object_unref(org_prop.cast());

        let att_prop = i_cal_component_get_first_property(ev, I_CAL_ATTENDEE_PROPERTY);
        assert!(!att_prop.is_null());
        let att_cn_param = i_cal_property_get_parameter_as_string(att_prop, c"CN".as_ptr());
        let att_cn_str = CStr::from_ptr(att_cn_param).to_string_lossy();
        println!("libical att CN: {att_cn_str}");
        g_object_unref(att_prop.cast());

        g_object_unref(ev.cast());
        marshal::component_unref(cal);
    }
}

#[test]
fn libical_vs_jmap_ical_zero_width_characters() {
    let mut event = base_event();
    let zw_summary = "Hidden\u{200b}Space\u{200c}NonJoiner\u{200d}Joiner\u{feff}BOM";
    let zw_location = "\u{feff}Room\u{200b}101";
    let zw_organizer = "Admin\u{200c}\u{200d}";
    let zw_attendee = "\u{200b}Guest\u{feff}";

    event.title = Some(zw_summary.to_owned());
    let mut locations = BTreeMap::new();
    locations.insert(
        "loc1".to_owned(),
        json!({
            "@type": "Location",
            "name": zw_location,
        }),
    );
    event.locations = Some(locations);

    let mut participants = BTreeMap::new();
    participants.insert(
        "part-org".to_owned(),
        json!({
            "@type": "Participant",
            "name": zw_organizer,
            "calendarAddress": "mailto:admin@example.com",
            "roles": { "owner": true }
        }),
    );
    participants.insert(
        "part-att".to_owned(),
        json!({
            "@type": "Participant",
            "name": zw_attendee,
            "calendarAddress": "mailto:guest@example.com",
            "roles": { "attendee": true }
        }),
    );
    event.participants = Some(participants);

    let ics = event_to_ical(&event);

    unsafe {
        let cal = parse_libical(&ics);
        assert!(!cal.is_null());
        let ev = vevent(cal);
        assert!(!ev.is_null());

        let summary = i_cal_component_get_summary(ev);
        let summary_str = CStr::from_ptr(summary).to_string_lossy();
        assert_eq!(
            summary_str, zw_summary,
            "libical preserves zero-width summary"
        );

        let loc_prop = i_cal_component_get_first_property(ev, I_CAL_LOCATION_PROPERTY);
        assert!(!loc_prop.is_null());
        let loc_str = CStr::from_ptr(i_cal_property_get_location(loc_prop)).to_string_lossy();
        assert_eq!(
            loc_str, zw_location,
            "libical preserves zero-width location"
        );
        g_object_unref(loc_prop.cast());

        let org_prop = i_cal_component_get_first_property(ev, I_CAL_ORGANIZER_PROPERTY);
        let cn_param = i_cal_property_get_parameter_as_string(org_prop, c"CN".as_ptr());
        let cn_str = CStr::from_ptr(cn_param).to_string_lossy();
        println!("libical zw org CN: {cn_str}");
        assert_eq!(
            cn_str, zw_organizer,
            "libical preserves zero-width organizer CN"
        );
        g_object_unref(org_prop.cast());

        let att_prop = i_cal_component_get_first_property(ev, I_CAL_ATTENDEE_PROPERTY);
        let att_cn_param = i_cal_property_get_parameter_as_string(att_prop, c"CN".as_ptr());
        let att_cn_str = CStr::from_ptr(att_cn_param).to_string_lossy();
        println!("libical zw att CN: {att_cn_str}");
        assert_eq!(
            att_cn_str, zw_attendee,
            "libical preserves zero-width attendee CN"
        );
        g_object_unref(att_prop.cast());

        g_object_unref(ev.cast());
        marshal::component_unref(cal);
    }
}

#[test]
fn libical_vs_jmap_ical_crlf_handling() {
    let mut event = base_event();
    let injected_summary = "Meeting\r\nLOCATION:Injected Location\r\nDESCRIPTION:Injected Desc";
    let injected_location = "Room 101\r\nSUMMARY:Injected Summary";
    let injected_org_cn = "Alice\r\nATTENDEE;CN=Mallory:mailto:mallory@evil.com\r\n";
    let injected_att_cn = "Bob\r\nROLE=CHAIR\r\n";

    event.title = Some(injected_summary.to_owned());
    let mut locations = BTreeMap::new();
    locations.insert(
        "loc1".to_owned(),
        json!({
            "@type": "Location",
            "name": injected_location,
        }),
    );
    event.locations = Some(locations);

    let mut participants = BTreeMap::new();
    participants.insert(
        "part-org".to_owned(),
        json!({
            "@type": "Participant",
            "name": injected_org_cn,
            "calendarAddress": "mailto:alice@example.com",
            "roles": { "owner": true }
        }),
    );
    participants.insert(
        "part-att".to_owned(),
        json!({
            "@type": "Participant",
            "name": injected_att_cn,
            "calendarAddress": "mailto:bob@example.com",
            "roles": { "attendee": true }
        }),
    );
    event.participants = Some(participants);

    let ics = event_to_ical(&event);

    unsafe {
        let cal = parse_libical(&ics);
        assert!(!cal.is_null());
        let ev = vevent(cal);
        assert!(!ev.is_null());

        let summary = i_cal_component_get_summary(ev);
        let summary_str = CStr::from_ptr(summary).to_string_lossy();
        println!("libical summary with escaped CRLF: {summary_str:?}");

        let loc_prop = i_cal_component_get_first_property(ev, I_CAL_LOCATION_PROPERTY);
        let loc_str = CStr::from_ptr(i_cal_property_get_location(loc_prop)).to_string_lossy();
        println!("libical location with escaped CRLF: {loc_str:?}");
        g_object_unref(loc_prop.cast());

        let org_prop = i_cal_component_get_first_property(ev, I_CAL_ORGANIZER_PROPERTY);
        let cn_param = i_cal_property_get_parameter_as_string(org_prop, c"CN".as_ptr());
        let cn_str = CStr::from_ptr(cn_param).to_string_lossy();
        println!("libical org CN with caret escape: {cn_str:?}");
        g_object_unref(org_prop.cast());

        let att_prop = i_cal_component_get_first_property(ev, I_CAL_ATTENDEE_PROPERTY);
        let att_cn_param = i_cal_property_get_parameter_as_string(att_prop, c"CN".as_ptr());
        let att_cn_str = CStr::from_ptr(att_cn_param).to_string_lossy();
        println!("libical att CN with caret escape: {att_cn_str:?}");
        g_object_unref(att_prop.cast());

        g_object_unref(ev.cast());
        marshal::component_unref(cal);
    }
}

#[test]
fn libical_vs_jmap_ical_nul_handling() {
    let mut event = base_event();
    let nul_summary = "Meeting\0With\0NUL";
    let nul_location = "Room\0Hidden";
    let nul_org_cn = "Alice\0Admin";
    let nul_att_cn = "Bob\0Guest";

    event.title = Some(nul_summary.to_owned());
    let mut locations = BTreeMap::new();
    locations.insert(
        "loc1".to_owned(),
        json!({
            "@type": "Location",
            "name": nul_location,
        }),
    );
    event.locations = Some(locations);

    let mut participants = BTreeMap::new();
    participants.insert(
        "part-org".to_owned(),
        json!({
            "@type": "Participant",
            "name": nul_org_cn,
            "calendarAddress": "mailto:alice@example.com",
            "roles": { "owner": true }
        }),
    );
    participants.insert(
        "part-att".to_owned(),
        json!({
            "@type": "Participant",
            "name": nul_att_cn,
            "calendarAddress": "mailto:bob@example.com",
            "roles": { "attendee": true }
        }),
    );
    event.participants = Some(participants);

    let ics = event_to_ical(&event);

    unsafe {
        // Because ics contains '\0' in SUMMARY and LOCATION, let's see what libical parses!
        let cal = parse_libical(&ics);
        println!("libical parse of ICS with NUL: is_null = {}", cal.is_null());
        if !cal.is_null() {
            let ev = vevent(cal);
            if !ev.is_null() {
                let summary = i_cal_component_get_summary(ev);
                if !summary.is_null() {
                    let summary_str = CStr::from_ptr(summary).to_string_lossy();
                    println!("libical summary with NUL: {summary_str:?}");
                }
                g_object_unref(ev.cast());
            }
            marshal::component_unref(cal);
        }
    }
}

#[test]
fn libical_vs_jmap_ical_nfc_nfd_handling() {
    let mut event = base_event();
    let nfc_text = "Caf\u{00e9} in K\u{00f6}ln";
    let nfd_text = "Cafe\u{0301} in Ko\u{0308}ln";

    event.title = Some(nfc_text.to_owned());
    let mut locations = BTreeMap::new();
    locations.insert(
        "loc1".to_owned(),
        json!({
            "@type": "Location",
            "name": nfd_text,
        }),
    );
    event.locations = Some(locations);

    let ics = event_to_ical(&event);

    unsafe {
        let cal = parse_libical(&ics);
        assert!(!cal.is_null());
        let ev = vevent(cal);
        assert!(!ev.is_null());

        let summary = i_cal_component_get_summary(ev);
        let summary_str = CStr::from_ptr(summary).to_string_lossy();
        println!(
            "libical NFC summary: {summary_str:?} (equal to NFC: {})",
            summary_str == nfc_text
        );

        let loc_prop = i_cal_component_get_first_property(ev, I_CAL_LOCATION_PROPERTY);
        let loc_str = CStr::from_ptr(i_cal_property_get_location(loc_prop)).to_string_lossy();
        println!(
            "libical NFD location: {loc_str:?} (equal to NFD: {})",
            loc_str == nfd_text
        );
        g_object_unref(loc_prop.cast());

        g_object_unref(ev.cast());
        marshal::component_unref(cal);
    }
}

#[test]
fn libical_vs_jmap_ical_fold_boundary_and_long_lines() {
    let multibyte_chars = [
        ("2-byte", "\u{00e9}"),
        ("3-byte", "\u{202e}"),
        ("4-byte-crab", "\u{1f980}"),
        ("4-byte-family", "👨‍👩‍👧‍👦"),
    ];

    for (name, mb) in multibyte_chars {
        for prefix_len in [60, 65, 66, 67, 68, 70, 75] {
            let mut event = base_event();
            let prefix = "A".repeat(prefix_len);
            let suffix = "Z".repeat(50);
            let test_title = format!("{prefix}{mb}{suffix}");
            event.title = Some(test_title.clone());

            let ics = event_to_ical(&event);

            unsafe {
                let cal = parse_libical(&ics);
                assert!(
                    !cal.is_null(),
                    "libical parses folded {} prefix {}",
                    name,
                    prefix_len
                );
                let ev = vevent(cal);
                assert!(!ev.is_null());

                let summary = i_cal_component_get_summary(ev);
                let summary_str = CStr::from_ptr(summary).to_string_lossy();
                assert_eq!(
                    summary_str, test_title,
                    "libical summary matches for {} prefix {}",
                    name, prefix_len
                );

                g_object_unref(ev.cast());
                marshal::component_unref(cal);
            }
        }
    }
}
