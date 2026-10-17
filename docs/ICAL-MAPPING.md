<!--
SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
SPDX-License-Identifier: GPL-3.0-or-later
-->

# iCalendar to JSCalendar mapping

How `jmap-ical` maps the iCalendar that Evolution Data Server (EDS) hands a
backend to a JMAP `CalendarEvent`, and back.

- iCalendar: RFC 5545, with RFC 7986 (`CONFERENCE`, `IMAGE`), RFC 9074
  (`VALARM` `UID`) and RFC 8607 (`SIZE`), parsed and written with `calcard`.
- JSCalendar: RFC 8984 and draft-ietf-calext-jscalendarbis-22 ("bis").
- JMAP for Calendars: draft-ietf-jmap-calendars-33. Its §5 defines a
  `CalendarEvent` as a bis Event.

Where the code lives:

| Crate / file | Role |
| :--- | :--- |
| `jmap-ical/src/event.rs` | `event_to_ical`, `ical_to_event`, `scheduling_ical`, the `maps_*` predicates |
| `jmap-ical/src/zone.rs` | Offset in force at an instant, from a document's own `VTIMEZONE` |
| `jmap-ical/src/freebusy.rs` | `BusyPeriod` to `VFREEBUSY` |
| `jmap-cal-sync/src/patch.rs` | Edit: turns an edited component into a `CalendarEvent/set` PatchObject |
| `jmap-cal-sync/src/lib.rs` | Create path (`save_component_with`) |
| `jmap-proto/src/calendars.rs` | The `CalendarEvent` type |

RFC section numbers below are RFC 8984 unless a bis or JMAP section is named.

---

## 1. Design

1. **Edits are patches, not replacements.** A `VEVENT` shows only part of a
   JSCalendar event. A save sends a PatchObject that names only properties in
   `MAPPED_PROPERTIES` (19 entries). Everything else on the server stays.
2. **The diff baseline is the server's event after one round trip.**
   `patch::diff` compares the edited event with
   `ical_to_event(event_to_ical(server_event))`, not with the server event.
   That baseline is exactly what Evolution was shown, so normalisation done by
   the mapping (`UTC` read back as `Etc/UTC`, `INTERVAL=1` dropped, an unknown
   `status` dropped) never shows up as an edit.
3. **Predicates guard partly shown properties.** When a property can be shown
   only in part, a predicate says so and the save leaves that property alone:
   `maps_locations`, `maps_virtual_locations`, `maps_keyword`, `maps_alerts`,
   `maps_recurrence_rule`, `maps_recurrence_override`,
   `sends_recurrence_override` and `maps_time_zone`.
4. **Map keys travel with the line.** `LOCATION`, `CONFERENCE`, `ATTACH` and
   `IMAGE` carry `X-JMAP-KEY=<key>`. A `VALARM` carries the key as its RFC 9074
   §4 `UID`. A key that is not an RFC 8984 §1.4.1 `Id` (1 to 255 octets of
   `A-Za-z0-9_-`) is read as absent. A line with no usable key gets an
   invented one: `l1` (location), `v1, v2, ...` (conference), `k1, k2, ...`
   (link), `a1, a2, ...` (alert), `p1, p2, ...` (participant). Invented keys
   skip keys that the document already names.
5. **Unreadable means absent.** A value the mapping cannot read is dropped, not
   passed through and not an error. `ical_to_event` fails only for a broken
   document or one with no `VEVENT` (section 9.3).
6. **Fixed point.** A second round trip changes nothing (section 6).

---

## 2. Property table

"In" is `ical_to_event`, "Out" is `event_to_ical`.

| iCalendar | JSCalendar | In | Out | Section |
| :--- | :--- | :--- | :--- | :--- |
| `UID` | `id` (or `uid`) | `id` | `id`, else `uid` | 3.1 |
| `X-JMAP-UID` | `uid` | yes | when `uid` is set | 3.1 |
| `SUMMARY` | `title` | yes, empty is absent | when not empty | |
| `DESCRIPTION` | `description` | yes, empty is absent | when not empty | |
| `DTSTART` | `start`, `timeZone`, `showWithoutTime` | yes | yes | 3.2 |
| `DURATION` | `duration` | preferred | always used for the length | 3.2 |
| `DTEND` | `duration` | when no valid `DURATION` | never | 3.2 |
| `STATUS` | `status` | closed vocabulary | closed vocabulary | 3.3 |
| `TRANSP` | `freeBusyStatus` | closed vocabulary | closed vocabulary | 3.3 |
| `PRIORITY` | `priority` | 0 to 9 | 0 to 9 | 3.3 |
| `CLASS` | `privacy` | closed vocabulary | closed vocabulary | 3.3 |
| `LOCATION` | `locations` (one entry) | yes | first named entry | 3.4 |
| `CONFERENCE` | `virtualLocations` | every line | every entry with a URI | 3.5 |
| `ATTACH` | `links` | URI values only | non-icon links | 3.6 |
| `IMAGE` | `links` with `rel: "icon"` | yes | icon links | 3.6 |
| `CATEGORIES` | `keywords` | every line | one line | 3.7 |
| `ORGANIZER` | owner participant, `organizerCalendarAddress` | yes | first owner | 3.8 |
| `ATTENDEE` | `participants` | yes | yes | 3.8 |
| `RRULE` | `recurrenceRule` | first readable line | yes | 3.9 |
| `EXDATE` | `recurrenceOverrides` `{"excluded": true}` | yes | yes | 8 |
| `RDATE` | `recurrenceOverrides` `{}` or `{"duration": ...}` | yes | yes | 8 |
| `VEVENT` with `RECURRENCE-ID` | `recurrenceOverrides` patch | yes | yes | 8 |
| `VALARM` | `alerts` | `ACTION:DISPLAY` only | display alerts only | 7 |
| `VTIMEZONE` | `timeZones` | custom zones only | custom zones only | 4 |
| `CREATED` | `created` | no | yes | 9.1 |
| `DTSTAMP`, `LAST-MODIFIED` | `updated` | no | yes | 9.1 |
| `VERSION`, `PRODID` | | ignored | `VERSION:2.0`, own `PRODID` | 9.1 |
| `METHOD` | | ignored | only from `scheduling_ical` | 9.1 |
| `VFREEBUSY` | `BusyPeriod` | | yes | 3.10 |

