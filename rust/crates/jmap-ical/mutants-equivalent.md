<!--
SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
SPDX-License-Identifier: GPL-3.0-or-later
-->

# Genuinely Equivalent Mutants in `jmap-ical`

## `src/event.rs`

- `crates/jmap-ical/src/event.rs:174:23: replace > with >= in fold_overlong_lines`: `cut` is of type `usize` so `cut >= 0` is always true; when `cut == 0`, `!rest.is_char_boundary(0)` is always false, terminating the loop identically.
- `crates/jmap-ical/src/event.rs:2083:9: delete field show_without_time from struct CalendarEvent expression in modified_instance`: `show_without_time` on recurrence override instances is never read during iCalendar rendering (instance date formatting is governed by `as_a_date` derived from the parent event's `shows_without_time`).
- `crates/jmap-ical/src/event.rs:2117:9: delete field color from struct CalendarEvent expression in modified_instance`: `color` (RFC 8984 Section 4.1.7) has no mapping to RFC 5545 components and is never inspected or emitted during override instance rendering.
- `crates/jmap-ical/src/event.rs:2118:9: delete field locale from struct CalendarEvent expression in modified_instance`: `locale` (RFC 8984 Section 4.1.8) has no mapping to RFC 5545 components and is never inspected or emitted during override instance rendering.
- `crates/jmap-ical/src/event.rs:2119:9: delete field localizations from struct CalendarEvent expression in modified_instance`: `localizations` (RFC 8984 Section 4.1.9) is omitted from iCalendar rendering and is never inspected during override instance rendering.
- `crates/jmap-ical/src/event.rs:2857:19: replace > with >= in unique_tzid_to_iana`: `tzid` starts with `/` so the first part in `tzid.split('/')` is `""`, which never matches `IANA_AREAS`, `"UTC"`, or `"GMT"`; for all subsequent parts `offset > 0`.
- `crates/jmap-ical/src/event.rs:3219:9: delete field calendar_ids from struct CalendarEvent expression in read_vevent`: `calendar_ids` is explicitly assigned `None`, which is identical to the default value supplied by `..CalendarEvent::default()`.
- `crates/jmap-ical/src/event.rs:3225:9: delete field version from struct CalendarEvent expression in read_vevent`: `version` is explicitly assigned `None`, which is identical to the default value supplied by `..CalendarEvent::default()`.
- `crates/jmap-ical/src/event.rs:3232:9: delete field created from struct CalendarEvent expression in read_vevent`: `created` is explicitly assigned `None`, which is identical to the default value supplied by `..CalendarEvent::default()`.
- `crates/jmap-ical/src/event.rs:3233:9: delete field updated from struct CalendarEvent expression in read_vevent`: `updated` is explicitly assigned `None`, which is identical to the default value supplied by `..CalendarEvent::default()`.
- `crates/jmap-ical/src/event.rs:3265:9: delete field participants from struct CalendarEvent expression in read_vevent`: `participants` is explicitly assigned `None`, which is identical to the default value supplied by `..CalendarEvent::default()`.
- `crates/jmap-ical/src/event.rs:3273:9: delete field recurrence_overrides from struct CalendarEvent expression in read_vevent`: `recurrence_overrides` is explicitly assigned `None`, which is identical to the default value supplied by `..CalendarEvent::default()`.
- `crates/jmap-ical/src/event.rs:3279:9: delete field time_zones from struct CalendarEvent expression in read_vevent`: `time_zones` is explicitly assigned `None`, which is identical to the default value supplied by `..CalendarEvent::default()`.
- `crates/jmap-ical/src/event.rs:3280:9: delete field extra from struct CalendarEvent expression in read_vevent`: `extra` is explicitly assigned `Default::default()`, which is identical to the default value supplied by `..CalendarEvent::default()`.
- `crates/jmap-ical/src/event.rs:4118:42: replace || with && in date_time_digits`: `time.len() < 6` implies `!time.is_char_boundary(6)` because index 6 is beyond string length; since `B` implies `D`, `A || B || C || D` is logically equivalent to `A || (B && C) || D`.
