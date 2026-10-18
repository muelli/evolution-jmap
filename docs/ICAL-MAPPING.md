<!--
SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
SPDX-License-Identifier: GPL-3.0-or-later
-->

# iCalendar to JSCalendar mapping

This document describes how `jmap-ical` maps the iCalendar that Evolution Data
Server (EDS) gives to a backend to a JMAP `CalendarEvent`, and back.

- iCalendar: RFC 5545, with RFC 7986 (`CONFERENCE`, `IMAGE`), RFC 9074
  (`VALARM` `UID`) and RFC 8607 (`SIZE`). `calcard` parses and writes it.
- JSCalendar: RFC 8984 and draft-ietf-calext-jscalendarbis-22 ("bis").
- JMAP for Calendars: draft-ietf-jmap-calendars-33. Its §5 defines a
  `CalendarEvent` as a bis Event.

This table shows where the code lives:

| Crate / file | Role |
| :--- | :--- |
| `jmap-ical/src/event.rs` | `event_to_ical`, `ical_to_event`, `scheduling_ical`, the `maps_*` predicates |
| `jmap-ical/src/zone.rs` | Offset in force at an instant, from the own `VTIMEZONE` of a document |
| `jmap-ical/src/freebusy.rs` | `BusyPeriod` to `VFREEBUSY` |
| `jmap-cal-sync/src/patch.rs` | Edit: turns an edited component into a `CalendarEvent/set` PatchObject |
| `jmap-cal-sync/src/lib.rs` | Create path (`save_component_with`) |
| `jmap-proto/src/calendars.rs` | The `CalendarEvent` type |

The RFC section numbers below are RFC 8984 sections, unless the text names a
bis or JMAP section.

---

## 1. Design

1. **Edits are patches, not replacements.** A `VEVENT` shows only a part of a
   JSCalendar event. A save sends a PatchObject. The PatchObject names only the
   properties in `MAPPED_PROPERTIES` (19 entries). Everything else stays on the
   server.
2. **The diff baseline is the event of the server after one round trip.**
   `patch::diff` compares the edited event with
   `ical_to_event(event_to_ical(server_event))`. It does not compare it with
   the event of the server. That baseline is exactly what Evolution showed to
   the user. The normalisation that the mapping does therefore never shows up
   as an edit. Examples of normalisation: `UTC` is read back as `Etc/UTC`,
   `INTERVAL=1` is dropped, and an unknown `status` is dropped.
3. **Predicates guard properties that the mapping shows only in part.** If the
   mapping can show a property only in part, a predicate says so. The save
   then leaves that property alone. These are the predicates:
   `maps_locations`, `maps_virtual_locations`, `maps_keyword`, `maps_alerts`,
   `maps_recurrence_rule`, `maps_recurrence_override`,
   `sends_recurrence_override` and `maps_time_zone`.
4. **Map keys travel with the line.** `LOCATION`, `CONFERENCE`, `ATTACH` and
   `IMAGE` carry `X-JMAP-KEY=<key>`. A `VALARM` carries the key as its RFC 9074
   §4 `UID`. A key that is not an RFC 8984 §1.4.1 `Id` (1 to 255 octets of
   `A-Za-z0-9_-`) is read as absent. A line with no usable key gets an
   invented key:
   - `l1` (location)
   - `v1, v2, ...` (conference)
   - `k1, k2, ...` (link)
   - `a1, a2, ...` (alert)
   - `p1, p2, ...` (participant)

   An invented key skips the keys that the document already names.
5. **Unreadable means absent.** The mapping drops a value that it cannot read.
   It does not pass the value through. It does not report an error.
   `ical_to_event` fails only for a broken document or for a document with no
   `VEVENT` (section 9.3).
6. **Fixed point.** A second round trip changes nothing (section 6).

---

## 2. Property table

"In" is `ical_to_event`. "Out" is `event_to_ical`.

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

The mapping does not map anything else. See section 9.2.

---

## 3. Fields

### 3.1 Identity

- `UID` carries the JMAP `id`. EDS keys its cache on `UID`. It passes the
  `UID` back to `load_component_sync` and `remove_component_sync`. Before the
  first `CalendarEvent/set`, there is no `id`. The `uid` replaces it.
- `X-JMAP-UID` carries the JSCalendar `uid`. On read, `UID` becomes `id` and
  `X-JMAP-UID` becomes `uid`.
- A detached instance carries the `UID` of the series.
- **Create:** the `UID` of a new Evolution event is a local name. It is not a
  JMAP id. `save_component_with` moves it into `uid`, unless `X-JMAP-UID`
  already set one. The identity that iTIP mail quotes therefore survives. The
  server assigns the `id`.

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

The mapping reads a date that does not exist (month 13, 30 February, hour 25)
as no start. It accepts a leap second (`:60`).

**Writing `DTSTART`** (`dated`). `EXDATE`, `RDATE` and `RECURRENCE-ID` use the
same function. All of them therefore agree.

| Event | Written as |
| :--- | :--- |
| All-day (see below) | `DTSTART;VALUE=DATE:20260115` |
| `timeZone` is `Etc/UTC` or `UTC` | `DTSTART:20260115T130000Z` |
| Other `timeZone` | `DTSTART;TZID=<timeZone>:20260115T130000` |
| No `timeZone` | `DTSTART:20260115T130000` (floating) |