Everything else is not mapped: see section 9.2.

---

## 3. Fields

### 3.1 Identity

- `UID` carries the JMAP `id`, because EDS keys its cache on `UID` and passes
  it back to `load_component_sync` and `remove_component_sync`. Before the
  first `CalendarEvent/set` there is no `id`, and `uid` stands in.
- `X-JMAP-UID` carries the JSCalendar `uid`. On read, `UID` becomes `id` and
  `X-JMAP-UID` becomes `uid`.
- A detached instance carries the series' `UID`.
- **Create:** the `UID` of a new Evolution event is a local name, not a JMAP
  id. `save_component_with` moves it into `uid` (unless `X-JMAP-UID` already
  set one), so the identity that iTIP mail quotes survives, and lets the
  server assign `id`.

Tests: `uid_is_the_jmap_id_and_the_jscalendar_uid_is_kept_aside`,
`a_new_event_with_no_id_is_identified_by_its_jscalendar_uid` (`jmap-ical`),
`a_new_events_icalendar_uid_becomes_the_jscalendar_uid` (`jmap-cal-sync/tests/save.rs`).

### 3.2 Start, duration, all-day

**Reading `DTSTART`** (`read_start`):

| Value | `start` | `timeZone` | `showWithoutTime` |
| :--- | :--- | :--- | :--- |
| Date only (`20260115`, with or without `VALUE=DATE`) | `2026-01-15T00:00:00` | none (any `TZID` ignored, RFC 5545 §3.2.19) | `true` |
| Date-time ending in `Z` | local digits | `Etc/UTC` | absent |
| Date-time with `TZID` | local digits | resolved as in section 4 | absent |
| Date-time without `TZID` | local digits | none (floating) | absent |

A date that does not exist (month 13, 30 February, hour 25) is read as no
start. A leap second (`:60`) is accepted.

**Writing `DTSTART`** (`dated`; `EXDATE`, `RDATE` and `RECURRENCE-ID` use the
same function so that all of them agree):

| Event | Written as |
| :--- | :--- |
| All-day (see below) | `DTSTART;VALUE=DATE:20260115` |
| `timeZone` is `Etc/UTC` or `UTC` | `DTSTART:20260115T130000Z` |
| Other `timeZone` | `DTSTART;TZID=<timeZone>:20260115T130000` |
| No `timeZone` | `DTSTART:20260115T130000` (floating) |

**All-day.** `showWithoutTime: true` is written as a DATE only when RFC 5545
can hold it (`shows_without_time`): no `timeZone`, start at midnight, a
duration of whole days (or none), an `UNTIL` at midnight, no
`BYHOUR`/`BYMINUTE`/`BYSECOND` in the written rule, and every override at
midnight with whole-day lengths and no zone of its own. Otherwise the event is
written as a timed event. The diff baseline loses the flag in the same way, so
a save does not clear it on the server.

**Duration.**

- Out: always `DURATION`, never `DTEND`.
- In: `DURATION` if it is a valid length, else `DTEND - DTSTART`.
- `stated_duration` accepts `P[nW][nD][T[nH][nM][nS]]` with each unit at most
  once, in that order, in either case. It drops a leading `+` and leading
  zeros, writes any all-zero length as `PT0S` and refuses a negative length
  (RFC 8984 §1.4.6 has none).
- `DTEND - DTSTART` is taken on the wall clock: whole days become `nD`, the
  rest `TnHnMnS`. A difference of zero or less gives no duration.

Known loss: RFC 8984 §1.4.6 treats hours, minutes and seconds as exact time.
The wall-clock difference is therefore one hour off for an event that spans a
daylight-saving change (`01:00` to `04:00` on 25 October 2026 in
`Europe/Berlin` is read as `PT3H`; the real length is 4 hours). A `DTEND` in a
different zone than `DTSTART` is off by the offset between the two zones.
Evolution writes both ends in one zone. Pinned by
`differential_oracle_dst_transition_spanning_event_duration_and_roundtrip_stability`
(fixture `dst_transition_spanning_export.ics`).

Tests: `an_events_length_may_arrive_as_a_dtend_instead_of_a_duration`,
`a_duration_wins_over_a_dtend_that_contradicts_it`,
`an_all_day_event_the_date_form_cannot_hold_stays_a_date_time`,
`utc_is_spelled_with_a_trailing_z_and_no_tzid`.

### 3.3 Closed vocabularies

Values outside each table are dropped in both directions. Case is ignored on
read (RFC 5545 §3.1 makes enumerated values case-insensitive).

| JSCalendar | iCalendar | Notes |
| :--- | :--- | :--- |
| `status`: `confirmed`, `tentative`, `cancelled` | `STATUS`: `CONFIRMED`, `TENTATIVE`, `CANCELLED` | |
| `freeBusyStatus`: `busy`, `free` | `TRANSP`: `OPAQUE`, `TRANSPARENT` | Both formats default to busy/opaque, so an absent value is written as no line. |
| `priority`: 0 to 9 | `PRIORITY`: 0 to 9 | Same meaning (0 undefined, 1 highest). 0 is written as `PRIORITY:0`. |
| `privacy`: `public`, `private`, `secret` | `CLASS`: `PUBLIC`, `PRIVATE`, `CONFIDENTIAL` | `public` is written although it is the default, because Evolution writes `CLASS` on every save. `CLASS:secret` is not read as `secret`. |

Because Evolution writes `CLASS` on every save, the first save of an event that
has no `privacy` sends `privacy: "public"` once. That value is the default, so
the write changes nothing.

### 3.4 Locations (`LOCATION`)

