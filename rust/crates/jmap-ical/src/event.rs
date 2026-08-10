// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! JSCalendar [`CalendarEvent`] ↔ iCalendar `VEVENT`.
//!
//! The mapped set is the one the calendar backend needs to be useful — UID,
//! SUMMARY, DESCRIPTION, DTSTART (with its time zone), DURATION, STATUS and
//! RRULE — and no more. Everything else on an event (participants, alarms,
//! locations, links, …) is *dropped*, which is only safe because saving goes
//! back to the server as a PatchObject naming the mapped properties: a
//! property we never mapped is a property we never overwrite. See
//! [`MAPPED_PROPERTIES`] and [`maps_recurrence_rule`], which are that
//! knowledge in machine-readable form.
//!
//! Nothing here fails on unrecognised input. A property whose value the
//! mapping cannot read is treated as absent, because an event that loses a
//! field is better than a calendar that refuses to open; only a document
//! without any `VEVENT` in it is an error.

use jmap_proto::calendars::{CalendarEvent, RecurrenceRule};

use crate::error::ICalError;
use crate::syntax::{self, Component, Property};

/// Carries the JSCalendar `uid` when the iCalendar `UID` is taken by the JMAP
/// id, mirroring `X-JMAP-UID` on the address book side.
const X_JMAP_UID: &str = "X-JMAP-UID";

/// The `PRODID` of every calendar this crate emits.
const PRODID: &str = "-//evolution-jmap//JMAP calendar backend//EN";

/// The JSCalendar spelling of `Etc/UTC`, the one the client and the mock use.
const UTC: &str = "Etc/UTC";

/// JSCalendar `status` values and their iCalendar `STATUS` spelling. Both sets
/// are closed, so a value outside this table is dropped rather than passed
/// through in the other format's clothes.
const STATUSES: [(&str, &str); 3] = [
    ("confirmed", "CONFIRMED"),
    ("cancelled", "CANCELLED"),
    ("tentative", "TENTATIVE"),
];

/// The JSCalendar properties this mapping covers, and therefore the only ones
/// a save may name in a `CalendarEvent/set` update patch.
pub const MAPPED_PROPERTIES: [&str; 6] = [
    "title",
    "description",
    "start",
    "timeZone",
    "duration",
    "status",
];

/// Whether a recurrence rule survives the trip through iCalendar.
///
/// Only `frequency`, `interval`, `count` and `until` are modeled; `byDay` and
/// the rest of RFC 8984 §4.3.3 ride in [`RecurrenceRule::extra`] and would be
/// lost. A caller that patches `recurrenceRules` for a rule this returns
/// `false` for narrows the user's recurrence behind their back.
///
/// A rule [`rule_to_rrule`] refuses outright fails this too, so the save path
/// never patches over a recurrence the user was not shown.
pub fn maps_recurrence_rule(rule: &RecurrenceRule) -> bool {
    rule.extra.is_empty() && writable(rule)
}

/// Whether an `RRULE` can carry this rule's frequency and its end — the two
/// parts that cannot be narrowed, only lost.
///
/// A rule that names no frequency has no `RRULE` spelling at all: an empty one
/// is rejected by libical and means nothing to a reader. A rule whose `until`
/// is not a date-time this mapping can write must be refused rather than
/// written without it, because a recurrence that ends and one that never does
/// are different events, and the unbounded one repeats into every week of the
/// user's calendar for ever.
fn writable(rule: &RecurrenceRule) -> bool {
    !rule.frequency.is_empty()
        && rule
            .until
            .as_deref()
            .is_none_or(|until| to_ical_date_time(until).is_some())
}

/// Render an event as an iCalendar object, ready for
/// `i_cal_component_new_from_string()`.
pub fn event_to_ical(event: &CalendarEvent) -> String {
    let mut vevent = Component::new("VEVENT");

    // EDS keys its cache on the iCalendar UID and passes it back to
    // load_component_sync()/remove_component_sync(), so it has to be the
    // identifier the JMAP methods take — the server-assigned id. The
    // JSCalendar uid, which is a different namespace, rides alongside; before
    // the first CalendarEvent/set there is no id and it stands in.
    if let Some(uid) = event
        .id
        .as_ref()
        .map(|id| id.as_str())
        .or(event.uid.as_deref())
    {
        vevent = vevent.with(Property::new("UID", uid));
    }
    if let Some(uid) = &event.uid {
        vevent = vevent.with(Property::new(X_JMAP_UID, uid));
    }

    for (name, value) in [
        ("SUMMARY", &event.title),
        ("DESCRIPTION", &event.description),
    ] {
        if let Some(value) = value.as_deref().filter(|value| !value.is_empty()) {
            vevent = vevent.with(Property::new(name, value));
        }
    }

    if let Some(start) = event.start.as_deref().and_then(to_ical_date_time) {
        let zone = event.time_zone.as_deref();
        vevent = vevent.with(match zone {
            // Form 2, a UTC instant. Form 3 with TZID=Etc/UTC would be legal
            // but obliges us to ship a VTIMEZONE for it.
            Some(zone) if is_utc(zone) => Property::raw("DTSTART", &format!("{start}Z")),
            // Form 3. libical resolves an IANA name from its built-in zone
            // table, so no VTIMEZONE is emitted; a zone it does not know falls
            // back to floating on its side, which is the same guess we would
            // have to make.
            Some(zone) => Property::raw("DTSTART", &start).with_param("TZID", zone),
            // Form 1, floating. Inventing UTC here would move the event.
            None => Property::raw("DTSTART", &start),
        });
    }

    if let Some(duration) = event.duration.as_deref().filter(|value| !value.is_empty()) {
        // ISO 8601 durations, spelled identically on both sides.
        vevent = vevent.with(Property::raw("DURATION", duration));
    }

    if let Some(status) = event.status.as_deref().and_then(|status| {
        STATUSES
            .iter()
            .find(|(jscalendar, _)| jscalendar.eq_ignore_ascii_case(status))
    }) {
        vevent = vevent.with(Property::raw("STATUS", status.1));
    }

    for rule in event.recurrence_rules.iter().flatten() {
        if let Some(value) = rule_to_rrule(rule, event.time_zone.as_deref()) {
            vevent = vevent.with(Property::raw("RRULE", &value));
        }
    }

    Component::new("VCALENDAR")
        .with(Property::raw("VERSION", "2.0"))
        .with(Property::raw("PRODID", PRODID))
        .with_child(vevent)
        .to_ics()
}