**All-day.** The mapping writes `showWithoutTime: true` as a DATE only if
RFC 5545 can hold it (`shows_without_time`). These conditions must be true:

- There is no `timeZone`.
- The start is at midnight.
- The duration is whole days (or there is no duration).
- An `UNTIL` is at midnight.
- The written rule has no `BYHOUR`, `BYMINUTE` or `BYSECOND`.
- Every override is at midnight with whole-day lengths and has no zone of its
  own.

If one condition is false, the mapping writes the event as a timed event. The
diff baseline loses the flag in the same way. A save therefore does not clear
the flag on the server.

**Duration.**

- Out: the mapping always writes `DURATION`. It never writes `DTEND`.
- In: the mapping uses `DURATION` if it is a valid length. Otherwise it uses
  `DTEND - DTSTART`.
- `stated_duration` accepts `P[nW][nD][T[nH][nM][nS]]`. Each unit can occur at
  most once, in that order, in either case. The function drops a leading `+`
  and leading zeros. It writes any length with only zeros as `PT0S`. It refuses
  a negative length (RFC 8984 §1.4.6 has none).
- The mapping takes `DTEND - DTSTART` on the wall clock. Whole days become
  `nD`. The rest becomes `TnHnMnS`. A difference of zero or less gives no
  duration.

Known loss: RFC 8984 §1.4.6 treats hours, minutes and seconds as exact time.
The wall-clock difference is therefore one hour wrong for an event that spans a
daylight-saving change. Example: `01:00` to `04:00` on 25 October 2026 in
`Europe/Berlin` is read as `PT3H`. The real length is 4 hours. A `DTEND` in a
different zone than `DTSTART` is wrong by the offset between the two zones.
Evolution writes both ends in one zone. The test
`differential_oracle_dst_transition_spanning_event_duration_and_roundtrip_stability`
pins this (fixture `dst_transition_spanning_export.ics`).

Tests: `an_events_length_may_arrive_as_a_dtend_instead_of_a_duration`,
`a_duration_wins_over_a_dtend_that_contradicts_it`,
`an_all_day_event_the_date_form_cannot_hold_stays_a_date_time`,
`utc_is_spelled_with_a_trailing_z_and_no_tzid`.

### 3.3 Closed vocabularies

The mapping drops values outside each table in both directions. On read, it
ignores the case (RFC 5545 §3.1 makes enumerated values case-insensitive).

| JSCalendar | iCalendar | Notes |
| :--- | :--- | :--- |
| `status`: `confirmed`, `tentative`, `cancelled` | `STATUS`: `CONFIRMED`, `TENTATIVE`, `CANCELLED` | |
| `freeBusyStatus`: `busy`, `free` | `TRANSP`: `OPAQUE`, `TRANSPARENT` | Both formats default to busy/opaque. An absent value is therefore written as no line. |
| `priority`: 0 to 9 | `PRIORITY`: 0 to 9 | Same meaning (0 undefined, 1 highest). 0 is written as `PRIORITY:0`. |
| `privacy`: `public`, `private`, `secret` | `CLASS`: `PUBLIC`, `PRIVATE`, `CONFIDENTIAL` | `public` is written although it is the default, because Evolution writes `CLASS` on every save. `CLASS:secret` is not read as `secret`. |

Evolution writes `CLASS` on every save. The first save of an event that has no
`privacy` therefore sends `privacy: "public"` once. That value is the default.
The write changes nothing.

### 3.4 Locations (`LOCATION`)

RFC 5545 §3.6.1 allows one `LOCATION` for each `VEVENT`. JSCalendar has a map.

- Out: the mapping writes the first entry in key order that has a non-empty
  `name`, as `LOCATION;X-JMAP-KEY=<key>:<name>`.
- In: one entry `{"@type": "Location", "name": ...}` under the `X-JMAP-KEY`, or
  `l1`.
- `maps_locations` is true if the map has at most one entry. The entry must
  have a non-empty key. It must be an object whose `name` is absent or a
  string.
- Save (`diff_locations`): the save renames the entry of the server in place
  (`locations/<key>/name`). The key comes from the event of the server. If the
  user clears the name, the save removes the whole property when the entry held
  only `name` and `@type`. Otherwise it removes only the name. A first location
  writes the whole property (RFC 8620 §5.3 requires the parent path to exist).

Lossy: the mapping does not draw `coordinates`, `description`,
`locationTypes`, `relativeTo`, `timeZone` and every location after the first.
They stay on the server, because the save patches only the name. The mapping
does not read `GEO`. It does not use `mainLocationId` to choose the drawn
entry.

### 3.5 Virtual locations (`CONFERENCE`)

- Out: one line for each entry whose `uri` is a URI (scheme, `:`, something
  after it, no whitespace):
  `CONFERENCE;VALUE=URI;FEATURE=<...>;LABEL=<name>;X-JMAP-KEY=<key>:<uri>`.
  RFC 7986 §5.11 requires `VALUE=URI`.