RFC 5545 §3.6.1 allows one `LOCATION` per `VEVENT`; JSCalendar has a map.

- Out: the first entry in key order that has a non-empty `name`, as
  `LOCATION;X-JMAP-KEY=<key>:<name>`.
- In: one entry `{"@type": "Location", "name": ...}` under the `X-JMAP-KEY`, or
  `l1`.
- `maps_locations` is true when the map has at most one entry, with a
  non-empty key, that is an object whose `name` is absent or a string.
- Save (`diff_locations`): renames the server's entry in place
  (`locations/<key>/name`, key taken from the server's event). Clearing the
  name removes the whole property if the entry held only `name` and `@type`,
  else only the name. A first location writes the property whole (RFC 8620
  §5.3 requires the parent path to exist).

Lossy: `coordinates`, `description`, `locationTypes`, `relativeTo`,
`timeZone` and every location after the first are not drawn. They stay on the
server because the save patches only the name. `GEO` is not read.
`mainLocationId` is not used to choose the drawn entry.

### 3.5 Virtual locations (`CONFERENCE`)

- Out: one line per entry whose `uri` is a URI (scheme, `:`, something after,
  no whitespace):
  `CONFERENCE;VALUE=URI;FEATURE=<...>;LABEL=<name>;X-JMAP-KEY=<key>:<uri>`.
  `VALUE=URI` is required by RFC 7986 §5.11.
- `features` (RFC 8984 §4.2.6) and `FEATURE` (RFC 7986 §6.3) share seven values:
  `audio`, `chat`, `feed`, `moderator`, `phone`, `screen`, `video`. They are
  written in that order. Other values are dropped.
- In: every `CONFERENCE` line. Only the property name `CONFERENCE` is read.
  `LABEL` becomes `name`.
- `maps_virtual_locations` is true when every entry has a non-empty key, a
  drawable `uri`, a string `name` (or none) and only known `features` set to
  `true`.
- Save (`diff_virtual_locations`): patches `virtualLocations/<key>/uri`,
  `/name` and `/features` of entries the server holds. A line that disappeared
  is not read as a deletion: Evolution has no UI for conferences and may drop
  the line. A line under a key the server does not hold is not created. The
  create path writes the property whole.

Lossy: `description` is not drawn and stays on the server.

### 3.6 Links (`ATTACH`, `IMAGE`)

RFC 8984 §4.2.7 keeps one `links` map. iCalendar splits it in two:

| Link | Written as |
| :--- | :--- |
| `rel: "icon"` | `IMAGE;VALUE=URI;DISPLAY=<display>;FMTTYPE=<type>;X-JMAP-KEY=<key>:<href>` (RFC 7986 §5.10) |
| anything else | `ATTACH;FMTTYPE=<type>;SIZE=<size>;X-JMAP-KEY=<key>:<href>` (RFC 5545 §3.8.1.1; `URI` is the default value type) |

- `href` must be a URI, or the link is not written.
- `FMTTYPE` is written only for a bare `type/subtype` made of RFC 6838 §4.2
  restricted-name characters. A type with parameters
  (`text/plain; charset=utf-8`) is left off.
- `SIZE` (RFC 8607 §4.1) is written only for a non-negative integer.
- `DISPLAY` values: `badge`, `graphic`, `fullsize`, `thumbnail` (RFC 7986 §6.1).
  `display` is accepted as a string or as a bis set.
- In: every `ATTACH` and `IMAGE` with a URI value. `IMAGE` gives `rel: "icon"`
  and `display: {"<value>": true}` (bis §1.5.11 set form). `SIZE` is read only
  on `ATTACH`. Keys are `X-JMAP-KEY` or `k1, k2, ...`.
- Dropped on read: inline binary attachments (`VALUE=BINARY`), and `file:`
  URIs, which is how Evolution refers to a file the user attached from disk.
  Sending a local path would expose it to every client of the account;
  uploading the file as a blob is not implemented.
- Save (`diff_links`): only `links/<key>/href`, and only when the server's
  entry under that key has the `href` the line was drawn from. `contentType`,
  `size` and `rel` are not written back. Missing lines are not deletions and
  unknown keys are not created.

Lossy: `title`, `cid` and the `FILENAME` parameter (RFC 8607 §4.2) are not
mapped.

### 3.7 Keywords (`CATEGORIES`)

- Out: one `CATEGORIES` line with every tag that `maps_keyword` accepts, in
  sorted order. No line when no tag is drawable.
- `maps_keyword` refuses a value other than `true`, an empty or
  whitespace-only tag, and a tag containing CR.
- In: every `CATEGORIES` line, split on commas. Each tag is trimmed; empty tags
  are dropped; duplicates collapse. Trimming on the first read gives the same
  result as `calcard`'s trim on the second read, so the round trip is a fixed
  point.
- Save (`diff_keywords`): the set is replaced whole. Tags the line could not
  show are copied from the server's set into the new one, so a save never
  deletes a tag the user did not see.

### 3.8 Participants (`ORGANIZER`, `ATTENDEE`)

**Out** (`drawn_participants`, map order):

- A participant's address is `calendarAddress`, else `sendTo.imip`. It must be
  a URI, or the participant is not written.
- The first participant with role `owner` gets `ORGANIZER;CN=<name>:<address>`.
  An owner with no other role the table below can write gets only that line.
  An owner with another role also gets an `ATTENDEE` line.
- Every other participant gets
  `ATTENDEE;CN;CUTYPE;ROLE;PARTSTAT;RSVP=TRUE:<address>`, each parameter only
  when known.

| JSCalendar | iCalendar |
| :--- | :--- |
| `roles` (first match wins): `chair`, `informational`, `optional`, `required`, `attendee` | `ROLE`: `CHAIR`, `NON-PARTICIPANT`, `OPT-PARTICIPANT`, `REQ-PARTICIPANT`, `REQ-PARTICIPANT` |
| `kind`: `individual`, `group`, `resource`, `location` | `CUTYPE`: `INDIVIDUAL`, `GROUP`, `RESOURCE`, `ROOM` |
| `participationStatus`: `needs-action`, `accepted`, `declined`, `tentative`, `delegated` | `PARTSTAT`: same words, upper case |
| `expectReply: true` | `RSVP=TRUE` |
| `name` (not empty) | `CN` |

**In** (`read_participants`, `read_organizer`):

- `ORGANIZER` becomes `organizerCalendarAddress` and a participant with
  `roles: {"owner": true}`. An `ATTENDEE` with the same address (compared
  case-insensitively) merges into that participant.
- Each `ATTENDEE` becomes a participant with `calendarAddress` (never the
  reserved `sendTo`), `name`, `kind`, `participationStatus` and
  `expectReply`, each only when readable.
- Roles: `REQ-PARTICIPANT`, no `ROLE` or an unknown `ROLE` give
  `{"attendee": true}`; `OPT-PARTICIPANT` gives `attendee` and `optional`;
  `CHAIR` gives `attendee` and `chair`; `NON-PARTICIPANT` gives
  `{"informational": true}`. These are the sets the writer draws each `ROLE`
  from, so the round trip is stable.
- A line whose value is not a URI is dropped. Keys are `p1, p2, ...` in
  document order, organizer first. No `X-JMAP-KEY` rides on these lines:
  guests are matched by address.

**Save** (`diff_participants`): both `participants` and
`organizerCalendarAddress` are in `MAPPED_PROPERTIES`.

- Each guest is matched by address to the server's own entry, and only the
  members an `ATTENDEE` line shows are patched
  (`participants/<key>/<member>`), so `sendTo`, `invitedBy` and scheduling
  members stay.
- Roles are compared as servers normalise them (section 10). Changed roles are
  written whole, keeping roles no `ROLE` can spell.
- An added guest gets a `pN` key the server does not use. A removed guest is
  removed by key. An address the server files under several keys is left
  alone.

Not mapped: `SENT-BY`, `DIR`, `DELEGATED-TO`/`-FROM`, `MEMBER`,
`SCHEDULE-AGENT`/`-STATUS`/`-FORCE-SEND`, non-iMIP `sendTo` methods.

Sending invitations is not part of the mapping: see `MEETING-SCHEDULING.md`.

Tests: `the_guest_list_of_a_meeting_is_read_back_as_participants`,
`each_role_reads_back_as_the_set_that_draws_it` (`jmap-ical`);
`jmap-cal-sync/tests/participants.rs`.

### 3.9 Recurrence rule (`RRULE`)

bis §3.3.3 has one `recurrenceRule`, where RFC 8984 §4.3.3 had a
`recurrenceRules` array. `jmap-proto` models the singular form. Only the first
readable `RRULE` is read; further lines are dropped (Evolution writes one).

| Part | JSCalendar | Written when |
| :--- | :--- | :--- |
| `FREQ` | `frequency` | always; a rule with no frequency is not written |
| `COUNT` | `count` | set |
| `UNTIL` | `until` | set and writable (section 5) |
| `INTERVAL` | `interval` | not 1 |
| `BYSECOND` / `BYMINUTE` / `BYHOUR` | `bySecond` / `byMinute` / `byHour` | values in 0..60 / 0..59 / 0..23 |
| `BYDAY` | `byDay` (NDay) | every token is a weekday; ordinal non-zero and only with `MONTHLY`/`YEARLY` |
| `BYMONTHDAY` | `byMonthDay` | values in -31..-1, 1..31; not with `WEEKLY` |
| `BYYEARDAY` | `byYearDay` | values in -366..-1, 1..366; not with `DAILY`/`WEEKLY`/`MONTHLY` |
| `BYWEEKNO` | `byWeekNo` | values in -53..-1, 1..53; `YEARLY` only |
| `BYMONTH` | `byMonth` | `1` to `12` without leading zero; leap months (`5L`) refused |
| `BYSETPOS` | `bySetPosition` | values in -366..-1, 1..366; only beside another written `BY` part |
| `WKST` | `firstDayOfWeek` | lowercase two-letter day; `mo` is not written |

- Each `BY` part is written whole or not at all. Dropping one value of a part
  describes a different recurrence.
- Parts follow libical's order (`COUNT`/`UNTIL` before `INTERVAL`, time parts
  before day parts), so a rule that comes back from EDS's cache compares equal.
  `WKST=MO` and `INTERVAL=1` are left off because libical drops them. Measured
  in `jmap-backend-cal/tests/marshal.rs`.