/// Read an iCalendar object's first `VEVENT` into a calendar event.
///
/// The `id` is whatever the component's `UID` says, which for an event
/// Evolution has just created is a locally invented string rather than a JMAP
/// id — the caller knows which case it is in and must drop it before sending
/// a create.
pub fn ical_to_event(text: &str) -> Result<CalendarEvent, ICalError> {
    let calendar = syntax::parse(text)?;
    let vevent = calendar.child("VEVENT").ok_or(ICalError::NoEvent)?;

    let text = |name: &str| vevent.text(name).filter(|value| !value.is_empty());
    let (start, time_zone) = read_start(vevent);

    let rules: Vec<RecurrenceRule> = vevent
        .all("RRULE")
        .into_iter()
        .filter_map(|property| rrule_to_rule(&property.raw_value()))
        .collect();

    Ok(CalendarEvent {
        id: text("UID").map(Into::into),
        // Membership follows from which EDS source is being served, not from
        // the component, so the backend fills it in on create.
        calendar_ids: None,
        event_type: Some("Event".to_owned()),
        uid: text(X_JMAP_UID),
        title: text("SUMMARY"),
        description: text("DESCRIPTION"),
        start,
        time_zone,
        duration: vevent
            .property("DURATION")
            .map(Property::raw_value)
            .filter(|value| !value.is_empty()),
        status: vevent.text("STATUS").and_then(|status| {
            STATUSES
                .iter()
                .find(|(_, ical)| ical.eq_ignore_ascii_case(&status))
                .map(|(jscalendar, _)| (*jscalendar).to_owned())
        }),
        recurrence_rules: (!rules.is_empty()).then_some(rules),
        extra: Default::default(),
    })
}

/// The event's start as a JSCalendar LocalDateTime and its time zone.
fn read_start(vevent: &Component) -> (Option<String>, Option<String>) {
    let Some(property) = vevent.property("DTSTART") else {
        return (None, None);
    };
    let value = property.raw_value();
    let Some(start) = to_local_date_time(&value) else {
        return (None, None);
    };
    let zone = match value.ends_with('Z') {
        true => Some(UTC.to_owned()),
        false => property
            .param("TZID")
            .filter(|zone| !zone.is_empty())
            .map(str::to_owned),
    };
    (Some(start), zone)
}

fn is_utc(zone: &str) -> bool {
    zone.eq_ignore_ascii_case(UTC) || zone.eq_ignore_ascii_case("UTC")
}

/// `2026-01-15T13:00:00` → `20260115T130000`.
fn to_ical_date_time(local: &str) -> Option<String> {
    let (date, time) = local.split_once('T')?;
    let date: String = strip(date, '-', 8)?;
    let time: String = strip(time, ':', 6)?;
    exists(&date, &time).then(|| format!("{date}T{time}"))
}

/// `20260115T130000`, `20260115T130000Z` or `20260115` (`VALUE=DATE`) →
/// `2026-01-15T13:00:00`. A date without a time is read as midnight:
/// `showWithoutTime` is not modeled yet, and an all-day event that lost its
/// start entirely would be worse than one pinned to the top of the day.
fn to_local_date_time(value: &str) -> Option<String> {
    let value = value.strip_suffix(['Z', 'z']).unwrap_or(value);
    let (date, time) = match value.split_once('T') {
        Some((date, time)) => (date, time),
        None => (value, "000000"),
    };
    if date.len() != 8 || time.len() < 6 {
        return None;
    }
    // Sub-second precision is legal in neither format's DATE-TIME, but a
    // trailing fraction is easy to ignore and hard to guess at.
    let time = &time[..6];
    if !date.bytes().chain(time.bytes()).all(|b| b.is_ascii_digit()) {
        return None;
    }
    if !exists(date, time) {
        return None;
    }
    Some(format!(
        "{}-{}-{}T{}:{}:{}",
        &date[..4],
        &date[4..6],
        &date[6..],
        &time[..2],
        &time[2..4],
        &time[4..],
    ))
}