- `features` (RFC 8984 §4.2.6) and `FEATURE` (RFC 7986 §6.3) share seven
  values: `audio`, `chat`, `feed`, `moderator`, `phone`, `screen`, `video`. The
  mapping writes them in that order. It drops other values.
- In: every `CONFERENCE` line. The mapping reads only the property name
  `CONFERENCE`. `LABEL` becomes `name`.
- `maps_virtual_locations` is true if every entry has these items:
  - a non-empty key
  - a drawable `uri`
  - a string `name` (or none)
  - only known `features`, set to `true`
- Save (`diff_virtual_locations`): the save patches `virtualLocations/<key>/uri`,
  `/name` and `/features` of the entries that the server holds. The save does
  not read a line that disappeared as a deletion. Evolution has no UI for
  conferences and can drop the line. The save does not create a line under a
  key that the server does not hold. The create path writes the whole
  property.

Lossy: the mapping does not draw `description`. It stays on the server.

### 3.6 Links (`ATTACH`, `IMAGE`)

RFC 8984 §4.2.7 keeps one `links` map. iCalendar splits it in two:

| Link | Written as |
| :--- | :--- |
| `rel: "icon"` | `IMAGE;VALUE=URI;DISPLAY=<display>;FMTTYPE=<type>;X-JMAP-KEY=<key>:<href>` (RFC 7986 §5.10) |
| anything else | `ATTACH;FMTTYPE=<type>;SIZE=<size>;X-JMAP-KEY=<key>:<href>` (RFC 5545 §3.8.1.1; `URI` is the default value type) |

- `href` must be a URI. Otherwise the mapping does not write the link.
- The mapping writes `FMTTYPE` only for a bare `type/subtype` that consists of
  RFC 6838 §4.2 restricted-name characters. It omits a type with parameters
  (`text/plain; charset=utf-8`).
- The mapping writes `SIZE` (RFC 8607 §4.1) only for a non-negative integer.
- `DISPLAY` values: `badge`, `graphic`, `fullsize`, `thumbnail` (RFC 7986 §6.1).
  The mapping accepts `display` as a string or as a bis set.
- In: every `ATTACH` and `IMAGE` with a URI value. `IMAGE` gives `rel: "icon"`
  and `display: {"<value>": true}` (bis §1.5.11 set form). The mapping reads
  `SIZE` only on `ATTACH`. The keys are `X-JMAP-KEY` or `k1, k2, ...`.
- The mapping drops these items on read:
  - inline binary attachments (`VALUE=BINARY`)
  - `file:` URIs. Evolution uses them to refer to a file that the user
    attached from disk. A local path that is sent to the server would expose
    the path to every client of the account. The mapping does not upload the
    file as a blob.
- Save (`diff_links`): the save writes only `links/<key>/href`. It writes it
  only if the entry of the server under that key has the `href` from which the
  mapping drew the line. The save does not write `contentType`, `size` and
  `rel` back. A missing line is not a deletion. The save does not create
  unknown keys.

Lossy: the mapping does not map `title`, `cid` and the `FILENAME` parameter
(RFC 8607 §4.2).

### 3.7 Keywords (`CATEGORIES`)

- Out: one `CATEGORIES` line with every tag that `maps_keyword` accepts, in
  sorted order. If no tag is drawable, the mapping writes no line.
- `maps_keyword` refuses these tags: a value other than `true`, an empty tag,
  a tag with only whitespace, and a tag that contains CR.
- In: every `CATEGORIES` line, split on commas. The mapping trims each tag. It
  drops empty tags. Duplicates collapse. Trimming on the first read gives the
  same result as the trim of `calcard` on the second read. The round trip is
  therefore a fixed point.
- Save (`diff_keywords`): the save replaces the whole set. It copies the tags
  that the line could not show from the set of the server into the new set. A
  save therefore never deletes a tag that the user did not see.

### 3.8 Participants (`ORGANIZER`, `ATTENDEE`)

**Out** (`drawn_participants`, map order):

- The address of a participant is `calendarAddress`, else `sendTo.imip`. It
  must be a URI. Otherwise the mapping does not write the participant.
- The first participant with the role `owner` gets
  `ORGANIZER;CN=<name>:<address>`.
  - An owner with no other role that the table below can write gets only that
    line.
  - An owner with another role also gets an `ATTENDEE` line.
- Every other participant gets
  `ATTENDEE;CN;CUTYPE;ROLE;PARTSTAT;RSVP=TRUE:<address>`. The mapping writes
  each parameter only if it is known.

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
  reserved `sendTo`), `name`, `kind`, `participationStatus` and `expectReply`.
  The mapping sets each of them only if it is readable.
- Roles:
  - `REQ-PARTICIPANT`, no `ROLE`, or an unknown `ROLE` gives
    `{"attendee": true}`.
  - `OPT-PARTICIPANT` gives `attendee` and `optional`.
  - `CHAIR` gives `attendee` and `chair`.
  - `NON-PARTICIPANT` gives `{"informational": true}`.

  The writer draws each `ROLE` from these sets. The round trip is therefore
  stable.
- The mapping drops a line whose value is not a URI. The keys are `p1, p2, ...`
  in document order, organizer first. No `X-JMAP-KEY` is on these lines. The
  mapping matches guests by address.