- `maps_recurrence_rule` is false for a rule with `rscale`, `skip`, unmodeled
  members, an unwritable `UNTIL`, or any `BY` part the writer would leave off.
  A rule it refuses is still written as far as possible.
- In: unknown parts are dropped (an `RSCALE=CHINESE` copied into JSCalendar
  would be rejected). An unreadable token is kept in a form the writer refuses
  (`0` for day numbers, `u32::MAX` for time parts, the whole token for `BYDAY`),
  so the save sees the rule as partly shown instead of narrowing it. `FREQ`
  must be one of the seven frequencies. An `UNTIL` that has no date-time shape
  at all ends parsing of the rule.
- Save: an edit replaces `recurrenceRule` whole, or leaves it alone when either
  the server's or the edited rule fails `maps_recurrence_rule`. A create with
  such a rule fails with `SyncError::Unsendable`; filing one occurrence where
  the user asked for a series would be worse.

### 3.10 Free/busy (`VFREEBUSY`)

`busy_periods_to_vfreebusy` renders `Principal/getAvailability` busy periods
(JMAP Calendars §2.2) as the bare `VFREEBUSY` that EDS's
`get_free_busy_sync` expects:

- `DTSTART`/`DTEND` from the requested window, `ATTENDEE:mailto:<address>`
  (`mailto:` added unless present, case-insensitively), and one
  `FREEBUSY;FBTYPE=<type>:<start>/<end>` per period, in the order given. Periods
  are not clipped to the window.