/// Remove `separator` and check that exactly `digits` digits are left.
fn strip(value: &str, separator: char, digits: usize) -> Option<String> {
    let stripped: String = value.chars().filter(|c| *c != separator).collect();
    (stripped.len() == digits && stripped.bytes().all(|b| b.is_ascii_digit())).then_some(stripped)
}

/// Whether `YYYYMMDD` and `HHMMSS` digits name an instant that exists.
///
/// Digits in the right places are not a date, and neither format's reader
/// checks which: calcard reads `20261315T250000` into a date-time whose month
/// is 13, and libical is asked for the value only after this mapping has
/// written it. So the check is here, and a month of 13 is treated the same way
/// as a value that cannot be read at all — as absent — because both directions
/// are worse than losing the property. Handed to libical, an impossible
/// `DTSTART` costs the whole component and with it every field of the event;
/// sent to the server, `"start": "2026-13-15T25:00:00"` is not a JSCalendar
/// LocalDateTime and fails the entire `CalendarEvent/set`, so the user's edit
/// to the title is lost along with it.
///
/// Both callers have already established that the arguments are 8 and 6 ASCII
/// digits.
fn exists(date: &str, time: &str) -> bool {
    let field = |value: &str| value.parse::<u32>().unwrap_or(u32::MAX);
    let (year, month, day) = (field(&date[..4]), field(&date[4..6]), field(&date[6..8]));
    let (hour, minute, second) = (field(&time[..2]), field(&time[2..4]), field(&time[4..6]));
    (1..=12).contains(&month)
        && (1..=days_in_month(year, month)).contains(&day)
        && hour <= 23
        && minute <= 59
        // RFC 5545 §3.3.12 and RFC 3339 §5.6 both allow the leap second, and a
        // server that stores one has to get it back unchanged.
        && second <= 60
}

/// The length of a month, in the proleptic Gregorian calendar both formats use.
fn days_in_month(year: u32, month: u32) -> u32 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if year.is_multiple_of(4) && (!year.is_multiple_of(100) || year.is_multiple_of(400)) => {
            29
        }
        2 => 28,
        _ => 0,
    }
}

/// An `RRULE` value, or `None` for a rule [`writable`] refuses.
///
/// A rule carrying unmodeled parts in `extra` *is* written, narrowed to what an
/// `RRULE` holds — showing a weekly event on the wrong days beats showing none
/// — and [`maps_recurrence_rule`] is how the save path knows not to write that
/// narrowing back.
fn rule_to_rrule(rule: &RecurrenceRule, time_zone: Option<&str>) -> Option<String> {
    if !writable(rule) {
        return None;
    }
    let mut parts = vec![format!("FREQ={}", rule.frequency.to_ascii_uppercase())];
    // INTERVAL=1 is the RFC 5545 default and only makes the line longer.
    if let Some(interval) = rule.interval.filter(|interval| *interval != 1) {
        parts.push(format!("INTERVAL={interval}"));
    }
    if let Some(count) = rule.count {
        parts.push(format!("COUNT={count}"));
    }
    if let Some(until) = rule.until.as_deref().and_then(to_ical_date_time) {
        // JSCalendar's `until` is a local time in the event's own zone, so it
        // is spelled the way DTSTART is. RFC 5545 §3.3.10 asks for a UTC
        // instant when DTSTART carries a TZID; converting one would need a
        // zone database, which this crate deliberately does not depend on, so
        // a zoned event's UNTIL stays local. It round-trips, and libical reads
        // it in the event's zone.
        let suffix = match time_zone {
            Some(zone) if is_utc(zone) => "Z",
            _ => "",
        };
        parts.push(format!("UNTIL={until}{suffix}"));
    }
    Some(parts.join(";"))
}

/// The reverse. Parts outside the modeled set are dropped rather than parked
/// in `extra`: a `BYDAY=MO` copied verbatim into JSCalendar would be rejected
/// by the server, whose `byDay` is an array of objects.
fn rrule_to_rule(value: &str) -> Option<RecurrenceRule> {
    let mut rule = RecurrenceRule::default();
    for part in value.split(';') {
        let Some((key, value)) = part.split_once('=') else {
            continue;
        };
        match key.to_ascii_uppercase().as_str() {
            "FREQ" => rule.frequency = value.to_ascii_lowercase(),
            "INTERVAL" => rule.interval = value.parse().ok(),
            "COUNT" => rule.count = value.parse().ok(),
            "UNTIL" => rule.until = to_local_date_time(value),
            _ => {}
        }
    }
    if rule.frequency.is_empty() {
        return None;
    }
    rule.rule_type = Some("RecurrenceRule".to_owned());
    Some(rule)
}