**Save** (`diff_participants`): `participants` and `organizerCalendarAddress`
are both in `MAPPED_PROPERTIES`.

- The save matches each guest by address to the own entry of the server. It
  patches only the members that an `ATTENDEE` line shows
  (`participants/<key>/<member>`). `sendTo`, `invitedBy` and the scheduling
  members therefore stay.
- The save compares roles as servers normalise them (section 10). It writes
  changed roles whole. It keeps the roles that no `ROLE` can spell.
- An added guest gets a `pN` key that the server does not use. The save removes
  a removed guest by key. The save leaves alone an address that the server
  files under several keys.

Not mapped: `SENT-BY`, `DIR`, `DELEGATED-TO` and `DELEGATED-FROM`, `MEMBER`,
`SCHEDULE-AGENT`, `SCHEDULE-STATUS` and `SCHEDULE-FORCE-SEND`, and `sendTo`
methods that are not iMIP.

Sending invitations is not part of the mapping. See `MEETING-SCHEDULING.md`.

Tests: `the_guest_list_of_a_meeting_is_read_back_as_participants`,
`each_role_reads_back_as_the_set_that_draws_it` (`jmap-ical`);
`jmap-cal-sync/tests/participants.rs`.

### 3.9 Recurrence rule (`RRULE`)

bis §3.3.3 has one `recurrenceRule`. RFC 8984 §4.3.3 had a `recurrenceRules`
array. `jmap-proto` models the singular form. The mapping reads only the first
readable `RRULE`. It drops further lines. Evolution writes one line.

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

- The mapping writes each `BY` part whole or not at all. If it drops one value
  of a part, the rule describes a different recurrence.
- The parts follow the order of libical (`COUNT` and `UNTIL` before
  `INTERVAL`, time parts before day parts). A rule that comes back from the
  cache of EDS therefore compares equal. The mapping omits `WKST=MO` and
  `INTERVAL=1`, because libical drops them.
  `jmap-backend-cal/tests/marshal.rs` measures this.
- `maps_recurrence_rule` is false in these cases: the rule has `rscale`,
  `skip`, unmodeled members or an unwritable `UNTIL`, or the rule has a `BY`
  part that the writer would omit. The mapping still writes a rule that it
  refuses, as far as possible.
- In: the mapping drops unknown parts. An `RSCALE=CHINESE` that the mapping
  copied into JSCalendar would be rejected. The mapping keeps an unreadable
  token in a form that the writer refuses (`0` for day numbers, `u32::MAX` for
  time parts, the whole token for `BYDAY`). The save therefore sees the rule as
  partly shown. It does not narrow the rule. `FREQ` must be one of the seven
  frequencies. An `UNTIL` that has no date-time shape ends the parsing of the
  rule.
- Save: an edit replaces `recurrenceRule` whole. It leaves the rule alone if
  the rule of the server or the edited rule fails `maps_recurrence_rule`. A
  create with such a rule fails with `SyncError::Unsendable`. Filing one
  occurrence where the user asked for a series would be worse.

### 3.10 Free/busy (`VFREEBUSY`)

`busy_periods_to_vfreebusy` renders the busy periods of `Principal/getAvailability`
(JMAP Calendars §2.2) as the bare `VFREEBUSY` that `get_free_busy_sync` of EDS
expects:

- It writes `DTSTART` and `DTEND` from the requested window.
- It writes `ATTENDEE:mailto:<address>`. The mapping adds `mailto:` unless it
  is present (compared case-insensitively).
- It writes one `FREEBUSY;FBTYPE=<type>:<start>/<end>` for each period, in the
  order given. The mapping does not clip periods to the window.
- `busyStatus` `tentative` becomes `BUSY-TENTATIVE`. `unavailable` becomes
  `BUSY-UNAVAILABLE`. Anything else becomes `BUSY`.
- The mapping truncates fractional seconds. A window or period that the
  mapping cannot read causes a refusal of the whole component (`None`). No
  time is therefore shown as free by mistake.
- There is no `UID` or `DTSTAMP`, as in `ecmb_get_free_busy_sync`.

Tests: `jmap-ical/tests/freebusy.rs`.

---

## 4. Time zones

### 4.1 Reading a `TZID`

`zone_of` resolves the `TZID` on a property in this order:

1. If the document has a `VTIMEZONE` with that `TZID`, and that component names
   a zone: its `X-LIC-LOCATION` (only if the `TZID` itself is not IANA-shaped
   and the location is), else `resolve_canonical_time_zone(TZID)`.
2. The function takes an IANA-shaped `TZID` as written.
3. A `TZID` with a solidus prefix, whose tail starts at an IANA area
   (`Africa`, `America`, `Antarctica`, `Arctic`, `Asia`, `Atlantic`,
   `Australia`, `Brazil`, `Canada`, `Chile`, `Etc`, `Europe`, `Indian`,
   `Mexico`, `Pacific`, `US`, or `UTC`/`GMT`), becomes that tail:
   `/freeassociation.sourceforge.net/Europe/Berlin` becomes `Europe/Berlin`.
   libical names its built-in zones in this way. Every zoned component that
   Evolution saves therefore needs this step.
4. The function keeps anything else unchanged.