- `busyStatus` `tentative` becomes `BUSY-TENTATIVE`, `unavailable` becomes
  `BUSY-UNAVAILABLE`, anything else becomes `BUSY`.
- Fractional seconds are truncated. A window or period that cannot be read
  refuses the whole component (`None`), so no time is shown as free by mistake.
- No `UID` or `DTSTAMP`, as in `ecmb_get_free_busy_sync`.

Tests: `jmap-ical/tests/freebusy.rs`.

---

## 4. Time zones

### 4.1 Reading a `TZID`

`zone_of` resolves the `TZID` on a property in this order:

1. If the document has a `VTIMEZONE` with that `TZID`, and that component names
   a zone: its `X-LIC-LOCATION` (only when the `TZID` itself is not IANA-shaped
   and the location is), else `resolve_canonical_time_zone(TZID)`.
2. An IANA-shaped `TZID` is taken as written.
3. A solidus-prefixed `TZID` whose tail starts at an IANA area (`Africa`,
   `America`, `Antarctica`, `Arctic`, `Asia`, `Atlantic`, `Australia`,
   `Brazil`, `Canada`, `Chile`, `Etc`, `Europe`, `Indian`, `Mexico`, `Pacific`,
   `US`, or `UTC`/`GMT`) becomes that tail:
   `/freeassociation.sourceforge.net/Europe/Berlin` becomes `Europe/Berlin`.
   libical names its built-in zones this way, so every zoned component that
   Evolution saves needs this step.
4. Anything else is kept unchanged.

`resolve_canonical_time_zone` tries, in order: the Windows table
`WINDOWS_TIME_ZONES` (138 names from CLDR `windowsZones`, for example
`W. Europe Standard Time` to `Europe/Berlin`, `UTC-11` to `Etc/GMT+11`), the
IANA shape, and the globally unique tail.

`names_time_zone` checks the shape only: `/`-separated non-empty segments of
`A-Za-z0-9_-+`. Membership in the IANA database is the server's check.

A Windows name is translated only through a `VTIMEZONE` in the same document.
A bare `TZID=W. Europe Standard Time` with no `VTIMEZONE` stays unchanged and
is refused by `maps_time_zone`.

Tests: `windows_time_zone_names_from_real_exporters_map_to_canonical_iana_zones`,
`globally_unique_form_tzids_with_iana_tails_map_to_canonical_iana_zones`,
`vtimezone_with_explicit_x_lic_location_takes_precedence_over_windows_table`,
`windows_time_zone_names_are_refused_as_unsendable_by_design`.

### 4.2 Custom zones and `timeZones`

RFC 8984 §1.4.8 allows two forms of `TimeZoneId`: an IANA name, or a
solidus-prefixed identifier that is legal only beside a `timeZones` entry
(§4.7.2) that defines it.

- In (`read_time_zones`): for each solidus-prefixed zone that the series or an
  override names, and that has no IANA tail, the matching `VTIMEZONE` is read
  into `timeZones`. Only a definition that can be written back whole is kept.
- Out (`drawn_time_zones`): one `VTIMEZONE` per custom zone named by the series
  or an override and defined in `timeZones`, each once, before the events.
  No `VTIMEZONE` is written for IANA zones (libical resolves them from its own
  table) or for all-day events.
- `definition_of` finds a definition under the identifier with or without its
  leading `/`, because RFC 8984 is ambiguous about the key.
- `maps_time_zone` is true for no zone, an IANA-shaped name, or a custom
  identifier the event defines (`defines_time_zone`).

| `VTIMEZONE` | `TimeZone` / `TimeZoneRule` |
| :--- | :--- |
| `TZID` | `tzId` |
| `STANDARD`, `DAYLIGHT` | `standard`, `daylight` arrays |
| `DTSTART` (local, against `TZOFFSETFROM`) | `start` |
| `TZOFFSETFROM`, `TZOFFSETTO` | `offsetFrom`, `offsetTo` (colons accepted on read; `-0000` refused; seconds written only when not zero) |
| `RRULE` | `recurrenceRules` array (on write, `recurrenceRule` as object or array is also accepted) |
| `TZNAME` | `names` set |

A definition is all or nothing: a missing required property, an unreadable
offset, a rule the writer cannot express in full, or no observance at all
drops the whole definition. Half a zone gives a wrong time, which is worse than
a floating one. Not written: `aliases`, `url`, `validUntil`, rule `comments`,
observance `recurrenceOverrides`.

### 4.3 Saving a zone

- Create: a zone `maps_time_zone` refuses is dropped and the event is filed
  floating; `prune_time_zones` then removes definitions that nothing names, but
  keeps those an override still names.
- Edit: an unsendable zone leaves the server's zone alone. A custom zone in the
  patch brings its definition as `timeZones/<id>`, only when the server has none
  for it; if the server has no `timeZones` at all, the map is written whole.
  An edit never removes definitions.

Tests: `a_new_event_naming_a_zone_nothing_defines_is_filed_floating`,
`a_new_event_in_a_zone_only_the_document_defines_carries_its_definition`,
`a_zone_the_document_could_not_name_leaves_the_servers_alone`
(`jmap-cal-sync/tests/save.rs`).

---

## 5. `UNTIL` and time zones

RFC 5545 §3.3.10 wants `UNTIL` as a UTC instant when `DTSTART` has a `TZID`.
RFC 8984 §4.3.3 wants a local date-time in the event's zone.

**Out** (`rule_to_rrule`):

| Event | `UNTIL` written as |
| :--- | :--- |
| All-day | `UNTIL=20261231` |
| `timeZone` is UTC | `UNTIL=20261231T235959Z` |
| Other zone or floating | `UNTIL=20261231T235959` (local, no `Z`) |
| `VTIMEZONE` observance | converted to UTC with `TZOFFSETFROM`, with `Z` |

The local form for zoned events deviates from RFC 5545 §3.3.10: converting
needs a zone database that this crate does not carry. libical reads it in the
event's zone, and it round-trips.

