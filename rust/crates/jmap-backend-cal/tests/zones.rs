// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Validation of timezone offset conversions across system zoneinfo definitions against libical.

use std::ffi::{CStr, CString};
use std::path::PathBuf;

use eds_sys::{
    i_cal_time_as_ical_string, i_cal_time_convert_to_zone, i_cal_time_new_from_string,
    i_cal_timezone_get_builtin_timezone, i_cal_timezone_get_component,
};
use glib_sys::g_free;
use gobject_sys::g_object_unref;
use jmap_backend_cal::marshal;

/// Path to system zoneinfo directory.
const ZONEINFO: &str = "/usr/share/zoneinfo";

/// Probe instants across the year testing standard and DST offsets.
const PROBES: [&str; 4] = [
    "20260128T070000Z",
    "20260328T233000Z",
    "20260329T233000Z",
    "20260901T070000Z",
];

/// Returns sorted list of all valid IANA timezone names found in system zoneinfo.
fn zone_names() -> Vec<String> {
    let root = PathBuf::from(ZONEINFO);
    let mut names = Vec::new();
    let mut pending = vec![root.clone()];
    while let Some(directory) = pending.pop() {
        let Ok(entries) = std::fs::read_dir(&directory) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let Ok(name) = path.strip_prefix(&root) else {
                continue;
            };
            let name = name.to_string_lossy().into_owned();
            if name.starts_with("posix") || name.starts_with("right") || name == "localtime" {
                continue;
            }
            if path.is_dir() {
                pending.push(path);
            } else if std::fs::read(&path).is_ok_and(|bytes| bytes.starts_with(b"TZif")) {
                names.push(name);
            }
        }
    }
    names.sort();
    names
}

/// Returns rendered VTIMEZONE and TZID for named builtin timezone.
fn definition(name: &str) -> Option<(String, String)> {
    let name = CString::new(name).ok()?;
    // SAFETY: `name` is NUL-terminated and valid for the call. The builtin zone
    // is the library's and is not unreffed; the component it hands back is
    // transfer full and is dropped here, the rendering having copied it.
    let rendered = unsafe {
        let zone = i_cal_timezone_get_builtin_timezone(name.as_ptr());
        if zone.is_null() {
            return None;
        }
        let component = i_cal_timezone_get_component(zone);
        let rendered = marshal::ical_from_component(component);
        if !component.is_null() {
            g_object_unref(component.cast());
        }
        rendered?
    };
    let tzid = rendered
        .lines()
        .find_map(|line| line.strip_prefix("TZID:"))?
        .trim_end()
        .to_owned();
    Some((rendered, tzid))
}

/// Returns libical conversion of UTC instant into local time for named zone.
fn libical_reads(name: &str, utc: &str) -> String {
    let name = CString::new(name).expect("a zone name with no NUL in it");
    let utc = CString::new(utc).expect("an instant with no NUL in it");
    // SAFETY: both strings are NUL-terminated and valid for the calls. The
    // builtin zone belongs to the library; the two times and the rendered
    // string are transfer full and are all released here.
    unsafe {
        let zone = i_cal_timezone_get_builtin_timezone(name.as_ptr());
        assert!(!zone.is_null(), "libical no longer resolves {name:?}");
        let instant = i_cal_time_new_from_string(utc.as_ptr());
        assert!(!instant.is_null(), "libical will not read {utc:?}");
        let local = i_cal_time_convert_to_zone(instant, zone);
        assert!(!local.is_null(), "libical will not convert {utc:?}");
        let rendered = i_cal_time_as_ical_string(local);
        assert!(!rendered.is_null(), "libical will not render {utc:?}");
        let text = CStr::from_ptr(rendered).to_string_lossy().into_owned();
        g_free(rendered.cast());
        g_object_unref(local.cast());
        g_object_unref(instant.cast());
        local_date_time(&text)
    }
}

/// `20260901T090000` as `2026-09-01T09:00:00`, the two formats' spellings of
/// the same wall clock time. Anything else comes back as it was, so that a
/// mismatch shows the value rather than a mangling of it.
fn local_date_time(stamp: &str) -> String {
    match (stamp.len(), stamp.split_once('T')) {
        (15, Some((date, time))) => format!(
            "{}-{}-{}T{}:{}:{}",
            &date[..4],
            &date[4..6],
            &date[6..],
            &time[..2],
            &time[2..4],
            &time[4..]
        ),
        _ => stamp.to_owned(),
    }
}

/// A recurring appointment in `tzid`, in a document defining that zone the way
/// a saved one does, ending at the UTC instant `until`.
fn recurring_in(definition: &str, tzid: &str, until: &str) -> String {
    format!(
        "BEGIN:VCALENDAR\r\n\
         VERSION:2.0\r\n\
         PRODID:-//evolution-jmap//JMAP calendar backend//EN\r\n\
         {definition}\
         BEGIN:VEVENT\r\n\
         UID:K1\r\n\
         SUMMARY:Standup\r\n\
         DTSTART;TZID={tzid}:20260115T090000\r\n\
         RRULE:FREQ=WEEKLY;UNTIL={until}\r\n\
         END:VEVENT\r\n\
         END:VCALENDAR\r\n"
    )
}

/// The floor below which a green run would be saying nothing.
///
/// Every assertion here is inside a loop over what the filesystem holds, so a
/// missing `/usr/share/zoneinfo`, a libical built with its own table under
/// another path, or a walk that quietly stopped filtering everything out would
/// all pass by examining nothing. tzdata has shipped upwards of five hundred
/// zones for many years; four hundred is well under that and well over zero.
const FLOOR: usize = 400;

#[test]
fn every_zone_libical_ships_converts_a_recurrences_utc_until() {
    let mut examined = 0;
    for name in zone_names() {
        let Some((definition, tzid)) = definition(&name) else {
            continue;
        };
        examined += 1;
        for until in PROBES {
            let ics = recurring_in(&definition, &tzid, until);
            let rules = jmap_ical::ical_to_event(&ics)
                .expect("the envelope is a calendar object")
                .recurrence_rule
                .expect("the rule survived the mapping");

            assert_eq!(
                rules.until.as_deref(),
                Some(libical_reads(&name, until).as_str()),
                "{name} at {until} does not read the way libical reads it"
            );
            assert!(
                jmap_ical::maps_recurrence_rule(&rules),
                "{name} at {until} cannot be sent, so a create carrying it is refused"
            );
        }
    }
    assert!(
        examined >= FLOOR,
        "only {examined} zones were examined, which is too few for this to mean anything — \
         is {ZONEINFO} still where libical's table is?"
    );
}