`resolve_canonical_time_zone` tries these sources in order:

1. The Windows table `WINDOWS_TIME_ZONES` (138 names from CLDR `windowsZones`,
   for example `W. Europe Standard Time` to `Europe/Berlin`, `UTC-11` to
   `Etc/GMT+11`).
2. The IANA shape.
3. The globally unique tail.

`names_time_zone` checks only the shape: `/`-separated non-empty segments of
`A-Za-z0-9_-+`. The server checks the membership in the IANA database.

The mapping translates a Windows name only through a `VTIMEZONE` in the same
document. A bare `TZID=W. Europe Standard Time` with no `VTIMEZONE` stays
unchanged. `maps_time_zone` refuses it.

Tests: `windows_time_zone_names_from_real_exporters_map_to_canonical_iana_zones`,
`globally_unique_form_tzids_with_iana_tails_map_to_canonical_iana_zones`,
`vtimezone_with_explicit_x_lic_location_takes_precedence_over_windows_table`,
`windows_time_zone_names_are_refused_as_unsendable_by_design`.

### 4.2 Custom zones and `timeZones`

RFC 8984 §1.4.8 allows two forms of `TimeZoneId`: an IANA name, or an
identifier with a solidus prefix. The second form is legal only beside a
`timeZones` entry (§4.7.2) that defines it.

- In (`read_time_zones`): for each zone with a solidus prefix that the series
  or an override names, and that has no IANA tail, the mapping reads the
  matching `VTIMEZONE` into `timeZones`. The mapping keeps only a definition
  that it can write back whole.
- Out (`drawn_time_zones`): one `VTIMEZONE` for each custom zone that the series
  or an override names and that `timeZones` defines. Each zone occurs once,
  before the events. The mapping writes no `VTIMEZONE` for IANA zones (libical
  resolves them from its own table) or for all-day events.
- `definition_of` finds a definition under the identifier with or without its
  leading `/`, because RFC 8984 is ambiguous about the key.
- `maps_time_zone` is true for no zone, for an IANA-shaped name, or for a
  custom identifier that the event defines (`defines_time_zone`).

| `VTIMEZONE` | `TimeZone` / `TimeZoneRule` |
| :--- | :--- |
| `TZID` | `tzId` |
| `STANDARD`, `DAYLIGHT` | `standard`, `daylight` arrays |
| `DTSTART` (local, against `TZOFFSETFROM`) | `start` |
| `TZOFFSETFROM`, `TZOFFSETTO` | `offsetFrom`, `offsetTo` (colons accepted on read; `-0000` refused; seconds written only when not zero) |
| `RRULE` | `recurrenceRules` array (on write, `recurrenceRule` as object or array is also accepted) |
| `TZNAME` | `names` set |

A definition is all or nothing. The mapping drops the whole definition in
these cases:

- a required property is missing
- an offset is unreadable
- the writer cannot express a rule in full
- there is no observance at all

Half a zone gives a wrong time. This is worse than a floating time. Not
written: `aliases`, `url`, `validUntil`, rule `comments`, observance
`recurrenceOverrides`.

### 4.3 Saving a zone

- Create: the mapping drops a zone that `maps_time_zone` refuses. It files the
  event as floating. `prune_time_zones` then removes the definitions that
  nothing names. It keeps the definitions that an override still names.
- Edit: an unsendable zone leaves the zone of the server alone. A custom zone
  in the patch brings its definition as `timeZones/<id>`. This happens only if
  the server has no definition for it. If the server has no `timeZones` at all,
  the save writes the whole map. An edit never removes definitions.

Tests: `a_new_event_naming_a_zone_nothing_defines_is_filed_floating`,
`a_new_event_in_a_zone_only_the_document_defines_carries_its_definition`,
`a_zone_the_document_could_not_name_leaves_the_servers_alone`
(`jmap-cal-sync/tests/save.rs`).

---

## 5. `UNTIL` and time zones

RFC 5545 §3.3.10 wants `UNTIL` as a UTC instant if `DTSTART` has a `TZID`.
RFC 8984 §4.3.3 wants a local date-time in the zone of the event.

**Out** (`rule_to_rrule`):

| Event | `UNTIL` written as |
| :--- | :--- |
| All-day | `UNTIL=20261231` |
| `timeZone` is UTC | `UNTIL=20261231T235959Z` |
| Other zone or floating | `UNTIL=20261231T235959` (local, no `Z`) |
| `VTIMEZONE` observance | converted to UTC with `TZOFFSETFROM`, with `Z` |

The local form for zoned events deviates from RFC 5545 §3.3.10. A conversion
needs a zone database that this crate does not carry. libical reads the local
form in the zone of the event, and it round-trips.

**In** (`read_until`):

- The mapping converts a `Z` value on a zoned rule with the own `VTIMEZONE` of
  the document (`zone::offset_at`).
- If the zone is UTC or the event floats, the mapping uses the digits as they
  are.
- If the document does not define the zone in a form that `zone.rs` can
  evaluate, the mapping keeps the value with its `Z`. This value is not a
  LocalDateTime. The following happens:
  1. `maps_recurrence_rule` refuses the rule.
  2. `unstateable_until` returns the value.
  3. A create fails with a message that names the date and the zone.