**In** (`read_until`): a `Z` value on a zoned rule is converted with the
document's own `VTIMEZONE` (`zone::offset_at`). If the zone is UTC or the
event floats, the digits are used as they are. If the document does not define
the zone in a form `zone.rs` can evaluate, the value is kept with its `Z`. That
is not a LocalDateTime, so `maps_recurrence_rule` refuses the rule,
`unstateable_until` returns the value, and a create fails with a message that
names the date and the zone.

**`zone.rs` evaluates** only yearly transition rules with interval 1, one
`BYMONTH`, a day given by `BYDAY` with an ordinal, by `BYDAY` with a
`BYMONTHDAY` list, by one `BYMONTHDAY`, or by `DTSTART`, and optional
`BYHOUR`/`BYMINUTE`/`BYSECOND`, `COUNT`, `UNTIL` and `WKST` (ignored). Dated
`RDATE`s count; `RDATE` periods are refused. It searches up to 40 years back.
Any other shape makes the offset unknown.

Tests: `a_zoned_rules_utc_until_is_converted_through_the_documents_own_vtimezone`,
`a_zone_whose_transitions_cannot_be_worked_out_leaves_the_until_alone`,
`an_observances_until_is_drawn_as_the_utc_instant_it_names`,
`a_series_end_that_cannot_be_stated_is_refused_by_naming_the_zone_and_the_date`
(`jmap-cal-sync/tests/save.rs`).

---

## 6. Fixed point

For any input, after one import and export, a second round trip changes
nothing:

```
ics1 -> Event1 -> ics2 -> Event2 -> ics3 -> Event3
ics2 == ics3 (byte for byte) and Event2 == Event3
```

The save path depends on this: the baseline (section 1) must render the same
way every time. Pinned by `jmap-ical/tests/fuzz_smoke.rs`,
`prop_event_roundtrip_reaches_fixed_point_stability` and
`prop_ical_roundtrip_reaches_fixed_point_stability`
(`jmap-ical/tests/proptest_fuzz.rs`), and
`real_exporter_fixture_corpus_table_driven_roundtrip` (`tests/event.rs`).

---

## 7. Alerts (`VALARM`)

### 7.1 Writing

Each drawable alert becomes, in key order:

```
BEGIN:VALARM
UID:<key>
ACTION:DISPLAY
TRIGGER[;RELATED=END]:<offset>
DESCRIPTION:<event title>      (only when the title is not empty)
END:VALARM
```

RFC 5545 §3.6.6 requires a `DESCRIPTION` on a display alarm, and a JSCalendar
Alert has no text, so the event title is used.

`drawn_alert` writes an alert only when all of these hold:

- the key is an `Id`;
- the object has no members except `@type`, `trigger` and `action`;
- `@type` is absent or `Alert`, `action` is absent or `display`;
- `trigger` has no members except `@type`, `offset` and `relativeTo`; its
  `@type` is absent or `OffsetTrigger`; `offset` is a signed duration;
  `relativeTo` is absent, `start` or `end`.

`maps_alerts` is true when every alert is drawable and `useDefaultAlerts` is not
`true` (as a field or in `extra`). With `useDefaultAlerts: true` no `VALARM` is
written.

### 7.2 Reading

Only `ACTION:DISPLAY` with a duration `TRIGGER` and `RELATED` absent, `START`
or `END` is read, as
`{"@type": "Alert", "trigger": {"@type": "OffsetTrigger", "offset": ..., ["relativeTo": "end"]}, "action": "display"}`.
The offset keeps its sign, drops a leading `+`, and writes zero as `PT0S` or
`-PT0S`.

Dropped on read: `ACTION:AUDIO`, `EMAIL`, `PROCEDURE`; `TRIGGER;VALUE=DATE-TIME`;
`REPEAT` and the alarm's `DURATION` (only the first trigger is kept);
`ACKNOWLEDGED`; the alarm's own `DESCRIPTION`/`SUMMARY`; vendor properties.

Keys: the alarm's `UID` when it is an `Id`, else `a1, a2, ...` in document
order, skipping keys already taken. Evolution writes `X-EVOLUTION-ALARM-UID`
and no `UID`, so a reminder just added in Evolution arrives without a key.
Positional keys stay the same when the same component is read twice. Two alarms
with the same `UID` collapse into one entry.

### 7.3 Saving

`alerts` is replaced whole, and only when `maps_alerts` is true for the
server's event. An alert the line cannot show (mail, sound, absolute trigger,
`acknowledged`, any extra member) would otherwise be deleted by the save. An
acknowledged alert would also come back undismissed.

Why not more: an `EMAIL` alarm needs `ATTENDEE` and `SUMMARY` (RFC 5545 §3.6.6)
that an Alert does not carry; a sound or a program has no RFC 8984 action;
an offset guessed from an absolute trigger would move when the event moves.

### 7.4 `useDefaultAlerts`

iCalendar has no way to say "use the user's default reminders". The flag is
not read or written. When it is `true`, the series and every instance are
written without `VALARM`s, `maps_alerts` is false, and an override that
restates `alerts` is refused.

Tests: `a_reminder_is_a_valarm_of_its_own`,
`a_reminder_the_component_cannot_show_is_flagged_rather_than_drawn`,
`an_alarm_this_mapping_cannot_read_is_dropped_rather_than_guessed_at`,
`an_alarm_that_names_itself_no_id_gets_a_key_of_its_own`,
`an_event_that_uses_the_default_reminders_is_drawn_with_none`.

---

## 8. Recurrence overrides

`recurrenceOverrides` (RFC 8984 §4.3.5) is keyed by the LocalDateTime of the
occurrence. iCalendar uses three forms:

| Override | iCalendar |
| :--- | :--- |
| `{"excluded": true}` | date in `EXDATE` (RFC 5545 §3.8.5.1) |
| `{}` | date in `RDATE` (§3.8.5.2) |
| patch of `OVERRIDE_PROPERTIES` | own `VEVENT` with `RECURRENCE-ID` (§3.8.4.4) |
| `RDATE;VALUE=PERIOD` | `{"duration": ...}` when the length differs from the series, else `{}` |

