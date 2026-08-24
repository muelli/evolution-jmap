// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Every zone an appointment can be in, through the one conversion that needs
//! to know what a zone's offset *was*.
//!
//! RFC 5545 §3.3.10 states a recurrence's `UNTIL` as a UTC instant beside a
//! zoned `DTSTART`, RFC 8984 §4.3.3 states it as a local time in that zone, and
//! `jmap-ical` converts between them by reading the transitions out of the
//! `VTIMEZONE` in the same object — deliberately, rather than by shipping a
//! zone database. A definition written in a shape it will not count costs the
//! whole conversion: the end of the series is kept verbatim, `maps_recurrence_
//! rule` refuses it, and the user is told their recurring appointment cannot be
//! saved.
//!
//! Which makes "what shapes are actually written" a question about the
//! *producer*, and on the save path the producer is libical:
//! `marshal::icalendar_from_instances` copies libical's own builtin
//! `VTIMEZONE` into the save envelope for every `TZID` the components refer to.
//! So this drives every zone libical ships, not a handful chosen here —
//! `jmap-ical`'s own tests spell out the individual shapes, and this is the
//! measurement that says whether that list is the list that occurs.
//!
//! Two assertions per instant, and the second is the point of the first: that
//! the conversion happened, and that it agrees with **libical's own** answer
//! for the same instant in the same zone. A conversion that is merely produced
//! is worth nothing — an offset an hour out reads as a perfectly ordinary
//! `UNTIL` all the way to the server.

use std::ffi::{CStr, CString};
use std::path::PathBuf;

use eds_sys::{
    i_cal_time_as_ical_string, i_cal_time_convert_to_zone, i_cal_time_new_from_string,
    i_cal_timezone_get_builtin_timezone, i_cal_timezone_get_component,
};
use glib_sys::g_free;
use gobject_sys::g_object_unref;
use jmap_backend_cal::marshal;

/// Where tzdata keeps the zones libical resolves names out of. libical is built
/// against the system database here rather than with its own copy, so this
/// directory *is* its table; see [`zone_names`] for what happens if it is not
/// there.
const ZONEINFO: &str = "/usr/share/zoneinfo";

/// Instants spread across a year, each one a `UNTIL` in its own right.
///
/// Four rather than one because a zone's offset is a function of the instant:
/// a single probe in August would let every southern-hemisphere zone through
/// on its winter offset. Two of them sit a day either side of the last Sunday
/// in March, which is when most of Europe moves, so a rule read off by one
/// transition shows up as an hour rather than as nothing at all.
const PROBES: [&str; 4] = [
    "20260128T070000Z",
    "20260328T233000Z",
    "20260329T233000Z",
    "20260901T070000Z",
];

/// Every IANA name the system database holds, in sorted order.
///
/// Walked rather than read out of `zone.tab`, which lists only the canonical
/// names of *countries* — `Eire`, `GB`, `Etc/GMT-14` and the rest resolve just
/// as well and are just as much a zone an appointment can be in. `posix/` and
/// `right/` are the same zones again under two interpretations of leap seconds,
/// which libical does not resolve by those names.
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

/// libical's `VTIMEZONE` for a zone, rendered, and the `TZID` it carries — the
/// solidus-prefixed identifier libical writes for its own builtin zones, which
/// is what a `DTSTART` Evolution wrote names.
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

/// What libical makes of `utc` in the zone `name`, as an RFC 8984 §1.4.4
/// LocalDateTime — the answer `jmap-ical` has to agree with.
///
/// This is the oracle, and it is deliberately a different implementation of the
/// same question: libical resolves the instant through the zone object it built
/// out of tzdata's binary file, while `jmap-ical` reads the transition rules out
/// of the `VTIMEZONE` text. Two ways of asking, one answer.
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