**`zone.rs` evaluates** only yearly transition rules with interval 1. The day
must be given in one of these ways:

- a `BYMONTH` and a `BYDAY` with an ordinal
- a `BYMONTH` and a `BYDAY` with a `BYMONTHDAY` list
- a `BYMONTH` and one `BYMONTHDAY`
- `DTSTART`

The rule can also have `BYHOUR`, `BYMINUTE`, `BYSECOND`, `COUNT`, `UNTIL` and
`WKST` (ignored). Dated `RDATE`s count. `RDATE` periods are refused. The
function searches up to 40 years back. Any other shape makes the offset
unknown.

Tests: `a_zoned_rules_utc_until_is_converted_through_the_documents_own_vtimezone`,
`a_zone_whose_transitions_cannot_be_worked_out_leaves_the_until_alone`,
`an_observances_until_is_drawn_as_the_utc_instant_it_names`,
`a_series_end_that_cannot_be_stated_is_refused_by_naming_the_zone_and_the_date`
(`jmap-cal-sync/tests/save.rs`).

---

## 6. Fixed point

For any input, after one import and one export, a second round trip changes
nothing:

```
ics1 -> Event1 -> ics2 -> Event2 -> ics3 -> Event3
ics2 == ics3 (byte for byte) and Event2 == Event3
```

The save path depends on this. The baseline (section 1) must render in the same
way every time. These tests pin it: `jmap-ical/tests/fuzz_smoke.rs`,
`prop_event_roundtrip_reaches_fixed_point_stability` and
`prop_ical_roundtrip_reaches_fixed_point_stability`
(`jmap-ical/tests/proptest_fuzz.rs`), and
`real_exporter_fixture_corpus_table_driven_roundtrip` (`tests/event.rs`).

---

## 7. Alerts (`VALARM`)

### 7.1 Writing

Each drawable alert becomes this text, in key order:

```
BEGIN:VALARM
UID:<key>
ACTION:DISPLAY
TRIGGER[;RELATED=END]:<offset>
DESCRIPTION:<event title>      (only when the title is not empty)
END:VALARM
```

RFC 5545 §3.6.6 requires a `DESCRIPTION` on a display alarm. A JSCalendar Alert
has no text. The mapping therefore uses the event title.

`drawn_alert` writes an alert only if all of these conditions are true:

- The key is an `Id`.
- The object has no members except `@type`, `trigger` and `action`.
- `@type` is absent or `Alert`. `action` is absent or `display`.
- `trigger` has no members except `@type`, `offset` and `relativeTo`. Its
  `@type` is absent or `OffsetTrigger`. `offset` is a signed duration.
  `relativeTo` is absent, `start` or `end`.

`maps_alerts` is true if every alert is drawable and `useDefaultAlerts` is not
`true` (as a field or in `extra`). If `useDefaultAlerts` is `true`, the mapping
writes no `VALARM`.

### 7.2 Reading

The mapping reads only `ACTION:DISPLAY` with a duration `TRIGGER` and a
`RELATED` that is absent, `START` or `END`. It reads it as
`{"@type": "Alert", "trigger": {"@type": "OffsetTrigger", "offset": ..., ["relativeTo": "end"]}, "action": "display"}`.
The offset keeps its sign and drops a leading `+`. The mapping writes zero as
`PT0S` or `-PT0S`.

`ACKNOWLEDGED` (RFC 9074 §6.1) maps to `acknowledged` (RFC 8984 §4.5.2) in both
directions. Both properties tell that the user dismissed the reminder at that
instant. The value is a UTC date-time. The mapping writes it back with an upper
case `Z`, so a save does not patch the value to itself. EDS writes this property
when the user dismisses a reminder (`e_cal_util_set_alarm_acknowledged`). A
series thus keeps its reminders for the later occurrences.

The mapping drops these items on read:

- `ACTION:AUDIO`, `EMAIL` and `PROCEDURE`
- `TRIGGER;VALUE=DATE-TIME`
- `REPEAT` and the `DURATION` of the alarm (the mapping keeps only the first
  trigger)
- an `ACKNOWLEDGED` with a `TZID`, or one that does not name an instant
  (RFC 9074 §6.1 does not allow it; the reminder then fires again, which is a
  smaller loss than to lose the reminder)
- the own `DESCRIPTION` and `SUMMARY` of the alarm
- vendor properties

Keys: the `UID` of the alarm if it is an `Id`. Otherwise `a1, a2, ...` in
document order, which skip the keys that are already taken. Evolution writes
`X-EVOLUTION-ALARM-UID` and no `UID`. A reminder that the user just added in
Evolution therefore arrives without a key. Positional keys stay the same if the
mapping reads the same component twice. Two alarms with the same `UID` collapse
into one entry.

### 7.3 Saving

The save replaces `alerts` whole. It does this only if `maps_alerts` is true for
the event of the server. Without this rule, the save would delete an alert that
the line cannot show (mail, sound, absolute trigger, an `acknowledged` that is
not a UTC date-time, any extra member). The mapping draws `@type`, `trigger`,
`action` and `acknowledged`.

Why the mapping does not do more:

- An `EMAIL` alarm needs `ATTENDEE` and `SUMMARY` (RFC 5545 §3.6.6). An Alert
  does not carry them.