### 8.1 Restatable properties

`OVERRIDE_PROPERTIES` (11): `title`, `description`, `start`, `timeZone`,
`duration`, `status`, `freeBusyStatus`, `priority`, `privacy`, `keywords`,
`alerts`.

- `timeZone` is restatable because the instance's own `DTSTART` carries its own
  `TZID`.
- `keywords` and `alerts` are restatable because the instance component has its
  own `CATEGORIES` and `VALARM`s.
- `locations`, `virtualLocations`, `links` and `participants` are not: the
  instance inherits the series' values, and a patch would have to reach inside
  an entry the instance does not have.
- `showWithoutTime` is decided once for the whole document.

### 8.2 Writing

- `EXDATE` and `RDATE` are one line each, in the series' value type and zone.
- An instance `VEVENT` starts as a copy of the series (everything inherited,
  including participants, places and links), then applies the patch fields
  that `draws_override_field` accepts.
- `RECURRENCE-ID` is written in the series' zone (it names the occurrence the
  rule generated); the instance `DTSTART` is written in the instance's own
  zone.
- An override with nothing drawable (for example only a `locations` patch) is
  written as a bare `RDATE`, so the occurrence still shows at the series'
  title.

### 8.3 Reading

- The series is the `VEVENT` without `RECURRENCE-ID`, wherever it is in the
  document; EDS passes all instances of a `UID` in no fixed order. A document
  with only detached instances is read from the first one; the others are
  dropped.
- `RDATE` is read first, then `EXDATE`, then detached `VEVENT`s. A later source
  wins for the same instant, so `EXDATE` beats `RDATE` and a detached instance
  beats both.
- A detached `VEVENT` with a `RANGE` parameter (`THISANDFUTURE`, RFC 5545
  §3.2.13) is skipped: one override entry cannot express "this and all later
  occurrences".
- `instance_patch` compares the 11 properties with the series. A property the
  series has and the instance lacks becomes `null`. `start` is compared with the
  key, not with the series.

### 8.4 Checks (`maps_override_field`)

| Field | Accepted |
| :--- | :--- |
| key | a writable date-time |
| `excluded` | `true` only with no other field |
| `title`, `description` | non-empty string or `null` (an empty string would come back as `null`) |
| `status`, `freeBusyStatus`, `privacy` | known value or `null` |
| `priority` | integer 0 to 9 or `null` |
| `keywords` | non-empty set of drawable tags, or `null` |
| `alerts` | non-empty map of drawable alerts, or `null`; never when the series has `useDefaultAlerts` |
| `start` | writable date-time |
| `timeZone` | `null` or IANA-shaped; `sends_recurrence_override` also accepts a custom zone the series defines |
| `duration` | valid length or `null` |
| any other | refused |

Save (`diff_overrides`): `recurrenceOverrides` is replaced whole, or left alone
when any server override fails `sends_recurrence_override` or an edited one
cannot be sent. Deleting one occurrence in Evolution arrives as an `excluded`
override; restoring it removes the override.

Tests: `an_instance_edited_on_its_own_is_a_vevent_of_its_own`,
`an_instance_moved_to_another_zone_carries_it_on_its_own_start`,
`the_series_is_the_vevent_without_a_recurrence_id_whatever_the_order`,
`an_override_for_this_and_future_instances_is_not_read_as_one_instance`,
`an_override_the_mapping_cannot_draw_is_still_placed_at_the_parents_title`,
`deleting_one_occurrence_reaches_the_server_as_an_excluded_override`
(`jmap-cal-sync/tests/save.rs`).

---

## 9. Not mapped, lossy, robustness

### 9.1 Store-owned and envelope data

| Data | Rule | Why |
| :--- | :--- | :--- |
| `id` | `UID` both ways (section 3.1) | EDS keys on it |
| `created` | written as `CREATED`, never read | server-owned |
| `updated` | written as `DTSTAMP` and `LAST-MODIFIED`, never read | server-owned; libical stamps `DTSTAMP` with the local clock, which a save must not send. RFC 5545 §3.8.7.2 makes `DTSTAMP` equal to `LAST-MODIFIED` when there is no `METHOD`. |
| no `updated` | no `DTSTAMP` line | inventing "now" would make every rendering different |
| `version` | `"2.0"` set on the top-level event by `ical_to_event`, never on override instances | bis §3.1.2 requires it on a top-level object and forbids it on embedded ones |
| `@type` | `"Event"` set on read | |
| `PRODID`, `VERSION`, `CALSCALE`, `METHOD` | ignored on read; written as `VERSION:2.0` and `PRODID:-//evolution-jmap//JMAP calendar backend//EN`; `METHOD` only by `scheduling_ical` | envelope of the document, not event data |
| `SEQUENCE` | not mapped | server-owned |

### 9.2 Not carried

JSCalendar members that do not reach the component stay on the server, because
the save patches only what changed among mapped properties:

| JSCalendar | Why |
| :--- | :--- |
| `calendarIds` | membership comes from the EDS source; set by the create path |
| `isDraft`, `isOrigin`, `hideAttendees`, `mayInviteSelf`, `mayInviteOthers`, `scheduleSequence`, `scheduleUpdated` | server or scheduling state (JMAP Calendars §5) |
| `useDefaultAlerts` | no iCalendar form (section 7.4) |
| `color`, `locale`, `localizations`, `endTimeZone`, `mainLocationId` | no counterpart in the `VEVENT` that Evolution edits |
| `Location` members other than `name` | section 3.4 |
| `VirtualLocation.description` | section 3.5 |
| `Link.title`, `Link.cid` | section 3.6 |
| participant members other than those in section 3.8 | section 3.8 |
| `rscale`, `skip` | Gregorian only (section 3.9) |
| any member the mapping does not model | `CalendarEvent.extra` is never filled from iCalendar |