- A sound or a program has no RFC 8984 action.
- An offset that the mapping guesses from an absolute trigger would move when
  the event moves.

### 7.4 `useDefaultAlerts`

iCalendar has no way to say "use the default reminders of the user". The
mapping does not read or write the flag. If the flag is `true`, these things
happen:

- The mapping writes the series and every instance without `VALARM`s.
- `maps_alerts` is false.
- The save refuses an override that restates `alerts`.

Tests: `a_reminder_is_a_valarm_of_its_own`,
`a_reminder_the_component_cannot_show_is_flagged_rather_than_drawn`,
`an_alarm_this_mapping_cannot_read_is_dropped_rather_than_guessed_at`,
`an_alarm_that_names_itself_no_id_gets_a_key_of_its_own`,
`an_event_that_uses_the_default_reminders_is_drawn_with_none`.

---

## 8. Recurrence overrides

`recurrenceOverrides` (RFC 8984 §4.3.5) uses the LocalDateTime of the
occurrence as its key. iCalendar uses three forms:

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

- `timeZone` is restatable, because the own `DTSTART` of the instance carries
  its own `TZID`.
- `keywords` and `alerts` are restatable, because the instance component has
  its own `CATEGORIES` and `VALARM`s.
- `locations`, `virtualLocations`, `links` and `participants` are not
  restatable. The instance inherits the values of the series. A patch would
  have to reach inside an entry that the instance does not have.
- The mapping decides `showWithoutTime` once for the whole document.

### 8.2 Writing

- `EXDATE` and `RDATE` are one line each, in the value type and zone of the
  series.
- An instance `VEVENT` starts as a copy of the series. It inherits everything,
  including participants, places and links. Then the mapping applies the patch
  fields that `draws_override_field` accepts.
- The mapping writes `RECURRENCE-ID` in the zone of the series. It names the
  occurrence that the rule generated. The mapping writes the instance `DTSTART`
  in the own zone of the instance.
- If an override has nothing drawable (for example only a `locations` patch),
  the mapping writes a bare `RDATE`. The occurrence then still shows with the
  title of the series.

### 8.3 Reading

- The series is the `VEVENT` without `RECURRENCE-ID`, wherever it is in the
  document. EDS passes all instances of a `UID` in no fixed order. If a
  document has only detached instances, the mapping reads the first one. It
  drops the others.
- The mapping reads `RDATE` first, then `EXDATE`, then detached `VEVENT`s. For
  the same instant, a later source wins. `EXDATE` therefore beats `RDATE`, and
  a detached instance beats both.
- The mapping skips a detached `VEVENT` with a `RANGE` parameter
  (`THISANDFUTURE`, RFC 5545 §3.2.13). One override entry cannot express "this
  and all later occurrences".
- `instance_patch` compares the 11 properties with the series. A property that
  the series has and the instance lacks becomes `null`. The function compares
  `start` with the key. It does not compare it with the series.

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
| `timeZone` | `null` or IANA-shaped; `sends_recurrence_override` also accepts a custom zone that the series defines |
| `duration` | valid length or `null` |
| any other | refused |

Save (`diff_overrides`): the save replaces `recurrenceOverrides` whole. It
leaves them alone if any server override fails `sends_recurrence_override` or
if it cannot send an edited override. If the user deletes one occurrence in
Evolution, it arrives as an `excluded` override. If the user restores the
occurrence, the save removes the override.

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
| no `updated` | no `DTSTAMP` line | an invented "now" would make every rendering different |
| `version` | `"2.0"` set on the top-level event by `ical_to_event`, never on override instances | bis §3.1.2 requires it on a top-level object and forbids it on embedded objects |
| `@type` | `"Event"` set on read | |
| `PRODID`, `VERSION`, `CALSCALE`, `METHOD` | ignored on read; written as `VERSION:2.0` and `PRODID:-//evolution-jmap//JMAP calendar backend//EN`; `METHOD` only by `scheduling_ical` | envelope of the document, not event data |
| `SEQUENCE` | not mapped | server-owned |

### 9.2 Not carried

The save patches only what changed among the mapped properties. JSCalendar
members that do not reach the component therefore stay on the server:

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
| any member that the mapping does not model | `CalendarEvent.extra` is never filled from iCalendar |

The mapping drops this iCalendar data on read:

- `GEO`, `URL`, `RELATED-TO`, `RESOURCES`, `CONTACT`, `COMMENT`, `COLOR`,
  `ALTREP` and `LANGUAGE`
- every `X-` property
- binary and `file:` attachments
- `RRULE` lines after the first
- `EXRULE`
- `RANGE` instances
- the alarm data that section 7.2 lists
- all `VTODO` and `VJOURNAL` components

### 9.3 Parsing and writing

- `parse_ical` refuses these inputs:
  - input that is not a `VCALENDAR` (`NotACalendar`)
  - an unclosed `BEGIN` (`Unterminated`)
  - a mismatched `END` (`Mismatched`)
  - content after the calendar (`Trailing`)
  - nesting deeper than `MAX_DEPTH` (32, `TooDeep`)

  A leading byte-order mark is ignored. A calendar without a `VEVENT` is
  `NoEvent`.