iCalendar data that is dropped on read: `GEO`, `URL`, `RELATED-TO`,
`RESOURCES`, `CONTACT`, `COMMENT`, `COLOR`, `ALTREP` and `LANGUAGE`, every
`X-` property, binary and `file:` attachments, `RRULE` lines after the first,
`EXRULE`, `RANGE` instances, the alarm data listed in section 7.2, and all
`VTODO`/`VJOURNAL` components.

### 9.3 Parsing and writing

- `parse_ical` refuses: input that is not a `VCALENDAR` (`NotACalendar`), an
  unclosed `BEGIN` (`Unterminated`), a mismatched `END` (`Mismatched`), content
  after the calendar (`Trailing`), nesting deeper than `MAX_DEPTH` (32,
  `TooDeep`). A leading byte-order mark is ignored. A calendar without a
  `VEVENT` is `NoEvent`.
- Output is folded at 75 octets (RFC 5545 §3.1) at UTF-8 character boundaries,
  never between a `\` and the character it escapes. `calcard` (0.3.14, see
  `rust/Cargo.toml`) folds by itself; `fold_overlong_lines` is a second pass
  that guarantees
  `prop_emitted_ical_lines_target_75_octets_and_are_valid_utf8`.
- A CR or LF inside a value cannot add a property: see
  `jmap-ical/tests/hostile.rs`.

---

## 10. Server and client quirks the code handles

| Who | Behaviour | Handling |
| :--- | :--- | :--- |
| Fastmail | Rejects a `CalendarEvent/set` create without `version`, and with `"1.0"` (`invalidProperties: ["version"]`) | `ical_to_event` sets `version: "2.0"` (section 9.1) |
| Stalwart, Fastmail | Store a guest with no `roles` (meaning `attendee`) and an organizer with `owner` alone | `diff_participants` compares roles in that normal form, so a save does not patch them back |
| Some servers | A create response may carry only `id` (RFC 8620 §5.3 allows it) | The create path reloads the event before rendering it; `jmap-cal-sync/tests/terse_create.rs` |
| libical | Drops `WKST=MO`; writes `RRULE` parts in its own order | Same omission and order on write (section 3.9) |
| libical | Refuses a whole component over one bad line | Unwritable values are left off instead (dates, durations, `RRULE` parts, `FMTTYPE`) |
| libical | Names built-in zones `/freeassociation.sourceforge.net/<zone>` | Tail extracted (section 4.1) |
| libical | Floats a `TZID` it cannot resolve | Custom zones carry their `VTIMEZONE` (section 4.2) |
| Evolution | Writes `DTEND`, not `DURATION` | `DTEND` read (section 3.2) |
| Evolution | Writes `X-EVOLUTION-ALARM-UID`, not `UID`, on new alarms | Positional alert keys (section 7.2) |
| Evolution | Writes `CLASS` on every save | `CLASS:PUBLIC` always written (section 3.3) |
| Evolution | May drop `X-JMAP-KEY` when it rewrites `LOCATION` | Location key taken from the server's event (section 3.4) |
| Evolution | Stores attachments as `file:` URIs | Not read (section 3.6) |
| Outlook / Exchange | Windows zone names in `TZID` | Translated through the document's `VTIMEZONE` (section 4.1) |

---

## 11. Specification versions

| Topic | Rule |
| :--- | :--- |
| `version` | bis §3.1.2; value `"2.0"` because JMAP Calendars §5 defines a `CalendarEvent` as a bis Event. |
| `recurrenceRule` | Singular (bis §3.3.3). RFC 8984 `recurrenceRules` (§4.3.3) is not modeled on the event. A `TimeZoneRule` still uses `recurrenceRules` (RFC 8984 §4.7.2). |
| `calendarAddress` | bis §3.4.6. Written on read; `sendTo.imip` is read only as a fallback and never written. |
| `organizerCalendarAddress` | bis §3.4.4. Read from `ORGANIZER` and patched on save. |
| `Link.display` | Read as the bis set (`{"badge": true}`, bis §1.5.11); a string is also accepted on write. |
| `timeZones` | Not in bis (bis §3.6 has only `timeZone`), but kept for custom zones (section 4.2), since a solidus-prefixed `timeZone` is meaningless without its definition. Serialized whenever set. `jmap-ical/tests/reserved_properties.rs` checks that no other reserved or obsolete bis property is written. |
| `localizations` | Read and never serialized (`jmap-proto`). |
| `useDefaultAlerts` | JMAP Calendars §5 and §6.1; RFC 8984 §4.5.1. |
| `sendSchedulingMessages` | JMAP Calendars §5.9; never set by the mapping. See `MEETING-SCHEDULING.md`. |

---

## 12. Tests

| Where | What |
| :--- | :--- |
| `jmap-ical/tests/event.rs` | Unit tests per rule; real exporter fixtures (`real_exporter_fixture_*`) |
| `jmap-ical/tests/fixtures/` | Exports from Apple, Cyrus, Evolution, Google, Nextcloud, Outlook/M365, SOGo, Thunderbird (plus detached overrides), and three edge cases: custom `VTIMEZONE`, DST-spanning, zero-length event |
| `jmap-ical/tests/proptest_roundtrip.rs` | Round trip of generated events, modulo the fields in section 9 |
| `jmap-ical/tests/proptest_fuzz.rs`, `fuzz_smoke.rs` | Fixed point (section 6), line length |
| `jmap-ical/tests/hostile.rs` | Injection, nesting, truncation |
| `jmap-ical/tests/reserved_properties.rs` | No reserved bis names on output |
| `jmap-ical/tests/freebusy.rs` | Section 3.10 |
| `jmap-ical/tests/scale.rs` | 10,000-event round trip |
| `jmap-cal-sync/tests/save.rs`, `participants.rs`, `proptest_diff.rs` | Save path (patches) |
| `jmap-backend-cal/tests/marshal.rs` | What libical does to the output |