- The output is folded at 75 octets (RFC 5545 §3.1) at UTF-8 character
  boundaries. The fold is never between a `\` and the character that it
  escapes. `calcard` (0.3.14, see `rust/Cargo.toml`) folds by itself.
  `fold_overlong_lines` is a second pass. It guarantees
  `prop_emitted_ical_lines_target_75_octets_and_are_valid_utf8`.
- A CR or LF inside a value cannot add a property. See
  `jmap-ical/tests/hostile.rs`.

---

## 10. Server and client quirks the code handles

| Who | Behaviour | Handling |
| :--- | :--- | :--- |
| Fastmail | Rejects a `CalendarEvent/set` create without `version`, and with `"1.0"` (`invalidProperties: ["version"]`) | `ical_to_event` sets `version: "2.0"` (section 9.1) |
| Stalwart, Fastmail | Store a guest with no `roles` (meaning `attendee`) and an organizer with `owner` alone | `diff_participants` compares roles in that normal form, so a save does not patch them back |
| Some servers | A create response can carry only `id` (RFC 8620 §5.3 allows it) | The create path reloads the event before it renders it; `jmap-cal-sync/tests/terse_create.rs` |
| libical | Drops `WKST=MO`; writes `RRULE` parts in its own order | Same omission and order on write (section 3.9) |
| libical | Refuses a whole component because of one bad line | The mapping omits unwritable values (dates, durations, `RRULE` parts, `FMTTYPE`) |
| libical | Names built-in zones `/freeassociation.sourceforge.net/<zone>` | The mapping extracts the tail (section 4.1) |
| libical | Floats a `TZID` that it cannot resolve | Custom zones carry their `VTIMEZONE` (section 4.2) |
| Evolution | Writes `DTEND`, not `DURATION` | The mapping reads `DTEND` (section 3.2) |
| Evolution | Writes `X-EVOLUTION-ALARM-UID`, not `UID`, on new alarms | Positional alert keys (section 7.2) |
| Evolution | Writes `CLASS` on every save | The mapping always writes `CLASS:PUBLIC` (section 3.3) |
| Evolution | Can drop `X-JMAP-KEY` when it rewrites `LOCATION` | The mapping takes the location key from the event of the server (section 3.4) |
| Evolution | Stores attachments as `file:` URIs | The mapping does not read them (section 3.6) |
| Outlook / Exchange | Windows zone names in `TZID` | The mapping translates them through the `VTIMEZONE` of the document (section 4.1) |

---

## 11. Specification versions

| Topic | Rule |
| :--- | :--- |
| `version` | bis §3.1.2; value `"2.0"` because JMAP Calendars §5 defines a `CalendarEvent` as a bis Event. |
| `recurrenceRule` | Singular (bis §3.3.3). The event does not model RFC 8984 `recurrenceRules` (§4.3.3). A `TimeZoneRule` still uses `recurrenceRules` (RFC 8984 §4.7.2). |
| `calendarAddress` | bis §3.4.6. Written on read; `sendTo.imip` is read only as a fallback and never written. |
| `organizerCalendarAddress` | bis §3.4.4. Read from `ORGANIZER` and patched on save. |
| `Link.display` | Read as the bis set (`{"badge": true}`, bis §1.5.11); a string is also accepted on write. |
| `timeZones` | Not in bis (bis §3.6 has only `timeZone`), but kept for custom zones (section 4.2), because a `timeZone` with a solidus prefix is meaningless without its definition. Serialized whenever set. `jmap-ical/tests/reserved_properties.rs` checks that the mapping writes no other reserved or obsolete bis property. |
| `localizations` | Read and never serialized (`jmap-proto`). |
| `useDefaultAlerts` | JMAP Calendars §5 and §6.1; RFC 8984 §4.5.1. |
| `sendSchedulingMessages` | JMAP Calendars §5.9; the mapping never sets it. See `MEETING-SCHEDULING.md`. |

---

## 12. Tests

| Where | What |
| :--- | :--- |
| `jmap-ical/tests/event.rs` | Unit tests for each rule; real exporter fixtures (`real_exporter_fixture_*`) |
| `jmap-ical/tests/fixtures/` | Exports from Apple, Cyrus, Evolution, Google, Nextcloud, Outlook/M365, SOGo, Thunderbird (plus detached overrides), and three edge cases: custom `VTIMEZONE`, DST-spanning, zero-length event |
| `jmap-ical/tests/proptest_roundtrip.rs` | Round trip of generated events, except the fields in section 9 |
| `jmap-ical/tests/proptest_fuzz.rs`, `fuzz_smoke.rs` | Fixed point (section 6), line length |
| `jmap-ical/tests/hostile.rs` | Injection, nesting, truncation |
| `jmap-ical/tests/reserved_properties.rs` | No reserved bis names on output |
| `jmap-ical/tests/freebusy.rs` | Section 3.10 |
| `jmap-ical/tests/scale.rs` | 10,000-event round trip |
| `jmap-cal-sync/tests/save.rs`, `participants.rs`, `proptest_diff.rs` | Save path (patches) |
| `jmap-backend-cal/tests/marshal.rs` | What libical does to the output |
