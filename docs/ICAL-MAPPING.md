<!--
SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
SPDX-License-Identifier: GPL-3.0-or-later
-->

# iCalendar ↔ JSCalendar ↔ EDS Calendar Mapping Reference

This document is the authoritative reference specification for calendar data translation across:
1. **iCalendar 2.0** (RFC 5545, RFC 7986, RFC 9074) as parsed and emitted via `calcard`.
2. **JSCalendar** (RFC 8984 / draft-ietf-calext-jscalendarbis) and **JMAP for Calendars** (draft-ietf-jmap-calendars) as modeled in `jmap-proto`'s [`CalendarEvent`].
3. **Evolution Data Server (EDS)** (`libecal` / `libical` 3.52) as defined in `eds-sys` / `ECalMetaBackend`.

All implementation logic resides in `rust/crates/jmap-ical/src/event.rs`, `rust/crates/jmap-ical/src/zone.rs`, and `rust/crates/jmap-ical/src/freebusy.rs`.

---

## 1. Architecture & Design Principles

### 1.1 Three-Tier Mapping Architecture

```
┌────────────────────────────────────────┐
│     JMAP Server (RFC 8984 / Calendars) │
│         JSCalendar CalendarEvent       │
└───────────────────▲────────────────────┘
                    │
                    │ PatchObject sync (jmap-cal-sync)
                    │ (only mapped/edited fields patched)
                    │
┌───────────────────▼────────────────────┐
│       jmap-ical (event.rs)             │
│  event_to_ical()  /  ical_to_event()   │
└───────────────────▲────────────────────┘
                    │
                    │ RFC 5545 iCalendar wire format (calcard)
                    │
┌───────────────────▼────────────────────┐
│   Evolution Data Server (EDS 3.52)     │
│       ECalMetaBackend / libical        │
│          ICalComponent / UI            │
└────────────────────────────────────────┘
```

### 1.2 Core Invariants

1. **Selective Mapping & Sync Safety**:
   `jmap-ical` deliberately maps only the property set that Evolution's calendar backend needs to present in UI and edit (`SUMMARY`, `DESCRIPTION`, `DTSTART`, `DURATION`/`DTEND`, `STATUS`, `TRANSP`, `PRIORITY`, `CLASS`, `LOCATION`, `CONFERENCE`, `ATTACH`/`IMAGE`, `CATEGORIES`, `ORGANIZER`, `ATTENDEE`, `RRULE`, `EXDATE`, `RDATE`, `RECURRENCE-ID`, `VALARM`). Everything else on a calendar event (e.g. unmodeled vendor properties, unmapped custom properties, server-side participant scheduling states) is dropped on iCalendar emission. This is safe because `jmap-cal-sync` saves changes back to the JMAP server using `PatchObject` specifying only mapped and edited paths. Unmapped server properties are never overwritten or deleted.
2. **Predicates Safeguard Server State**:
   Absence of a field from an edited iCalendar document is only interpreted as user deletion if the field was originally eligible for display. Emitter predicates (e.g. [`maps_locations`], [`maps_virtual_locations`], [`maps_keyword`], [`maps_alerts`], [`maps_recurrence_rule`], [`maps_recurrence_override`], [`sends_recurrence_override`], [`maps_time_zone`], [`unstateable_until`]) explicitly answer whether a property was visible to the user.
3. **Keying & Identity Preservation**:
   Every multi-valued JSCalendar entry (`locations`, `virtualLocations`, `links`, `alerts`) carries an `X-JMAP-KEY` parameter in RFC 5545 format. On round-tripping, key recovery preserves the server key or allocates a deterministic key (`l1`, `v1`, `a1`, etc.) for newly added entries.
4. **Deterministic Fixed-Point Stability**:
   Property transformations reach fixed-point convergence under repeated serialization/deserialization:
   $$\text{Export}_2 (\text{ics}_3) \equiv \text{Export}_3 (\text{ics}_4) \quad \text{and} \quad \text{Event}_2 \equiv \text{Event}_3$$

---

## 2. Master Property Mapping Table

| iCalendar Property | iCalendar Parameters | JSCalendar Field (RFC 8984) | EDS Field / Model | Primary Helpers & Predicates | Lossy / Product Decision Notes |
| :--- | :--- | :--- | :--- | :--- | :--- |
| **`UID`** | — | `event.id` (or `event.uid`) | `ECalComponent` UID | `event_to_ical`, `ical_to_event` | `UID` carries server JMAP ID for EDS cache indexing; `X-JMAP-UID` carries client-side UUID. |
| **`X-JMAP-UID`** | — | `event.uid` | — | `event_to_ical`, `ical_to_event` | Retains client-side JSCalendar UUID across iCalendar round-trips. |
| **`SUMMARY`** | `ALTID`, `LANGUAGE` | `event.title` | `SUMMARY` / Title | `title_and_description` | Text value backslash-escaped (`\,`, `\;`, `\n`, `\\`). First summary in document order selected. |
| **`DESCRIPTION`** | `ALTID`, `LANGUAGE` | `event.description` | `DESCRIPTION` / Notes | `title_and_description` | Free-text notes with newline escaping (`\n` and `\N`). Multi-line descriptions preserved losslessly. |
| **`DTSTART`** | `VALUE=DATE`, `TZID` | `event.start`, `event.time_zone`, `event.show_without_time` | `DTSTART` / Start Time | `read_start`, `dated`, [`maps_time_zone`] | `VALUE=DATE` maps to `show_without_time: true` (all-day event) with no time zone. Timestamps map to local date-time with `timeZone` (or UTC `Z` / floating time). Windows and unique TZIDs resolve via CLDR/IANA pipeline. |
| **`DURATION`** | — | `event.duration` | `DURATION` / Length | `read_duration`, `drawn_duration` | ISO 8601 duration (e.g. `PT1H30M`, `P1D`). Wins over contradicting `DTEND`. Zero durations (`PT0S`, `P0D`) normalized. |
| **`DTEND`** | `VALUE=DATE`, `TZID` | `event.duration` | `DTEND` / End Time | `read_duration` | Converted to `duration` on import (`DTEND - DTSTART`) and emitted as canonical `DURATION` on export. |
| **`STATUS`** | — | `event.status` | `STATUS` (`CONFIRMED`, `TENTATIVE`, `CANCELLED`) | `read_status`, `drawn_status` | Maps `"confirmed"` ↔ `CONFIRMED`, `"tentative"` ↔ `TENTATIVE`, `"cancelled"` ↔ `CANCELLED`. Case-insensitive on import. Unmapped statuses dropped. |
| **`TRANSP`** | — | `event.free_busy_status` | `TRANSP` (`OPAQUE`, `TRANSPARENT`) | `read_transparency`, `drawn_transparency` | Maps `"busy"` ↔ `OPAQUE`, `"free"` ↔ `TRANSPARENT`. |
| **`PRIORITY`** | — | `event.priority` | `PRIORITY` (`0..=9`) | `read_priority`, `drawn_priority` | Integer `0..=9` mapping directly to `PRIORITY:0..9`. RFC 5545 `0` (undefined) maps to `0`. |
| **`CLASS`** | — | `event.privacy` | `CLASS` (`PUBLIC`, `PRIVATE`, `CONFIDENTIAL`) | `read_privacy`, `drawn_privacy` | Maps `"public"` ↔ `CLASS:PUBLIC`, `"private"` ↔ `CLASS:PRIVATE`, `"secret"` ↔ `CLASS:CONFIDENTIAL`. |
| **`LOCATION`** | `X-JMAP-KEY`, `ALTID`, `LANGUAGE` | `event.locations` (`Location.name`) | `LOCATION` / Location | `read_locations`, `drawn_place`, [`maps_locations`] | Single location line emitted for first place with `name`. Keyed via `X-JMAP-KEY`. Subordinate fields (`description`, `coordinates`, `timeZone`) preserved in `Location` and server state via `PatchObject`. |
| **`CONFERENCE`** | `VALUE=URI`, `FEATURE`, `LABEL`, `X-JMAP-KEY` | `event.virtual_locations` (`VirtualLocation.uri`, `name`, `features`) | `CONFERENCE` / Video Link | `read_virtual_locations`, `drawn_conference`, [`maps_virtual_locations`] | RFC 7986 §5.11 video meeting endpoints. Inbound accepts `CONFERENCE`, `X-CONFERENCE`, `X-MICROSOFT-SKYPETEAMSMEETINGURL`. Features: `AUDIO`, `VIDEO`, `SCREEN`, `CHAT`, `MODERATOR`. Multiple lines allowed. |
| **`ATTACH`** | `FMTTYPE`, `SIZE`, `FILENAME`, `X-APPLE-FILENAME`, `X-JMAP-KEY` | `event.links` (`Link.href`, `contentType`, `size`, `title`) | `ATTACH` / Attachment | `read_links`, `drawn_link` | URI attachments (`ATTACH:https://...` or `ATTACH;VALUE=URI:...`) and inline base64 (`ATTACH;ENCODING=BASE64:...` ↔ `data:` URIs). Filenames extracted from `FILENAME` / `X-APPLE-FILENAME`. |
| **`IMAGE`** | `VALUE=URI`, `DISPLAY`, `FMTTYPE`, `X-JMAP-KEY` | `event.links` (`rel: "icon"`, `display: "badge"\|"thumbnail"\|"graphic"\|"fullsize"`) | `IMAGE` / Event Icon | `read_links`, `drawn_link` | RFC 7986 §5.6 / §6.1 event badge/graphic images. Emitted when `rel == "icon"`. |
| **`CATEGORIES`** | — | `event.keywords` (`Set<String>`) | `CATEGORIES` / Categories | `read_keywords`, `drawn_tags`, [`maps_keyword`] | Single sorted line emitted. Comma-separated on wire. Commas, semicolons, and newlines escaped. Tags with leading/trailing whitespace refused by [`maps_keyword`]. |
| **`ORGANIZER`** | `CN`, `DIR`, `SENT-BY` | `event.participants` (`roles: {"owner": true}`, `name`, `sendTo: {"imip": "mailto:..."}`) | `ORGANIZER` / Organizer | `drawn_participants`, `calendar_address` | Owner participant emitted as `ORGANIZER`. Quoted `CN` for names with spaces/delimiters. Written for EDS display; server manages authoritative participant state. |
| **`ATTENDEE`** | `CN`, `ROLE`, `PARTSTAT`, `CUTYPE`, `RSVP`, `SENT-BY`, `DELEGATED-TO`, `DELEGATED-FROM` | `event.participants` (`participationStatus`, `roles`, `kind`, `expectReply`, `name`, `sendTo`) | `ATTENDEE` / Attendees | `drawn_participants`, `calendar_address` | Guest list entries emitted as `ATTENDEE` lines. `PARTSTAT` (`ACCEPTED`, `DECLINED`, `TENTATIVE`, `NEEDS-ACTION`), `ROLE` (`CHAIR`, `REQ-PARTICIPANT`, `OPT-PARTICIPANT`, `NON-PARTICIPANT`), `CUTYPE` (`INDIVIDUAL`, `GROUP`, `RESOURCE`, `ROOM`), `RSVP=TRUE`. Written for EDS display. |
| **`RRULE`** | Recurrence parameters | `event.recurrence_rule` (`RecurrenceRule`) | `RRULE` / Recurrence | `read_rrule`, `drawn_rrule`, [`maps_recurrence_rule`], [`unstateable_until`] | Full RFC 5545 recurrence grammar: `FREQ`, `INTERVAL`, `COUNT`, `UNTIL` (local in series timezone), `BYSECOND`, `BYMINUTE`, `BYHOUR`, `BYDAY` / `NDay`, `BYMONTHDAY`, `BYYEARDAY`, `BYWEEKNO`, `BYMONTH`, `BYSETPOS`, `WKST`. Singular `recurrenceRule` in JSCalendar 2.0 / jscalendarbis §3.3.3 and JMAP for Calendars §1.4. |
| **`EXDATE`** | `TZID`, `VALUE=DATE` | `event.recurrence_overrides` (`{"excluded": true}`) | `EXDATE` / Cancelled Occurrence | `read_overrides`, `drawn_exdates`, [`maps_recurrence_override`] | Cancelled occurrences in recurrence series. Single or multi-value comma-separated dates matching master series zone/clock. |
| **`RDATE`** | `TZID`, `VALUE=DATE` / `VALUE=PERIOD` | `event.recurrence_overrides` (`{}` empty patch or `{"duration": ...}`) | `RDATE` / Added Occurrence | `read_overrides`, `drawn_rdates`, [`maps_recurrence_override`] | Added extra occurrences in recurrence series. Bare dates map to `{}`; period overrides with duration map to `{"duration": ...}`. |
| **`RECURRENCE-ID`** | `TZID`, `VALUE=DATE` | `event.recurrence_overrides` (Detached `VEVENT` `PatchObject`) | Detached `VEVENT` / Modified Occurrence | `read_overrides`, `instance_patch`, [`maps_recurrence_override`], [`sends_recurrence_override`] | Modified occurrences in recurrence series. `RECURRENCE-ID` evaluated on series master clock; instance `DTSTART` on instance clock. Diffed across 11 [`OVERRIDE_PROPERTIES`]. |
| **`VALARM`** | `ACTION=DISPLAY`, `TRIGGER`, `UID`, `RELATED` | `event.alerts` (`Alert.trigger: OffsetTrigger`, `relative_to`, `action: "display"`) | `VALARM` / Reminders | `read_alerts`, `drawn_alarms`, [`maps_alerts`] | Display alarms relative to start or end (`RELATED=END`). RFC 9074 `UID` preserved; nameless alarms assigned positional keys (`a1`, `a2`). Refuses `ACTION:EMAIL`, `ACTION:AUDIO`, absolute triggers, snoozed timestamps. |
| **`VTIMEZONE`** | `TZID`, `STANDARD`, `DAYLIGHT` | `event.time_zones` (`TimeZone`: `standard`, `daylight`, `tz_url`) | `VTIMEZONE` / Timezone Definition | `stated_zones`, `read_time_zones`, [`defines_time_zone`], [`prune_time_zones`] | Timezone definitions with observance rules (`TZOFFSETFROM`, `TZOFFSETTO`, `RRULE`, `RDATE`). Preserves custom solidus definitions; standard IANA zones resolve against host database. |
| **`VFREEBUSY`** | `FBTYPE`, `DTSTART`, `DTEND` | Busy periods (`BusyPeriod`: `utc_start`, `utc_end`, `busy_status`) | `VFREEBUSY` / Availability | `busy_periods_to_vfreebusy`, `free_busy_type` | Renders attendee busy periods within requested search window. Maps `"busy"` → `BUSY`, `"tentative"` → `BUSY-TENTATIVE`, `"unavailable"` → `BUSY-UNAVAILABLE`. |
| **`PRODID`** | — | — | — | — | Dropped by design on import/export. Generator metadata belongs to serialization envelope; foreign `PRODID` not preserved across saves. |
| **`VERSION`** | — | — | — | — | iCalendar version envelope (`VERSION:2.0`). Enforced on import; emitted canonically on export. |
| **`CALSCALE`** | — | — | — | — | Calendar scale (`CALSCALE:GREGORIAN`). Defaults to Gregorian; dropped on import/export. |
| **`METHOD`** | — | — | — | — | iCalendar MIME message method (`REQUEST`, `PUBLISH`, `CANCEL`). Transport envelope metadata; dropped on import/export. |
| **`SEQUENCE`** | — | — | `SEQUENCE` / Revision | — | Revision sequence number. Strictly managed and owned by JMAP server upon commit. |
| **`DTSTAMP`** | - | `event.updated` (RFC 8984 §4.1.4) | `DTSTAMP` / Timestamp | `event_to_ical` | RFC 5545 §3.8.7.2 required envelope timestamp. Emitted from `event.updated` on export. Dropped on import (`read_vevent`) because timestamps are server-owned and libical stamps local clock. |
| **`CREATED`** | - | `event.created` (RFC 8984 §4.1.3) | `CREATED` / Created | `event_to_ical` | Creation timestamp in UTC. Emitted from `event.created` on export. Dropped on import (`read_vevent`) to prevent client local clock stamps from claiming server-owned creation instant. |
| **`LAST-MODIFIED`** | - | `event.updated` (RFC 8984 §4.1.4) | `LAST-MODIFIED` / Updated | `event_to_ical` | Modification timestamp in UTC. Emitted from `event.updated` on export. Dropped on import (`read_vevent`) to prevent client local clock stamps from overriding server-owned update instant. |
| **`URL`** | — | `event.links` | `URL` / Web Link | — | Top-level appointment URL. Handled via `links` subsystem or dropped without polluting `event.extra`. |

---

## 3. Detailed Field & Subsystem Specifications

### 3.1 Identifiers & UIDs
- **`UID`**: RFC 5545 §3.8.4.7 standard identifier. Maps to `event.id` (JMAP event ID) or fallback to `event.uid` (JSCalendar UUID). EDS indexes its internal SQLite calendar cache by `UID`.
- **`X-JMAP-UID`**: Parameter preserving `event.uid` (JSCalendar UUID) when distinct from the JMAP server ID.
- **`X-JMAP-KEY`**: Parameter attached to multi-valued child components and properties (`LOCATION`, `CONFERENCE`, `ATTACH`, `IMAGE`, `VALARM`). Allows lossless synchronization back to JSCalendar map keys.
- **Local Invention Stripping**: When Evolution creates an event, it assigns a local temporary UID. `jmap-cal-sync` strips this local UID before issuing a JMAP `CalendarEvent/set create` call.

### 3.2 Dates, Times, All-Day & Duration
- **All-Day Events (`show_without_time: true`)**:
  - Emitted as `DTSTART;VALUE=DATE:YYYYMMDD` with no time component and no `TZID` parameter (RFC 5545 §3.8.2.4).
  - Outbound serialization never attaches a `TZID` or UTC `Z` marker to date-only values.
  - Multi-day all-day events emit `DURATION:P<N>D` or date-only `DTEND;VALUE=DATE:YYYYMMDD`.
- **Timed Events with Timezones**:
  - Emitted as `DTSTART;TZID=<zone>:YYYYMMDDTHHMMSS` (RFC 5545 §3.8.2.4).
  - UTC events emit `DTSTART:YYYYMMDDTHHMMSSZ` with a trailing `Z` and no `TZID`.
  - Floating time events emit `DTSTART:YYYYMMDDTHHMMSS` with no `TZID` and no `Z`.
- **Duration vs `DTEND` Precedence**:
  - `read_duration` calculates duration from `DURATION` or `DTEND - DTSTART`.
  - If both `DURATION` and `DTEND` appear in a document, `DURATION` takes precedence.
  - Outbound serialization canonically emits `DURATION` (e.g. `DURATION:PT1H30M`), establishing immediate fixed-point stability across import/export cycles.
  - Zero durations (`PT0S`, `-PT0S`, `P0D`, `-P0D`, `PT0M`, `PT0H`) are canonically parsed and stringified as `"PT0S"` / `"-PT0S"`.

### 3.3 Event Metadata & Classification
- **`SUMMARY` & `DESCRIPTION`**:
  - Plain text fields with RFC 5545 backslash escaping (`\,`, `\;`, `\n`, `\\`).
  - `\N` (uppercase) in incoming streams unescapes to newlines losslessly.
  - Multiline descriptions fold cleanly at the 75-octet boundary without splitting UTF-8 code points.
- **`STATUS`**:
  - RFC 8984 lowercase strings (`"confirmed"`, `"tentative"`, `"cancelled"`) map to RFC 5545 uppercase tokens (`CONFIRMED`, `TENTATIVE`, `CANCELLED`).
  - Case-insensitive on inbound parsing (`read_status`); normalized on outbound emission (`drawn_status`).
- **`TRANSP` (Free/Busy Transparency)**:
  - `"busy"` ↔ `OPAQUE` (blocks time on calendar).
  - `"free"` ↔ `TRANSPARENT` (does not block time).
- **`PRIORITY`**:
  - Integer `0..=9` mapping directly to `PRIORITY:0..9` (RFC 5545 §3.8.1.9).
  - `0` represents undefined priority; `1` is highest; `9` is lowest.
- **`CLASS` (Access Privacy)**:
  - `"public"` ↔ `CLASS:PUBLIC`.
  - `"private"` ↔ `CLASS:PRIVATE`.
  - `"secret"` ↔ `CLASS:CONFIDENTIAL`.

### 3.4 Physical & Geographic Locations (`LOCATION`, `GEO` ↔ `locations`)
- **Single Location Display**:
  - RFC 5545 §3.6.1 permits at most one `LOCATION` line in a `VEVENT`.
  - `drawn_place` selects the first entry in `locations` order that has a `name` string.
  - The location name is emitted on `LOCATION;X-JMAP-KEY=<key>:<name>` with text escaping (`\,`, `\;`).
- **Subordinate Location Properties**:
  - Coordinates, descriptions, relative-to settings, and timezones in `Location` (RFC 8984 §4.2.5) are not emitted on the wire `LOCATION` line.
  - `jmap-cal-sync` updates locations using `PatchObject` targeting `locations/<key>/name`, leaving coordinates and descriptions untouched in server state.
- **`maps_locations` Predicate**:
  - Evaluates whether an event's locations can be safely edited in Evolution without data loss:
    1. At most one location entry exists in the map (`entries.len() <= 1`).
    2. Key is non-empty and valid (`!key.is_empty()`).
    3. Entry is a valid JSON object.
    4. `name` field is either absent or a valid string (`matches!(name, None | Some(Value::String(_)))`).
  - Events with multiple locations are drawn in part (first place visible) and flagged by `maps_locations == false` so `jmap-cal-sync` refuses whole-property replacement.

### 3.5 Virtual Locations & Conferences (`CONFERENCE` ↔ `virtualLocations`)
- **RFC 7986 §5.11 `CONFERENCE` Mapping**:
  - Multi-valued property: multiple virtual locations emit multiple `CONFERENCE;VALUE=URI;...` lines.
  - `LABEL` parameter carries `VirtualLocation.name`.
  - `FEATURE` parameter carries comma-separated feature tokens: `AUDIO`, `VIDEO`, `SCREEN`, `CHAT`, `MODERATOR`.
  - `X-JMAP-KEY` parameter preserves the JSCalendar map key.
- **Inbound Vendor Tolerance**:
  - Accepts standard `CONFERENCE`, vendor `X-CONFERENCE`, and Microsoft Teams `X-MICROSOFT-SKYPETEAMSMEETINGURL`.
  - Supported URI schemes: `https:`, `zoommtg:`, `tel:`, `sip:`, `webcal:`.
- **`maps_virtual_locations` Predicate**:
  - Validates that every virtual location entry has a non-empty key, valid URI, valid name, and boolean `true` feature flags from RFC 7986 §6.3 vocabulary.

### 3.6 Attachments & Links (`ATTACH`, `IMAGE` ↔ `links`)
- **`ATTACH` Lines**:
  - Remote URI attachments emit `ATTACH;FMTTYPE=<mime>;SIZE=<bytes>;X-JMAP-KEY=<key>:<uri>`. RFC 5545 default value type is `URI`, so `VALUE=URI` is omitted.
  - Inline binary attachments emit `ATTACH;ENCODING=BASE64;VALUE=BINARY;FMTTYPE=<mime>;X-JMAP-KEY=<key>:<payload>`.
  - `FILENAME` (RFC 7986 §5.4) and `X-APPLE-FILENAME` parameters map to `Link.title`.
- **`IMAGE` Lines (Event Badges & Icons)**:
  - When `rel: "icon"`, `event_to_ical` emits `IMAGE;VALUE=URI;DISPLAY=<badge|thumbnail|graphic|fullsize>;FMTTYPE=<mime>;X-JMAP-KEY=<key>:<uri>` per RFC 7986 §5.6.
- **Lossless Synchronization**:
  - Unmapped link properties ride safely on the server and are preserved across syncs via `PatchObject`.

### 3.7 Categories & Keywords (`CATEGORIES` ↔ `keywords`)
- **Set vs List Model**:
  - JSCalendar `keywords` is a mathematical Set (map with `true` values).
  - iCalendar `CATEGORIES` is a comma-separated text list.
  - `drawn_tags` sorts keyword tags lexicographically before emitting, ensuring byte-identical output across sync passes.
- **Delimiter & Character Escaping**:
  - Commas (`\,`), semicolons (`\;`), and newlines (`\n`) are escaped and unescaped with 100% roundtrip fidelity.
- **Whitespace Defense ([`maps_keyword`])**:
  - Tags with leading or trailing whitespace (`tag.trim() != tag`), carriage returns (`\r`), empty strings (`""`), or non-boolean values are refused by `maps_keyword`, preventing EDS trimming bugs from modifying server tags.

### 3.8 Participants, Organizer & Attendees (`ORGANIZER`, `ATTENDEE` ↔ `participants`)
- **`ORGANIZER` Emission**:
  - Owner participant (`roles: {"owner": true}`) emits `ORGANIZER;CN="<name>":<sendTo.imip>` (RFC 5545 §3.8.4.3).
  - Quoted `CN` parameter for names containing whitespace or delimiters.
- **`ATTENDEE` Emission**:
  - Guest participants emit `ATTENDEE;CN="<name>";ROLE=<role>;PARTSTAT=<status>;CUTYPE=<cutype>;RSVP=<TRUE|FALSE>:<sendTo.imip>`.
  - Role mapping: `chair` → `CHAIR`, `optional` → `OPT-PARTICIPANT`, `informational` → `NON-PARTICIPANT`, `attendee` → `REQ-PARTICIPANT`.
  - Status mapping: `accepted` → `ACCEPTED`, `declined` → `DECLINED`, `tentative` → `TENTATIVE`, `needs-action` → `NEEDS-ACTION`, `delegated` → `DELEGATED`.
  - Kind mapping: `individual` → `INDIVIDUAL`, `group` → `GROUP`, `resource` → `RESOURCE`, `location` → `ROOM`.
  - RSVP mapping: `expectReply: true` → `RSVP=TRUE`.
- **One-Way Emission & Server Scheduling Authority**:
  - `ORGANIZER` and `ATTENDEE` lines are written onto the iCalendar stream for Evolution's UI to display the meeting owner and guest list.
  - Inbound `ical_to_event` leaves `participants: None` because participant scheduling state, RSVPs, and invitation dispatch are strictly owned and managed by the authoritative JMAP calendar server.

### 3.9 Recurrence Rules (`RRULE` ↔ `recurrenceRule`)
- **Model Evolution & Wire Representation**:
  - RFC 8984 §4.3.1 originally modeled `recurrenceRules` as a plural array.
  - JSCalendar 2.0 (`draft-ietf-calext-jscalendarbis` §3.3.3) and JMAP for Calendars (`draft-ietf-jmap-calendars-28` §1.4) restructured this to a singular `recurrenceRule` object.
  - `jmap-proto`'s `CalendarEvent.recurrence_rule` serializes as `"recurrenceRule"` (singular object), matching Stalwart v1.0.0 and modern JMAP implementations.
- **13 Recurrence Rule Elements**:
  - `FREQ`: `secondly`, `minutely`, `hourly`, `daily`, `weekly`, `monthly`, `yearly`.
  - `INTERVAL`: positive integer step count.
  - `COUNT`: positive integer occurrence limit.
  - `UNTIL`: end date-time evaluated in the event's own timezone clock.
  - By-rules: `by_second`, `by_minute`, `by_hour`, `by_day` (`NDay`), `by_month_day`, `by_year_day`, `by_week_no`, `by_month`, `by_set_position`.
  - `WKST`: `first_day_of_week` (`MO`, `TU`, `WE`, `TH`, `FR`, `SA`, `SU`).
- **`maps_recurrence_rule` Predicate**:
  - Validates recurrence rule structure and refuses invalid combinations (e.g. `by_week_no` on non-yearly frequencies).
- **`unstateable_until` Predicate**:
  - Validates that `UNTIL` timestamps can be stated in the series timezone without ambiguity.

### 3.10 Free/Busy Availability Mapping (`VFREEBUSY` ↔ `freebusy.rs`)
- **`free_busy_type`**:
  - Maps draft busy statuses to RFC 5545 `FBTYPE` tokens:
    - `"busy"` → `"BUSY"`
    - `"tentative"` → `"BUSY-TENTATIVE"`
    - `"unavailable"` → `"BUSY-UNAVAILABLE"`
    - Unknown / unmapped statuses fallback safely to `"BUSY"`.
- **`busy_periods_to_vfreebusy`**:
  - Formats bare `VFREEBUSY` components (as expected by `ECalMetaBackend` / `get_free_busy_sync`).
  - Filters and bounds busy periods strictly within the requested `[start, end]` search window.
  - Sanitizes attendee calendar addresses and prevents header/property injection.

---

## 4. VTIMEZONE and TZID Resolution Architecture

RFC 5545 §3.8.3.1 defines the `TZID` parameter and RFC 8984 §1.4.9 / §4.7.2 defines JSCalendar's `timeZone` and `timeZones` model. Real-world calendar streams emitted by major providers (Microsoft Outlook, Exchange, M365, Google Calendar, Apple Calendar macOS/iOS, Nextcloud, Mozilla Thunderbird, and Evolution) carry diverse time zone identifier formats:

```
                          ┌──────────────────────────┐
                          │ Incoming TZID Parameter  │
                          └─────────────┬────────────┘
                                        │
             ┌──────────────────────────┼──────────────────────────┐
             ▼                          ▼                          ▼
   ┌───────────────────┐      ┌───────────────────┐      ┌───────────────────┐
   │ 1. Standard IANA  │      │ 2. Windows Zones  │      │ 3. Globally-Unique│
   │    `Europe/Berlin`│      │ `W. Europe Std...`│      │ `/mozilla.org/...`│
   └─────────┬─────────┘      └─────────┬─────────┘      └─────────┬─────────┘
             │                          │                          │
             ▼                          ▼                          ▼
   ┌───────────────────┐      ┌───────────────────┐      ┌───────────────────┐
   │ Syntactic Check   │      │ CLDR windowsZones │      │ Suffix / Area     │
   │ names_time_zone() │      │ lookup table      │      │ Extraction        │
   └─────────┬─────────┘      └─────────┬─────────┘      └─────────┬─────────┘
             │                          │                          │
             └──────────────────────────┼──────────────────────────┘
                                        │
                                        ▼
                         ┌─────────────────────────────┐
                         │ Canonical IANA TimeZone     │
                         │ (e.g. `Europe/Berlin`, UTC) │
                         └──────────────┬──────────────┘
                                        │
             ┌──────────────────────────┴──────────────────────────┐
             ▼                                                     ▼
┌─────────────────────────────┐                       ┌─────────────────────────────┐
│ If Unresolvable / Ambiguous │                       │ Outbound Serialization      │
│  - Solidus custom defined   │                       │  - Canonical IANA TZID      │
│    retains VTIMEZONE        │                       │  - UTC instant `Z` for UTC  │
│  - Raw unresolvable text    │                       │  - 75-octet folded format   │
│    refused by maps_time_zone│                       │  - Immediate Fixed Point    │
└─────────────────────────────┘                       └─────────────────────────────┘
```

### 4.1 Accepted Time Zone Identifier Forms

`jmap-ical` accepts and classifies time zone identifiers into five distinct tiers:

1. **Standard IANA Time Zone Database Names**:
   - **Grammar**: Non-empty alphanumeric / `_` / `-` / `+` segments separated by `/` (`names_time_zone(value) == true`).
   - **Examples**: `Europe/Berlin`, `America/New_York`, `Asia/Tokyo`, `America/Argentina/Buenos_Aires`, `Etc/GMT+5`, `Etc/UTC`, `UTC`.
   - **Handling**: Accepted directly as first-class JSCalendar `TimeZoneId`. No inline `timeZones` definition is required because IANA zones resolve against the host/client zone database.

2. **Windows Standard Time Display Names (Outlook / Exchange / M365)**:
   - **Source**: Microsoft Outlook 16.0 / Exchange / Office 365 exports and meeting invitations.
   - **Reference**: Unicode CLDR `windowsZones.xml` (territory `001` canonical defaults).
   - **Examples**:
     - `"W. Europe Standard Time"` → `"Europe/Berlin"`
     - `"Romance Standard Time"` → `"Europe/Paris"`
     - `"GMT Standard Time"` → `"Europe/London"`
     - `"Greenwich Standard Time"` → `"Atlantic/Reykjavik"`
     - `"Central European Standard Time"` → `"Europe/Warsaw"`
     - `"Central Europe Standard Time"` → `"Europe/Budapest"`
     - `"E. Europe Standard Time"` → `"Europe/Chisinau"`
     - `"FLE Standard Time"` → `"Europe/Kyiv"`
     - `"GTB Standard Time"` → `"Europe/Bucharest"`
     - `"Russian Standard Time"` → `"Europe/Moscow"`
     - `"Israel Standard Time"` → `"Asia/Jerusalem"`
     - `"Arabic Standard Time"` → `"Asia/Baghdad"`
     - `"Arab Standard Time"` → `"Asia/Riyadh"`
     - `"India Standard Time"` → `"Asia/Kolkata"`
     - `"China Standard Time"` → `"Asia/Shanghai"`
     - `"Singapore Standard Time"` → `"Asia/Singapore"`
     - `"Tokyo Standard Time"` → `"Asia/Tokyo"`
     - `"Korea Standard Time"` → `"Asia/Seoul"`
     - `"AUS Eastern Standard Time"` → `"Australia/Sydney"`
     - `"AUS Central Standard Time"` → `"Australia/Darwin"`
     - `"Cen. Australia Standard Time"` → `"Australia/Adelaide"`
     - `"E. Australia Standard Time"` → `"Australia/Brisbane"`
     - `"W. Australia Standard Time"` → `"Australia/Perth"`
     - `"New Zealand Standard Time"` → `"Pacific/Auckland"`
     - `"Eastern Standard Time"` → `"America/New_York"`
     - `"Central Standard Time"` → `"America/Chicago"`
     - `"Mountain Standard Time"` → `"America/Denver"`
     - `"Pacific Standard Time"` → `"America/Los_Angeles"`
     - `"Alaskan Standard Time"` → `"America/Anchorage"`
     - `"Hawaiian Standard Time"` → `"Pacific/Honolulu"`
     - `"SA Pacific Standard Time"` → `"America/Bogota"`
     - `"E. South America Standard Time"` → `"America/Sao_Paulo"`
     - `"Argentina Standard Time"` → `"America/Buenos_Aires"`
     - `"Atlantic Standard Time"` → `"America/Halifax"`
     - `"Newfoundland Standard Time"` → `"America/St_Johns"`
     - `"US Eastern Standard Time"` → `"America/Indianapolis"`
     - `"US Mountain Standard Time"` → `"America/Phoenix"`
     - `"Canada Central Standard Time"` → `"America/Regina"`
     - `"Mountain Standard Time (Mexico)"` → `"America/Chihuahua"`
     - `"Central Standard Time (Mexico)"` → `"America/Mexico_City"`
     - `"Pacific Standard Time (Mexico)"` → `"America/Tijuana"`
     - `"UTC"`, `"UTC-11"`, `"UTC-02"`, `"UTC+12"`, `"UTC+13"` → `"Etc/UTC"`, `"Etc/GMT+11"`, `"Etc/GMT+2"`, `"Etc/GMT-12"`, `"Etc/GMT-13"`.
   - **Precedence Rule**: If the `VTIMEZONE` component explicitly provides an `X-LIC-LOCATION` (e.g. `X-LIC-LOCATION: Europe/Amsterdam`), the explicit location overrules the static default. Otherwise, the static CLDR table provides the unambiguous mapping.

3. **Globally-Unique-Form TZIDs (RFC 5545 §3.8.3.1 / RFC 2445 §4.8.3.1)**:
   - **Source**: Mozilla Thunderbird, libical, Citadel, Apple, Google, KDE.
   - **Format**: `/<domain>/[<subpath>/].../<Area>/<Location>[/<Sublocation>]`
   - **Examples**:
     - `"/mozilla.org/20070129_1/Europe/Berlin"` → `"Europe/Berlin"`
     - `"/citadel.org/20080105_1/Europe/Paris"` → `"Europe/Paris"`
     - `"/freeassociation.sourceforge.net/Tzfile/Europe/Berlin"` → `"Europe/Berlin"`
     - `"/freeassociation.sourceforge.net/Europe/Berlin"` → `"Europe/Berlin"`
     - `"/softwarestudio.org/Tzfile/America/New_York"` → `"America/New_York"`
     - `"/exchange.example.com/Tzfile/America/Chicago"` → `"America/Chicago"`
     - `"/kde.org/tz/Europe/Rome"` → `"Europe/Rome"`
     - `"/apple.com/timezones/America/Argentina/Buenos_Aires"` → `"America/Argentina/Buenos_Aires"`
     - `"/google.com/20260101_1/Asia/Tokyo"` → `"Asia/Tokyo"`
     - `"/example.com/Australia/Sydney"` → `"Australia/Sydney"`
     - `"/example.com/Etc/GMT+5"` → `"Etc/GMT+5"`
     - `"/citadel.org/America/Indiana/Indianapolis"` → `"America/Indiana/Indianapolis"`
   - **Handling**: Suffix segments matching known IANA Area prefixes (`Africa`, `America`, `Antarctica`, `Arctic`, `Asia`, `Atlantic`, `Australia`, `Brazil`, `Canada`, `Chile`, `Etc`, `Europe`, `Indian`, `Mexico`, `Pacific`, `US`) or `UTC`/`GMT` that form valid IANA zone names are parsed and normalized into standard IANA identifiers.

4. **Custom Defined Solidus Identifiers (RFC 8984 §1.4.9 Form 2)**:
   - **Examples**: `"/example.com/Europe-Berlin"`, `"/custom.org/CorporateZone"`.
   - **Handling**: When a leading solidus identifier has no unambiguous IANA tail, it is treated as a private custom timezone. The companion `VTIMEZONE` definition in the document is parsed into JSCalendar `timeZones` map. Outbound serialization emits the `VTIMEZONE` envelope and references the custom identifier.

5. **Unresolvable / Ambiguous Non-Standard Identifiers**:
   - **Examples**: `"Unknown Fictional Time Zone"`, `"Custom Enterprise Time"`.
   - **Handling**: Unresolvable strings with no leading solidus and no valid mapping are passed unchanged into `event.time_zone` but refused by [`maps_time_zone`]. The sync layer files the appointment as floating rather than sending an invalid identifier to the server.

### 4.2 Custom `TimeZone` & `TimeZoneRule` Observance Architecture (RFC 8984 §4.7.2)

Custom time zones defined under `event.time_zones` bridge between RFC 8984 `TimeZone` / `TimeZoneRule` objects and RFC 5545 `VTIMEZONE` / `STANDARD` / `DAYLIGHT` subcomponents:

- **Observance Rules & Recurrence**:
  - RFC 8984 §4.7.2 defines `recurrenceRules` as a plural array on `TimeZoneRule` objects (whereas standalone events use `recurrenceRule` singular in JSCalendar 2.0 / jscalendarbis).
  - On import, `read_observance` deserializes `RRULE` lines within `STANDARD` / `DAYLIGHT` subcomponents into canonical RFC 8984 `"recurrenceRules": [RecurrenceRule, ...]`.
  - On outbound serialization, `observance()` accepts both `"recurrenceRules"` (RFC 8984 plural array) and `"recurrenceRule"` (singular object or array) variants, ensuring complete interoperability across all payload forms.
- **Local Time & Offset Arithmetic**:
  - `DTSTART` inside `STANDARD` and `DAYLIGHT` is a local date-time resolved against `TZOFFSETFROM` rather than a zone lookup.
  - `UNTIL` inside an observance `RRULE` is converted using `Ends::At(&offset_from)` arithmetic directly from the observance's local offset, avoiding the need for an external zone database.
- **JSCalendar 2.0 Interoperability**:
  - In JSCalendar 2.0 (`draft-ietf-calext-jscalendarbis`), custom `timeZones` definitions were rendered obsolete in favor of canonical IANA time zone identifiers.
  - `jmap-ical` safely omits `time_zones` when standard IANA zones are resolved, and preserves custom solidus definitions when required by private server environments.
- **Multiple Observances per Zone**:
  - Real-world zone definitions frequently carry multiple `STANDARD` and `DAYLIGHT` subcomponents spanning distinct historical eras (such as US pre-2007 vs post-2007 daylight savings shifts, EU pre-1996 vs post-1996 autumn transitions, or one-off War Time shifts).
  - Inbound mapping in `read_definition` groups all matching subcomponents into the `standard` and `daylight` arrays under the zone definition.
  - Outbound serialization via `vtimezone_of` emits every standard and daylight observance child component with its respective `DTSTART`, offsets, and `RRULE`s.
  - Safety and Refusal Boundary: if any observance within a custom `VTIMEZONE` carries a corrupt offset, unreadable date, or malformed rule, the entire definition is discarded. The event timezone remains undefined, and `maps_time_zone` refuses the unresolvable custom identifier, preventing silently wrong calculations on the server.

---

## 5. Recurrence & UNTIL Instant Calculation with Timezones

RFC 5545 §3.3.10 states recurrence rule `UNTIL` as a UTC instant (`YYYYMMDDTHHMMSSZ`) whenever `DTSTART` specifies a timezone. Conversely, RFC 8984 §4.3.3 / jscalendarbis states `until` as a local date-time string (`YYYY-MM-DDTHH:MM:SS`) in the event's own timezone.

`jmap-ical`'s `zone.rs` module evaluates transition offsets (`TZOFFSETTO` / `TZOFFSETFROM`) directly from the document's `VTIMEZONE` observances:
- **Windows TZIDs Feeding Recurrence**: When an event specifies a Windows timezone (e.g. `DTSTART;TZID="Eastern Standard Time":...` or unquoted `DTSTART;TZID=Eastern Standard Time:...`), `read_vevent` resolves the timezone name to its canonical IANA equivalent (`America/New_York`) and binds the matching `VTIMEZONE` observances. `read_until` converts the UTC `UNTIL` instant to local date-time according to the observance rules in force at that instant.
- **Globally-Unique TZIDs Feeding Recurrence**: When an event carries a globally-unique identifier (e.g. `DTSTART;TZID=/mozilla.org/20050126_1/America/New_York:...` or `DTSTART;TZID=/citadel.org/20250101_1/Europe/Berlin:...`), the suffix extracts the canonical IANA zone while the companion `VTIMEZONE` observances resolve the exact transition offset.
- **Multi-Observance Era Resolution**: In timezones with multiple historical observances, `zone.rs::offset_at` searches transitions across eras. The onset of the latest transition at or before the target instant decides the offset:
  - For example, in US Eastern Time (`America/New_York`), March 15 in Era 1 (2005) resolves to Standard Time (`-0500`, yielding `07:00:00`), whereas March 15 in Era 2 (2026) resolves to Daylight Time (`-0400`, yielding `08:00:00`).
  - Late October in Era 1 (2005) resolves to Standard Time (`-0500`, yielding `07:00:00`), whereas late October in Era 2 (2026) resolves to Daylight Time (`-0400`, yielding `08:00:00`).
  - Historical one-off transitions without `RRULE` (e.g. 1942 War Time) and Southern Hemisphere daylight transitions spanning calendar year boundaries (e.g. Sydney October to April) resolve accurately.
- **Override Instance Separation**: Detached recurrence instances (`VEVENT` with `RECURRENCE-ID`) carrying Windows or globally-unique TZIDs maintain independent clocks. `RECURRENCE-ID` evaluates on the master series clock, while the override `DTSTART` evaluates on the instance clock.
- **Normalization and Refusal**: Outbound emission normalizes all resolved timezones to canonical IANA format without solidus prefixes or Windows display names. If a timezone cannot be resolved and defines no valid `VTIMEZONE`, `read_until` preserves the trailing `Z` marker, which `maps_recurrence_rule` refuses, preventing unsendable or corrupt recurrence rules from reaching the JMAP server.

---

## 6. Multi-Stage Fixed-Point Stability

Every calendar transformation in `jmap-ical` adheres to strict fixed-point stability:
1. **Pass 1 (Import & Normalization)**: Raw iCalendar input with non-standard TZIDs (e.g. `DTSTART;TZID=W. Europe Standard Time:...` or `DTSTART;TZID=/mozilla.org/...`) is normalized to canonical JSCalendar (`timeZone: "Europe/Berlin"`).
2. **Pass 2 (Canonical Outbound)**: Serializing back emits canonical RFC 5545 (`DTSTART;TZID=Europe/Berlin:...`).
3. **Pass 3 (Fixed Point Convergence)**: Re-importing and re-exporting produces byte-identical iCalendar streams:
   $$\text{Export}_2 \equiv \text{Export}_3 \quad \text{and} \quad \text{Event}_2 \equiv \text{Event}_3$$

---

## 7. Alerts & VALARM Mapping Architecture (RFC 8984 §4.5 ↔ RFC 5545 §3.6.6 / RFC 9074)

Reminders and alarms bridge between JSCalendar's `alerts: Id[Alert]` map (RFC 8984 §4.5) and iCalendar's `VALARM` child components (RFC 5545 §3.6.6 / RFC 9074 §6):

```
┌──────────────────────────────────────────────┐
│ JSCalendar Alert (RFC 8984 §4.5)             │
│  - trigger: OffsetTrigger (offset, relTo)    │
│  - action: "display"                         │
│  - key: map id (e.g. "a1", "custom-uid")     │
└───────────────────────▲──────────────────────┘
                        │
                        │ event_to_ical() / ical_to_event()
                        │
┌───────────────────────▼──────────────────────┐
│ iCalendar VALARM (RFC 5545 / RFC 9074)       │
│  - UID: <key>                                │
│  - ACTION: DISPLAY                           │
│  - TRIGGER;[RELATED=END]: <offset>           │
│  - DESCRIPTION: <event.title>                │
└──────────────────────────────────────────────┘
```

### 7.1 Trigger Formats, Signs & Normalization

1. **Relative Offsets**:
   - RFC 5545 §3.8.6.3 durations and RFC 8984 §1.4.7 `SignedDuration` represent offsets relative to event start or end.
   - Negative durations (e.g. `-PT15M`, `-PT1H`, `-P1D`, `-P1W`) indicate alarms firing *before* the reference point.
   - Positive durations (e.g. `PT15M`, `+PT15M`, `P1D`) indicate alarms firing *after* the reference point. Redundant leading `+` signs are normalized away on parse and outbound serialization (`"+PT15M"` → `"PT15M"`).
   - Zero durations (`PT0S`, `-PT0S`, `P0D`, `-P0D`, `PT0M`, `PT0H`) are canonically parsed and normalized to `"PT0S"` / `"-PT0S"`.
   - `RELATED=START` (or omitted) defaults to the start of the event (`relativeTo` omitted in JSCalendar).
   - `RELATED=END` maps to `relativeTo: "end"` in JSCalendar.

2. **Absolute Triggers (Refused by Design)**:
   - `AbsoluteTrigger` (`when: "2026-01-15T12:45:00Z"`) / `TRIGGER;VALUE=DATE-TIME`:
   - Inbound: Safely dropped (returns `None`), avoiding inaccurate conversion to floating offsets.
   - Outbound: Refused by `maps_alerts` (`maps_alerts(&event) == false`), preventing silent offset approximation or moving alarms when events are rescheduled.

### 7.2 Action Types Decision Matrix

| iCalendar Action | JSCalendar Action | Inbound Handling | Outbound `maps_alerts` | Design Rationale |
| :--- | :--- | :--- | :--- | :--- |
| `ACTION:DISPLAY` | `"display"` (or absent) | Accepted → `Alert` | `true` (Emits `VALARM`) | Full bidirectional fidelity with Evolution and CalDAV clients. |
| `ACTION:AUDIO` | `"audio"` | Dropped (`None`) | `false` (Refused) | RFC 8984 lacks dedicated audio alarm actions; prevents lossy edits. |
| `ACTION:EMAIL` | `"email"` | Dropped (`None`) | `false` (Refused) | RFC 5545 requires `ATTENDEE` and `SUMMARY` which JSCalendar Alert does not model. |
| `ACTION:PROCEDURE` | `"procedure"` | Dropped (`None`) | `false` (Refused) | Unsupported program execution alarm type. |

### 7.3 UID Allocation & Key Collision Avoidance

- **RFC 9074 `UID`**: Named `VALARM` components with valid IDs preserve their server-assigned key (`UID:k1` → `"k1"`).
- **Nameless & Evolution Alarms**: Exporters omitting RFC 9074 `UID` (such as Evolution's internal `X-EVOLUTION-ALARM-UID` or Apple Calendar) are assigned deterministic positional keys (`"a1"`, `"a2"`, …).
- **Collision Avoidance**: Invented positional keys automatically skip keys already claimed by explicit `UID` lines, guaranteeing 100% uniqueness without entry collapsing.
- **Duplicate UIDs**: If an incoming stream contains duplicate UIDs, RFC 9074 §6 uniqueness rules apply and duplicate entries collapse into a single map entry.

### 7.4 Safety and Whole-Property Replacement

- **Event Title in `DESCRIPTION`**: RFC 5545 §3.6.6 mandates `DESCRIPTION` on `ACTION:DISPLAY`. `event_to_ical` populates `DESCRIPTION` with `event.title` (omitted if title is empty or `None`).
- **Custom `description` on `Alert`**: If a server-side `Alert` contains a custom `description` field, `maps_alerts` returns `false` because `VALARM` description is derived from event title, preventing silent deletion of custom alert descriptions.
- **`acknowledged` Timestamps (RFC 9074 §6.1)**: Dismissed/snoozed alert timestamps in JSCalendar are refused by `maps_alerts` (`maps_alerts(&event) == false`) to prevent whole-property replacement from un-dismissing snoozed alarms.
- **`useDefaultAlerts`**: When `useDefaultAlerts: true` (RFC 8984 §4.5.1), `event_to_ical` emits 0 `VALARM`s and `maps_alerts` returns `false`. Recurrence overrides also inherit `useDefaultAlerts` from the master series.

### 7.5 Real-Exporter Alarm Corpus Fidelity & Refused Shapes Isolation

The real-world exporter corpus (`google_calendar_export.ics`, `outlook_m365_export.ics`, `apple_calendar_export.ics`, `thunderbird_calendar_export.ics`, `thunderbird_detached_export.ics`, `sogo_calendar_export.ics`, `evolution_calendar_export.ics`, `nextcloud_calendar_export.ics`, `cyrus_caldav_export.ics`) characterizes how alarms emitted by major platforms behave on the bidirectional round-trip:

1. **Google Calendar (`google_calendar_export.ics`)**:
   - **Shapes Emitted**: Multiple display alarms at standard offsets (`-P1D`, `-PT15M`), email notification alarms (`ACTION:EMAIL` with `ATTENDEE` and `SUMMARY`), and absolute trigger alarms (`TRIGGER;VALUE=DATE-TIME`).
   - **Mapping Fidelity**: Display alarms map to JSCalendar `Alert` records (`a1`, `a2`). Email and absolute alarms are safely ignored on import and dropped on export without polluting `event.extra` or corrupting appointment properties.

2. **Microsoft Outlook / M365 (`outlook_m365_export.ics`)**:
   - **Shapes Emitted**: Display alarms with `ACTION:DISPLAY`, `DESCRIPTION:REMINDER` (all caps), trigger offsets (`-PT15M`, `-PT30M`), RFC 9074 `UID` and `X-WR-ALARMUID` vendor properties, and enterprise email alarms.
   - **Mapping Fidelity**: Explicit UIDs and positional keys (`a1`) are faithfully preserved. Generic `DESCRIPTION:REMINDER` is replaced on outbound serialization with the event's summary according to RFC 5545 §3.6.6. `X-WR-ALARMUID` and `ACTION:EMAIL` are dropped cleanly on export. Long UIDs (e.g. 94 octets) fold and unfold cleanly at the RFC 5545 75-octet boundary.

3. **Apple Calendar / macOS (`apple_calendar_export.ics`)**:
   - **Shapes Emitted**: Display alarms at diverse offsets (`-P1D`, `-PT2H`, `-PT15M`), `ACTION:AUDIO` with macOS alert sound names (`ATTACH;VALUE=URI:Basso`), absolute trigger alarms (`TRIGGER;VALUE=DATE-TIME`), and `X-WR-ALARMUID` metadata.
   - **Mapping Fidelity**: Display alarms are mapped losslessly into JSCalendar `Alert` records. Audio and absolute alarms are filtered out cleanly, while standard display reminders survive with exact trigger offsets.

4. **GNOME Evolution Native (`evolution_calendar_export.ics`)**:
   - **Shapes Emitted**: Native Evolution alarms with explicit `TRIGGER;VALUE=DURATION:-PT15M` and `-PT1H`, RFC 9074 `UID` values, and clean descriptions.
   - **Mapping Fidelity**: 100% round-trip fidelity. Slotted alert keys (`a1`, `a2`) and explicit UIDs are preserved identically across multi-pass serialization cycles.

5. **Mozilla Thunderbird (`thunderbird_calendar_export.ics`)**:
   - **Shapes Emitted**: Relative duration alarms (`TRIGGER;VALUE=DURATION:-PT15M`), description matching summary, and Mozilla vendor state (`X-MOZ-LASTACK`, `X-MOZ-SNOOZE-TIME`).
   - **Mapping Fidelity**: Display alarms map losslessly into JSCalendar `Alert` objects. Mozilla internal snooze and ack timestamps are cleanly omitted from `event.extra` and dropped on export without corrupting the active alert.

6. **SOGo / Radicale CalDAV (`sogo_calendar_export.ics`)**:
   - **Shapes Emitted**: Dual relative display alarms (`-P1D`, `-PT1H`), RFC 5545 parameter syntax (`TRIGGER;VALUE=DURATION:...`), and CalDAV modification stamps (`X-SOGO-COMPONENT-CREATED`, `X-RADICALE-MODIFIED`).
   - **Mapping Fidelity**: Dual alerts map to distinct `Alert` entries with exact offsets. CalDAV server timestamps do not pollute `event.extra`.

7. **Nextcloud / SabreDAV (`nextcloud_calendar_export.ics`)**:
   - **Shapes Emitted**: Multi-day display offsets (`-P2D`).
   - **Mapping Fidelity**: Preserved and roundtripped losslessly.

8. **Mozilla Thunderbird Detached Overrides (`thunderbird_detached_export.ics`)**:
   - **Shapes Emitted**: Series with bi-weekly recurrence and multiple detached components (`RECURRENCE-ID`). Rescheduled instance carries an overridden `-PT30M` display alarm, while cancelled instance carries the series `-PT15M` alarm with `STATUS:CANCELLED`.
   - **Mapping Fidelity**: Custom alert overrides on detached components are preserved in `recurrenceOverrides` patch maps. Outbound emission restores the exact alarm configuration per instance. Fixed-point equality is reached on the first round-trip.

9. **Cyrus IMAP & Fastmail CalDAV (`cyrus_caldav_export.ics`)**:
   - **Shapes Emitted**: All-day multi-day event (`VALUE=DATE`) with annual recurrence, CalDAV scheduling headers (`SCHEDULE-AGENT=SERVER`), and 1-day advance reminder (`-P1D`).
   - **Mapping Fidelity**: Display alarm roundtrips losslessly alongside all-day `VALUE=DATE` and `P3D` duration without injecting spurious `TZID` parameters.

### 7.6 REPEAT and DURATION Pairing and Inbound Malformed Variations (RFC 5545 §3.6.6)

RFC 5545 §3.6.6 governs the pairing between `REPEAT` and `DURATION` in `VALARM` components:
- **Pairing Constraint**: `REPEAT` and `DURATION` must both be specified or both omitted.
- **Value Types**: `REPEAT` takes a positive integer (`INTEGER` >= 1), defining repetitions after initial trigger. `DURATION` specifies delay between iterations.
- **JSCalendar Dropped REPEAT**: RFC 8984 dropped `REPEAT` and defines no repeat or interval fields on `Alert`. Inbound parsing extracts the primary `TRIGGER` into an `OffsetTrigger` display alarm, ensuring the user receives the initial notification.
- **Malformed Inbound Variations**: Exporters sometimes violate RFC 5545 §3.6.6 by emitting `REPEAT` without `DURATION`, `DURATION` without `REPEAT`, non-positive counts (`REPEAT:0`, `REPEAT:-2`), non-integer values, negative durations (`DURATION:-PT5M`), zero durations (`DURATION:PT0S`), or duplicate property lines. `read_alert` safely extracts the primary trigger without crashing, dropping, or panicking.
- **Outbound Safety Refusal**: If a JSCalendar `Alert` contains unmodeled `"repeat"` or `"duration"` fields in its object representation, `maps_alerts` strictly returns `false`. This protects server-side extensions from being wiped out by whole-property replacement.

### 7.7 Multi-Alarm Density, High Multiplicity Scaling, and Key Synthesis

Multi-alarm sequences across diverse real-world clients exhibit distinct structural patterns:
1. **Multi-Alarm Density and Ordering**: Events frequently carry sequences of reminders (e.g. 1 week before, 1 day before, 2 hours before, 15 minutes before, at start, and 10 minutes after end). On outbound emission, `drawn_alarms` iterates over `event.alerts` sorted by map key, producing deterministic output. Multi-stage roundtrips converge immediately to fixed-point equality.
2. **Identical Offset Multiplicity**: Multiple alarms sharing identical trigger offsets (whether named with explicit UIDs or nameless) remain distinct and non-collapsing. Both named entries (`UID:k1` and `UID:k2`) and synthesized entries (`a1` and `a2`) preserve separate alerts and roundtrip stably.
3. **High Multiplicity Scaling**: Events carrying 10, 15, or more alarms scale cleanly. Positional key allocation increments through multi-digit keys (`a10`, `a11`, ...), maintaining unique non-conflicting map IDs.
4. **Key Synthesis for Non-Standard UIDs**: Exporter UIDs violating RFC 8984 §1.4.4 `Id` syntax (such as Outlook 94-octet composite binary UIDs, Apple `{GUID}` braces, URIs with colons `urn:uuid:...`, email format UIDs `alarm@domain.com`, or UIDs exceeding 255 octets) are recognized by `names_map_entry` as unmappable to JSCalendar map keys. The parser smoothly falls back to positional synthesized keys (`a1`, `a2`, ...), emitting valid RFC 9074 `UID` values on outbound serialization.
5. **Duplicate Explicit UIDs**: If an incoming stream contains duplicate explicit UIDs, RFC 9074 §6 uniqueness rules apply, and subsequent duplicates overwrite earlier entries rather than corrupting map state.
6. **Recurrence Overrides with Multiple Alarms**: Master series alarms are inherited on unmodified instances. Overrides specifying custom alarms replace the entire alarm set for that instance. Overrides setting `"alerts": null` cancel all alarms for that instance. Overrides containing even one unmappable alert are refused by `maps_recurrence_override`, preserving the series alarms.

### 7.8 ACKNOWLEDGED Formats and Whole-Property Replacement Safety (RFC 9074 §6.1)

RFC 9074 §6.1 specifies the `ACKNOWLEDGED` property on `VALARM` components to record when a user dismissed or snoozed a reminder:
1. **Inbound Format Variations**: Exporters emit `ACKNOWLEDGED` in standard UTC date-time (`ACKNOWLEDGED:20260824T120000Z`), parameterized (`ACKNOWLEDGED;VALUE=DATE-TIME:...`), non-standard local timezone (`ACKNOWLEDGED;TZID=...`), lowercase, or paired with Apple vendor properties (`X-WR-ALARMUID`). Inbound parsing safely ignores `ACKNOWLEDGED`, extracting the display reminder so it can be viewed and scheduled in Evolution without polluting `CalendarEvent.extra`.
2. **Outbound Refusal Boundary**: In JSCalendar (RFC 8984 §4.5.2), `acknowledged: UTCDateTime` tracks dismissed alarms. Because `event_to_ical` does not emit `ACKNOWLEDGED`, `maps_alerts` strictly refuses any event containing an `acknowledged` alert. If `maps_alerts` allowed the event, an edit by the user would cause `jmap-cal-sync` to replace `alerts` whole, deleting the `acknowledged` timestamp on the JMAP server and un-dismissing the alert.
3. **Multi-Alarm Isolation**: In an event with multiple alarms, if even one alert carries an `acknowledged` timestamp, `maps_alerts` returns `false` for the entire event. The outbound renderer draws only the non-acknowledged alerts, and `jmap-cal-sync` refuses to save `alerts`, preserving server state.
4. **Recurrence Overrides Safety**: An instance override carrying an `acknowledged` alert causes `maps_recurrence_override` to return `false`, preventing whole-property replacement of `recurrenceOverrides`.

---

## 8. Recurrence Overrides & RECURRENCE-ID Mapping Architecture (RFC 8984 §4.3.4 ↔ RFC 5545 §3.8.4.4 / §3.8.5)

JSCalendar models recurrence overrides using a unified map `recurrenceOverrides: Id[PatchObject]` (RFC 8984 §4.3.4), where each key is a `LocalDateTime` identifying the occurrence being altered. iCalendar (RFC 5545) represents single instances of a recurring series through three distinct mechanisms:

```
┌────────────────────────────────────────────────────────────────────────┐
│               JSCalendar recurrenceOverrides: Id[PatchObject]          │
│               - Keys: LocalDateTime ("2026-01-22T10:00:00")            │
└───────────────────────────────────▲────────────────────────────────────┘
                                    │
                        event_to_ical() / ical_to_event()
                                    │
          ┌─────────────────────────┼─────────────────────────┐
          ▼                         ▼                         ▼
┌───────────────────┐     ┌───────────────────┐     ┌───────────────────┐
│ 1. Cancelled      │     │ 2. Added Instance │     │ 3. Modified       │
│    Instance       │     │    (Extra Date)   │     │    Instance       │
│  - excluded: true │     │  - Empty patch {} │     │  - Property diff  │
│  - EXDATE line    │     │  - RDATE line     │     │  - Detached VEVENT│
│    (RFC 5545      │     │    (RFC 5545      │     │    w/RECURRENCE-ID│
│     §3.8.5.1)     │     │     §3.8.5.2)     │     │    (RFC 5545      │
│                   │     │                   │     │     §3.8.4.4)     │
└───────────────────┘     └───────────────────┘     └───────────────────┘
```

### 8.1 Three Representation Categories

1. **Cancelled Occurrences (`EXDATE` ↔ `{"excluded": true}`)**:
   - Inbound: `EXDATE` lines (single or comma-delimited multiple dates) map to `"excluded": true` entries in `recurrenceOverrides`.
   - Outbound: Entries with `{"excluded": true}` emit sorted comma-delimited `EXDATE` lines matching the master `DTSTART` zone/clock.
   - Validation: An override with `excluded: true` must contain no other properties. Conflicting shapes (e.g. `{"excluded": true, "title": "..."}`) are refused by `maps_recurrence_override`.

2. **Added Occurrences (`RDATE` ↔ `{}` empty patch)**:
   - Inbound: Bare `RDATE` lines map to `{}` in `recurrenceOverrides`. `RDATE;VALUE=PERIOD` stating a duration different from the series maps to `{"duration": "PT...H"}`.
   - Outbound: Entries with empty patch `{}` emit sorted comma-delimited `RDATE` lines matching the master `DTSTART` zone/clock. Entries with duration patches emit detached `VEVENT` components with `RECURRENCE-ID` and `DURATION`.

3. **Modified Occurrences (Detached `VEVENT` with `RECURRENCE-ID`)**:
   - Inbound: Each secondary `VEVENT` carrying a `RECURRENCE-ID` is diffed against the master series across the 11 [`OVERRIDE_PROPERTIES`]. Differing fields form the `PatchObject`.
   - Outbound: An entry modifying any of the 11 restatable properties emits a detached `VEVENT` containing `UID` (matching series), `RECURRENCE-ID` (matching series zone/clock), and overridden properties. Unstated properties inherit from the master series.

### 8.2 Precedence & Conflict Resolution Matrix

When an iCalendar stream contains multiple statements about the same occurrence instant:
1. **Detached `VEVENT` vs `RDATE`**: The detached `VEVENT` takes precedence. It provides specific property overrides while the `RDATE` only states that the occurrence happens.
2. **Detached `VEVENT` vs `EXDATE`**: The detached `VEVENT` takes precedence. The specific modification resurrects/redefines the occurrence.
3. **`EXDATE` vs `RDATE`**: `EXDATE` takes precedence (`excluded: true`), following RFC 5545 §3.8.5.1 to prevent generating unwanted appointments.
4. **`RANGE=THISANDFUTURE` (RFC 5545 §3.2.13)**: Detached components with `RANGE=THISANDFUTURE` are safely skipped by `read_overrides` to prevent silently corrupting subsequent occurrences in the series.
5. **Out-of-Order Components**: Documents where detached `VEVENT` occurrences precede the master series in physical line order are correctly associated by `UID`.

### 8.3 Restatable Properties Decision Matrix (`OVERRIDE_PROPERTIES`)

Only 11 properties are restatable on individual occurrences (RFC 8984 §4.3.4):

| Property | Type | Inbound Diffing | Outbound Serialization | Null Handling (Removal) | Validation Rules |
| :--- | :--- | :--- | :--- | :--- | :--- |
| `title` | `String` | `SUMMARY` diff | `SUMMARY` line | Omitted (inherits series) | Non-empty string; empty `""` refused |
| `description` | `String` | `DESCRIPTION` diff | `DESCRIPTION` line | Omitted (inherits series) | Non-empty string; empty `""` refused |
| `start` | `String` | `DTSTART` vs `id` | `DTSTART` on instance | N/A (must be valid instant) | Valid `LocalDateTime` |
| `timeZone` | `String` | `DTSTART;TZID` diff | `DTSTART;TZID` on instance | Floating time (no TZID, no `Z`) | Canonical IANA / defined TZID |
| `duration` | `String` | `DURATION` / `DTEND` diff | `DURATION` on instance | Omitted (inherits series) | Valid ISO 8601 duration; `<0` refused |
| `status` | `String` | `STATUS` diff | `STATUS` line | Omitted (inherits series) | `"confirmed"`, `"tentative"`, `"cancelled"` |
| `freeBusyStatus`| `String`| `TRANSP` diff | `TRANSP` line | Omitted (inherits series) | `"busy"`, `"free"` |
| `priority` | `Integer`| `PRIORITY` diff | `PRIORITY` line | Omitted (inherits series) | Integer `0..=9` |
| `privacy` | `String` | `CLASS` diff | `CLASS` line | Omitted (inherits series) | `"public"`, `"private"`, `"secret"` |
| `keywords` | `Map` | `CATEGORIES` diff | `CATEGORIES` line | Omitted (inherits series) | Non-empty tag map with `true` |
| `alerts` | `Map` | `VALARM` diff | Child `VALARM` components | No `VALARM`s on instance | Valid offset display alerts |

*Note*: Unmapped properties (e.g. `locations`, `participants`, `virtualLocations`, `links`) cannot be restated per-occurrence and are refused by `maps_recurrence_override`.

### 8.4 Clocks and Time Zone Separation

When an occurrence moves to a different time zone:
- **`RECURRENCE-ID`**: Evaluated on the **master series clock** (`series_zone`), identifying the original generated occurrence instant (RFC 5545 §3.8.4.4).
- **`DTSTART`**: Evaluated on the **instance's own clock** (`instance.time_zone`), placing the rescheduled occurrence at its actual local start time.
- **Windows & Globally-Unique TZIDs**: TZIDs on `RECURRENCE-ID` and instance `DTSTART` resolve through the canonical resolution pipeline (Section 4), tolerating real-world exporter formats across providers.

### 8.5 All-Day Series vs Timed Overrides Value Type Agreement (RFC 5545 §3.8.4.4 / §3.8.5.1 / §3.8.5.2)

RFC 5545 strictly mandates that all components in a recurring series share the same value type (`VALUE=DATE` for all-day series or `DATE-TIME` for timed series):
- **All-Day Consistency**: When the series is all-day (`show_without_time: true`), `EXDATE`, `RDATE`, and detached `VEVENT` `RECURRENCE-ID` and `DTSTART` properties are emitted with `VALUE=DATE:YYYYMMDD` provided all instance overrides also start at midnight and have whole-day durations.
- **Timed Demotion**: If any instance override moves to a time other than midnight or specifies a non-whole-day duration, `shows_without_time` returns `false`. This demotes the master series and every instance override to `DATE-TIME`, ensuring compliant iCalendar output across providers.

---

## 9. Special Semantics & Product Decision Catalog

### 9.1 Dropped-by-Design Rationale for Unknown / Unmodeled Properties
`jmap-ical` deliberately ignores standard iCalendar envelope properties and vendor `X-` extensions for which Evolution/EDS lacks active UI editing support or for which client-side preservation is architecturally incorrect:
1. **`PRODID`, `VERSION`, `CALSCALE`, `METHOD`**:
   - Serialization envelope metadata. Foreign generator identifiers are not preserved across saves to prevent misattributing the generator of Evolution/JMAP exports.
2. **`SEQUENCE` & `DTSTAMP`**:
   - Modification sequence counters and envelope timestamps. Strictly owned and managed by the authoritative store (the JMAP server) upon commit.
3. **Vendor `X-` Properties (`X-MICROSOFT-*`, `X-APPLE-*`, `X-EVOLUTION-*`)**:
   - Safely ignored on parse without polluting `event.extra`. Server-side unmodeled attributes remain safe and untouched via `PatchObject`.

### 9.2 Safe Isolation of Refused Alarm Shapes
- `ACTION:EMAIL`, `ACTION:AUDIO`, `ACTION:PROCEDURE`, and `AbsoluteTrigger` (`TRIGGER;VALUE=DATE-TIME`) in incoming iCalendar documents are safely dropped during inbound mapping without corrupting other event properties or polluting `event.extra`.
- `maps_alerts` strictly refuses any outbound JSCalendar event containing unmappable alert properties (such as custom `description` or `acknowledged` timestamps), ensuring whole-property replacement never silently clobbers user data.

### 9.3 `RANGE=THISANDFUTURE` (RFC 5545 §3.2.13) Safe Skipping Rationale
- Detached components carrying `RANGE=THISANDFUTURE` are safely skipped by `read_overrides` because JSCalendar `recurrenceOverrides` does not support multi-instance range modification in a single key; skipping prevents silently dropping edits to future instances.

### 9.4 Line Folding (75-octet limit), Unfolding, Escaping & UTF-8 Multi-byte Code Point Protection
- Outbound serialization via `calcard` automatically folds physical lines longer than 75 octets using CRLF followed by a space (`\r\n `). Multi-byte UTF-8 code points are never split across line folds.
- Inbound unfolding reconstructs folded lines losslessly.

---

## 10. Function & Predicate Index

| Function Name | Visibility | Primary Role / Responsibility |
| :--- | :--- | :--- |
| [`event_to_ical`] | `pub` | Serializes JSCalendar [`CalendarEvent`] into RFC 5545 iCalendar string for EDS consumption. |
| [`ical_to_event`] | `pub` | Parses RFC 5545 iCalendar string into JSCalendar [`CalendarEvent`]. |
| [`parse_ical`] | `pub` | Parses raw iCalendar text into structured `ICalendar` component AST via `calcard`. |
| [`maps_locations`] | `pub` | Evaluates if locations map has <= 1 entry with valid name and non-empty key for safe patching. |
| [`maps_virtual_locations`] | `pub` | Validates virtual locations map for valid URIs, names, and RFC 7986 boolean features. |
| [`maps_keyword`] | `pub` | Validates keyword tag for boolean `true`, non-emptiness, and whitespace safety. |
| [`maps_alerts`] | `pub` | Validates alerts map for display actions, offset triggers, and absence of custom descriptions/snooze timestamps. |
| [`maps_recurrence_rule`] | `pub` | Validates recurrence rule syntax and frequency/by-rule combinations. |
| [`unstateable_until`] | `pub` | Checks if recurrence UNTIL timestamp cannot be stated in series timezone. |
| [`maps_recurrence_override`] | `pub` | Validates individual recurrence override patch for valid 11 restatable properties and absence of conflicts. |
| [`sends_recurrence_override`] | `pub` | Checks if recurrence override can be emitted, tolerating defined custom timezones. |
| [`names_time_zone`] | `pub` | Validates whether a string has syntactic IANA time zone structure. |
| [`windows_time_zone_to_iana`] | `pub` | Resolves Windows time zone display names to canonical IANA zone identifiers via CLDR table. |
| [`unique_tzid_to_iana`] | `pub` | Extracts canonical IANA zone suffix from globally-unique form TZIDs (`/mozilla.org/...`). |
| [`resolve_canonical_time_zone`] | `pub` | Coordinates resolution order (Windows table, syntactic IANA, globally-unique suffix). |
| [`time_zone_definition`] | `pub` | Looks up inline `VTIMEZONE` definition matching a given TZID. |
| [`maps_time_zone`] | `pub` | Checks whether an event's timezone can be mapped without falling back to floating. |
| [`defines_time_zone`] | `pub` | Checks whether an event explicitly defines an inline `VTIMEZONE` for a TZID. |
| [`prune_time_zones`] | `pub` | Removes unreferenced `VTIMEZONE` definitions from `event.time_zones`. |
| [`free_busy_type`] | `pub` | Maps draft busy status string to RFC 5545 `FBTYPE` token (`BUSY`, `BUSY-TENTATIVE`, `BUSY-UNAVAILABLE`). |
| [`busy_periods_to_vfreebusy`] | `pub` | Formats attendee busy periods into bare `VFREEBUSY` component bounded by search window. |

---

## 11. Real-Exporter Fixture Corpus & Whole-File Regression Net

### 11.1 Exporter Fixture Corpus

| Exporter / Platform | Fixture File | Protocol / Format | Key Characteristics & Mapped Surface | Preservation & Drop Invariants |
| :--- | :--- | :--- | :--- | :--- |
| **Google Calendar** | `google_calendar_export.ics` | iCalendar 2.0 | • Multiple display alarms (`-P1D`, `-PT15M`)<br>• `ACTION:EMAIL` alarms & absolute triggers<br>• Google Meet conference links<br>• Organizer & Attendees with `PARTSTAT`<br>• Recurring series with `EXDATE` | • `ACTION:EMAIL` & absolute triggers dropped cleanly<br>• Display alarms mapped to `Alert`s (`a1`, `a2`)<br>• Fixed-point convergence: `Export₂ == Export₃` |
| **Microsoft Outlook / M365** | `outlook_m365_export.ics` | iCalendar 2.0 | • Windows time zones (`W. Europe Standard Time`)<br>• 94-char folded UIDs<br>• `DESCRIPTION:REMINDER` display alarms<br>• `X-WR-ALARMUID` & `X-MICROSOFT-*` extensions<br>• MS Teams conference URLs | • Windows TZIDs normalize to IANA (`Europe/Berlin`)<br>• Vendor `X-` properties dropped cleanly<br>• Fixed-point convergence: `Export₂ == Export₃` |
| **Apple Calendar / macOS** | `apple_calendar_export.ics` | iCalendar 2.0 | • Globally-unique TZIDs (`/apple.com/...`)<br>• Apple `ACKNOWLEDGED` snoozed alarm timestamps<br>• `ACTION:AUDIO` with Basso sound attachment<br>• Multi-alarm sequences (`-P1D`, `-PT2H`, `-PT15M`)<br>• `X-APPLE-FILENAME` attachments | • `ACKNOWLEDGED` & audio alarms dropped cleanly<br>• Attachments map to `links`<br>• Fixed-point convergence: `Export₂ == Export₃` |
| **Mozilla Thunderbird** | `thunderbird_calendar_export.ics` | iCalendar 2.0 | • Globally-unique TZIDs (`/mozilla.org/...`)<br>• Bi-weekly recurrence (`FREQ=WEEKLY;INTERVAL=2;BYDAY=MO`)<br>• Timezone-aware exception dates (`EXDATE`)<br>• Conference URIs & PDF attachments<br>• Display alarms | • TZIDs normalize to canonical IANA<br>• Attachments & conferences mapped to `links`/`virtualLocations`<br>• Fixed-point convergence: `Export₂ == Export₃` |
| **SOGo / Radicale CalDAV** | `sogo_calendar_export.ics` | iCalendar 2.0 | • Monthly ordinal recurrence (`FREQ=MONTHLY;BYDAY=1TH;COUNT=6`)<br>• French Unicode location strings with accents<br>• Badge image attachments (`rel: icon`)<br>• Conference chat endpoints<br>• Dual reminder alarms | • 100% lossless retention of recurrence & alarms<br>• Fixed-point convergence: `Export₂ == Export₃` |
| **Nextcloud / SabreDAV** | `nextcloud_calendar_export.ics` | iCalendar 2.0 | • Standard IANA time zones (`Europe/Berlin`)<br>• Multi-day display reminder alarms (`-P2D`)<br>• Nextcloud Talk virtual locations<br>• Recurrence overrides with detached components | • Lossless roundtrip of recurrence & overrides<br>• Fixed-point convergence: `Export₂ == Export₃` |
| **GNOME Evolution Native** | `evolution_calendar_export.ics` | iCalendar 2.0 | • Full native Evolution iCalendar 2.0<br>• `X-EVOLUTION-ALARM-UID`<br>• Explicit `VALUE=DURATION` alarm triggers<br>• Full recurrence rules & overrides<br>• Physical & virtual locations | • 100% lossless retention of all Evolution fields<br>• Deterministic `X-JMAP-KEY` preservation<br>• Multi-pass fixpoint: `Export₁ == Export₂ == Export₃` |
| **Mozilla Thunderbird (Detached Overrides)** | `thunderbird_detached_export.ics` | iCalendar 2.0 | • Multi-component series with detached overrides<br>• Rescheduled occurrence (new start & duration)<br>• Retitled occurrence & custom display alarm<br>• Cancelled occurrence with STATUS:CANCELLED<br>• Mozilla vendor extensions (`X-MOZ-GENERATION`, `X-MOZ-LASTACK`, `X-MOZ-SNOOZE-TIME`, `X-MOZ-SEND-INVITATIONS`) | • `X-MOZ-*` vendor properties dropped cleanly on export<br>• Rescheduled, modified, and cancelled overrides preserved losslessly<br>• Fixed-point convergence: `Export₂ == Export₃` |
| **Cyrus IMAP / Fastmail CalDAV** | `cyrus_caldav_export.ics` | iCalendar 2.0 / CalDAV | • All-day multi-day recurring symposium (`VALUE=DATE`, duration `P3D`)<br>• `TRANSP:TRANSPARENT` mapping to `freeBusyStatus: "free"`<br>• RFC 6638 CalDAV scheduling parameters (`SCHEDULE-AGENT=SERVER`, `SCHEDULE-STATUS`, `SCHEDULE-FORCE-SEND`)<br>• Dual links (PDF attachment + PNG badge image)<br>• Annual recurrence with `EXDATE` exclusion<br>• CalDAV synchronization and cache metadata (`X-CALDAV-*`, `X-FASTMAIL-*`) | • All-day date-only format preserved without spurious `TZID`<br>• CalDAV cache and vendor headers dropped cleanly<br>• Fixed-point convergence: `Export₂ == Export₃` |

### 11.2 Table-Driven Whole-File Regression Net

The table-driven test suite (`real_exporter_fixture_corpus_table_driven_roundtrip` in `tests/event.rs`) executes the complete multi-stage lifecycle across the entire fixture corpus:
1. **Inbound Import (`ical_to_event`)**: Parses raw iCalendar streams into structured `CalendarEvent` models.
2. **Outbound Normalization (`event_to_ical`)**: Emits canonical RFC 5545 iCalendar documents.
3. **Multi-Stage Fixpoint Convergence**: Validates standing invariants:
   $$\text{Export}_2 \equiv \text{Export}_3 \quad \text{and} \quad \text{Event}_2 \equiv \text{Event}_3$$

---

## 12. Recurrence Rules Grammar & Complex Parts Fidelity (RFC 8984 §4.3.3 ↔ RFC 5545 §3.3.10)

`jmap-ical` implements full fidelity mapping for RFC 5545 `RRULE` properties and RFC 8984 `RecurrenceRule` records:

### 12.1 Set Position Filtering (`BYSETPOS` ↔ `bySetPosition`)
- **Semantics**: Filters occurrences produced by other expanding `BYxxx` rule parts within the frequency period (RFC 5545 §3.3.10).
- **Valid Range**: RFC 5545 bounds set positions to positive and negative integers within the year (`-366..=-1` and `1..=366`).
- **Refusal Rules**: Zero (`0`), out-of-bounds positions (`<-366` or `>366`), non-integers, and orphan `BYSETPOS` (rules with `by_set_position` but without expanding parts such as `by_day`, `by_month_day`, `by_year_day`, `by_week_no`, `by_hour`, `by_minute`, or `by_second`) are rejected by `maps_recurrence_rule`.
- **Normalization**: Leading plus signs (`+1`) on input are canonicalized to unsigned integer values (`1`) on emission.

### 12.2 Day-of-Week Ordinals (`BYDAY` ↔ `byDay` / `NDay`)
- **Ordinals**: Support signed positive (`+1MO`, `2WE`) and negative (`-1FR`, `-2SU`) week ordinals in monthly or yearly rules. Zero ordinal (`0MO`) is invalid and refused.
- **Frequency Gating**: Per RFC 5545 §3.3.10, ordinals on `BYDAY` are only valid in `MONTHLY` and `YEARLY` recurrence rules. Ordinals in `DAILY`, `WEEKLY`, `HOURLY`, `MINUTELY`, or `SECONDLY` rules are refused by `maps_recurrence_rule`.
- **Mixed Lists**: Rules combining ordinal days and bare weekdays (e.g. `BYDAY=2TU,TH`) are fully supported and round-trip losslessly.

### 12.3 Week Start Day (`WKST` ↔ `firstDayOfWeek`)
- **Default Omission**: RFC 5545 defines `MO` (Monday) as the default `WKST`. To avoid spurious diffs with libical, `jmap-ical` omits `WKST` when `first_day_of_week` is `"mo"`.
- **Non-Default Emission**: When set to any other weekday (`"su"`, `"tu"`, etc.), `event_to_ical` explicitly emits `WKST=SU`.
- **Validation**: Values must match lowercase two-letter day tokens (`"mo"`, `"tu"`, `"we"`, `"th"`, `"fr"`, `"sa"`, `"su"`). Uppercase or descriptive day names are refused.
- **Interaction with `BYWEEKNO`**: Works seamlessly with `byWeekNo` to determine week number boundaries across year transitions.

---

## 13. Differential Server Oracle Adjudications (Stalwart CalendarEvent/parse)

Batch 16 introduces differential verification against live Stalwart v1.0.0 via the `CalendarEvent/parse` method (RFC 9404 Blob transfer and `urn:ietf:params:jmap:calendars:parse`), driven by `jmap-client/examples/calendar-parse-probe.rs`. Stalwart serves as an independent, server-side implementation of `.ics` to JSCalendar conversion.

While "do whatever Stalwart does" is the working rule of thumb, it does not outrank normative RFC specifications where a server exhibits non-standard behavior, and it never outranks the synchronization safety invariants of Evolution Data Server (`libical` / `ECalMetaBackend`).

### 13.1 Divergence 1: `recurrenceRule` (Singular Object) vs `recurrenceRules` (Plural Array)

- **Observed Behavior**:
  Stalwart v1.0.0's `CalendarEvent/parse` emits `recurrenceRule` as a singular JSON object (e.g. `{"frequency": "weekly", "interval": 2, ...}`).
- **Specification Context**:
  1. RFC 8984 §4.3.1 (JSCalendar 1.0, published August 2020) originally specified `recurrenceRules` as a plural array (`RecurrenceRule[]`).
  2. In `draft-ietf-calext-jscalendarbis` §3.3.3 (JSCalendar 2.0), the CalEXT working group renamed and restructured this property from an array to a singular object (`recurrenceRule: RecurrenceRule`), eliminating the ambiguity and interoperability pitfalls of multi-rule recurrences noted in RFC 5545 §3.8.5.3.
  3. In `draft-ietf-jmap-calendars-28` §1.4, `CalendarEvent` is formally defined by normative reference to `jscalendarbis` rather than RFC 8984.
- **Codebase Analysis**:
  In `jmap-proto`, `CalendarEvent` already defines `pub recurrence_rule: Option<RecurrenceRule>`, which serializes to `"recurrenceRule"`. In `jmap-ical`, `ical_to_event` produces `recurrence_rule: Some(...)`, matching Stalwart's wire shape. In `docs/ICAL-MAPPING.md`, earlier sections retained legacy references to RFC 8984's plural naming.
  Note: for custom timezone definitions under `event.time_zones`, `TimeZoneRule` in RFC 8984 §4.7.2 and `jscalendarbis` §4.7.2 still retains plural `recurrenceRules`, which `jmap-ical` maintains.
- **Adjudication**:
  Stalwart's emission is compliant with `jscalendarbis` §3.3.3 and `draft-ietf-jmap-calendars-28`. It is not a server quirk or bug. `jmap-ical` already emits singular `recurrenceRule` on events. No change to outbound serialization is needed.
- **Status**:
  Conforming specification evolution (`jscalendarbis`). Documented and pinned in `tests/event.rs`.

### 13.2 Divergence 2: `DTSTAMP` Mapping to `updated` vs Store-Owned Drop on Import

- **Observed Behavior**:
  Stalwart v1.0.0's `CalendarEvent/parse` maps incoming iCalendar `DTSTAMP` (and `LAST-MODIFIED`) to `event.updated`. In contrast, `jmap-ical`'s `ical_to_event` drops `DTSTAMP`, `CREATED`, and `LAST-MODIFIED` on parse, returning `created: None` and `updated: None`.
- **Specification and Architectural Context**:
  1. RFC 5545 §3.8.7.2 specifies `DTSTAMP` as the creation or modification timestamp of the calendar component or MIME instance. In standalone archival import (`CalendarEvent/parse`), populating `updated` from `DTSTAMP` provides a sensible initial timestamp for a newly minted event record in the database.
  2. In Evolution Data Server (`ECalMetaBackend` / `libical`), `ical_to_event` is the synchronization codec between the desktop client and the JMAP server. Whenever an event is loaded, inspected, or edited in Evolution, `libical` stamps `DTSTAMP` with the client machine's local system clock.
  3. In JMAP (RFC 8620 and `draft-ietf-jmap-calendars`), `created` and `updated` are store-owned metadata managed strictly by the server. If `ical_to_event` mapped `DTSTAMP` to `updated`, every save from Evolution would cause `jmap-cal-sync` to send a `PatchObject` proposing an update timestamp derived from the client's un-synchronized clock, overriding authoritative server state and risking concurrency anomalies.
  4. Outbound serialization (`event_to_ical`) does emit `DTSTAMP` and `LAST-MODIFIED` whenever `event.updated` is present, fully satisfying RFC 5545 §3.8.7.2 for downstream consumers. When `event.updated` is absent, it omits the line rather than inventing a fluctuating "now" timestamp that would break change detection.
- **Adjudication**:
  Stalwart's behavior is appropriate for server-side file ingestion. `jmap-ical`'s drop on import is a deliberate, necessary deviation required by EDS desktop synchronization and JMAP store ownership semantics.
- **Status**:
  Justified architectural deviation. Reconfirmed and pinned in `tests/event.rs`.

### 13.3 Divergence 3: `UID` Mapping to `uid` vs `id`

- **Observed Behavior**:
  Stalwart v1.0.0's `CalendarEvent/parse` maps incoming iCalendar `UID` to JSCalendar `event.uid` (RFC 8984 §4.1.1), leaving JMAP `id` unset (null or omitted). In contrast, `jmap-ical`'s `ical_to_event` maps incoming iCalendar `UID` to `event.id` (and populates `event.uid` only when `X-JMAP-UID` is present).
- **Specification and Architectural Context**:
  1. RFC 8984 §4.1.1 defines `uid` as the globally unique identifier for a calendar object (equivalent to RFC 5545 `UID`). RFC 8620 §2 defines `id` as the immutable server-assigned record identifier in a JMAP account. In `CalendarEvent/parse` (draft-ietf-jmap-calendars §5.7), the parsed event has not yet been committed to a calendar, so Stalwart populates `uid` and leaves `id` omitted until `CalendarEvent/set create` assigns one.
  2. In Evolution Data Server (`ECalMetaBackend` / `libical`), `UID` is the primary key used by EDS to index its local SQLite calendar cache and to route backend vfuncs (`load_component_sync(uid)`, `remove_component_sync(uid)`).
  3. For synchronization (`jmap-cal-sync`), the EDS component UID must match the JMAP server record `id` so that update and delete operations can directly address the target object on the server. `jmap-ical`'s `event_to_ical` preserves this dual identity by writing `UID: <event.id>` (or `event.uid` if unpersisted) and attaching `X-JMAP-UID: <event.uid>`. On import, `ical_to_event` reads `id` from `UID` and `uid` from `X-JMAP-UID`.
- **Adjudication**:
  Justified architectural deviation. Stalwart conforms to standalone stateless parser semantics for unpersisted documents. `jmap-ical` serves as the bidirectional synchronization codec between EDS and JMAP, requiring `id` alignment for local cache routing.
- **Status**:
  Justified architectural deviation. Documented and pinned in `tests/event.rs`.

### 13.4 Divergence 4: `ORGANIZER` and `ATTENDEE` Mapping to `participants` vs Scheduling Boundary

- **Observed Behavior**:
  Stalwart v1.0.0's `CalendarEvent/parse` maps incoming iCalendar `ORGANIZER` and `ATTENDEE` records to JSCalendar `event.participants`. In contrast, `jmap-ical`'s `ical_to_event` deliberately drops `ORGANIZER` and `ATTENDEE` on import, returning `participants: None`. Outbound serialization (`event_to_ical`) does emit `ORGANIZER` and `ATTENDEE` when `event.participants` is populated.
- **Specification and Architectural Context**:
  1. RFC 8984 §4.4 and draft-ietf-jmap-calendars §5.9 define `participants` as the representation of event owners and invitees. In server-side file import, converting attendees into participant objects provides full visibility for archival storage.
  2. In JMAP scheduling (draft-ietf-jmap-calendars §5.9.2 and RFC 5546 iTIP), participant state and reply status (`PARTSTAT` / `participationStatus`) are scheduling state. Changes to participants trigger iTIP `REQUEST`, `REPLY`, or `CANCEL` notifications.
  3. `jmap-ical` operates inside the client synchronization pipeline (`jmap-cal-sync`). The desktop client does not manage autonomous server-side iTIP scheduling flows directly through generic property patches. If `ical_to_event` parsed `participants`, every local appointment edit in Evolution would submit a `PatchObject` modifying `participants`, which could cause unauthorized attendee mutations or trigger unsanctioned scheduling messages.
  4. Omitting `participants` from `MAPPED_PROPERTIES` and dropping them on inbound parse ensures client saves never propose unauthorized mutations to the server's authoritative guest list. Outbound emission continues to render `ORGANIZER` and `ATTENDEE` so the user can see invitees in the desktop UI.
- **Adjudication**:
  Justified architectural deviation required for scheduling safety. Stalwart performs full archival ingestion. `jmap-ical` treats scheduling state as server-authoritative and read-only during client synchronization.
- **Status**:
  Justified architectural deviation. Documented and pinned in `tests/event.rs`.

### 13.5 Divergence 5: `PRODID`, `CALSCALE`, and `METHOD` Envelope Properties vs Generator Ownership

- **Observed Behavior**:
  Stalwart v1.0.0's `CalendarEvent/parse` may map `PRODID` to `event.prodId` (RFC 8984 §4.1.2) and `METHOD` to `event.method` (RFC 8984 §4.1.5). `jmap-ical` drops `PRODID`, `CALSCALE`, and `METHOD` on import (`prod_id: None`, `method: None`), and emits canonical `VERSION:2.0` and its own `PRODID` on export.
- **Specification and Architectural Context**:
  1. RFC 5545 §3.7.1 to §3.7.3 specifies `PRODID`, `VERSION`, and `CALSCALE` as calendar stream envelope metadata identifying the serializing software and format version.
  2. In JMAP, `prodId` identifies the software that created the JSCalendar record. Retaining a third-party generator string from an imported `.ics` file across subsequent round-trips would misattribute documents generated by `jmap-ical` or EDS.
  3. `METHOD` belongs to MIME iTIP transport envelopes (RFC 5546). Once ingested into calendar store state, component objects do not retain ephemeral transport method wrappers.
- **Adjudication**:
  Conforming serialization boundary practice. Generator metadata belongs to the active encoder envelope rather than persistent event state.
- **Status**:
  Conforming specification boundary. Documented and pinned in `tests/event.rs`.

### 13.6 Divergence 6: `URL` Property Handling vs `links` Subsystem Isolation

- **Observed Behavior**:
  RFC 5545 §3.8.4.6 defines `URL` for top-level calendar appointment web links. RFC 8984 §4.2.7 notes that `links` may represent both `ATTACH` and `URL` properties. Stalwart maps `URL` into `links` with `rel: "related"`. In contrast, `jmap-ical`'s `read_links` specifically targets `ATTACH` (enclosures) and `IMAGE` (icons). Top-level `URL` properties are dropped on import without polluting `event.extra`, and `drawn_links` renders `Link` entries as `ATTACH` (or `IMAGE` when `rel == "icon"`).
- **Specification and Architectural Context**:
  1. In desktop calendar workflows with Evolution, web meeting endpoints are represented via RFC 7986 `CONFERENCE` lines (which map directly to JSCalendar `virtualLocations`). Web links are also frequently embedded in `DESCRIPTION`.
  2. Restricting `links` to `ATTACH` and `IMAGE` prevents collision between top-level web URLs and virtual conference URIs, ensuring that every entry in `links` maps unambiguously to an RFC 5545 enclosure or RFC 7986 icon without duplicate content lines.
- **Adjudication**:
  Deliberate mapping simplification and isolation. Prevents collisions with `virtualLocations` while preserving document round-trip determinism.
- **Status**:
  Deliberate mapping design. Documented and pinned in `tests/event.rs`.

### 13.7 Divergence 7: `GEO` Coordinates and Location Map Key Synthesis vs Single-String Model

- **Observed Behavior**:
  Stalwart v1.0.0 parses RFC 5545 §3.8.1.6 `GEO:lat;lon` (and `VLOCATION` subcomponents) into `Location.coordinates` formatted as an RFC 5870 `geo:` URI (e.g. `"coordinates": "geo:37.386013,-122.082932"`). Stalwart synthesizes map keys using UUID5 hashes or `JSID` parameters. In contrast, `jmap-ical` maps incoming `LOCATION` text to `locations` with stable positional keys (`"l1"`, or `X-JMAP-KEY`) and drops `GEO` on import without polluting `event.extra`.
- **Specification and Architectural Context**:
  1. RFC 8984 §4.2.5 defines `Location` supporting `name`, `description`, `coordinates`, and `timeZone`.
  2. In Evolution Data Server (`ECalMetaBackend` / `libical`), calendar appointments store location as a single unstructured text string accessed via `e_cal_component_get_location`. EDS does not provide a dedicated geographic coordinate entry in its standard appointment editor.
  3. Generating volatile or content-hashed map keys (such as UUID5) creates churn during desktop client diffing. `jmap-cal-sync` relies on stable keys (`X-JMAP-KEY` or `"l1"`) to perform in-place property patching (`locations/<key>/name`) rather than full-map replacements.
  4. Outbound serialization emits `LOCATION` with `X-JMAP-KEY: <key>` and omits `GEO` when only `name` is present, maintaining immediate fixpoint stability.
- **Adjudication**:
  Justified architectural deviation. Stalwart provides comprehensive standalone conversion for geographic data. `jmap-ical` optimizes for EDS appointment model compatibility and stable in-place synchronization.
- **Status**:
  Justified architectural deviation. Documented and pinned in `tests/event.rs`.

### 13.8 Divergence 8: `SEQUENCE` Revision Counter Mapping vs Store-Owned Drop on Import

- **Observed Behavior**:
  Stalwart v1.0.0 maps incoming `SEQUENCE:n` to JSCalendar `"sequence": n`. In contrast, `jmap-ical` drops `SEQUENCE` on inbound parse without polluting `event.extra`, and does not emit `SEQUENCE` on outbound export.
- **Specification and Architectural Context**:
  1. RFC 5545 §3.8.7.4 defines `SEQUENCE` as the revision sequence number of a calendar component, incremented when significant changes occur.
  2. In `draft-ietf-jmap-calendars-28` §5.1 and §5.2, `sequence` is revision state managed and owned by the JMAP server upon commit. When an event is updated without an explicit sequence or with a sequence lower than or equal to the current server value, the server automatically increments the sequence number.
  3. Populating `sequence` during client-side parsing would cause `jmap-cal-sync` to propose client-dictated revision numbers during updates, interfering with server conflict detection and optimistic concurrency controls.
- **Adjudication**:
  Justified architectural deviation. Stalwart acts as an unpersisted format converter. For client-server synchronization, revision sequence numbering is store-owned and strictly managed by the JMAP server.
- **Status**:
  Justified architectural deviation. Documented and pinned in `tests/event.rs`.

### 13.9 Divergence 9: `COLOR` Property vs Calendar-Level Source Styling Boundary

- **Observed Behavior**:
  Stalwart v1.0.0 maps RFC 7986 §5.9 `COLOR` (CSS color names or hex codes) to JSCalendar `event.color` (RFC 8984 §4.4.4). In contrast, `jmap-ical` drops `COLOR` on inbound parse without polluting `event.extra`, and does not emit `COLOR` on export.
- **Specification and Architectural Context**:
  1. RFC 7986 §5.9 defines `COLOR` for per-event display styling. RFC 8984 §4.4.4 models this as `color: String`.
  2. In Evolution Data Server, calendar appointments inherit display color from their parent calendar source via `E_SOURCE_EXTENSION_CALENDAR` (`e_source_get_extension`). EDS does not expose an event-specific color picker in the standard appointment editor.
  3. Dropping `COLOR` on import avoids displaying inconsistent styling overrides in desktop calendar views while preserving server-side color properties through `PatchObject` isolation.
- **Adjudication**:
  Deliberate mapping simplification for desktop UI architecture. Documented and pinned in `tests/event.rs`.
- **Status**:
  Deliberate mapping design. Documented and pinned in `tests/event.rs`.

### 13.10 Divergence 10: `RELATED-TO` Mapping vs Unmapped Relation Graph Isolation

- **Observed Behavior**:
  Stalwart v1.0.0 maps RFC 5545 §3.8.4.5 `RELATED-TO` (parent, child, or sibling relationship) to JSCalendar `event.relatedTo` (RFC 8984 §4.2.2). In contrast, `jmap-ical` drops `RELATED-TO` on inbound parse without polluting `event.extra`.
- **Specification and Architectural Context**:
  1. RFC 8984 §4.2.2 defines `relatedTo` as a map of target UIDs to relation types.
  2. While `jmap-book-sync` models `relatedTo` extensively for JSContact relationships, Evolution's calendar appointment editor has no facility to inspect or manipulate arbitrary appointment dependency graphs.
  3. Dropping `RELATED-TO` on inbound parse protects complex server-side relation graphs from uncoordinated whole-property modification during client desktop saves.
- **Adjudication**:
  Deliberate mapping boundary. Protects server-side relation structures from uncoordinated client saves.
- **Status**:
  Deliberate mapping design. Documented and pinned in `tests/event.rs`.

### 13.11 Divergence 11: `iCalendar` / `convertedProperties` Tracking Object Omission

- **Observed Behavior**:
  Stalwart v1.0.0 emits the non-normative RFC 8984 Appendix B `"iCalendar"` object (`"convertedProperties"`) to track original property names and parameters that were transformed during conversion. In contrast, `jmap-ical` does not parse, store, or emit `"iCalendar"` tracking metadata.
- **Specification and Architectural Context**:
  1. RFC 8984 Appendix B presents the `iCalendar` object as an optional approach for lossless round-trips through systems that do not understand JSCalendar natively.
  2. `draft-ietf-jmap-calendars-28` defines `CalendarEvent` without requiring or defining the `iCalendar` tracking property.
  3. `jmap-ical` implements direct, deterministic conversion based strictly on standard JSCalendar property schemas. Omitting parser bookkeeping dictionaries prevents foreign parser artifacts from polluting JMAP account records and avoids schema bloat.
- **Adjudication**:
  Conforming serialization boundary practice. Preserves clean protocol state without non-normative tracking objects.
- **Status**:
  Conforming specification boundary. Documented and pinned in `tests/event.rs`.

### 13.12 Divergence 12: `VALARM` Non-Display Actions (`ACTION:EMAIL`, `ACTION:AUDIO`) vs Display-Only Reminder Model

- **Observed Behavior**:
  Stalwart v1.0.0 parses RFC 5545 §3.6.6 `ACTION:EMAIL` into JSCalendar `Alert` with `action: "email"` (RFC 8984 §4.5.2), along with summary and recipient attendee parameters. Stalwart may drop or handle `ACTION:AUDIO`. In contrast, `jmap-ical`'s `read_alerts` strictly filters for `ACTION:DISPLAY`, dropping `ACTION:EMAIL`, `ACTION:AUDIO`, and legacy `ACTION:PROCEDURE` on inbound parse without polluting `event.extra`.
- **Specification and Architectural Context**:
  1. RFC 8984 §4.5.2 defines two standard actions for `Alert`: `"display"` and `"email"`.
  2. In JMAP for Calendars, an email alert instructs the server to dispatch an email notification to designated attendees at the scheduled trigger time.
  3. Evolution Data Server (`ECalComponentAlarm`) focuses on desktop user notifications (popups and sound cues). The standard appointment editor does not configure automated server-side email dispatch workflows.
  4. If `ical_to_event` imported email alerts as generic display alerts, client saves would either discard the email recipient list or submit unintended reminder configurations.
  5. Outbound synchronization safety: [`maps_alerts`] requires `action` to be `"display"` (or omitted, defaulting to display). If an event in server state contains an alert with `action: "email"`, [`maps_alerts`] strictly returns `false`. This prevents `jmap-cal-sync` from performing whole-property replacement on `alerts`, ensuring server-managed email alarms are protected from accidental deletion.
- **Adjudication**:
  Deliberate mapping simplification and synchronization boundary. Display alarms are fully supported; server-side email alarms are protected from overwrite via [`maps_alerts`].
- **Status**:
  Deliberate mapping design. Documented and pinned in `tests/event.rs`.

### 13.13 Divergence 13: `VALARM` Absolute Triggers (`TRIGGER;VALUE=DATE-TIME`) vs Relative `OffsetTrigger` Model

- **Observed Behavior**:
  Stalwart v1.0.0 parses RFC 5545 §3.8.6.3 `TRIGGER;VALUE=DATE-TIME:<iso>` into JSCalendar `AbsoluteTrigger` (RFC 8984 §4.5.4: `{"@type": "AbsoluteTrigger", "when": "<iso>"}`). In contrast, `jmap-ical`'s `read_alert` only parses relative duration triggers (`OffsetTrigger`), dropping absolute date-time triggers on inbound parse without polluting `event.extra`.
- **Specification and Architectural Context**:
  1. RFC 8984 §4.5.3 defines `OffsetTrigger` (firing relative to event start or end), while §4.5.4 defines `AbsoluteTrigger` (firing at a fixed instant in time regardless of event start).
  2. In Evolution Data Server and desktop calendar workflows, appointment reminders are designed to warn the user relative to event start (e.g. 15 minutes prior). When an appointment is rescheduled, relative reminders automatically shift with the event.
  3. If an incoming absolute trigger were approximated as an offset from current start time, rescheduling the appointment would incorrectly shift the fixed reminder time. If an absolute trigger were converted lossily, fixed reminder semantics would be violated.
  4. Outbound synchronization safety: [`maps_alerts`] requires triggers to be `OffsetTrigger`. Any `AbsoluteTrigger` causes [`maps_alerts`] to return `false`, preventing `jmap-cal-sync` from replacing `alerts` whole and keeping server-managed absolute triggers intact.
- **Adjudication**:
  Deliberate mapping simplification. Preserves relative trigger semantics for desktop UI and protects server-side absolute triggers via [`maps_alerts`].
- **Status**:
  Deliberate mapping design. Documented and pinned in `tests/event.rs`.

### 13.14 Divergence 14: `VALARM` `ACKNOWLEDGED` Timestamp vs Inbound Drop and Whole-Property Replacement Safety

- **Observed Behavior**:
  Stalwart v1.0.0 parses RFC 9074 §6.1 `ACKNOWLEDGED:<utc-datetime>` into JSCalendar `Alert.acknowledged: "<utc-datetime>"` (RFC 8984 §4.5.2). In contrast, `jmap-ical`'s `read_alert` drops `ACKNOWLEDGED` on inbound parse without polluting `event.extra`.
- **Specification and Architectural Context**:
  1. RFC 9074 §6.1 defines `ACKNOWLEDGED` to record the instant a user dismissed or snoozed a reminder, allowing multiple clients sharing a calendar to synchronize dismissal state.
  2. Evolution Data Server (`ECalComponentAlarm`) does not store per-alarm acknowledged timestamps in its local SQLite database; snoozing is handled ephemerally by the `evolution-alarm-notify` desktop daemon.
  3. In JMAP, `alerts` is replaced whole during client synchronization (`PatchObject` replacing the entire `alerts` map). If `ical_to_event` imported `acknowledged` or if EDS saved an event without the acknowledged timestamp, `jmap-cal-sync` would submit a replacement `alerts` map lacking `acknowledged`, un-dismissing snoozed reminders across all user devices.
  4. To guarantee dismissal safety, [`maps_alerts`] strictly returns `false` whenever any alert contains `acknowledged`. This signals to `jmap-cal-sync` that the alarm set cannot be safely overwritten whole, protecting server-side snooze and dismissal state.
- **Adjudication**:
  Justified architectural deviation essential for multi-device alarm dismissal safety.
- **Status**:
  Justified architectural deviation. Documented and pinned in `tests/event.rs`.

### 13.15 Divergence 15: `LANGUAGE` / `ALTID` Parameters and `localizations` vs Single-Locale Desktop Model

- **Observed Behavior**:
  Stalwart v1.0.0 parses multiple `SUMMARY` or `DESCRIPTION` entries carrying `LANGUAGE=<lang>` parameters (RFC 5545 §3.2.10) into JSCalendar `localizations: Map<LanguageTag, PatchObject>` (RFC 8984 §4.6.1). In contrast, `jmap-ical` selects the primary `SUMMARY` and `DESCRIPTION` (first in document order) and drops alternate localized lines without polluting `event.extra`.
- **Specification and Architectural Context**:
  1. RFC 8984 §4.6.1 defines `localizations` as a dictionary mapping BCP 47 language tags to patch objects overriding title, description, etc.
  2. Evolution Data Server appointments store a single `summary` string and a single `description` string targeted to the desktop user's active session locale. EDS does not provide a multi-language translation editor for appointment text.
  3. If `ical_to_event` attempted to synthesize localized variants or if the client attempted to patch translations, uncoordinated partial translations would overwrite server records.
  4. Outbound serialization: `event_to_ical` emits the primary `event.title` and `event.description`. Server-side `localizations` are not modified by `jmap-cal-sync` because patches target only mapped properties (`title`, `description`).
- **Adjudication**:
  Deliberate mapping simplification for single-locale desktop environment.
- **Status**:
  Deliberate mapping design. Documented and pinned in `tests/event.rs`.

### 13.16 Divergence 16: Vendor Extension Properties (`X-*`) and `CalendarEvent.extra` Cleanliness vs Payload Preservation

- **Observed Behavior**:
  Stalwart v1.0.0 and general RFC 8984 Appendix B parsers may collect unmapped vendor extension properties (`X-APPLE-*`, `X-MICROSOFT-*`, `X-MOZ-*`, `X-LIC-*`) into custom properties or dictionary records. In contrast, `jmap-ical`'s `ical_to_event` strictly ignores vendor `X-` properties on inbound parse without populating `CalendarEvent.extra` (`event.extra` remains completely empty).
- **Specification and Architectural Context**:
  1. RFC 5545 §3.8.8.2 permits experimental and vendor-specific extension properties prefixed with `X-`. RFC 8984 Appendix B describes preservation strategies for round-tripping unmodeled properties.
  2. In Evolution Data Server (`ECalMetaBackend` / `libical`), calendar appointments provide editing interfaces for standard properties (summary, description, dates, alarms, categories, attendees), but offer no UI editing or validation for foreign vendor extensions.
  3. If `ical_to_event` preserved arbitrary vendor properties in `event.extra`, the client synchronization pipeline (`jmap-cal-sync`) would serialize them as top-level properties in JMAP `CalendarEvent/set` create or update calls. Standard JMAP servers reject unknown top-level object properties with `invalidProperties` errors, failing the entire synchronization batch.
  4. Outbound serialization: `event_to_ical` ignores `event.extra` and emits only well-defined RFC 5545 properties.
- **Adjudication**:
  Deliberate mapping design and synchronization safety boundary. Keeping `event.extra` unpolluted prevents invalid property errors on JMAP servers.
- **Status**:
  Deliberate mapping design. Documented and pinned in `tests/event.rs`.

### 13.17 Divergence 17: Inline Binary Attachments (`ATTACH;VALUE=BINARY;ENCODING=BASE64:...`) and Local `file://` URIs vs Remote Resource Reference Model

- **Observed Behavior**:
  RFC 5545 §3.8.4.1 permits embedding inline binary attachments directly within `ATTACH` properties using base64 encoding (`ATTACH;VALUE=BINARY;ENCODING=BASE64:...`). Furthermore, Evolution's desktop client generates local `file://` URIs for files selected from the local disk before upload. Stalwart v1.0.0 leverages RFC 9404 Blob storage for binary payload transfer. In contrast, `jmap-ical`'s `read_links` drops inline binary attachments and filters out local `file://` URIs (`fetched_locally`), accepting only valid non-local URIs (`https://`, `http://`, `blobId:`, `data:`).
- **Specification and Architectural Context**:
  1. Inlining binary data inside calendar JSON objects bloats synchronization payloads and breaks the JMAP protocol design separating lightweight metadata from binary blobs (RFC 8620 §6 / RFC 9404).
  2. Publishing local `file://` URIs to a shared calendar leaks the user's local filesystem paths and creates unresolvable dead links for remote attendees.
  3. Dropping local `file://` URIs and inline base64 data enforces that only accessible network resources or uploaded blobs are shared.
  4. Outbound synchronization safety: [`maps_links`] verifies that every link has a valid remote URI, and `drawn_links` renders them with `X-JMAP-KEY` for in-place patching.
- **Adjudication**:
  Deliberate architectural boundary enforcing network accessibility and payload isolation.
- **Status**:
  Deliberate mapping design. Documented and pinned in `tests/event.rs`.

### 13.18 Divergence 18: Stream-Level Container Metadata (`X-WR-CALNAME`, `X-WR-TIMEZONE`) vs Calendar Container Isolation

- **Observed Behavior**:
  Calendar exporters (such as Google Calendar, Apple Calendar, and Thunderbird) frequently write stream-level metadata on the outer `VCALENDAR` envelope, such as `X-WR-CALNAME` for the calendar title and `X-WR-TIMEZONE` for the calendar default timezone. CalDAV servers and archival converters often use `X-WR-CALNAME` to name an entire imported calendar collection. In contrast, `jmap-ical` maps individual appointment records (`VEVENT`) and drops outer `VCALENDAR` metadata without polluting `event.extra`.
- **Specification and Architectural Context**:
  1. In JMAP (RFC 8620 / draft-ietf-jmap-calendars-28), calendar containers are distinct first-class objects (`Calendar` with `id`, `name`, `color`), and appointments belong to containers via `calendarIds: Map<Id, Boolean>`.
  2. Embedding container names in `CalendarEvent` introduces redundant denormalized data and risks naming conflicts when events are moved between calendars.
  3. In Evolution Data Server, calendar collection identity is governed by `ESource` objects in the account hierarchy rather than individual appointment components.
- **Adjudication**:
  Conforming protocol boundary and relational model isolation.
- **Status**:
  Conforming specification boundary. Documented and pinned in `tests/event.rs`.

### 13.19 Divergence 19: Classification and Privacy Vocabularies (`CLASS` vs `privacy`) and Non-Standard Token Filtering

- **Observed Behavior**:
  RFC 5545 §3.8.1.3 defines classification values `PUBLIC`, `PRIVATE`, and `CONFIDENTIAL`, while admitting x-name or iana-tokens. RFC 8984 §4.4.3 models `privacy` with values `public`, `private`, `secret`, leaving vocabulary open. Stalwart v1.0.0 may preserve non-standard `CLASS` values (e.g. `CLASS:RESTRICTED` or `CLASS:SECRET`) directly into `privacy`. In contrast, `jmap-ical`'s `read_privacy` strictly maps the shared three-value scale (`PUBLIC` to `"public"`, `PRIVATE` to `"private"`, and `CONFIDENTIAL` to `"secret"`), dropping non-standard tokens on import without polluting `event.extra`.
- **Specification and Architectural Context**:
  1. Evolution Data Server's appointment UI exposes a three-option classification menu (`Public`, `Private`, `Confidential`). Non-standard tokens cannot be presented in desktop UI and would produce inconsistent round-trips.
  2. Dropping unknown classifications defaults them safely to public (the default shared by both RFC 5545 and RFC 8984).
  3. Outbound serialization: `PRIVACIES` only emits the three canonical values, omitting `CLASS` when `event.privacy` is unset.
- **Adjudication**:
  Deliberate mapping boundary for desktop UI fidelity and specification interoperability.
- **Status**:
  Deliberate mapping design. Documented and pinned in `tests/event.rs`.

### 13.20 Divergence 20: `CATEGORIES` Property Splitting, Whitespace Trimming, and Keyword Map Value Filtering

- **Observed Behavior**:
  RFC 5545 §3.8.1.2 defines `CATEGORIES` as a comma-separated list of category tokens. RFC 8984 §4.4.2 models these as `keywords: Map<String, Boolean>` where each tag maps to `true`. Stalwart v1.0.0 parses multiple `CATEGORIES` lines and splits comma-separated strings into `keywords`. In contrast, `jmap-ical`'s `read_keywords`:
  1. Trims leading and trailing whitespace from each category (`tag.trim()`) to prevent visually duplicate tags.
  2. Discards empty category tokens, including consecutive delimiters (`CATEGORIES:A,,B`) and whitespace-only tags.
  3. Returns `None` (`event.keywords: None`) when no non-empty categories exist, avoiding empty `{}` map emission.
  4. Outbound serialization ([`maps_keyword`]):
     - Strictly requires `set == &Value::Bool(true)` and rejects tags with `false` or non-boolean values.
     - Rejects tags containing carriage returns (`\r`), because `syntax::fold_into` strips carriage returns as a protocol security invariant to prevent CRLF injection.
     - Emits a single sorted `CATEGORIES` line, escaping commas, semicolons, and newlines.
- **Specification and Architectural Context**:
  1. In Evolution Data Server (`ECalComponent` / `libical`), calendar appointment categories are edited through a tag entry interface. Trailing whitespace is invisible to desktop users and accidental.
  2. RFC 8984 §4.4.2 mandates that keyword values must be boolean `true`.
  3. In JMAP synchronization (`jmap-cal-sync`), `keywords` is replaced whole if modified. Returning `None` when no categories are specified prevents the client from proposing spurious patch diffs against server records that omit `keywords`.
- **Adjudication**:
  Deliberate mapping design and canonicalization boundary. Trimming whitespace and dropping empty tokens avoids duplicate tags and protects against CRLF injection.
- **Status**:
  Deliberate mapping design. Documented and pinned in `tests/event.rs`.

### 13.21 Divergence 21: `CONFERENCE` Property and `virtualLocations` Feature Parsing, Label Handling, and Map Key Allocation

- **Observed Behavior**:
  RFC 7986 §5.11 specifies `CONFERENCE` for audio/video conferencing. RFC 8984 §4.2.6 models these as `virtualLocations: Map<Id, VirtualLocation>`. Stalwart v1.0.0 parses `CONFERENCE` into `virtualLocations`, synthesizing keys using UUID5 hashes or sequence counters. In contrast, `jmap-ical`'s `read_virtual_locations`:
  1. Preserves the `X-JMAP-KEY` parameter across round-trips to retain the exact server dictionary key.
  2. If `X-JMAP-KEY` is missing or invalid, allocates deterministic collision-free positional keys (`"v1"`, `"v2"`).
  3. Validates that the property value is a well-formed URI via `names_a_uri`, dropping invalid non-URI lines.
  4. Parses `LABEL` parameter into `VirtualLocation.name`.
  5. Parses `FEATURE` parameter tokens (`AUDIO`, `VIDEO`, `SCREEN`, `CHAT`, `MODERATOR`) into lowercase booleans in `features`.
  6. Returns `None` when no valid conference endpoints exist.
  7. Outbound serialization (`drawn_conference`) renders `CONFERENCE;VALUE=URI;X-JMAP-KEY=<key>` with `LABEL` and uppercase `FEATURE` list.
- **Specification and Architectural Context**:
  1. In Evolution Data Server, meeting URLs are displayed to the user as clickable video conference links.
  2. If dictionary keys were dynamically hashed from URI content (e.g. UUID5), editing a conference label would generate a new key. This would force `jmap-cal-sync` to delete the old virtual location and insert a new one instead of performing in-place property patching (`virtualLocations/<key>/name`).
  3. Retaining `X-JMAP-KEY` and using deterministic keys ensures stable patch paths and prevents dictionary churn.
- **Adjudication**:
  Deliberate mapping design and synchronization stability boundary. Stable key retention via `X-JMAP-KEY` prevents map churn during synchronization.
- **Status**:
  Deliberate mapping design. Documented and pinned in `tests/event.rs`.

### 13.22 Divergence 22: `TRANSP` (Time Transparency) Default Semantics, Omission vs Explicit Busy, and Non-Standard Value Dropping

- **Observed Behavior**:
  RFC 5545 §3.8.2.7 defines `TRANSP:OPAQUE` (default) and `TRANSP:TRANSPARENT`. RFC 8984 §4.4.6 defines `freeBusyStatus`: `"busy"` (default) and `"free"`. Stalwart v1.0.0 defaults `freeBusyStatus` to `"busy"` during `CalendarEvent/parse` when `TRANSP` is omitted from the incoming `VEVENT`. In contrast, `jmap-ical`'s `read_transparency`:
  1. Maps `TRANSP:OPAQUE` to `Some("busy")` and `TRANSP:TRANSPARENT` to `Some("free")` case-insensitively.
  2. If `TRANSP` is omitted, returns `None` (`event.free_busy_status: None`).
  3. If `TRANSP` contains an unknown or non-standard token (e.g. `TRANSP:TENTATIVE`), drops it and returns `None` without polluting `event.extra`.
  4. Outbound serialization (`drawn_transparency`) emits `TRANSP:OPAQUE` when `free_busy_status == "busy"`, `TRANSP:TRANSPARENT` when `free_busy_status == "free"`, and omits `TRANSP` when `None`.
- **Specification and Architectural Context**:
  1. In JMAP client synchronization (`jmap-cal-sync`), the client detects user edits by computing a structural diff against the server's record.
  2. If `read_transparency` defaulted an omitted `TRANSP` to `Some("busy")`, an event imported without `TRANSP` would report `freeBusyStatus = "busy"`. If the server originally omitted `freeBusyStatus` (null/default), the client sync engine would generate a superfluous patch operation (`{"freeBusyStatus": "busy"}`).
  3. Returning `None` when unstated preserves semantic neutrality: what the component did not state is not claimed to be explicitly set.
- **Adjudication**:
  Justified architectural deviation for diff-based synchronization fidelity. Emitting `None` when unstated avoids spurious patch operations against server defaults.
- **Status**:
  Justified architectural deviation. Documented and pinned in `tests/event.rs`.

### 13.23 Divergence 23: `PRIORITY` Integer Range Clamping, Omission Semantics, and VTODO Task Isolation

- **Observed Behavior**:
  RFC 5545 §3.8.1.9 defines `PRIORITY` as an integer from 0 to 9 (0 undefined, 1 highest, 9 lowest). RFC 8984 §4.4.1 defines `priority: UnsignedInt` (0 to 9). Stalwart v1.0.0 parses 0 to 9, but behaviors on invalid or out-of-range priorities vary across parsers. In contrast, `jmap-ical`'s `read_priority`:
  1. Strictly enforces `0..=9`. Any integer outside `0..=9` (`-1`, `10`, `100`) or non-integer string is dropped, returning `None` (`event.priority: None`) without polluting `event.extra`.
  2. An omitted `PRIORITY` in the component returns `None`, rather than synthesizing `Some(0)`.
  3. Outbound serialization (`drawn_priority`):
     - An explicit `priority: Some(0)` in `CalendarEvent` is emitted on the wire as `PRIORITY:0` (explicitly stating undefined priority).
     - When `event.priority` is `None`, the `PRIORITY` line is omitted from the `VEVENT`.
     - Invalid out-of-range values in `event.priority` are dropped on export rather than writing invalid iCalendar syntax.
  4. Non-VEVENT isolation: CalDAV systems frequently place `PRIORITY` on `VTODO` components (tasks); `jmap-ical` strictly scopes parsing to `VEVENT`, discarding non-event components so task priorities do not leak into calendar appointment state.
- **Specification and Architectural Context**:
  1. RFC 5545 §3.8.1.9 explicitly restricts valid integer priorities to 0..9.
  2. Evolution Data Server (`ECalComponent` / `libical`) maps priority to a 0..9 scale with predefined UI levels (High: 1-4, Normal: 5, Low: 6-9, Undefined: 0).
  3. Dropping out-of-bounds numbers on import ensures EDS never enters an invalid state from corrupted third-party calendar exports.
  4. Preserving the distinction between `None` (omitted) and `Some(0)` (`PRIORITY:0`) ensures exact round-trip fidelity for clients that differentiate between an unstated priority and an explicitly cleared/undefined priority.
- **Adjudication**:
  Conforming specification validation and boundary robustness. Strict range enforcement prevents invalid states in desktop UI and ensures clean round-trips.
- **Status**:
  Conforming specification boundary. Documented and pinned in `tests/event.rs`.

### 13.24 Divergence 24: `STATUS` Property Mapping, Cancellation Semantics, Task-Status Rejection, and Default Status Omission vs Explicit "confirmed"

- **Observed Behavior**:
  RFC 5545 §3.8.1.11 defines `STATUS` for `VEVENT` with values `TENTATIVE`, `CONFIRMED`, `CANCELLED`. RFC 8984 §4.4.5 defines `status: String (default: "confirmed")` with standard values `"confirmed"`, `"cancelled"`, `"tentative"`. Stalwart v1.0.0's `CalendarEvent/parse` defaults an unstated `STATUS` in incoming `VEVENT` to `"confirmed"` (`status: "confirmed"`). In contrast, `jmap-ical`'s `read_vevent`:
  1. Case-insensitively maps `CONFIRMED` to `"confirmed"`, `CANCELLED` to `"cancelled"`, and `TENTATIVE` to `"tentative"`.
  2. When `STATUS` is omitted from `VEVENT`, returns `status: None` rather than synthesizing `Some("confirmed")`.
  3. If `STATUS` contains unknown values or task-only statuses (such as RFC 5545 `VTODO` values `NEEDS-ACTION`, `COMPLETED`, `IN-PROCESS` or `VJOURNAL` values `DRAFT`, `FINAL`), drops them to `None` without polluting `event.extra`.
  4. Outbound serialization (`ical_status`):
     - Emits `STATUS:CONFIRMED`, `STATUS:CANCELLED`, or `STATUS:TENTATIVE` when `event.status` matches one of the three standard values.
     - When `event.status` is `None` or an unmapped string, omits the `STATUS` line entirely.
- **Specification and Architectural Context**:
  1. In RFC 8984 §4.4.5, `status` defaults to `"confirmed"`. However, in JMAP client synchronization (`jmap-cal-sync`), the client detects user modifications by computing diffs against the server record. If `ical_to_event` defaulted an omitted `STATUS` to `"confirmed"`, an appointment imported without `STATUS` would propose `{"status": "confirmed"}` during sync if the server had omitted the property.
  2. Evolution Data Server (`ECalComponent` / `libical`) tracks appointment status via `e_cal_component_get_status`. Non-event statuses like `COMPLETED` belong to tasks (`VTODO`), not appointments. Dropping them on import ensures EDS appointment caches remain unpolluted.
  3. Preserving `None` for unstated status maintains semantic neutrality and prevents spurious sync mutations.
- **Adjudication**:
  Justified architectural deviation and conforming boundary validation. Emitting `None` when unstated avoids spurious patch operations against server records.
- **Status**:
  Justified architectural deviation. Documented and pinned in `tests/event.rs`.

### 13.25 Divergence 25: `DTEND` vs `DURATION` Calculation, Zero-Duration Representation, and Outbound Duration Preference

- **Observed Behavior**:
  RFC 5545 §3.8.2.2 and §3.8.2.4 permit an event to specify its bounds either using `DTSTART` + `DTEND` or `DTSTART` + `DURATION`. RFC 8984 §4.1.4 models event length strictly as `duration: Duration (default: "PT0S")`, with no standalone `end` property. Stalwart v1.0.0 parses `DTEND` to compute `duration`, and when neither `DTEND` nor `DURATION` is given, or when `DTSTART == DTEND`, it emits `"duration": "PT0S"`. In contrast, `jmap-ical`'s `read_duration`:
  1. Prioritizes explicit `DURATION` if present via `stated_duration(&value)`.
  2. If `DURATION` is absent, computes `seconds = end - start` from `DTSTART` and `DTEND`, converting to ISO 8601 duration via `to_duration(seconds)`.
  3. Calculated zero duration (`DTSTART == DTEND`) or negative duration (end before start) yields `None` (`duration: None`).
  4. Stated explicit `DURATION:PT0S` is preserved verbatim as `Some("PT0S")`, while negative stated durations (`DURATION:-PT1H`) are rejected as `None`.
  5. Outbound serialization: `vevent_of` always serializes `event.duration` as `DURATION`, and never emits `DTEND`.
- **Specification and Architectural Context**:
  1. In RFC 8984, an event has `start` and `duration`. Serializing `DURATION` on outbound export avoids calculating wall-clock end times across daylight saving transitions or timezone boundaries, where adding seconds to wall clock times can yield incorrect local calendar end dates.
  2. Returning `None` when duration is calculated as zero or unstated complies with diff-based sync: RFC 8984 defaults `duration` to `"PT0S"`, so omitting it leaves the property at its natural server default without client-asserted property patches.
  3. Negative durations (events ending before they start) violate temporal coherence; dropping them protects EDS and downstream consumers from malformed calendar data.
- **Adjudication**:
  Deliberate mapping design and synchronization fidelity boundary. Serializing `DURATION` avoids DST transition skew, and dropping zero/negative calculated duration preserves server defaults and prevents malformed time intervals.
- **Status**:
  Deliberate mapping design. Documented and pinned in `tests/event.rs`.

### 13.26 Divergence 26: `showWithoutTime` (All-Day Event) DATE vs DATE-TIME Representation, Floating Time Zone Stripping, and Midnight Alignment

- **Observed Behavior**:
  RFC 5545 §3.8.2.4 defines all-day events using `DTSTART;VALUE=DATE:YYYYMMDD` without a time component or `TZID` parameter. RFC 8984 §4.1.5 models all-day events as `showWithoutTime: Boolean (default: false)` with `start` as a `LocalDateTime` at midnight (`00:00:00`) and `timeZone: null`. Stalwart v1.0.0 parses `VALUE=DATE` into `showWithoutTime: true`, setting `timeZone` to null and `start` to midnight. In contrast, `jmap-ical`'s `read_start`:
  1. Detects `VALUE=DATE` (lack of `'T'` in raw string) and produces `start: Some("YYYY-MM-DDT00:00:00")`, `time_zone: None`, and `show_without_time: Some(true)`.
  2. For timed events (containing `'T'`), returns `show_without_time: None` rather than `Some(false)`.
  3. Outbound serialization ([`shows_without_time`]):
     - Strictly validates that all all-day invariants hold: `show_without_time == Some(true)`, `time_zone.is_none()`, `at_midnight(start)`, `duration` (if present) is whole days, recurrence `UNTIL` is at midnight without time-of-day sub-rules (`BYHOUR`, `BYMINUTE`, `BYSECOND`), and all override instances start at midnight.
     - If any invariant is violated (for example an event marked `showWithoutTime: true` that starts at 14:00 or has an hourly rule), `jmap-ical` safely falls back to serializing as a timed `DATE-TIME` event rather than generating invalid iCalendar syntax or truncating non-zero times.
- **Specification and Architectural Context**:
  1. RFC 5545 §3.2.19 explicitly forbids `TZID` on `VALUE=DATE` properties.
  2. In Evolution Data Server, all-day appointments are displayed in the day grid header rather than time slots. If a malformed event arrived with non-midnight start time or non-day duration, coercing it to `VALUE=DATE` would silently drop the start time.
  3. Falling back to `DATE-TIME` preserves temporal accuracy while allowing `jmap-cal-sync` to round-trip data safely.
- **Adjudication**:
  Conforming specification validation and defensive fallback design. Preserves exact timing when all-day invariants are violated, avoiding data truncation.
- **Status**:
  Conforming specification boundary. Documented and pinned in `tests/event.rs`.

### 13.27 Divergence 27: `EXDATE` and `RDATE` Instance Modeling via `recurrenceOverrides` (`excluded: true` vs `{}`), Period Durations, and `THISANDFUTURE` Boundary

- **Observed Behavior**:
  RFC 5545 §3.8.5.1 specifies `EXDATE` (exception dates) and §3.8.5.2 specifies `RDATE` (recurrence dates). RFC 8984 §4.3.4 models individual recurrence exceptions and additions inside `recurrenceOverrides: Map<LocalDateTime, PatchObject>`. Stalwart v1.0.0 maps `EXDATE` lines into `recurrenceOverrides` with `{"excluded": true}`, and `RDATE` lines into `recurrenceOverrides` with `{}`. In contrast, `jmap-ical`'s `read_overrides`:
  1. Maps `RDATE` values to `{}` (or `{"duration": length}` if `RDATE;VALUE=PERIOD` specifies an instance-specific length).
  2. Maps `EXDATE` values to `{"excluded": true}`. If an instant appears in both `RDATE` and `EXDATE`, `EXDATE` takes precedence per RFC 5545 §3.8.5.1.
  3. Detached `VEVENT` components with matching `RECURRENCE-ID` take precedence over `RDATE` and `EXDATE`, populating property-specific patch diffs.
  4. If a `RECURRENCE-ID` carries `RANGE=THISANDFUTURE` (RFC 5545 §3.2.13), `read_overrides` deliberately skips it rather than applying the patch to only a single occurrence, because JSCalendar `recurrenceOverrides` has no representation for series truncation or future ranges.
  5. If no overrides, additions, or exceptions exist, `read_overrides` returns `None` (`recurrence_overrides: None`) rather than emitting an empty `{}` map.
  6. Outbound serialization: emits `EXDATE` for instances with `excluded: true`, and separate `VEVENT` components with `RECURRENCE-ID` for modified instances.
- **Specification and Architectural Context**:
  1. JSCalendar models all recurrence exceptions (whether deleted instances, added dates, or modified occurrences) inside a unified `recurrenceOverrides` dictionary keyed by local start time.
  2. RFC 5545's `RANGE=THISANDFUTURE` splits a recurring series into two series at the given instant. Because `recurrenceOverrides` only modifies individual instances, applying `THISANDFUTURE` to a single key would alter that day but leave all subsequent instances in their unmodified series state, creating a severe semantic divergence.
  3. Returning `None` when no overrides exist avoids empty `{}` map diff churn in `jmap-cal-sync`.
- **Adjudication**:
  Deliberate mapping design and specification boundary safety. Handles `EXDATE` and `RDATE` cleanly while protecting recurrence integrity by skipping unrepresentable `THISANDFUTURE` ranges.
- **Status**:
  Deliberate mapping design. Documented and pinned in `tests/event.rs`.

### 13.28 Divergence 28: `useDefaultAlerts` Default Semantics, Omission vs Explicit False, and Notification Preference Model

- **Observed Behavior**:
  RFC 8984 §4.5.1 defines `useDefaultAlerts: Boolean (default: false)` to specify whether the user's default reminder alerts should be applied when no explicit alerts are defined. Stalwart v1.0.0 either omits `useDefaultAlerts` or sets it to `false` when parsing incoming `VEVENT` components without custom alert configurations. In contrast, `jmap-ical`'s `ical_to_event`:
  1. Returns `use_default_alerts: None` on imported `VEVENT` components (both with and without `VALARM`), avoiding spurious diffs against server defaults.
  2. On outbound export, if an event has `use_default_alerts: Some(true)` (or `useDefaultAlerts: true` in `event.extra`), `drawn_alarms` suppresses all `VALARM` emission (returns an empty alarm set).
  3. Emitter predicate [`maps_alerts`] strictly returns `false` when `use_default_alerts` is `true`, preventing whole-property replacement that would conflict with server-side default alerts.
  4. Emitter predicate [`maps_recurrence_override`] refuses alert overrides when the series uses default alerts, protecting instance reminder integrity.
- **Specification and Architectural Context**:
  1. In Evolution Data Server and desktop calendar clients, default reminder settings are client-side application preferences configured in desktop settings ("Default reminder before event") rather than per-event boolean properties stored in `VEVENT` records.
  2. RFC 5545 defines no standard property for `useDefaultAlerts`.
  3. Setting `useDefaultAlerts: true` or `false` explicitly in server state would dictate client notification behaviors across different devices. Returning `None` when unstated prevents `jmap-cal-sync` from proposing patch mutations against server records.
- **Adjudication**:
  Deliberate mapping design and synchronization boundary safety. Suppresses `VALARM` when default alerts are active, while keeping `use_default_alerts: None` when unstated to avoid patch churn.
- **Status**:
  Deliberate mapping design. Documented and pinned in `tests/event.rs`.

### 13.29 Divergence 29: Document Language and `locale` Tag vs Property-Level `LANGUAGE` Parameter Filtering

- **Observed Behavior**:
  RFC 8984 §4.1.6 defines `locale: String` (a BCP 47 language tag) specifying the default language for event text properties. RFC 5545 §3.2.10 specifies `LANGUAGE` as a property parameter on text values (e.g. `SUMMARY;LANGUAGE=fr:Réunion`, `DESCRIPTION;LANGUAGE=fr:Discussion`). Stalwart v1.0.0 parses property-level `LANGUAGE` parameters and may infer `event.locale`. In contrast, `jmap-ical`'s `ical_to_event`:
  1. Reads `SUMMARY` and `DESCRIPTION` text while ignoring `LANGUAGE` parameters, selecting the primary entry in document order and returning `locale: None`.
  2. Outbound serialization (`event_to_ical`) emits text without `LANGUAGE` parameters, ignoring any `locale` property on `CalendarEvent` and emitting no document language tags.
  3. Leaves `event.extra` completely clean without polluting custom maps with unmapped language tags.
- **Specification and Architectural Context**:
  1. Evolution Data Server runs in the user's active desktop locale environment (`LC_ALL` / `LANG`). EDS appointment components do not expose per-event language metadata or multi-locale text entry in standard UI.
  2. If `ical_to_event` attempted to infer `locale` or serialize `LANGUAGE` parameters, desktop saves would generate uncoordinated language tags on server records.
  3. Treating the desktop session as running in a single system locale preserves clean round-trips and keeps `event.extra` unpolluted.
- **Adjudication**:
  Deliberate mapping simplification for single-locale desktop environment.
- **Status**:
  Deliberate mapping design. Documented and pinned in `tests/event.rs`.

### 13.30 Divergence 30: Floating Time (`None`) vs UTC (`Etc/UTC`) vs Canonical IANA Zone Resolution and Solidus Identifiers

- **Observed Behavior**:
  RFC 5545 §3.3.5 defines three forms of `DATE-TIME`: floating (local time without zone), UTC (`Z`), and local time with `TZID`. RFC 8984 §1.4.9 and §4.1.4 define `timeZone` as an IANA timezone name, a solidus identifier (`/custom_id`), or `null` for floating/all-day. Stalwart v1.0.0 parses UTC timestamps into `start` and `timeZone: "Etc/UTC"` (or `"UTC"`). In contrast, `jmap-ical`'s `read_start`:
  1. Floating date-times (no `Z`, no `TZID`) yield `time_zone: None`.
  2. UTC timestamps (trailing `Z`) yield `time_zone: Some("Etc/UTC")`.
  3. Windows display names (`TZID="W. Europe Standard Time"`) resolve to canonical IANA names (`Europe/Berlin`) via CLDR mapping table when defined by `VTIMEZONE`.
  4. Mozilla and Apple unique prefixes (`TZID="/mozilla.org/.../Europe/Madrid"`) normalize to canonical IANA suffixes (`Europe/Madrid`).
  5. Custom solidus zones are retained verbatim as `time_zone: Some("/org.custom/zone")`.
  6. Outbound serialization:
     - UTC (`Etc/UTC` or `UTC`) serializes with `Z` suffix.
     - Floating time serializes without `TZID` and without `Z`.
     - Canonical IANA zones serialize with `TZID=<zone>`.
     - Custom solidus zones serialize with `TZID=<solidus-zone>`.
- **Specification and Architectural Context**:
  1. RFC 5545 §3.2.19 requires `TZID` for local times with timezone reference, while forbidding `TZID` on UTC or date-only values.
  2. In Evolution Data Server, floating events (such as "lunch at 12:00" regardless of travel) must remain floating without being anchored to a specific meridian. Preserving `time_zone: None` ensures floating appointments do not shift across time zones.
  3. Resolving Windows and globally unique TZIDs to canonical IANA names allows Evolution to look up transition rules directly in the system timezone database.
- **Adjudication**:
  Conforming specification validation and canonicalization boundary. Distinguishes floating, UTC, canonical IANA, and custom solidus timezones accurately across import and export.
- **Status**:
  Conforming specification boundary. Documented and pinned in `tests/event.rs`.

### 13.31 Divergence 31: `VTIMEZONE` Observance Rules Ingestion, Standard IANA Zone Pruning, and Custom Zone Containment

- **Observed Behavior**:
  RFC 5545 §3.6.5 specifies `VTIMEZONE` components containing `STANDARD` and `DAYLIGHT` observance subcomponents with transition rules (`TZOFFSETFROM`, `TZOFFSETTO`, `RRULE`, `RDATE`). RFC 8984 §4.7.2 models custom timezone definitions inside `timeZones: Map<TimeZoneId, TimeZone>`. Stalwart v1.0.0 parses `VTIMEZONE` definitions, dropping redundant standard IANA definitions. In contrast, `jmap-ical`'s `read_time_zones`:
  1. Drops inline `VTIMEZONE` components for recognized standard IANA zone names (`time_zones: None`), preventing multi-kilobyte JSON payload bloat in JMAP event state.
  2. Preserves `VTIMEZONE` components for custom solidus zones (`/example.com/custom_tz`) with their observance rules (`TZOFFSETFROM`, `TZOFFSETTO`, `RRULE`) in `event.time_zones`.
  3. Emitter helper [`prune_time_zones`] removes unreferenced custom timezone definitions when neither the master series nor any recurrence override refers to the custom zone.
  4. Outbound serialization: [`defines_time_zone`] confirms custom zone presence, and `event_to_ical` emits `VTIMEZONE` only for custom solidus zones while omitting redundant standard IANA `VTIMEZONE` blocks.
- **Specification and Architectural Context**:
  1. RFC 8984 §1.4.9 states that standard IANA timezone names do not require definitions in `timeZones`. Only custom timezones prefixed with a solidus must provide a definition.
  2. Including full `VTIMEZONE` definitions for standard zones like `Europe/Berlin` would inflate JMAP objects with redundant historical transition data that both the server and client already possess in their host timezone databases.
  3. Dropping standard `VTIMEZONE` definitions on import keeps JMAP objects compact, while retaining custom solidus definitions ensures non-standard zones remain fully interpretable.
- **Adjudication**:
  Conforming specification boundary and payload optimization. Keeps JMAP event records compact by omitting standard IANA zone definitions while preserving custom solidus timezones.
- **Status**:
  Conforming specification boundary. Documented and pinned in `tests/event.rs`.

### 13.32 Divergence 32: `RESOURCES` Equipment and Room Lists vs Unmapped Resource Property Isolation

- **Observed Behavior**:
  RFC 5545 §3.8.1.10 defines `RESOURCES` as a comma-separated list of equipment or resource names (such as `RESOURCES:EASEL,PROJECTOR,CONFERENCE ROOM A`). In RFC 8984 and `draft-ietf-calext-jscalendarbis` §4.4, resources can be represented as `participants` with `kind: "resource"` and role `"attendee"`, or within `locations` carrying `locationTypes: ["resource"]`. Stalwart v1.0.0's `CalendarEvent/parse` converts `RESOURCES` into resource participant or location entries, or preserves them in conversion tracking dictionaries. In contrast, `jmap-ical`'s `read_vevent` drops `RESOURCES` on inbound parse without polluting `event.extra` (`extra` remains completely empty, `participants` is `None`, and `locations` is `None` unless a standard `LOCATION` property is present). Outbound serialization (`event_to_ical`) does not emit `RESOURCES`.
- **Specification and Architectural Context**:
  1. In Evolution Data Server (`ECalComponent` / `libical`), calendar appointments handle room and equipment bookings primarily through attendee scheduling entries or plain text location strings rather than a dedicated multi-value `RESOURCES` field.
  2. In JMAP calendar synchronization (`jmap-cal-sync`), `participants` are server-managed and read-only on the client side (as established in Section 13.4). If `ical_to_event` mapped `RESOURCES` into `participants` or `locations`, local edits in Evolution would propose modifications to server-authoritative room and equipment bookings.
  3. Dropping `RESOURCES` on inbound parse protects server-side room and resource scheduling from unsanctioned client mutations during desktop synchronization.
- **Adjudication**:
  Deliberate mapping boundary. Protects server-side room and equipment booking state from uncoordinated client synchronization changes.
- **Status**:
  Deliberate mapping design. Documented and pinned in `tests/event.rs`.

### 13.33 Divergence 33: `CONTACT` and `SENT-BY` / `DIR` Scheduling Parameters vs Participant Isolation

- **Observed Behavior**:
  RFC 5545 §3.8.4.2 defines `CONTACT` to represent contact information (e.g. `CONTACT:Jane Doe, +1-555-0199`). RFC 5545 §3.2.18 and §3.2.6 specify `SENT-BY` (acting on behalf of) and `DIR` (directory URI reference) on `ORGANIZER` and `ATTENDEE`. RFC 8984 §4.4 models `sentBy` on `Participant` and participants with `roles: {"contact": true}`. Stalwart v1.0.0 parses `CONTACT` lines into participant entries or related contacts, and maps `SENT-BY` to `Participant.sentBy`. In contrast, `jmap-ical` drops `CONTACT` along with `ORGANIZER` and `ATTENDEE` on import (`participants: None`) without polluting `event.extra`. Outbound serialization renders `ORGANIZER` and `ATTENDEE` when `participants` is populated, but emits no `CONTACT` lines and omits `SENT-BY` and `DIR` parameters.
- **Specification and Architectural Context**:
  1. Evolution Data Server appointments do not expose a separate contact person field outside the standard organizer and attendee lists.
  2. As established in Section 13.4, client synchronization treats `participants` as server-authoritative. The client does not independently execute iTIP scheduling workflows or modify on-behalf-of delegations.
  3. Dropping `CONTACT`, `SENT-BY`, and `DIR` on inbound parse avoids generating partial or conflicting participant patch operations during client updates.
- **Adjudication**:
  Justified architectural deviation. Preserves scheduling boundary safety and avoids unauthorized participant patching during desktop synchronization.
- **Status**:
  Justified architectural deviation. Documented and pinned in `tests/event.rs`.

### 13.34 Divergence 34: `COMMENT` Notes vs Description Field Identity and Round-Trip Determinism

- **Observed Behavior**:
  RFC 5545 §3.8.1.4 defines `COMMENT` for providing non-editorial notes or comments regarding a calendar component. Exporters frequently include both `DESCRIPTION` and one or more `COMMENT` lines. RFC 8984 / `jscalendarbis` has no dedicated top-level `comment` property for events; notes are intended for `description`. Stalwart v1.0.0 may concatenate `COMMENT` into `description` or capture it in converted properties tracking metadata. In contrast, `jmap-ical` maps `DESCRIPTION` strictly to `event.description` and drops `COMMENT` on import without polluting `event.extra`. Outbound serialization emits `DESCRIPTION` and never emits `COMMENT`.
- **Specification and Architectural Context**:
  1. Evolution Data Server provides a single multiline description editor for appointments.
  2. If `ical_to_event` concatenated `COMMENT` into `event.description`, the two distinct iCalendar properties would be permanently conflated. On export, the concatenated text would be emitted as a single `DESCRIPTION`, destroying the original separation and duplicating content across successive imports and exports.
  3. Keeping `event.description` strictly 1-to-1 with `DESCRIPTION` ensures round-trip determinism and prevents text pollution.
- **Adjudication**:
  Deliberate mapping design. Preserves field identity and prevents description pollution and duplicate comment text across round-trips.
- **Status**:
  Deliberate mapping design. Documented and pinned in `tests/event.rs`.

### 13.35 Divergence 35: Attachment `FILENAME` / `X-APPLE-FILENAME` Parameters and `title` vs URI-Only Link Model

- **Observed Behavior**:
  RFC 5545 §3.8.4.1 defines `ATTACH` with URI values. Exporters frequently append filename parameters such as `FILENAME="agenda.pdf"` or Apple's `X-APPLE-FILENAME="meeting-minutes.pdf"`. RFC 8984 §1.4.11 models `Link` with `href`, `title`, `contentType`, and `size`. Stalwart v1.0.0 parses `FILENAME` and `X-APPLE-FILENAME` parameters into `Link.title`. In contrast, `jmap-ical`'s `read_links` extracts `href`, `contentType` (from `FMTTYPE`), and `size` (from `SIZE`), dropping `FILENAME` parameters and leaving `title` omitted from imported links. Outbound serialization (`drawn_link`) renders `ATTACH` with `FMTTYPE`, `SIZE`, and `X-JMAP-KEY`, omitting `FILENAME`.
- **Specification and Architectural Context**:
  1. RFC 5545 §3.8.4.1 does not standardize a `FILENAME` property parameter for `ATTACH` (it is an unstandardized extension or draft convention).
  2. Evolution Data Server derives attachment display titles directly from the URL or filename path of the attachment URI.
  3. In JMAP synchronization (`jmap-cal-sync`), attachments are tracked via `X-JMAP-KEY` for in-place property patching of `links/<key>/href`. Because server-side attachments can be remote web links or RFC 9404 blobs, omitting unstandardized filename parameters on export preserves RFC 5545 compliance while avoiding parameter drift.
- **Adjudication**:
  Conforming specification boundary and deliberate parameter simplification. Preserves standard RFC 5545 `ATTACH` syntax while maintaining stable link key tracking.
- **Status**:
  Conforming specification boundary. Documented and pinned in `tests/event.rs`.

### 13.36 Divergence 36: `IMAGE` Property (RFC 7986 §5.10) vs `ATTACH` and `Link` `rel: "icon"` / `display` Parameter Mapping

- **Observed Behavior**:
  RFC 7986 §5.10 defines the `IMAGE` property to associate graphic badges, event logos, or illustrations with a calendar component, requiring `VALUE=URI` on URI forms and admitting optional `DISPLAY` (`BADGE`, `GRAPHIC`, `FULLSIZE`, `THUMBNAIL`). RFC 8984 §1.4.11 and §4.2.7 model external resources as `links: Map<Id, Link>`, where `rel` indicates relationship (`"icon"` for graphic emblems and `"enclosure"` for standard document attachments) and `display` specifies presentation. Stalwart v1.0.0 parses `ATTACH` and `IMAGE` into `links`, but may treat links uniformly with `rel: "enclosure"` or omit `rel` altogether. In contrast, `jmap-ical`'s `read_links`:
  1. Identifies `IMAGE` lines and explicitly sets `rel: "icon"`.
  2. Maps `DISPLAY` parameter tokens case-insensitively to lowercase `display` strings (`"badge"`, `"graphic"`, `"fullsize"`, `"thumbnail"`).
  3. Leaves `rel` and `display` omitted on standard `ATTACH` lines, preserving default enclosure semantics.
  4. Outbound serialization (`drawn_link`) inspects `rel`: if `rel == "icon"`, it renders an `IMAGE` property with mandatory `VALUE=URI`, `DISPLAY`, `FMTTYPE`, and `X-JMAP-KEY`. Non-icon links are rendered as standard `ATTACH` properties with `FMTTYPE`, `SIZE`, and `X-JMAP-KEY`.
- **Specification and Architectural Context**:
  1. Evolution Data Server and desktop calendar frontends distinguish visually between an event badge or thumbnail icon and a downloadable file attachment.
  2. If `ical_to_event` stripped `IMAGE` property semantics down to generic attachments, round-tripping an appointment would convert graphic badges into regular file attachments, cluttering the attachment list and losing the icon association.
  3. Enforcing mandatory `VALUE=URI` on outbound `IMAGE` lines strictly conforms to the grammar requirements of RFC 7986 §5.10, ensuring interoperability with RFC 7986 compliant parsers.
- **Adjudication**:
  Conforming specification boundary and rich media representation fidelity. Preserves the visual distinction between icons and document attachments across round-trips.
- **Status**:
  Conforming specification boundary. Documented and pinned in `tests/event.rs`.

### 13.37 Divergence 37: Alternate Text Representations (`ALTREP` Parameter) on Text Properties vs URI-Link Separation

- **Observed Behavior**:
  RFC 5545 §3.2.2 defines the `ALTREP` parameter on `SUMMARY`, `DESCRIPTION`, and `LOCATION`, specifying an external URI pointing to an alternate representation of the text (e.g. `DESCRIPTION;ALTREP="https://example.com/desc.html":Meeting overview`). Stalwart v1.0.0 parses `ALTREP` and may convert it into `links` with `rel: "alternate"` or capture it in conversion tracking metadata. In contrast, `jmap-ical`'s `read_vevent` extracts only the raw text value of `SUMMARY`, `DESCRIPTION`, and `LOCATION`, silently ignoring `ALTREP` parameters on inbound parse without polluting `event.extra` (`extra` remains completely empty, and `links` is `None` unless standard `ATTACH` or `IMAGE` lines exist). Outbound serialization renders clean text properties without `ALTREP`.
- **Specification and Architectural Context**:
  1. RFC 8984 §4.1.2, §4.1.3, and §4.2.5 model `title`, `description`, and `locations` strictly as plain text strings, without property-level URI parameters.
  2. In Evolution Data Server (`ECalComponent` / `libical`), appointment text is edited directly in the desktop interface. Retaining unverified external `ALTREP` URIs could trigger unvetted external network requests or confuse the desktop editor with external references.
  3. In JMAP calendar synchronization (`jmap-cal-sync`), `links` represents user-managed attachments and media. If `ical_to_event` synthesized links from `ALTREP` parameters, local saves from Evolution would propose adding unvetted URI links to the server record, causing synchronization churn.
- **Adjudication**:
  Deliberate mapping simplification and synchronization boundary safety. Preserves plain-text integrity in EDS and prevents unauthorized link creation.
- **Status**:
  Deliberate mapping design. Documented and pinned in `tests/event.rs`.

### 13.38 Divergence 38: HTML Formatted Descriptions (`X-ALT-DESC` and `STYLED-DESCRIPTION`) vs Canonical Plain Text Description

- **Observed Behavior**:
  Calendaring clients and exporters (such as Microsoft Outlook, Apple Calendar, Google Calendar, and Thunderbird) frequently attach rich HTML descriptions using vendor extensions like `X-ALT-DESC;FMTTYPE=text/html:<html>...</html>` or RFC 9073 `STYLED-DESCRIPTION;VALUE=TEXT;FMTTYPE=text/html:<html>...</html>`. Stalwart v1.0.0 may convert HTML descriptions, expose them in conversion properties, or populate `descriptionContentType: "text/html"`. In contrast, `jmap-ical`'s `read_vevent` strictly maps standard `DESCRIPTION` to `event.description` as plain text, dropping `X-ALT-DESC` and `STYLED-DESCRIPTION` on inbound import without polluting `event.extra`. Outbound serialization (`event_to_ical`) emits standard `DESCRIPTION` and never synthesizes vendor or styled description lines.
- **Specification and Architectural Context**:
  1. Evolution Data Server's appointment editor natively presents and edits plain text descriptions.
  2. If `ical_to_event` populated `description` with HTML content or preserved `X-ALT-DESC` in `event.extra`, desktop users would be presented with raw HTML markup (tags, styles, entities), or `jmap-cal-sync` would attempt to send non-standard properties in JMAP `CalendarEvent/set` calls.
  3. Standard JMAP servers reject unknown top-level object properties with `invalidProperties` errors, failing the entire synchronization batch. Maintaining `description` as plain text guarantees clean, readable text in the EDS appointment UI and keeps synchronization safe.
- **Adjudication**:
  Deliberate mapping boundary for desktop UI fidelity and synchronization safety. Prevents HTML markup pollution in desktop editors and keeps JMAP event payloads clean.
- **Status**:
  Deliberate mapping design. Documented and pinned in `tests/event.rs`.

### 13.39 Divergence 39: Multi-Component Stream Isolation, Non-VEVENT Component Rejection (`VTODO`, `VJOURNAL`), and Single-Event Record Codec Model

- **Observed Behavior**:
  RFC 5545 §3.4 permits an iCalendar stream to contain arbitrary combinations of `VEVENT`, `VTODO` (tasks), `VJOURNAL` (memos/notes), `VFREEBUSY`, and multiple unrelated `VEVENT` series with distinct `UID`s. Stalwart v1.0.0's `CalendarEvent/parse` processes stream blobs and returns an array of parsed `CalendarEvent` objects (`{"parsed": {<blobId>: [ <Event>, ... ]}}`), extracting all events present in the payload. In contrast, `jmap-ical`'s `ical_to_event` is designed strictly as a single-event record synchronization codec:
  1. If an incoming stream contains only non-VEVENT components (such as `VTODO` or `VJOURNAL`) and no `VEVENT`, `ical_to_event` returns `Err(ICalError::NoEvent)`.
  2. If an incoming stream contains mixed components, `ical_to_event` isolates the `VEVENT` series and completely ignores `VTODO` or `VJOURNAL` components without polluting `event.extra`.
  3. When multiple `VEVENT`s exist, `ical_to_event` identifies the master series and detached recurrence overrides sharing that series, rather than parsing multi-event streams into separate calendar event collections.
  4. Outbound export (`event_to_ical`) produces a single `VEVENT` component and never emits non-event component blocks.
- **Specification and Architectural Context**:
  1. In Evolution Data Server (`ECalMetaBackend`), synchronization is indexed by object ID (`load_component_sync(uid)`, `save_component_sync(uid)`). Each EDS calendar component represents a single master event series.
  2. Multi-component separation and stream splitting is handled by EDS collection import routines or `libical` component iterators before invoking the record codec.
  3. Non-event components like tasks (`VTODO`) and memos (`VJOURNAL`) belong to separate EDS source types (`ECalClientSourceType::Tasks`, `Memos`) and distinct JMAP data models (such as JMAP Tasks). Enforcing single-event record isolation prevents cross-type pollution in calendar databases.
- **Adjudication**:
  Deliberate architectural boundary and component isolation. Separates single-event record synchronization from collection-level multi-component ingestion.
- **Status**:
  Deliberate architectural boundary. Documented and pinned in `tests/event.rs`.

### 13.40 Divergence 40: `VALARM` Repetition Loops (`REPEAT` and `DURATION` Properties) vs Alarm Loop Dropping

- **Observed Behavior**:
  RFC 5545 §3.8.6.2 and §3.8.6.3 specify `DURATION` (delay interval between repetitions) and `REPEAT` (number of additional times the alarm triggers, e.g. `REPEAT:4`, `DURATION:PT5M` for snoozing 4 times at 5-minute intervals). RFC 8984 §4.5.2 models `Alert` with `trigger` and `action`, but provides no properties for repeat counts or alarm snooze intervals. Stalwart v1.0.0 and server parsers either drop `REPEAT` and `DURATION` or capture them in converted properties or vendor extensions. In contrast, `jmap-ical`'s `read_alert`:
  1. Ignores `REPEAT` and `DURATION` on inbound `VALARM` components without polluting `event.extra` or the `Alert` object.
  2. Maps only standard `ACTION` (`"display"`) and `TRIGGER` (`OffsetTrigger`).
  3. Outbound serialization (`drawn_alert`) strictly checks that alert object keys contain only `@type`, `trigger`, and `action`, refusing any alert with unmodeled repeat loop fields to prevent malformed or invalid `VALARM` output.
  4. Outbound serialization emits clean `VALARM` blocks containing only `UID`, `ACTION`, `TRIGGER`, and synthesized `DESCRIPTION`, omitting `REPEAT` and `DURATION`.
- **Specification and Architectural Context**:
  1. In Evolution Data Server (`ECalComponent` / `libical`), snoozing and alarm repetition are runtime interactive behaviors managed by the desktop notification daemon (`evolution-alarm-notify`) rather than static properties persisted in `VEVENT` records. When a user snoozes an alarm in the desktop UI, the client records a local snooze timer or updates runtime alarm state.
  2. RFC 8984 explicitly omitted recurring alarm loops from `Alert` because multi-device push notification architectures handle alert delivery at trigger time rather than executing repetitive loops on the server.
  3. Dropping `REPEAT` and `DURATION` on inbound import prevents `jmap-cal-sync` from attempting to sync unmapped properties to the JMAP server, while ensuring standard `ACTION:DISPLAY` alerts remain fully functional in desktop calendar UI.
- **Adjudication**:
  Conforming specification boundary and client-side notification architecture alignment. Desktop notification daemons handle snoozing dynamically, so dropping static `REPEAT` and `DURATION` preserves clean JSCalendar models without loss of interactive reminder functionality.
- **Status**:
  Conforming specification boundary. Documented and pinned in `tests/event.rs`.

### 13.41 Divergence 41: `VALARM` Reminder Text (`DESCRIPTION` and `SUMMARY` Properties) vs Event Title Synthesis

- **Observed Behavior**:
  RFC 5545 §3.8.6.1 mandates that a `VALARM` with `ACTION:DISPLAY` MUST contain a `DESCRIPTION` property specifying the reminder text displayed to the user (e.g. `DESCRIPTION:Meeting with team`). RFC 5545 §3.8.6.2 also allows `SUMMARY` and `DESCRIPTION` on `ACTION:EMAIL`. In contrast, RFC 8984 §4.5.2 does not define a `description` or `summary` property on `Alert`: an alert is modeled as an abstract notification trigger (`trigger`, `action`), and its display text is inherently derived from the parent `CalendarEvent.title`. Stalwart v1.0.0 parses `VALARM` and drops the reminder's `DESCRIPTION` string, treating the alert as a pure trigger for the event. In `jmap-ical`:
  1. `read_alert` ignores `DESCRIPTION` and `SUMMARY` lines inside `VALARM` components on inbound import, keeping the `Alert` object clean without polluting `event.extra`.
  2. Outbound serialization (`drawn_alert`) synthesizes `DESCRIPTION` directly from `event.title` (or omits it if title is empty), satisfying the RFC 5545 §3.8.6.1 requirement without storing redundant reminder strings in `event.alerts`.
  3. If an event has no title (`event.title: None`), `drawn_alert` emits the `VALARM` without `DESCRIPTION` rather than inventing a dummy title string.
- **Specification and Architectural Context**:
  1. In Evolution Data Server and modern calendar clients, reminder popups display the appointment's summary or title. Storing a duplicate copy of the event title inside every `Alert` object would create data redundancy and introduce divergence risks if the event title is edited without updating each alert's description.
  2. If `ical_to_event` preserved custom reminder descriptions in `Alert.extra`, `jmap-cal-sync` would fail JMAP server schema validation (`invalidProperties`) on standard servers.
  3. Synthesizing `DESCRIPTION` from `event.title` on outbound export ensures full compatibility with legacy iCalendar consumers that expect a valid `DESCRIPTION` in `VALARM:DISPLAY` blocks.
- **Adjudication**:
  Deliberate mapping design and specification synthesis boundary. Derives reminder text from event title to eliminate redundant storage and prevent synchronization drift while satisfying RFC 5545 wire grammar.
- **Status**:
  Deliberate mapping design. Documented and pinned in `tests/event.rs`.

### 13.42 Divergence 42: Recurrence Rule `UNTIL` UTC vs Local `LocalDateTime` Timezone Conversion and Value Formatting

- **Observed Behavior**:
  RFC 5545 §3.3.10 states: "If the 'DTSTART' property is specified as a date with local time and time zone reference, then the UNTIL rule part MUST also be specified as a date with UTC time." RFC 8984 §4.3.1 specifies: `until: LocalDateTime... This date-time is in the timezone of the event if the recurrence rule has no timeZone property set... MUST NOT include a time zone offset or 'Z'`. Stalwart v1.0.0's `CalendarEvent/parse` converts incoming UTC `UNTIL` values into local `LocalDateTime` within the event's timezone. In `jmap-ical`:
  1. `read_until` converts UTC `UNTIL` timestamps to local date-time strings using the observance offset when the timezone definition (`Ends::In(Zoned { ... })` or `Ends::At(offset)`) is available, stripping trailing `Z` and shifting the clock to local time.
  2. In JSCalendar RFC 8984 §4.3.1, `until` is always modeled as a `LocalDateTime` (`YYYY-MM-DDTHH:MM:SS`), including all-day events (`YYYY-MM-DDT00:00:00`).
  3. On outbound serialization (`rule_to_rrule`), it renders `UNTIL` as a date-only string (`YYYYMMDD`) for all-day events (`showWithoutTime: true`), as UTC date-time (`YYYYMMDDTHHMMSSZ`) for UTC and observance rules, and as local date-time for zoned events where local representation is required by downstream desktop libical consumers.
  4. A malformed non-digit `UNTIL` token (e.g. `UNTIL=whenever`) is rejected, preventing invalid recurrence rules from being emitted.
- **Specification and Architectural Context**:
  1. In Evolution Data Server (`ECalComponent` / `libical`), recurrence rules for zoned appointments are evaluated using local time bounds aligned with the series start.
  2. For all-day events (`showWithoutTime: true`), RFC 5545 §3.3.10 requires `UNTIL` to match the value type of `DTSTART` (`VALUE=DATE`). Emitting a date-time for an all-day event's `UNTIL` violates RFC 5545 and causes libical recurrence iterators to miscalculate the final instance.
  3. Formatting `UNTIL` as date-only for all-day events and converting observance-bound endpoints to UTC guarantees robust recurrence evaluation across both EDS desktop clients and remote JMAP servers.
- **Adjudication**:
  Conforming specification conversion and timezone alignment. Converts between UTC wire timestamps and local `LocalDateTime` representations while preserving value-type parity for all-day events.
- **Status**:
  Conforming specification boundary. Documented and pinned in `tests/event.rs`.

### 13.43 Divergence 43: Recurrence Rule Ordinal Weekdays (`BYDAY` and `NDay` Modeling) and Weekday Token Normalization

- **Observed Behavior**:
  RFC 5545 §3.3.10 defines `BYDAY` with optional signed integer ordinals prefixing two-character weekday codes (such as `2MO` for the second Monday, `-1FR` for the last Friday of the period, or `MO,TU,WE` for recurring days). RFC 8984 §4.3.2 and `draft-ietf-calext-jscalendarbis` model these as `byDay: NDay[]`, where each `NDay` is an object containing `day: String` (`"mo"`, `"tu"`, etc.) and optional `nthOfPeriod: Integer` (`2`, `-1`). Stalwart v1.0.0 parses `BYDAY` tokens into lowercase day strings and `nthOfPeriod` integers. In `jmap-ical`:
  1. `read_rrule` parses `BYDAY` strings, splits comma-separated tokens, extracts positive and negative ordinals into `nth_of_period`, and normalizes day abbreviations to lowercase tokens.
  2. On outbound serialization, `by_day_part` converts `NDay` objects back to canonical uppercase RFC 5545 tokens (`2MO`, `-1FR`, `TU`).
  3. Emitter predicate `maps_recurrence_rule` verifies that all `byDay` elements are valid, refusing rules with invalid weekday names or malformed ordinals to prevent corrupting recurrence state.
- **Specification and Architectural Context**:
  1. In Evolution Data Server, monthly and yearly recurrence patterns commonly use ordinal weekday rules (e.g. "every second Tuesday of the month" or "last Friday of the quarter").
  2. RFC 8984 mandates lowercase two-letter strings (`"su"`, `"mo"`, `"tu"`, `"we"`, `"th"`, `"fr"`, `"sa"`) for `day`, while RFC 5545 requires uppercase tokens (`SU`, `MO`, `TU`, etc.).
  3. Mapping signed ordinals faithfully into `nthOfPeriod` and preserving negative offsets (such as `-1` for the last occurrence) ensures complex recurring appointments survive round-trips without shifting to incorrect calendar dates.
- **Adjudication**:
  Conforming specification validation and structured recurrence modeling. Losslessly maps ordinal weekday rules between RFC 5545 `BYDAY` strings and JSCalendar `NDay` structures with strict token normalization.
- **Status**:
  Conforming specification boundary. Documented and pinned in `tests/event.rs`.

### 13.44 Divergence 44: Recurrence Rule Set Positions (`BYSETPOS` and Negative Indexing) vs Instance Selection within Period and Multiple `BYxxx` Part Gating

- **Observed Behavior**:
  RFC 5545 §3.3.10 specifies `BYSETPOS` operating on occurrences generated by the recurrence rule within an interval, and mandates that `BYSETPOS` MUST only be specified in conjunction with another `BYxxx` rule part (e.g. `FREQ=MONTHLY;BYDAY=MO,TU,WE,TH,FR;BYSETPOS=-1` selects the last workday of the month; `BYSETPOS=1` selects the first). Negative values count backwards from the end of the set (`-1`, `-2`). RFC 8984 §4.3.1 models `bySetPosition: Integer[]`. Stalwart v1.0.0 parses `BYSETPOS` into `bySetPosition`. In `jmap-ical`:
  1. `read_rrule` parses `BYSETPOS` into `by_set_position: Vec<i32>`, preserving signed negative offsets.
  2. Outbound serialization (`by_set_position_part`) strictly enforces RFC 5545 §3.3.10 requirement that `BYSETPOS` MUST only be emitted if another `BYxxx` part is actually written (`selects_from_a_set`), because a standalone `BYSETPOS` without another `BYxxx` part is semantically undefined or redundant.
  3. Zero is rejected (`0` is invalid in RFC 5545 and RFC 8984). Values outside `-366..=-1 | 1..=366` are rejected.
  4. `maps_recurrence_rule` validates that `by_set_position` elements are valid set positions and accompany a valid `BYxxx` part, refusing rules with unmodeled or invalid positions.
- **Specification and Architectural Context**:
  1. In Evolution Data Server, monthly and annual schedules commonly specify set positions like "the last workday of the month" or "the first and third Fridays of the month".
  2. Emitting a standalone `BYSETPOS` without another `BYxxx` part violates RFC 5545 §3.3.10 grammar and causes libical in EDS to reject the entire `RRULE` property.
  3. Preserving signed negative offsets losslessly ensures recurring appointments aligned to the end of periods do not drift or disappear across round-trips.
- **Adjudication**:
  Conforming specification boundary and structural set selection validation. Enforces RFC 5545 prerequisite dependencies (`BYxxx` presence) while losslessly mapping negative set indices.
- **Status**:
  Conforming specification boundary. Documented and pinned in `tests/event.rs`.

### 13.45 Divergence 45: Recurrence Rule Month Representations (`BYMONTH` Numbers vs String Representations and Leap Month `5L` Refusal)

- **Observed Behavior**:
  RFC 5545 §3.3.10 specifies `BYMONTH` as integer month numbers `1..=12` (`BYMONTH=1,6,12`). In contrast, RFC 8984 §4.3.1 models `byMonth: String[]` (e.g. `["1", "6", "12"]`) to accommodate lunar/lunisolar calendar leap months with suffix `L` (such as `5L` under RFC 7529 `RSCALE`). Stalwart v1.0.0 parses `BYMONTH` into an array of strings. In `jmap-ical`:
  1. `read_rrule` parses `BYMONTH` integer tokens and converts them into string representations in `by_month`.
  2. On outbound serialization, `by_month_part` / `month_token` requires strings matching canonical numbers `1..=12` without leading zeros (rejecting `03`, which libical normalizes to `3`).
  3. Deliberately refuses leap months like `5L` because iCalendar requires RFC 7529 `RSCALE` to interpret leap months, which Gregorian series do not possess.
  4. `maps_recurrence_rule` returns false if any invalid or unrepresentable month string is present.
- **Specification and Architectural Context**:
  1. Evolution Data Server operates on standard Gregorian appointments.
  2. RFC 5545 `monthnum` syntax permits `1..12` or `01..12`. Because libical normalizes leading zeros on parse, emitting `03` would cause EDS cache reads to re-render `3`, generating false edit diffs.
  3. Emitting `5L` without an `RSCALE` declaration produces an illegal iCalendar value that causes libical to reject the recurrence rule.
- **Adjudication**:
  Conforming specification boundary and calendar system isolation. Restricts Gregorian series to canonical month numbers `1..=12` without leading zeroes and refuses unmapped `RSCALE` leap month tokens to prevent invalid iCalendar generation.
- **Status**:
  Conforming specification boundary. Documented and pinned in `tests/event.rs`.

### 13.46 Divergence 46: Recurrence Rule Work Week Start Day (`WKST` vs `firstDayOfWeek` Default Omission for Monday and Day Code Case Normalization)

- **Observed Behavior**:
  RFC 5545 §3.3.10 specifies `WKST=MO` (Monday is default). RFC 8984 §4.3.1 specifies `firstDayOfWeek: String (default: "mo")` with lowercase two-character day code. Stalwart v1.0.0 parses `WKST` and normalizes uppercase iCalendar weekday codes (`MO`, `SU`, etc.) to lowercase strings (`"mo"`, `"su"`), and omits `firstDayOfWeek` when it matches `"mo"`. In `jmap-ical`:
  1. `read_rrule` extracts `WKST` and lowercases it into `first_day_of_week`.
  2. On outbound export, `first_day_of_week_part` deliberately suppresses `WKST=MO` when `first_day_of_week == "mo"`, because Monday is RFC 5545's default and `libical` strips `WKST=MO` when reading recurrence rules into Evolution Data Server's cache.
  3. Non-Monday days (such as `WKST=SU`) are serialized in canonical uppercase (`WKST=SU`).
  4. Invalid tokens or non-lowercase day codes in the JSCalendar model are refused by `weekday_token` and flagged by `maps_recurrence_rule`.
- **Specification and Architectural Context**:
  1. In Evolution Data Server (`libical`), recurrence rule parsing drops `WKST=MO` as redundant default syntax.
  2. If `event_to_ical` emitted `WKST=MO`, reading the serialized component back from EDS would drop the parameter, which `jmap-cal-sync` would interpret as user removal of `firstDayOfWeek`, causing spurious patch churn.
  3. Suppressing redundant `WKST=MO` guarantees fixed-point stability between JMAP and libical internal storage.
- **Adjudication**:
  Deliberate mapping design and round-trip cache stability. Suppresses redundant default `WKST=MO` to avoid false cache-drop diffs in EDS while strictly validating weekday tokens.
- **Status**:
  Deliberate mapping design. Documented and pinned in `tests/event.rs`.

### 13.47 Divergence 47: Recurrence Rule Frequency Gates, Incompatible Part Pruning (`BYWEEKNO` on Non-Yearly, `BYMONTHDAY` on Weekly), and Combinatorial Refusal

- **Observed Behavior**:
  RFC 5545 §3.3.10 defines strict combinatorial constraints across `RRULE` parts: `BYWEEKNO` MUST NOT be specified when `FREQ` is not `YEARLY`; `BYMONTHDAY` MUST NOT be specified when `FREQ` is `WEEKLY`; `BYYEARDAY` MUST NOT be specified when `FREQ` is `DAILY`, `WEEKLY`, or `MONTHLY`. Stalwart v1.0.0's parser accepts or normalizes frequency parts according to JSCalendar schema rules, where frequency combinations may be loosely validated. In `jmap-ical`:
  1. Outbound serialization applies strict frequency gating (`by_week_no_part` emits `BYWEEKNO` only when `frequency == "yearly"`; `by_month_day_part` refuses `BYMONTHDAY` when `frequency == "weekly"`; `by_year_day_part` refuses `BYYEARDAY` when `frequency` is daily, weekly, or monthly).
  2. `maps_recurrence_rule` validates that incompatible combinations are not emitted, refusing rules with frequency mismatches.
- **Specification and Architectural Context**:
  1. RFC 5545 §3.3.10 explicitly prohibits combinations like `FREQ=WEEKLY;BYMONTHDAY=15` because days of the month do not fit cleanly inside repeating weekly periods.
  2. In libical and Evolution Data Server, encountering an invalid rule combination causes the parser to fail or discard the recurrence rule entirely.
  3. Refusing frequency-incompatible combinations at the codec boundary alerts `jmap-cal-sync` before attempting to write invalid recurrence expressions to storage.
- **Adjudication**:
  Conforming specification validation and libical component safety. Filters frequency-incompatible rule parts to prevent downstream calendar parsers and EDS from rejecting valid event components.
- **Status**:
  Conforming specification boundary. Documented and pinned in `tests/event.rs`.

### 13.48 Divergence 48: Recurrence Rule Interval (`INTERVAL`) Default Omission and Non-Positive Refusal (`INTERVAL=0` / Negative)

- **Observed Behavior**:
  RFC 5545 §3.3.10 specifies `INTERVAL` as an optional positive integer representing at which intervals the recurrence repeats, with default value `1`. RFC 8984 §4.3.1 defines `interval: UnsignedInt (default: 1)`. Non-positive integers (`0`, negative values) are invalid in both specifications. Stalwart v1.0.0 parses `INTERVAL` and omits `interval` when `interval == 1`. In `jmap-ical`:
  1. `rrule_to_rule` parses `INTERVAL` into `rule.interval: Option<u32>` using `value.parse().ok()`. Negative values fail unsigned integer parse and are dropped.
  2. Outbound serialization (`rule_to_rrule`) deliberately suppresses `INTERVAL=1` (`rule.interval.filter(|interval| *interval != 1)`), because `1` is the RFC 5545 default and `libical` strips `INTERVAL=1` from parsed components in EDS cache.
  3. Non-default intervals (`2`, `3`, etc.) are serialized explicitly as `INTERVAL=n`.
- **Specification and Architectural Context**:
  1. In Evolution Data Server (`ECalComponent` / `libical`), recurrence rules with `INTERVAL=1` have `INTERVAL=1` dropped upon ingestion into EDS storage.
  2. If `rule_to_rrule` emitted `INTERVAL=1`, reading the component back from EDS would drop `INTERVAL=1`, causing `jmap-cal-sync` to see a spurious diff against the server. Suppressing `INTERVAL=1` maintains fixpoint stability between EDS cache and JMAP.
  3. Emitting `INTERVAL=0` would create an invalid recurrence rule that causes libical to reject the component.
- **Adjudication**:
  Deliberate mapping design and round-trip cache stability. Suppresses redundant default `INTERVAL=1` to avoid false cache-drop diffs in EDS while rejecting non-positive interval values.
- **Status**:
  Deliberate mapping design. Documented and pinned in `tests/event.rs`.

### 13.49 Divergence 49: Recurrence Rule Endpoint Mutual Exclusivity (`COUNT` vs `UNTIL`) and Conflict Handling

- **Observed Behavior**:
  RFC 5545 §3.3.10 explicitly specifies: "The UNTIL or COUNT rule parts are optional, but UNTIL and COUNT rule parts MUST NOT occur in the same 'recurrence-rule'." In contrast, RFC 8984 §4.3.1 specifies a conflict resolution rule: "Both MUST NOT be present in the same RecurrenceRule; if both are present, the until rule part MUST be ignored." Stalwart v1.0.0 parses incoming rules and enforces RFC 8984 preference or rejects conflicting rules. In `jmap-ical`:
  1. Inbound parsing (`rrule_to_rule`) parses `COUNT` and `UNTIL` into `rule.count` and `rule.until`.
  2. Outbound serialization (`rule_to_rrule`) emits `COUNT` first if present, and `UNTIL` if present.
  3. Standard conforming events specify either `count` alone, `until` alone, or neither (unbounded recurrence).
  4. An event with `count` alone serializes `COUNT=n` and omits `UNTIL`. An event with `until` alone serializes `UNTIL=...` and omits `COUNT`. An unbounded event serializes neither endpoint.
- **Specification and Architectural Context**:
  1. RFC 5545 grammar strictly forbids simultaneous `COUNT` and `UNTIL`. In libical and Evolution Data Server, encountering an `RRULE` with both `COUNT` and `UNTIL` causes libical to reject the recurrence rule or discard the entire component.
  2. For clean events originating in EDS or JMAP, recurrence rules specify either a bounding date (`until`) or a repetition count (`count`), never both.
- **Adjudication**:
  Conforming specification boundary and libical component safety. Enforces mutual exclusivity between `COUNT` and `UNTIL` to prevent emitting illegal recurrence rules.
- **Status**:
  Conforming specification boundary. Documented and pinned in `tests/event.rs`.

### 13.50 Divergence 50: Recurrence Rule Time-of-Day Parts (`BYHOUR`, `BYMINUTE`, `BYSECOND`) and Leap Second 60 vs All-Day Event Gating

- **Observed Behavior**:
  RFC 5545 §3.3.10 specifies `BYHOUR` (0..23), `BYMINUTE` (0..59), and `BYSECOND` (0..60, leap second). RFC 8984 §4.3.1 models `byHour: UnsignedInt[]`, `byMinute: UnsignedInt[]`, and `bySecond: UnsignedInt[]`. RFC 5545 §3.3.10 mandates: "The BYSECOND, BYMINUTE and BYHOUR rule parts MUST NOT be specified when the associated 'DTSTART' property has a DATE value type." Stalwart v1.0.0 parses these parts into unsigned integer arrays. In `jmap-ical`:
  1. Inbound parsing maps `BYSECOND`, `BYMINUTE`, `BYHOUR` using `to_time_of_day`. Invalid or non-digit tokens parse to `u32::MAX`, allowing `time_of_day_part` to refuse the invalid list on export and `maps_recurrence_rule` to flag the corruption.
  2. Outbound serialization (`time_of_day_part`) enforces range bounds (`0..=23` for hour, `0..=59` for minute, `0..=60` for second) and accepts leap second `60`.
  3. Out-of-bounds values (hour > 23, minute > 59, second > 60) or empty lists cause `maps_recurrence_rule` to return false.
  4. All-day event gating: `shows_without_time` checks `names_a_time_of_day`. An event with `show_without_time: true` whose rule names a time of day is drawn as a timed `DATE-TIME` event instead of `DATE`, satisfying RFC 5545.
- **Specification and Architectural Context**:
  1. In Evolution Data Server, emitting `BYHOUR`/`BYMINUTE`/`BYSECOND` alongside a `VALUE=DATE` `DTSTART` causes libical to reject the recurrence rule.
  2. Upgrading all-day events with sub-day recurrence parts to timed representations preserves the recurrence schedule while maintaining RFC 5545 compliance.
  3. Accepting leap second 60 in `BYSECOND` matches libical's parser capability.
- **Adjudication**:
  Conforming specification boundary and value-type consistency. Preserves time-of-day recurrence precision while avoiding illegal `DATE` + `BYxxx` time-part combinations.
- **Status**:
  Conforming specification boundary. Documented and pinned in `tests/event.rs`.

### 13.51 Divergence 51: Recurrence Rule Calendar Scale (`RSCALE`) and Leap Handling (`SKIP`) vs Gregorian-Only Isolation and Refusal

- **Observed Behavior**:
  RFC 7529 defines `RSCALE` (e.g. `RSCALE=GREGORIAN`, `RSCALE=HEBREW`, `RSCALE=ISLAMIC`, `RSCALE=CHINESE`) and `SKIP` (`SKIP=OMIT`, `SKIP=BACKWARD`, `SKIP=FORWARD`) for non-Gregorian calendar systems and leap handling. RFC 8984 §4.3.1 defines `rscale: String (default: "gregorian")` and `skip: String (default: "omit")`. Stalwart v1.0.0 either drops `RSCALE`/`SKIP` or parses them if non-Gregorian calendars are supported. In `jmap-ical`:
  1. Inbound `rrule_to_rule` deliberately drops `RSCALE` and `SKIP` on parse without polluting `event.extra`.
  2. Outbound serialization does not emit `RSCALE` or `SKIP`.
  3. `maps_recurrence_rule` strictly requires `rule.rscale.is_none() && rule.skip.is_none()`, refusing any non-Gregorian recurrence rule to prevent EDS libical from failing or calculating corrupted occurrences.
- **Specification and Architectural Context**:
  1. Evolution Data Server and `libical` operate exclusively in the Gregorian calendar system. `libical` does not implement RFC 7529 non-Gregorian recurrence calculations.
  2. Emitting `RSCALE=CHINESE` or `SKIP=FORWARD` to libical would result in unparseable recurrence rules or corrupted appointment occurrences in EDS.
  3. Refusing non-Gregorian recurrence rules at the codec boundary protects local calendar storage from invalid series calculations.
- **Adjudication**:
  Conforming specification boundary and calendar system isolation. Restricts recurrence evaluation to the Gregorian calendar to prevent invalid calculations in EDS and libical.
- **Status**:
  Conforming specification boundary. Documented and pinned in `tests/event.rs`.

### 13.52 Divergence 52: Recurrence Rule Month Day Indexing (`BYMONTHDAY` Positive and Negative Days vs Zero Refusal)

- **Observed Behavior**:
  RFC 5545 §3.3.10 specifies `BYMONTHDAY` as a comma-separated list of days of the month, valid from 1 to 31 or -31 to -1 (where -1 represents the last day of the month, -2 the penultimate day). RFC 8984 §4.3.1 defines `byMonthDay: Integer[]` with identical signed values. Both specifications forbid zero (`0`). RFC 5545 §3.3.10 explicitly mandates: "The BYMONTHDAY rule part MUST NOT be specified when the associated 'FREQ' rule part is set to 'WEEKLY'." Stalwart v1.0.0 parses `BYMONTHDAY` into `byMonthDay`. In `jmap-ical`:
  1. Inbound parsing (`rrule_to_rule`) parses `BYMONTHDAY` using `to_month_day`. Non-numeric or invalid tokens parse to sentinel `0`.
  2. Outbound serialization (`by_month_day_part`) validates each day with `month_day_token`, which accepts `-31..=-1 | 1..=31` and returns `None` for `0` or out-of-bounds values.
  3. Frequency gating: `by_month_day_part` returns `None` if `frequency` is `"weekly"`, satisfying RFC 5545 §3.3.10.
  4. `maps_recurrence_rule` returns false if `by_month_day` contains `0`, values outside `-31..=31`, or if specified alongside weekly recurrence.
- **Specification and Architectural Context**:
  1. In Evolution Data Server (`ECalComponent` / `libical`), calendar weeks do not align with calendar months. Emitting `BYMONTHDAY` with `FREQ=WEEKLY` violates RFC 5545 grammar and causes `libical` to reject the recurrence rule or discard the component.
  2. Day zero does not exist in any calendar month. Permitting `0` would generate malformed iCalendar syntax that libical rejects.
  3. Preserving signed negative offsets (such as `-1` for the last day of the month) ensures that month-end recurring appointments calculate correct instances without date drift.
- **Adjudication**:
  Conforming specification boundary and libical component safety. Enforces valid signed day ranges `-31..=-1 | 1..=31`, rejects invalid day zero, and filters out weekly frequency combinations to prevent component rejection.
- **Status**:
  Conforming specification boundary. Documented and pinned in `tests/event.rs`.

### 13.53 Divergence 53: Recurrence Rule Year Day Indexing (`BYYEARDAY` Positive and Negative Days and Leap Year 366 vs Frequency Restriction)

- **Observed Behavior**:
  RFC 5545 §3.3.10 specifies `BYYEARDAY` as a comma-separated list of days of the year, valid from 1 to 366 or -366 to -1 (where 366 represents leap day and -1 represents December 31st). Day zero (`0`) is invalid. RFC 8984 §4.3.1 models `byYearDay: Integer[]`. RFC 5545 §3.3.10 specifies that `BYYEARDAY` MUST NOT be specified when `FREQ` is `DAILY`, `WEEKLY`, or `MONTHLY`. Stalwart v1.0.0 parses `BYYEARDAY` into `byYearDay`. In `jmap-ical`:
  1. Inbound parsing (`rrule_to_rule`) extracts `BYYEARDAY` tokens into `rule.by_year_day`.
  2. Outbound serialization (`by_year_day_part`) validates days with `year_day_token`, which accepts `-366..=-1 | 1..=366` and returns `None` for `0` or values outside that range.
  3. Frequency gating: `holds_a_year(&rule.frequency)` disallows `daily`, `weekly`, and `monthly`, while permitting `yearly` as well as sub-day frequencies (`hourly`, `minutely`, `secondly`) defined in RFC 5545.
  4. `maps_recurrence_rule` returns false if any year day is invalid or if the frequency cannot contain a year day.
- **Specification and Architectural Context**:
  1. Days of the year are defined for intervals that span a year or sub-day intervals within a year. In libical and Evolution Data Server, combining `BYYEARDAY` with daily, weekly, or monthly periods causes recurrence rule parsing to fail.
  2. Admitting day 366 and -366 is essential for scheduling leap-year specific occurrences.
  3. Rejecting day zero and enforcing valid bounds protects EDS storage from corrupted recurrence evaluations.
- **Adjudication**:
  Conforming specification boundary and calendar arithmetic safety. Supports full signed year-day ranges including leap day 366 while enforcing RFC 5545 frequency restrictions.
- **Status**:
  Conforming specification boundary. Documented and pinned in `tests/event.rs`.

### 13.54 Divergence 54: Recurrence Rule ISO Week Number Indexing (`BYWEEKNO` Positive and Negative Week Ordinals vs Zero Refusal and Yearly Frequency Gating)

- **Observed Behavior**:
  RFC 5545 §3.3.10 specifies `BYWEEKNO` as ordinals specifying weeks of the year per ISO 8601, valid from 1 to 53 or -53 to -1 (where 53 represents the leap week in long years, and -1 represents the final week). Zero (`0`) is invalid. RFC 8984 §4.3.1 defines `byWeekNo: Integer[]`. RFC 5545 §3.3.10 mandates: "The BYWEEKNO rule part MUST NOT be specified when the associated 'FREQ' rule part is set to anything other than 'YEARLY'." Stalwart v1.0.0 parses `BYWEEKNO` into `byWeekNo`. In `jmap-ical`:
  1. Inbound parsing (`rrule_to_rule`) parses `BYWEEKNO` into `rule.by_week_no`.
  2. Outbound serialization (`by_week_no_part`) validates each week with `week_no_token`, accepting `-53..=-1 | 1..=53` and rejecting `0` or numbers exceeding 53.
  3. Frequency gating: `by_week_no_part` strictly requires `rule.frequency.eq_ignore_ascii_case("yearly")`, refusing all other frequencies (including sub-day and monthly).
  4. `maps_recurrence_rule` verifies that week numbers are within range and frequency is yearly, flagging incompatible rules.
- **Specification and Architectural Context**:
  1. ISO 8601 week numbers exist exclusively in the context of an entire year. In Evolution Data Server (`ECalComponent` / `libical`), specifying `BYWEEKNO` with monthly or daily recurrence rules causes libical to reject the rule.
  2. Week numbers are coordinated with `firstDayOfWeek` (`WKST`), ensuring week boundaries align with ISO 8601 expectations.
  3. Refusing week zero and numbers outside 1..53 prevents emitting invalid syntax to libical or downstream JMAP servers.
- **Adjudication**:
  Conforming specification boundary and ISO 8601 calendar alignment. Enforces strict yearly frequency gating and signed week number ranges `-53..=-1 | 1..=53`.
- **Status**:
  Conforming specification boundary. Documented and pinned in `tests/event.rs`.

### 13.55 Divergence 55: Recurrence Rule Recurrence Count Bounds (`COUNT` Positive Integer Requirement vs Zero/Negative Refusal and Unbounded Series)

- **Observed Behavior**:
  RFC 5545 §3.3.10 specifies `COUNT` as an optional positive integer (1 or greater) defining the number of occurrences at which to range-bound the recurrence. `COUNT=0` and negative values are prohibited. RFC 8984 §4.3.1 defines `count: UnsignedInt`. Stalwart v1.0.0 parses `COUNT` into an unsigned integer. In `jmap-ical`:
  1. Inbound parsing (`rrule_to_rule`) parses `COUNT` into `rule.count: Option<u32>`.
  2. Outbound serialization (`rule_to_rrule`) emits `COUNT=n` when `rule.count` is present.
  3. Unbounded series representation: when neither `count` nor `until` is set in the JSCalendar model, outbound serialization emits an unbounded recurrence rule without synthesizing a dummy count.
  4. Non-positive count refusal: `COUNT=0` is not emitted for unbounded series, and non-numeric or negative values are dropped on parse.
- **Specification and Architectural Context**:
  1. In RFC 5545 grammar, `COUNT` must be a non-zero positive integer. In Evolution Data Server (`ECalComponent` / `libical`), encountering `COUNT=0` causes libical to reject the recurrence rule or create an empty occurrence set where the appointment vanishes from the calendar view.
  2. For series intended to repeat indefinitely, omitting `COUNT` and `UNTIL` entirely is the canonical representation across both RFC 5545 and RFC 8984.
  3. Enforcing positive count integers and unbounded omission prevents synchronization anomalies between EDS and JMAP storage.
- **Adjudication**:
  Conforming specification boundary and libical component safety. Restricts count values to positive integers and models unbounded recurrence through property omission.
- **Status**:
  Conforming specification boundary. Documented and pinned in `tests/event.rs`.

### 13.56 Divergence 56: Recurrence Override Instance Key (`id`) Local Date-Time Modeling vs ISO Instant / UTC Representation

- **Observed Behavior**:
  RFC 5545 §3.8.4.4 specifies `RECURRENCE-ID` as a date or date-time identifying a recurrence instance. When `DTSTART` uses a local time with timezone, `RECURRENCE-ID` specifies the matching local time with `TZID`. RFC 8984 §4.3.4 models `recurrenceOverrides: Map<LocalDateTime, PatchObject>`, where map keys are `LocalDateTime` strings matching the original instance start time. RFC 8984 §1.4.3 requires that a `LocalDateTime` MUST NOT include a timezone offset or 'Z'. Stalwart v1.0.0 parses `RECURRENCE-ID` into `recurrenceOverrides` using `LocalDateTime` strings. In `jmap-ical`:
  1. Inbound parsing (`read_overrides`): converts `RECURRENCE-ID` values into local date-time strings (`to_local_date_time`), stripping UTC offsets and trailing `Z` from the map key.
  2. Outbound serialization (`vevent_of_instance` / `event_to_ical`): validates that the key is a valid date-time (`to_ical_date_time(id)`), and formats `RECURRENCE-ID` with matching `TZID` or `VALUE=DATE` according to the series configuration.
  3. Validation predicate: `override_maps_by` ensures `to_ical_date_time(id).is_some()`, rejecting unparseable or malformed date-time keys.
- **Specification and Architectural Context**:
  1. In Evolution Data Server (`ECalComponent` / `libical`), detached instances must match the exact calculated recurrence instance time of the series.
  2. Storing UTC timestamps with 'Z' or arbitrary strings as `recurrenceOverrides` keys violates RFC 8984 §4.3.4 and causes key lookup mismatches in EDS.
  3. Formatting `RECURRENCE-ID` with the proper timezone parameter or `VALUE=DATE` ensures libical locates and binds the detached instance to the series occurrence.
- **Adjudication**:
  Conforming specification boundary and libical component alignment. Formats instance keys strictly as RFC 8984 `LocalDateTime` without timezone offset or Z, and renders `RECURRENCE-ID` with matching value type and timezone parameter.
- **Status**:
  Conforming specification boundary. Documented and pinned in `tests/event.rs`.

### 13.57 Divergence 57: Recurrence Override Cancellation (`excluded: true`) Purity vs Restated Property Conflict Rejection

- **Observed Behavior**:
  RFC 5545 §3.8.5.1 models cancelled instances using `EXDATE` lines on the master `VEVENT`. In RFC 8984 §4.3.4, an excluded instance is modeled as `{"excluded": true}` in `recurrenceOverrides`. RFC 8984 §4.3.4 explicitly mandates: "The excluded property, if present, MUST be true. If true, the PatchObject MUST NOT contain any other properties." Stalwart v1.0.0 parses `EXDATE` into `{"excluded": true}`. In `jmap-ical`:
  1. Inbound parsing: maps `EXDATE` lines to `{"excluded": true}`.
  2. Outbound serialization (`recurrence_dates`): emits `EXDATE` on the master component for instances with `excluded: true` and never emits detached `VEVENT` blocks for excluded instances.
  3. Validation predicate: `override_maps_by` enforces single-property purity (`if excluded(patch) { return fields.len() == 1; }`), rejecting patches combining `excluded: true` with other properties or non-boolean values.
- **Specification and Architectural Context**:
  1. In Evolution Data Server and libical, `EXDATE` cancels an occurrence entirely. There is no detached component in which to store an edited title, location, or alarm for an excluded occurrence.
  2. Permitting restated properties alongside `excluded: true` would deceive clients into expecting partial overrides on deleted instances or cause JMAP whole-property sync failures.
  3. Enforcing single-field purity guarantees RFC 8984 §4.3.4 compliance and libical storage safety.
- **Adjudication**:
  Conforming specification validation and libical structural integrity. Enforces single-field purity for `excluded: true` to prevent conflicting cancellation and property override states.
- **Status**:
  Conforming specification boundary. Documented and pinned in `tests/event.rs`.

### 13.58 Divergence 58: Recurrence Override Property Allowlist (`OVERRIDE_PROPERTIES`) vs Complex Sub-Object and Participant Isolation

- **Observed Behavior**:
  RFC 8984 §4.3.4 theoretically allows a PatchObject to patch any property defined on `CalendarEvent`. In contrast, RFC 5545 detached `VEVENT` components have limited semantics for per-instance sub-objects like participants, locations, and attachments. Modifying participants on individual occurrences in iCalendar requires full `ATTENDEE` / `ORGANIZER` rescheduling state which is prone to sync loops across servers. Stalwart v1.0.0's parser captures properties present on detached components, but may lose or unfaithfully represent complex sub-object patches. In `jmap-ical`:
  1. `OVERRIDE_PROPERTIES` defines a strict allowlist of 11 scalar and bounded properties: `title`, `description`, `start`, `timeZone`, `duration`, `status`, `freeBusyStatus`, `priority`, `privacy`, `keywords`, and `alerts`.
  2. `maps_override_field` returns `false` if any unlisted property (such as `locations`, `virtualLocations`, `participants`, `links`, or `locale`) is present in an override patch.
  3. Detached instances inherit `locations`, `virtual_locations`, `participants`, and `links` directly from the master series (`event.locations.clone()`, etc.).
  4. `recurrence_dates` falls back to emitting `RDATE` for overrides that cannot be fully drawn as detached components.
- **Specification and Architectural Context**:
  1. In Evolution Data Server (`ECalComponent`), detached instances share attendee lists and physical room locations with the master appointment.
  2. If an override patched `participants` or `locations` on a single occurrence, writing that to EDS or round-tripping through standard iCalendar would either drop the change or corrupt meeting invitations.
  3. Restricting override patches to the 11 supported properties prevents data loss and avoids generating invalid patch churn in `jmap-cal-sync`.
- **Adjudication**:
  Deliberate mapping design and synchronization boundary safety. Constrains per-instance overrides to properties that cleanly map to RFC 5545 detached components while inheriting complex structures from the parent series.
- **Status**:
  Deliberate mapping design. Documented and pinned in `tests/event.rs`.

### 13.59 Divergence 59: Recurrence Override TimeZone Scoping: Custom TimeZone Definitions vs Isolated Patch Rejection (`sends_recurrence_override` vs `maps_recurrence_override`)

- **Observed Behavior**:
  RFC 8984 §1.4.9 and §4.7.2 require custom timezone identifiers (such as `/example.com/CustomTZ` or vendor timezones) to be accompanied by a timezone definition in the root `timeZones` map. RFC 5545 §3.6.5 defines `VTIMEZONE` components at the `VCALENDAR` root, shared by all `VEVENT` components. Stalwart v1.0.0 parses timezone references and resolves them against root `VTIMEZONE` blocks or server-known zone tables. In `jmap-ical`:
  1. Isolated patch validation: `maps_recurrence_override` enforces `names_time_zone(tzid)` (standard IANA timezone names only), refusing custom timezones when `recurrenceOverrides` is patched in isolation.
  2. Full save and serialization: `sends_recurrence_override` uses `draws_override_field`, which accepts custom timezones if the series defines them (`defines_time_zone(series, tzid)`).
  3. Floating time: setting `"timeZone": null` is accepted, representing an instance that floats without timezone conversion.
- **Specification and Architectural Context**:
  1. If an override patch introduced a custom timezone identifier without providing the corresponding `timeZones` definition, the JMAP server would reject the patch (`invalidProperties`), or downstream consumers would fail to compute UTC start times.
  2. Evolution Data Server relies on libical's timezone cache; an undefined custom `TZID` on a `RECURRENCE-ID` causes recurrence expansion to fail.
  3. Distinguishing isolated override validation from full-document serialization prevents dangling timezone references.
- **Adjudication**:
  Conforming specification boundary and dependency validation. Enforces that custom timezone identifiers in recurrence overrides must be accompanied by explicit definitions in `timeZones`.
- **Status**:
  Conforming specification boundary. Documented and pinned in `tests/event.rs`.

### 13.60 Divergence 60: Recurrence Override Property Removal via `null` vs Absent Property Ingestion in Detached Components

- **Observed Behavior**:
  RFC 8984 §4.3.4 models per-instance modifications as a `PatchObject`. Under RFC 6902 / JSON merge-patch rules, setting a property to `null` deletes or unsets the property on that instance (such as `"status": null`, `"freeBusyStatus": null`, `"priority": null`, `"privacy": null`, `"keywords": null`, `"alerts": null`, `"description": null`, `"timeZone": null`). In contrast, RFC 5545 detached `VEVENT` components have no explicit "null" or "unset" token. When an occurrence clears a property, its detached `VEVENT` component simply omits the corresponding content line. Stalwart v1.0.0 parses detached `VEVENT` components into JSCalendar, setting omitted properties to null in `recurrenceOverrides` when differing from the master series. In `jmap-ical`:
  1. Inbound parsing (`read_overrides` -> `instance_patch`): compares the detached component against the master series across all restatable properties (`title`, `description`, `timeZone`, `duration`, `status`, `freeBusyStatus`, `privacy`, `priority`, `keywords`, `alerts`). When a property was present on the series but is absent on the detached component, `instance_patch` generates an explicit `null` in the patch object.
  2. Outbound serialization (`modified_instance` -> `vevent_of`): when an override patch sets a property to `null`, `modified_instance` clears that property on the instance (`instance.status = None`, etc.), and `vevent_of` omits the corresponding content line from the emitted detached `VEVENT`.
  3. Validation predicate: `maps_override_field` admits `value.is_null()` for restatable properties, ensuring callers can legally clear properties on individual occurrences.
- **Specification and Architectural Context**:
  1. In Evolution Data Server (`ECalComponent` / `libical`), an occurrence can have its status, category, alarm, or priority cleared relative to the master series.
  2. When EDS saves a detached occurrence without `STATUS` or without `CATEGORIES`, converting this to JSCalendar requires generating `null` to inform JMAP that the occurrence does not inherit the parent series property.
  3. Omitting `null` would cause the server or client sync to falsely inherit parent properties, making deleted alarms or cleared categories reappear after resync.
- **Adjudication**:
  Conforming specification mapping and fixpoint round-trip stability. Generates `null` for properties present on the series but omitted on the detached component, and suppresses iCalendar content lines when an override specifies `null`.
- **Status**:
  Conforming specification boundary. Documented and pinned in `tests/event.rs`.

### 13.61 Divergence 61: Recurrence Override Empty String (`""`) Refusal in `title` and `description` vs `null` Deletion

- **Observed Behavior**:
  In JSON and RFC 8984 §4.1.1 and §4.1.2, an empty string (`"title": ""`, `"description": ""`) is syntactically distinct from `null` (`"title": null`, `"description": null`). Setting `"title": ""` asserts that the event's title is explicitly the empty string, whereas `"title": null` removes the title property (or reverts to default). However, in RFC 5545 §3.8.1.12 (`SUMMARY`) and §3.8.1.5 (`DESCRIPTION`), a property line with empty text (`SUMMARY:` or `DESCRIPTION:`) is either prohibited by value type grammar, normalized to absent, or dropped by conformant generators. Stalwart v1.0.0 parses empty or missing summary lines as absent in JSCalendar. In `jmap-ical`:
  1. Outbound serialization (`vevent_of`): filters out empty strings (`filter(|value| !value.is_empty())`), omitting empty `SUMMARY` and `DESCRIPTION` lines from both master and detached components.
  2. Round-trip hazard: because empty strings are dropped during outbound serialization, a detached component with `"title": ""` would serialize without a `SUMMARY` line. When read back, `instance_patch` would observe `instance.title == None` and emit `{"title": null}` instead of `{"title": ""}`, breaking round-trip idempotence.
  3. Validation predicate: `maps_override_field` explicitly refuses empty strings for `title` and `description` (`value.is_null() || value.as_str().is_some_and(|text| !text.is_empty())`), while admitting `null`.
- **Specification and Architectural Context**:
  1. In Evolution Data Server and libical, emitting `SUMMARY:` with empty text causes libical to drop the property or generate invalid syntax.
  2. Permitting empty strings in JMAP patches would lead to unstable round-trips: saving `{"title": ""}` would serialize to no `SUMMARY`, which upon resync would turn into `{"title": null}` or inherit the series title, triggering spurious change notifications in `jmap-cal-sync`.
  3. Refusing empty strings at the validation boundary ensures that clients use `null` to clear a title or description.
- **Adjudication**:
  Conforming specification boundary and round-trip fixpoint safety. Refuses empty strings `""` in override patches while permitting `null` to remove properties cleanly.
- **Status**:
  Conforming specification boundary. Documented and pinned in `tests/event.rs`.

### 13.62 Divergence 62: Recurrence Override Rescheduled Start Time (`start` Property) vs Instance Key (`id` / `RECURRENCE-ID`)

- **Observed Behavior**:
  RFC 5545 §3.8.4.4 models recurring instances using `RECURRENCE-ID`, which identifies the original recurrence occurrence slot from the pattern. When an instance is rescheduled (e.g. postponed by two hours or moved to another day), the detached `VEVENT` retains `RECURRENCE-ID` pointing to the original recurrence slot, but its `DTSTART` specifies the new, rescheduled start time (`DTSTART != RECURRENCE-ID`). In RFC 8984 §4.3.4, `recurrenceOverrides` is keyed by the original recurrence start time (`id`), matching `RECURRENCE-ID`. An override patch only needs to specify `"start"` if the instance start time actually changed. RFC 8984 §4.3.4 states: "The start property, if present, overrides the start time of the occurrence." Stalwart v1.0.0 parses detached `VEVENT` components where `DTSTART != RECURRENCE-ID` and outputs `recurrenceOverrides[id]` containing `"start": "<new_time>"`. In `jmap-ical`:
  1. Inbound parsing (`instance_patch`): suppresses `"start"` when `DTSTART == id`, avoiding redundant property churn. When `DTSTART != id`, `instance_patch` includes `"start": "<rescheduled_datetime>"`.
  2. Outbound serialization (`modified_instance` -> `vevent_of`): defaults `instance.start` to `id` when `"start"` is omitted in the patch; updates `instance.start` when `"start"` is present; and renders `RECURRENCE-ID` at `id` (in series timezone) and `DTSTART` at `instance.start` (in instance timezone).
  3. Validation predicate: `maps_override_field` verifies that any overridden `"start"` parses as a valid date-time string via `to_ical_date_time`.
- **Specification and Architectural Context**:
  1. In Evolution Data Server, dragging an occurrence to another hour generates a detached `ECalComponent` with `RECURRENCE-ID` set to the original occurrence and `DTSTART` set to the new time.
  2. Suppressing redundant `start` when an occurrence is not rescheduled keeps JMAP patch payloads minimal and prevents patch diff noise.
  3. Correctly decoupling `RECURRENCE-ID` (series recurrence slot) from `DTSTART` (rescheduled instance time) ensures both EDS and remote JMAP servers accurately display rescheduled occurrences without duplicating or detaching them from the series.
- **Adjudication**:
  Conforming specification boundary and calendar rescheduling fidelity. Suppresses redundant `start` when matching `id`, emits `start` only when rescheduled, and maintains strict separation between `RECURRENCE-ID` (original slot) and `DTSTART` (effective occurrence time).
- **Status**:
  Conforming specification boundary. Documented and pinned in `tests/event.rs`.

### 13.63 Divergence 63: Recurrence Override Duration Modification and `RDATE` Period Length Calculation vs Series Duration Inheritance

- **Observed Behavior**:
  RFC 5545 §3.8.5.2 permits `RDATE` to specify occurrences as discrete date-times (such as `RDATE:20260908T100000Z`) or as explicit periods with duration or end time (such as `RDATE;VALUE=PERIOD:20260908T100000Z/PT2H` or `RDATE;VALUE=PERIOD:20260908T100000Z/20260908T120000Z`). In RFC 8984 §4.3.4, extra occurrences added via `RDATE` are modeled as entries in `recurrenceOverrides`. If an added occurrence has the same duration as the series, its patch is `{}` (empty patch). If it has a different duration, its patch is `{"duration": "<length>"}`. Stalwart v1.0.0 parses `RDATE` entries and detached components, omitting `"duration"` in `recurrenceOverrides` when matching the series default. In `jmap-ical`:
  1. Inbound parsing (`read_overrides`): for `RDATE` entries, `period_length(start, end)` calculates the period duration in seconds from wall-clock difference (`instant(end) - instant(start)`) and formats it as an ISO 8601 duration via `to_duration`. If the period length equals the series duration, an empty patch `{}` is produced. If it differs, `{"duration": length}` is emitted.
  2. Detached components: `instance_patch` compares `series.duration` vs `instance.duration`. If they differ, `patch.insert("duration", now)` is emitted. If matching, `duration` is omitted, allowing the instance to inherit series duration.
  3. Outbound serialization (`vevent_of`): an instance with modified duration emits its own `DURATION` line. An override with `{}` (bare `RDATE`) or matching duration emits `RDATE` on the master component without a detached `VEVENT`.
- **Specification and Architectural Context**:
  1. In Evolution Data Server and libical, recurring appointments with occasional extended sessions (such as an annual board meeting in a recurring monthly series) specify either a detached `VEVENT` with a custom `DURATION`/`DTEND` or an `RDATE;VALUE=PERIOD`.
  2. Calculating duration from period bounds and suppressing redundant durations that match the series ensures clean JSCalendar models and avoids false diffs in `jmap-cal-sync`.
  3. Preserving series duration inheritance for unmodified occurrences keeps detached components concise.
- **Adjudication**:
  Conforming specification boundary and duration calculation precision. Emits `duration` in override patches only when differing from series duration, calculates period lengths from `RDATE` start/end pairs, and preserves series duration inheritance.
- **Status**:
  Conforming specification boundary. Documented and pinned in `tests/event.rs`.

### 13.64 Divergence 64: Recurrence Override `status` Property Mapping, Cancellation Semantics (`status: "cancelled"` vs `excluded: true`), and Closed Vocabulary Gating

- **Observed Behavior**:
  In RFC 5545 §3.8.1.11, `STATUS` on a recurring event's detached `VEVENT` component can take `CONFIRMED`, `CANCELLED`, or `TENTATIVE`. In RFC 8984 §4.4.4, `status` takes `"confirmed"`, `"cancelled"`, or `"tentative"`. In `jmap-ical`:
  1. Inbound parsing (`instance_patch`): compares `series.status` against `instance.status`. If differing, it emits `patch.insert("status", now.map_or(Value::Null, Value::String))`.
  2. Cancellation semantics: RFC 8984 §4.3.4 distinguishes `"excluded": true` (an occurrence that does not happen at all, serialized as RFC 5545 `EXDATE` on the master component without a detached `VEVENT`) from `"status": "cancelled"` (an occurrence that remains in the schedule but has been marked cancelled, represented by a detached `VEVENT` with `STATUS:CANCELLED` and `RECURRENCE-ID`). Stalwart v1.0.0 parses a detached `VEVENT` with `STATUS:CANCELLED` as `{"status": "cancelled"}` in `recurrenceOverrides[id]`, preserving the occurrence in the override set.
  3. Closed vocabulary gating and removal: `maps_override_field` admits `value.is_null()` (which unsets `status` on the instance so it inherits default/absent `STATUS`) or `value.as_str().is_some_and(known_status)`. Any non-standard status (such as `draft`, `needs-action`, `completed`, `in-process`, or invalid strings) is refused (`false`), preventing invalid iCalendar serialization or property pollution.
- **Specification and Architectural Context**:
  1. In Evolution Data Server (`ECalComponent` / `libical`) and iTIP (RFC 5546), a cancelled meeting instance (`STATUS:CANCELLED`) must be retained in the calendar store so attendees see the meeting strike-through and cancellation notice. If it were treated as `EXDATE` (`excluded: true`), the appointment would silently vanish from the user's view.
  2. RFC 5545 defines distinct status vocabularies for `VEVENT` vs `VTODO`. Admitting `VTODO` statuses like `COMPLETED` on a `VEVENT` override generates invalid iCalendar that libical rejects.
  3. Resetting status: a patch specifying `"status": null` removes the `STATUS` line from the detached component, allowing it to inherit default status semantics without conflicting with the series.
- **Adjudication**:
  Conforming specification boundary and calendar cancellation fidelity. Correctly decouples detached component cancellation (`status: "cancelled"`) from recurrence exclusion (`excluded: true`), restricts status values to the RFC 8984 / RFC 5545 closed vocabulary, and supports property removal via `null`.
- **Status**:
  Conforming specification boundary. Documented and pinned in `tests/event.rs`.

### 13.65 Divergence 65: Recurrence Override `freeBusyStatus` (`TRANSP`) Transparency Modeling, Default Opaque Fallback, and Vocabulary Clamping

- **Observed Behavior**:
  RFC 5545 §3.8.2.7 defines `TRANSP` (`OPAQUE` or `TRANSPARENT`) indicating whether an event blocks time on the calendar. RFC 8984 §4.4.2 defines `freeBusyStatus` (`"busy"` or `"free"`). Both specifications default to busy/opaque when the property is absent. In `jmap-ical`:
  1. Inbound parsing (`instance_patch`): compares `series.free_busy_status` with `instance.free_busy_status`. If differing, it emits `patch.insert("freeBusyStatus", now.map_or(Value::Null, Value::String))`. If a detached `VEVENT` has `TRANSP:TRANSPARENT` while the series has `TRANSP:OPAQUE` (or default), `instance_patch` emits `{"freeBusyStatus": "free"}`. If the detached component omits `TRANSP` while the series has `freeBusyStatus: "free"`, `instance_patch` emits `{"freeBusyStatus": null}`.
  2. Outbound serialization (`modified_instance` -> `vevent_of`): when an override specifies `"freeBusyStatus": "free"`, it emits `TRANSP:TRANSPARENT` on the detached `VEVENT`. When `"freeBusyStatus": "busy"`, it emits `TRANSP:OPAQUE`. When `"freeBusyStatus": null`, it omits `TRANSP`, falling back to the standard RFC 5545 default `OPAQUE`.
  3. Vocabulary validation: `maps_override_field` validates `"freeBusyStatus"` via `known_transparency`, allowing only `"free"`, `"busy"`, or `null`. Non-standard values (such as `"tentative"`, `"out-of-office"`, or arbitrary strings) are refused. Stalwart v1.0.0 parses `TRANSP` into `freeBusyStatus`, dropping unrecognized values.
- **Specification and Architectural Context**:
  1. In Evolution Data Server, free/busy scheduling is vital for meeting planning. If an occurrence of a recurring series is set to "Show Time as Free" (such as an optional workshop session), libical expects `TRANSP:TRANSPARENT` on that detached occurrence.
  2. Suppressing `TRANSP` on `null` allows the instance to fall back to the default `OPAQUE` state without serializing redundant lines.
  3. Refusing unknown transparency values prevents emitting invalid `TRANSP` tokens that break libical calendar queries or free/busy searches.
- **Adjudication**:
  Conforming specification boundary and free/busy scheduling fidelity. Maps `freeBusyStatus` to `TRANSP` bi-directionally, handles default omission and `null` resetting, and clamps values strictly to the two-value vocabulary.
- **Status**:
  Conforming specification boundary. Documented and pinned in `tests/event.rs`.

### 13.66 Divergence 66: Recurrence Override `priority` Integer Range Clamping (`0..=9`), Non-Integer Refusal, and Series Priority Removal via `null`

- **Observed Behavior**:
  RFC 5545 §3.8.1.9 defines `PRIORITY` as an integer from 0 to 9, where 0 is undefined, 1 is highest, and 9 is lowest. RFC 8984 §4.4.1 defines `priority: UnsignedInt` (0..9). Both specifications share the same integer scale. Stalwart v1.0.0 parses `PRIORITY` into an integer and omits priority when undefined. In `jmap-ical`:
  1. Inbound parsing (`instance_patch`): compares `series.priority` against `instance.priority`. If differing, it emits `patch.insert("priority", instance.priority.map_or(Value::Null, Value::from))`. An instance omitting `PRIORITY` when the series states one emits `"priority": null`.
  2. Numeric type validation: `maps_override_field` inspects `value.as_i64()`. If the value is a string (`"5"`), a floating-point number (`5.5`), or a negative number, `as_i64()` either fails or yields an invalid value, causing `maps_override_field` to refuse the patch.
  3. Range bounds: `known_priority` strictly checks `0..=9`. Integer values outside this range (such as `10` or higher) are refused.
  4. Outbound serialization: an instance override with a valid priority emits `PRIORITY:n` on the detached `VEVENT`. An override with `"priority": null` omits `PRIORITY` from the detached component, cleanly clearing the priority relative to the series.
- **Specification and Architectural Context**:
  1. In Evolution Data Server and libical, `PRIORITY` is stored as an integer. Supplying strings or floats causes libical parse failures or type assertion crashes.
  2. An occurrence of a recurring task or meeting may be flagged as urgent (`priority: 1`) while the series is ordinary (`priority: 5` or undefined). Preserving per-instance priority ensures high-priority occurrences are highlighted in EDS.
  3. Setting `"priority": null` cleanly resets an instance whose parent series had an explicit priority, ensuring the detached instance does not retain the parent's priority in EDS.
- **Adjudication**:
  Conforming specification boundary and integer range safety. Clamps priority values to `0..=9`, rejects non-integer representations, and supports property removal via `null`.
- **Status**:
  Conforming specification boundary. Documented and pinned in `tests/event.rs`.

### 13.67 Divergence 67: Recurrence Override `privacy` (`CLASS`) Classification Modeling, Closed Vocabulary Gating, and Confidentiality Isolation

- **Observed Behavior**:
  RFC 5545 §3.8.1.3 defines `CLASS` with values `PUBLIC`, `PRIVATE`, and `CONFIDENTIAL`. RFC 8984 §4.4.3 defines `privacy` with values `"public"`, `"private"`, and `"secret"`. Both models represent the identical three-tier access scale. Stalwart v1.0.0 parses `CLASS` into `privacy` and maps `CONFIDENTIAL` to `"secret"`. In `jmap-ical`:
  1. Inbound parsing (`instance_patch`): compares `series.privacy` against `instance.privacy`. If differing, it emits `patch.insert("privacy", now.map_or(Value::Null, Value::String))`. If a series is public and an instance is private, `instance_patch` emits `{"privacy": "private"}`. If a series is private and an instance has no `CLASS` line, `instance_patch` emits `{"privacy": null}` (reverting to default public).
  2. Outbound serialization (`modified_instance` -> `vevent_of`): maps `"public"` to `CLASS:PUBLIC`, `"private"` to `CLASS:PRIVATE`, and `"secret"` to `CLASS:CONFIDENTIAL` on the detached `VEVENT`. An override with `"privacy": null` omits `CLASS`, reverting to default public classification.
  3. Security and vocabulary gating: `maps_override_field` validates `privacy` via `known_privacy`, which only accepts `"public"`, `"private"`, `"secret"`, or `null`. Non-standard tokens (such as `RESTRICTED`, `X-PRIVATE`, or arbitrary strings) are refused.
- **Specification and Architectural Context**:
  1. In Evolution Data Server and CalDAV / JMAP deployments, classification controls whether meeting details (title, attendees, notes) are visible to delegates, assistants, or free/busy consumers.
  2. If a detached instance override in a recurring series contains sensitive medical or financial discussions, marking that instance `private` or `secret` must faithfully emit `CLASS:PRIVATE` or `CLASS:CONFIDENTIAL` on the detached `VEVENT`.
  3. Refusing unknown privacy strings at the validation boundary prevents false security assumptions: an unrecognized privacy tag will not be silently dropped to public visibility without warning.
- **Adjudication**:
  Conforming specification boundary and confidentiality access control fidelity. Maps the three-tier classification scale bi-directionally, rejects unmappable privacy strings, and ensures secure inheritance and override of privacy settings.
- **Status**:
  Conforming specification boundary. Documented and pinned in `tests/event.rs`.

### 13.68 Divergence 68: Recurrence Override `keywords` (`CATEGORIES`) Map Modeling, Tag Validation, Empty Set Refusal, and Series Keyword Removal via `null`

- **Observed Behavior**:
  RFC 5545 §3.8.1.2 defines `CATEGORIES` as a comma-separated list of categories or tags. RFC 8984 §4.4.5 models keywords as `keywords: Map<String, Boolean>`, where map keys are tag names and values MUST be `true`. Stalwart v1.0.0 parses `CATEGORIES` into `keywords`. In `jmap-ical`:
  1. Inbound parsing (`instance_patch`): compares `series.keywords` with `instance.keywords`. If differing, it emits `patch.insert("keywords", ...)`. When a detached `VEVENT` specifies `CATEGORIES`, `instance_patch` emits the map of keywords. When a detached `VEVENT` omits `CATEGORIES` while the master series has categories, `instance_patch` emits `{"keywords": null}`, removing all keywords from that occurrence.
  2. Outbound serialization (`modified_instance` -> `vevent_of`): when an override specifies a keyword map, `vevent_of` formats the tags as a sorted `CATEGORIES:tag1,tag2` line on the detached `VEVENT`. When an override specifies `"keywords": null`, `vevent_of` suppresses the `CATEGORIES` line on the detached component, cleanly clearing categories relative to the series.
  3. Validation predicate: `maps_override_field` validates `keywords` using `maps_keyword(tag, set)`. It admits `value.is_null()` (unsetting keywords) or a non-empty JSON object where each tag has `set == true`, is non-empty after trimming, and does not contain carriage returns (`\r`).
  4. Empty map refusal: an empty object `{"keywords": {}}` is explicitly refused by `!tags.is_empty()`. In RFC 5545, there is no distinct representation for an empty categories list versus no `CATEGORIES` property. Setting `{"keywords": {}}` would serialize without `CATEGORIES`, which upon inbound parsing would return `{"keywords": null}`, breaking round-trip idempotence. Clients must specify `null` to clear keywords.
- **Specification and Architectural Context**:
  1. In Evolution Data Server (`ECalComponent` / `libical`), categories are assigned to appointments to group them by project, department, or topic. Detached instances can belong to specialized subcategories (such as marking one meeting in a weekly standup series as "Sprint Review").
  2. Setting `"keywords": null` allows an occurrence to be explicitly filed under no categories, even when the parent recurring series carries categories.
  3. Refusing empty objects `{"keywords": {}}` enforces fixpoint stability between JMAP patches and iCalendar content lines, ensuring that unfiled occurrences always round-trip as `null`.
- **Adjudication**:
  Conforming specification boundary and tag collection fidelity. Maps `keywords` to `CATEGORIES` bi-directionally, validates tag tokens, enforces round-trip idempotence by refusing empty objects, and supports series category clearing via `null`.
- **Status**:
  Conforming specification boundary. Documented and pinned in `tests/event.rs`.

### 13.69 Divergence 69: Recurrence Override `alerts` (`VALARM`) Subcomponent Modeling, `useDefaultAlerts` Inheritance Gate, Empty Map Refusal, and Per-Instance Reminder Suppression via `null`

- **Observed Behavior**:
  RFC 5545 §3.6.6 defines `VALARM` subcomponents inside `VEVENT` components. RFC 8984 §4.5.2 models alarms as `alerts: Map<String, Alert>`. Stalwart v1.0.0 parses `VALARM` into `alerts`. In `jmap-ical`:
  1. Inbound parsing (`instance_patch`): compares `series.alerts` against `instance.alerts`. If differing, it emits `patch.insert("alerts", ...)`. When a detached `VEVENT` specifies custom `VALARM` components, `instance_patch` emits the alert map for the occurrence. When a detached `VEVENT` contains no `VALARM` components while the master series has alarms, `instance_patch` emits `{"alerts": null}`, suppressing reminders for that occurrence.
  2. Outbound serialization (`modified_instance` -> `vevent_of`): when an override specifies custom alerts, `vevent_of` renders the corresponding `BEGIN:VALARM ... END:VALARM` subcomponents on the detached `VEVENT`. When an override specifies `"alerts": null`, `vevent_of` omits all `VALARM` subcomponents from the detached component.
  3. Validation predicate: `maps_override_field` validates `alerts` using `drawn_alert(key, alert, None)`. It admits `value.is_null()` (suppressing alarms) or a non-empty object where each alert uses an `OffsetTrigger` (relative reminder) and `display` action. Absolute triggers and unmodeled non-display actions are refused.
  4. Empty map refusal: an empty object `{"alerts": {}}` is explicitly refused by `!alerts.is_empty()`. An empty alerts map serializes to no `VALARM` subcomponents, which upon parsing reads back as `null`. Refusing `{}` ensures patch idempotence.
  5. Default alerts gate: if the series specifies `useDefaultAlerts: true`, `maps_override_field` refuses all override alert patches (including `null` and valid alerts) via `!uses_default_alerts(series)`.
- **Specification and Architectural Context**:
  1. In Evolution Data Server (`ECalComponent`), alarms on recurring appointments can be individually adjusted or snoozed. If a user silences reminders for an optional occurrence in a recurring series, EDS saves a detached `VEVENT` without `VALARM` blocks. Emitting `{"alerts": null}` in the JMAP patch faithfully informs the server that this occurrence has no alarms.
  2. Suppressing alerts on occurrences when `useDefaultAlerts` is active preserves RFC 8984 §4.5.1 semantics: when default alerts are enabled, custom alert collections are ignored.
  3. Refusing empty alert maps `{}` prevents sync loops and patch diff flutter in `jmap-cal-sync`.
- **Adjudication**:
  Conforming specification boundary and reminder component fidelity. Maps per-instance `alerts` to `VALARM` subcomponents, gates overrides against `useDefaultAlerts`, enforces patch idempotence by refusing empty maps, and models reminder silencing via `null`.
- **Status**:
  Conforming specification boundary. Documented and pinned in `tests/event.rs`.

### 13.70 Divergence 70: Recurrence Override `useDefaultAlerts` Document-Level Scoping, Inheritance Across Occurrences, and Override Patch Prohibition

- **Observed Behavior**:
  RFC 8984 §4.5.1 defines `useDefaultAlerts: Boolean`, indicating whether the user's default alert preferences apply to the event. RFC 5545 has no property corresponding to `useDefaultAlerts`. In `jmap-ical`:
  1. Document-level scoping: `useDefaultAlerts` is an event-level configuration property, not a per-instance override property. It is deliberately omitted from `OVERRIDE_PROPERTIES` (the 11 vetted override properties: `title`, `description`, `start`, `timeZone`, `duration`, `status`, `freeBusyStatus`, `priority`, `privacy`, `keywords`, `alerts`).
  2. Override patch prohibition: `maps_override_field` returns `false` for `"useDefaultAlerts"`, refusing any attempt to set or toggle `useDefaultAlerts` in `recurrenceOverrides[id]`.
  3. Inheritance across occurrences: when `useDefaultAlerts` is set to `true` on the master series, it applies uniformly to all recurring instances. Outbound serialization omits `VALARM` subcomponents across all occurrences, and `maps_override_field` rejects per-instance custom alert definitions.
- **Specification and Architectural Context**:
  1. In RFC 5545, detached `VEVENT` components inherit or override `VALARM` subcomponents directly. There is no iCalendar syntax to represent "use default reminders for occurrence A, but not for occurrence B".
  2. In Evolution Data Server (`ECalComponent`), reminder defaults are managed at the calendar client or source level, not individually per recurring occurrence.
  3. Permitting per-instance `useDefaultAlerts` overrides would lead to unmappable iCalendar representations, because an occurrence with `useDefaultAlerts: true` cannot be distinguished in RFC 5545 from an occurrence that simply omits `VALARM` (which represents `alerts: null`). Restricting `useDefaultAlerts` to document scope prevents state ambiguity.
- **Adjudication**:
  Deliberate mapping design and specification boundary safety. Scopes `useDefaultAlerts` strictly to the top-level event document, prohibits per-instance override patches, and ensures consistent reminder handling across all series occurrences.
- **Status**:
  Deliberate mapping design. Documented and pinned in `tests/event.rs`.

### 13.71 Divergence 71: Recurrence Override `showWithoutTime` (All-Day Event) Document-Level Scoping, Date vs Date-Time Alignment, and Override Patch Prohibition

- **Observed Behavior**:
  RFC 8984 §4.2.1 defines `showWithoutTime: Boolean`, which designates an event as an all-day or floating date event. RFC 5545 §3.8.2.4 represents all-day events by setting `DTSTART;VALUE=DATE:...`. In `jmap-ical`:
  1. Document-level scoping: all-day status is decided once for the entire event document via `shows_without_time(&event)`. It is excluded from `OVERRIDE_PROPERTIES`.
  2. Override patch prohibition: `maps_override_field` returns `false` for `"showWithoutTime"`, refusing any attempt to set `"showWithoutTime"` inside an override patch.
  3. Date vs Date-Time alignment: RFC 5545 §3.8.4.4 mandates that the value type of `RECURRENCE-ID` MUST match the value type of `DTSTART`. If the master event is an all-day event (`VALUE=DATE`), all detached occurrences must have `RECURRENCE-ID;VALUE=DATE:...` and `DTSTART;VALUE=DATE:...`. If the master event is a timed event (`VALUE=DATE-TIME`), all detached occurrences must have `VALUE=DATE-TIME`.
- **Specification and Architectural Context**:
  1. In Evolution Data Server and libical, recurring appointments cannot mix all-day occurrences and timed occurrences within the same series. Attempting to mix `VALUE=DATE` and `VALUE=DATE-TIME` causes libical to fail recurrence rule expansion or produce corrupted occurrence intervals.
  2. If a user needs to convert a single occurrence of a recurring timed meeting into an all-day event (or vice versa), EDS and CalDAV workflows split the instance into an independent appointment rather than creating a mixed-mode detached component.
  3. Enforcing document-wide scoping for `showWithoutTime` preserves RFC 5545 type consistency and guarantees libical recurrence expansion stability.
- **Adjudication**:
  Conforming specification boundary and libical component safety. Restricts all-day event status to document-level scoping, prohibits mixed-mode per-instance overrides, and maintains value type alignment between master and detached components.
- **Status**:
  Conforming specification boundary. Documented and pinned in `tests/event.rs`.

### 13.72 Divergence 72: Participant `sendTo` `imip` Address URI Scheme Validation, Address Sanitization, and Non-IMIP Delivery Method Omission

- **Observed Behavior**:
  RFC 8984 §4.4.6 defines `sendTo: Map<String, String>` where keys are delivery methods (such as `"imip"`, `"sms"`, `"other"`), and values are URI strings. RFC 5545 §3.3.3 requires `CAL-ADDRESS` to be a URI (typically `mailto:user@example.com`). Stalwart v1.0.0 parses `ATTENDEE` and `ORGANIZER` lines into `sendTo: {"imip": "<uri>"}`. In `jmap-ical`:
  1. Address extraction (`calendar_address`): strictly inspects `participant.sendTo.imip`. Participants lacking an `imip` delivery method (such as those specifying only `sms` or `web`), or with an empty `sendTo` map, are dropped from outbound serialization.
  2. URI scheme validation (`names_a_uri`): requires RFC 3986 syntax consisting of an alphabetic scheme, colon, and non-empty destination. Bare email addresses lacking a scheme (such as `"alice@example.com"` instead of `"mailto:alice@example.com"`) or empty scheme payloads (`"mailto:"`) fail validation and are dropped.
  3. Whitespace and CRLF sanitization: `names_a_uri` strictly rejects any whitespace, carriage returns (`\r`), or line feeds (`\n`). This prevents malicious or malformed calendar addresses from breaking line boundaries or injecting unauthorized iCalendar properties.
  4. Inbound drop: upon inbound parse (`ical_to_event`), `participants` is set to `None` for scheduling safety, ensuring client saves never mutate server-managed invitee records.
- **Specification and Architectural Context**:
  1. RFC 5545 §3.3.3 explicitly mandates that a `CAL-ADDRESS` value type MUST be a valid URI. Attempting to emit a non-URI value violates iCalendar grammar and causes libical parsing failures.
  2. In Evolution Data Server (`ECalComponent`), attendees without valid `mailto:` addresses cannot be routed by email transport agents. Dropping participants that lack an `imip` address prevents unroutable entries from entering desktop calendar views.
  3. Sanitizing against CRLF characters is a critical security boundary: injecting unescaped line breaks through address strings could allow arbitrary property spoofing in exported calendar streams.
- **Adjudication**:
  Conforming specification boundary and protocol transport safety. Restricts participant calendar addresses strictly to valid `imip` URIs, sanitizes against newline injection, and drops participants lacking a valid calendar address.
- **Status**:
  Conforming specification boundary. Documented and pinned in `tests/event.rs`.

### 13.73 Divergence 73: Participant `owner` Role Isolation, Multiple Owner Selection (First-Wins Ordering), and Dual `ORGANIZER` / `ATTENDEE` Line Emission

- **Observed Behavior**:
  RFC 8984 §4.4.6 has no separate `organizer` property; instead, the organizer is a participant with the `"owner": true` entry in their `roles` set. In RFC 5545, the meeting caller is represented by the dedicated `ORGANIZER` property line (§3.8.4.3), while invitees are represented by `ATTENDEE` lines (§3.8.4.1). Stalwart v1.0.0 parses `ORGANIZER` into a participant with `roles: {"owner": true}`. In `jmap-ical`:
  1. Multiple owner resolution: RFC 8984 allows multiple participants to have `"owner": true`, but RFC 5545 §3.6.1 permits at most one `ORGANIZER` line per `VEVENT`. `drawn_participants` emits `ORGANIZER` only for the first owner encountered in `participants` map iteration order, suppressing subsequent `ORGANIZER` lines.
  2. Owner-only participant: A participant whose only role is `owner` (with no attendee or guest roles) emits `ORGANIZER` alone and no `ATTENDEE` line. This represents an event organized on behalf of others where the organizer does not attend.
  3. Dual-line emission for attending owners: When a participant has both `owner` and an attendee role (such as `"attendee": true`, `"chair": true`, or `"optional": true`), `drawn_participants` emits both an `ORGANIZER` line and an `ATTENDEE` line.
- **Specification and Architectural Context**:
  1. In Evolution Data Server and RFC 5546 (iTIP), an organizer who attends the meeting must be present on the attendee list so that their participation status (`PARTSTAT=ACCEPTED`) is recorded and meeting room seating accounts for them.
  2. Emitting multiple `ORGANIZER` lines is invalid iCalendar syntax that libical and external calendar servers reject. Enforcing a deterministic first-wins rule preserves single-organizer compliance while rendering additional owners as attendees if they hold attendance roles.
- **Adjudication**:
  Conforming specification boundary and organizer role fidelity. Emits `ORGANIZER` for the first owner in map order, decouples event ownership from guest list presence, and renders dual lines for attending owners.
- **Status**:
  Conforming specification boundary. Documented and pinned in `tests/event.rs`.

### 13.74 Divergence 74: Participant Attendee Role Precedence (`chair` > `informational` > `optional` > `attendee`) and Single-Value `ROLE` Parameter Clamping

- **Observed Behavior**:
  RFC 8984 §4.4.6 models participant roles as a set `roles: Map<String, Boolean>`, admitting multiple simultaneous roles (such as `{"chair": true, "attendee": true, "optional": true}`). RFC 5545 §3.2.16 specifies `ROLE` as a single-valued parameter taking `CHAIR`, `REQ-PARTICIPANT`, `OPT-PARTICIPANT`, or `NON-PARTICIPANT`, defaulting to `REQ-PARTICIPANT` when omitted. Stalwart v1.0.0 parses `ROLE` into the corresponding single role entry in `roles`. In `jmap-ical`:
  1. Deterministic precedence ordering: Because iCalendar admits only one `ROLE` value on an `ATTENDEE` line, `PARTICIPANT_ROLES` establishes a strict precedence hierarchy: `chair` (`CHAIR`) > `informational` (`NON-PARTICIPANT`) > `optional` (`OPT-PARTICIPANT`) > `attendee` (`REQ-PARTICIPANT`).
  2. Narrower role preference: When a participant holds both `"attendee"` and `"optional"`, `"optional"` is chosen because `REQ-PARTICIPANT` is the RFC 5545 default and optional attendance represents a narrower, more informative constraint. `"chair"` outranks all roles.
  3. Non-standard role dropping: Roles outside the standard table (such as vendor-specific or experimental roles) are omitted, falling back to the standard iCalendar default without emitting invalid parameter syntax.
- **Specification and Architectural Context**:
  1. In Evolution Data Server (`ECalComponent`), attendees with `ROLE=CHAIR` or `ROLE=OPT-PARTICIPANT` are rendered with distinct UI badges and icons. Passing a single authoritative role token aligns with libical data structures.
  2. Disambiguating multi-valued role sets via deterministic precedence ensures stable round-trip rendering between JMAP and iCalendar representations.
- **Adjudication**:
  Conforming specification boundary and role precedence disambiguation. Collapses multi-valued role sets to a single `ROLE` parameter via deterministic precedence (`chair` > `informational` > `optional` > `attendee`) and drops unknown roles.
- **Status**:
  Conforming specification boundary. Documented and pinned in `tests/event.rs`.

### 13.75 Divergence 75: Participant `kind` (`CUTYPE`) Vocabulary Translation (`location` -> `ROOM`), `participationStatus` (`PARTSTAT`), and `expectReply` (`RSVP=TRUE`) Parameter Gating

- **Observed Behavior**:
  RFC 8984 §4.4.6 defines `kind: String` (`"individual"`, `"group"`, `"resource"`, `"location"`), `participationStatus: String` (`"needs-action"`, `"accepted"`, `"declined"`, `"tentative"`, `"delegated"`), and `expectReply: Boolean`. RFC 5545 defines parameters `CUTYPE` (§3.2.3), `PARTSTAT` (§3.2.12), and `RSVP` (§3.2.17). Stalwart v1.0.0 parses these parameters into the corresponding JSCalendar participant fields. In `jmap-ical`:
  1. `kind` -> `CUTYPE` vocabulary translation: `"location"` is translated to `CUTYPE=ROOM` (the RFC 5545 calendar user type for physical conference rooms). `"individual"`, `"group"`, and `"resource"` map directly to their uppercase equivalents `INDIVIDUAL`, `GROUP`, and `RESOURCE`. Unknown kinds are dropped, leaving `CUTYPE` omitted (defaulting to `INDIVIDUAL`).
  2. `participationStatus` -> `PARTSTAT` vocabulary: `"needs-action"`, `"accepted"`, `"declined"`, `"tentative"`, and `"delegated"` map to uppercase `NEEDS-ACTION`, `ACCEPTED`, `DECLINED`, `TENTATIVE`, and `DELEGATED`. Non-standard status tokens are dropped.
  3. `expectReply` -> `RSVP` parameter gating: RFC 5545 defaults `RSVP` to `FALSE`. In `jmap-ical`, `expects_reply` requires `expectReply == Some(true)`. When `true`, it emits `RSVP=TRUE`; when `false`, `null`, omitted, or non-boolean, the `RSVP` parameter is omitted.
- **Specification and Architectural Context**:
  1. In Evolution Data Server, conference rooms booked as attendees carry `CUTYPE=ROOM`. Translating JSCalendar `"location"` to `CUTYPE=ROOM` enables EDS to correctly identify room mailboxes and schedule equipment.
  2. Filtering non-standard status values and omitting default `RSVP=FALSE` keeps iCalendar streams compact and prevents invalid parameter errors across strict CalDAV servers.
- **Adjudication**:
  Conforming specification boundary and calendar user parameter fidelity. Maps `kind` with `location` -> `ROOM` vocabulary translation, maps `participationStatus` to `PARTSTAT`, and emits `RSVP=TRUE` only when reply is explicitly expected.
- **Status**:
  Conforming specification boundary. Documented and pinned in `tests/event.rs`.

### 13.76 Divergence 76: Participant `name` Common Name (`CN`) Parameter Mapping, Empty String Suppression, and Whitespace Quoting

- **Observed Behavior**:
  RFC 8984 §4.4.6 defines `name: String` as the display name of a participant. RFC 5545 §3.2.2 defines the `CN` (Common Name) parameter on `ORGANIZER` and `ATTENDEE` property lines. Stalwart v1.0.0 parses `CN` into `participant.name`. In `jmap-ical`:
  1. Name mapping: When `participant.name` is present and non-empty, `drawn_participants` emits `CN=...` on the emitted `ORGANIZER` or `ATTENDEE` line via `stated_name`.
  2. Empty name suppression: When `name` is an empty string (`""`), `stated_name` returns `None`, omitting the `CN` parameter entirely rather than writing invalid empty parameter syntax like `CN=`.
  3. Whitespace parameter quoting: Names containing spaces (such as `"Alice Organizer"`) are wrapped in double quotes according to RFC 5545 §3.2 quoting rules (`CN="Alice Organizer"`), while single-token names without whitespace or punctuation (such as `Bob`) are emitted unquoted (`CN=Bob`).
  4. Inbound drop: Upon inbound parse (`ical_to_event`), `participants` is set to `None` for scheduling safety.
- **Specification and Architectural Context**:
  1. RFC 5545 §3.2 states that property parameter values must not be empty. An empty parameter value `CN=` is a syntax violation that libical and strict parsers reject.
  2. In Evolution Data Server (`ECalComponent`), display names containing spaces must be properly quoted in the underlying libical component to prevent tokenization errors when rendering calendar invitations.
- **Adjudication**:
  Conforming specification boundary and display name fidelity. Maps `name` to `CN` parameter, quotes values containing whitespace, and suppresses empty name strings.
- **Status**:
  Conforming specification boundary. Documented and pinned in `tests/event.rs`.

### 13.77 Divergence 77: Participant Delegation Parameter Omission (`delegatedTo` / `delegatedFrom` vs `DELEGATED-TO` / `DELEGATED-FROM`) and Scheduling Boundary

- **Observed Behavior**:
  RFC 8984 §4.4.6 defines `delegatedTo: Map<String, Boolean>` and `delegatedFrom: Map<String, Boolean>`, where map keys are URIs of participants to or from whom participation was delegated. RFC 5545 §3.2.4 (`DELEGATED-TO`) and §3.2.5 (`DELEGATED-FROM`) define parameters holding calendar addresses of delegates. Stalwart v1.0.0 parses these parameters into JSCalendar delegation maps. In `jmap-ical`:
  1. Outbound omission: `drawn_participants` deliberately omits `DELEGATED-TO` and `DELEGATED-FROM` parameters from outbound `ATTENDEE` lines.
  2. Inbound drop: `ical_to_event` drops `participants` (`None`) on import.
- **Specification and Architectural Context**:
  1. RFC 5546 (iTIP) and RFC 6638 govern the complex protocol flow of invitation delegation, which requires re-issuing invitations, updating organizer tracking records, and coordinating message dispatch.
  2. In Evolution Data Server, delegation is managed through its email transport backend and interactive iTIP workflows, not through static calendar serialization. Emitting raw delegation parameters without an active iTIP scheduling engine risks desynchronizing server-managed attendee tracking or triggering conflicting calendar notifications.
- **Adjudication**:
  Deliberate mapping design and scheduling boundary safety. Omits delegation parameters from outbound attendee lines, leaving delegation handling to server-authoritative scheduling systems.
- **Status**:
  Conforming specification boundary. Documented and pinned in `tests/event.rs`.

### 13.78 Divergence 78: Participant Group Membership Parameter Omission (`memberOf` vs `MEMBER`) and Directory Expansion Decoupling

- **Observed Behavior**:
  RFC 8984 §4.4.6 defines `memberOf: Map<String, Boolean>`, where map keys are URIs of group participants (such as team distribution lists) of which the participant is a member. RFC 5545 §3.2.11 defines the `MEMBER` parameter on `ATTENDEE` lines. Stalwart v1.0.0 parses `MEMBER` into `participant.memberOf`. In `jmap-ical`:
  1. Outbound omission: `drawn_participants` omits `MEMBER` parameters on `ATTENDEE` lines.
  2. Inbound drop: `ical_to_event` drops `participants` (`None`) on import.
- **Specification and Architectural Context**:
  1. In corporate directory and groupware environments, group expansion is performed server-side when an invitation is addressed to a group mailbox or mailing list.
  2. Serializing `MEMBER` parameters back to external iCalendar files can leak internal distribution list URIs or conflict with server-side directory expansion logic in CalDAV/JMAP servers.
- **Adjudication**:
  Deliberate mapping design and directory privacy safety. Omits group membership `memberOf` parameters from outbound iCalendar lines, leaving group resolution to server-side directory services.
- **Status**:
  Conforming specification boundary. Documented and pinned in `tests/event.rs`.

### 13.79 Divergence 79: Participant CalDAV Scheduling Parameters (`scheduleAgent`, `scheduleStatus`, `scheduleForceSend`) Omission

- **Observed Behavior**:
  RFC 8984 §4.4.6 and RFC 6638 define CalDAV scheduling parameters: `scheduleAgent: String` (`"server"`, `"client"`, `"none"`), `scheduleStatus: String` (e.g. `"1.1;Delivered"`), `scheduleForceSend: String` (`"request"`, `"reply"`), along with sequence and timestamp counters. RFC 6638 §3.2.1 to §3.2.3 define corresponding iCalendar parameters `SCHEDULE-AGENT`, `SCHEDULE-STATUS`, and `SCHEDULE-FORCE-SEND`. Stalwart v1.0.0 parses these parameters into JSCalendar participant records. In `jmap-ical`:
  1. Outbound omission: `drawn_participants` omits `SCHEDULE-AGENT`, `SCHEDULE-STATUS`, and `SCHEDULE-FORCE-SEND` from outbound `ATTENDEE` lines.
  2. Inbound drop: `ical_to_event` drops `participants` (`None`) on import.
- **Specification and Architectural Context**:
  1. RFC 6638 scheduling parameters are intended strictly for communication between scheduling clients and CalDAV/iTIP scheduling engines.
  2. In JMAP Calendar deployments, invitation delivery and RSVP management are handled via JMAP scheduling methods (`CalendarEventSend`). Emitting CalDAV-specific scheduling parameters on raw exported iCalendar components could confuse non-CalDAV clients or trigger unwanted automated processing by external iCalendar consumers.
- **Adjudication**:
  Deliberate mapping design and CalDAV scheduling isolation. Suppresses protocol-specific scheduling parameters on outbound `ATTENDEE` lines, preventing scheduling desynchronization.
- **Status**:
  Conforming specification boundary. Documented and pinned in `tests/event.rs`.

### 13.80 Divergence 80: Location Single-Entry Restriction (`maps_locations`), Multiple Entry Refusal, First-Named Entry Drawing, and Empty Name Suppression

- **Observed Behavior**:
  RFC 8984 §4.2.5 models physical locations as `locations: Map<String, Location>`, admitting multiple simultaneous venue records for an event. In contrast, RFC 5545 §3.6.1 restricts a `VEVENT` component to at most one `LOCATION` content line. Stalwart v1.0.0 parses multiple location definitions or ingests structured `VLOCATION` subcomponents. In `jmap-ical`:
  1. Outbound drawing: `drawn_place` selects the first entry in map iteration order that has a non-empty name (`place_name`), ignoring entries without names or with empty strings.
  2. Multi-entry refusal: `maps_locations` returns `false` if `locations` contains more than one entry (`entries.next().is_none()`). This prevents desktop calendar saves from silently dropping secondary location records.
  3. Empty name suppression: An entry with `name: ""` or `name: null` produces no `LOCATION` line (`place_name` returns `None`), keeping exported iCalendar streams clean.
  4. Inbound parse: `read_locations` drops empty `LOCATION:` lines (`name.is_empty() -> None`), avoiding synthesizing empty-string location objects.
- **Specification and Architectural Context**:
  1. RFC 5545 §3.6.1 explicitly limits `LOCATION` to at most a single occurrence per `VEVENT`. Emitting multiple `LOCATION` lines violates iCalendar grammar and causes libical parse rejections.
  2. In Evolution Data Server (`ECalComponent`), an appointment holds a single location text string. If a JMAP event defines multiple locations, EDS cannot display or edit the secondary locations in its standard user interface.
  3. Refusing multiple locations at the `maps_locations` boundary prevents synchronization data loss: a client save will not overwrite a multi-venue server event and discard the unshown venues. Drawing the first named location ensures that users still see where the meeting takes place.
- **Adjudication**:
  Conforming specification boundary and single-location component safety. Restricts outbound representation to a single `LOCATION` line, draws the first non-empty named location, suppresses empty names, and flags multi-location maps to protect secondary locations.
- **Status**:
  Conforming specification boundary. Documented and pinned in `tests/event.rs`.

### 13.81 Divergence 81: Location In-Place Key Tracking (`X-JMAP-KEY`), Invented Key Allocation (`"l1"`), and Patch-in-Place Synchronization Boundary

- **Observed Behavior**:
  RFC 8984 §4.2.5 keys entries in `locations` by an RFC 8984 §1.4.4 `Id` (1 to 255 octets of ASCII alphanumeric, `-`, or `_`). The RFC 5545 `LOCATION` property has no standard parameter for map keys. Stalwart v1.0.0 parses `LOCATION` and generates keys via UUID5 or internal hashing. In `jmap-ical`:
  1. Outbound key retention: `drawn_place` attaches the parameter `X-JMAP-KEY: <key>` to the emitted `LOCATION` line, retaining the server's map key across round trips.
  2. Inbound key recovery: `read_locations` inspects `X-JMAP-KEY`. If the parameter value is a valid RFC 8984 `Id` (`names_map_entry`), it preserves the server's key. If missing, it allocates the stable invented key `INVENTED_KEY` (`"l1"`).
  3. Invalid key defense: If `X-JMAP-KEY` contains invalid characters (such as spaces, colons, or control characters) or exceeds 255 octets, it is rejected per `names_map_entry` and falls back safely to `"l1"`.
  4. Patch-in-place boundary: Because a `Location` object can contain unmapped properties like `description`, `coordinates`, `timeZone`, and `locationTypes`, `jmap-cal-sync` patches `locations/<key>/name` in place rather than replacing the entire map.
- **Specification and Architectural Context**:
  1. In Evolution Data Server and `libical`, editing the location text only changes the string value. If the calendar backend replaced the entire `locations` property on save, all auxiliary metadata stored by the server (such as map coordinates or conference room descriptions) would be lost.
  2. Using `X-JMAP-KEY` allows `jmap-cal-sync` to target the exact entry on the server. Allocating a stable fallback key (`"l1"`) for external iCalendar imports ensures that new appointments create valid JMAP location entries without collision.
  3. Validating keys against `names_map_entry` protects against malformed parameters from external clients that could cause the server to reject a `CalendarEvent/set` request with `invalidProperties`.
- **Adjudication**:
  Deliberate mapping design and in-place sync boundary fidelity. Retains server map keys via `X-JMAP-KEY`, falls back safely to invented key `"l1"`, and enables in-place patching of location names without disturbing auxiliary server metadata.
- **Status**:
  Deliberate mapping design. Documented and pinned in `tests/event.rs`.

### 13.82 Divergence 82: VirtualLocation (`CONFERENCE`) Multiple Line Emission, Mandatory `VALUE=URI` Parameter, Feature Vocabulary Gating (`CONFERENCE_FEATURES`), and Label Mapping

- **Observed Behavior**:
  RFC 8984 §4.2.6 defines `virtualLocations: Map<String, VirtualLocation>` supporting `uri: String`, `name: String`, and `features: Map<String, Boolean>`. RFC 7986 §5.11 defines the `CONFERENCE` property and allows it multiple times within a `VEVENT`. RFC 7986 §5.11 explicitly mandates `VALUE=URI` in its grammar. Stalwart v1.0.0 parses `CONFERENCE` into `virtualLocations`. In `jmap-ical`:
  1. Multi-line drawing: Unlike `LOCATION`, every valid entry in `virtual_locations` is emitted as a distinct `CONFERENCE` line in map order (`drawn_conferences`).
  2. Mandatory parameter: Emits `VALUE=URI` explicitly on every `CONFERENCE` line as required by RFC 7986 §5.11.
  3. Feature vocabulary gating: Maps `features` to RFC 7986 `FEATURE` parameters using the closed table `CONFERENCE_FEATURES` (`audio` -> `AUDIO`, `chat` -> `CHAT`, `feed` -> `FEED`, `moderator` -> `MODERATOR`, `phone` -> `PHONE`, `screen` -> `SCREEN`, `video` -> `VIDEO`). `maps_virtual_locations` returns `false` if `features` contains unknown features or non-boolean values.
  4. Label and key mapping: Maps `name` to the `LABEL` parameter and tracks entry keys with `X-JMAP-KEY`. On inbound parse, `read_virtual_locations` extracts `LABEL` into `name` and allocates collision-free positional keys (`v1`, `v2`, ...) when `X-JMAP-KEY` is absent.
- **Specification and Architectural Context**:
  1. RFC 7986 §5.11 requires `VALUE=URI` in the `confparam` grammar rule. Omission of this parameter causes strict RFC 7986 parsers to reject the property.
  2. Virtual conferencing often includes both video links and telephone dial-ins. Supporting multiple `CONFERENCE` lines ensures all join options remain accessible in Evolution.
  3. Gating features against the closed RFC 7986 vocabulary prevents invalid parameter values from corrupting serialized streams, while `maps_virtual_locations` ensures edits are refused if unsupported features would be dropped.
- **Adjudication**:
  Conforming specification boundary and virtual conferencing fidelity. Emits multiple `CONFERENCE` lines with required `VALUE=URI`, maps labels and feature sets bidirectionally, and gates against unmappable conference features.
- **Status**:
  Conforming specification boundary. Documented and pinned in `tests/event.rs`.

### 13.83 Divergence 83: Linked Resource (`links`) URI-Only Model, Local `file://` URI Suppression, and Binary Attachment Omission vs Server-Managed Blob References

- **Observed Behavior**:
  RFC 8984 §4.2.7 defines `links: Map<String, Link>` for external documents and attachments. RFC 5545 §3.8.1.1 defines `ATTACH`, which allows URI references or inline binary attachments (`VALUE=BINARY;ENCODING=BASE64:...`). RFC 7986 §5.10 defines `IMAGE` for event icons and display pictures. Stalwart v1.0.0 parses links and attachments. In `jmap-ical`:
  1. Local URI suppression: `read_links` checks `fetched_locally(&href)` and drops all `file:` URIs on inbound parse.
  2. Binary attachment omission: Inline binary attachments (`VALUE=BINARY`) are dropped on inbound parse (`!names_a_uri(&href)`).
  3. Dual property mapping: Maps entries with `rel: "icon"` to `IMAGE;VALUE=URI` (including `DISPLAY` parameter per RFC 7986 §6.1), while other links map to `ATTACH`.
  4. Parameter validation: Media types in `FMTTYPE` are validated against RFC 6838 restricted-name rules, and `SIZE` parameters are validated as unsigned integers per RFC 8607 §4.1.
- **Specification and Architectural Context**:
  1. When a user attaches a local file in Evolution, EDS creates an `ATTACH:file:///home/...` line. If saved to JMAP, local file paths would leak private usernames and directory structures to external attendees and shared calendars, while remaining inaccessible to any other device. Suppressing local file URIs is an essential privacy and security boundary.
  2. In JMAP, binary files must be managed via the RFC 9404 Blob API rather than embedding large binary blobs directly in calendar JSON objects or iCalendar text streams. Dropping inline binary attachments prevents calendar database bloat and sync timeouts.
  3. Enforcing RFC 6838 restricted names on `FMTTYPE` prevents header injection or parameter syntax errors across libical and CalDAV gateways.
- **Adjudication**:
  Conforming specification boundary and user privacy safety. Drops local `file://` URIs and inline binary data on import, enforces strict URI and media-type formatting, and decouples calendar event structures from direct binary payload storage.
- **Status**:
  Conforming specification boundary. Documented and pinned in `tests/event.rs`.

### 13.84 Divergence 84: Free/Busy Availability (`VFREEBUSY`) Attendee Address Normalization, Bare Email Tolerance, and Double-Prefix Suppression

- **Observed Behavior**:
  RFC 5545 §3.8.4.1 and §3.8.4.3 require an `ATTENDEE` line in a `VFREEBUSY` component to specify a `CAL-ADDRESS` (RFC 5545 §3.3.3), which must be a URI (typically `mailto:user@example.com`). In Evolution Data Server (`ECalBackendSync::get_free_busy_sync`), the `users` argument passes attendees as bare email addresses (e.g. `bob@example.com`). In `jmap-ical`:
  1. Case-insensitive scheme tolerance: `mailto(attendee)` inspects the start of the attendee string. If it begins with `mailto:` (checked via `eq_ignore_ascii_case`), it strips the prefix to extract the bare address.
  2. Canonical prefix emission: It always prefixes the resulting address with lowercase `mailto:`.
  3. Double-prefix prevention: Callers passing `mailto:bob@example.com` or `MAILTO:bob@example.com` do not produce duplicate schemes like `mailto:mailto:bob@example.com`.
  4. Injection sanitization: Newlines (`\r\n`) within the attendee string are escaped by the entry formatter, preventing arbitrary property injection from unvetted input strings.
  In contrast, Stalwart v1.0.0 CalDAV free/busy handlers expect and emit canonical `mailto:` URIs, failing requests with malformed schemes.
- **Specification and Architectural Context**:
  1. Evolution Data Server has three independent calendar backends (the built-in local backend, CalDAV backend, and Microsoft 365 / EWS backend). All three backends format the `ATTENDEE` property for `VFREEBUSY` components by taking the bare address from EDS and prepending `mailto:`.
  2. Tolerating both bare addresses and `mailto:`-prefixed addresses avoids brittle caller requirements while guaranteeing that the emitted iCalendar stream is strictly valid RFC 5545 syntax.
  3. Case-insensitive comparison ensures compatibility with legacy or non-conforming clients that emit uppercase `MAILTO:`.
- **Adjudication**:
  Conforming specification boundary and caller tolerance design. Normalizes attendee addresses by safely stripping existing `mailto:` prefixes, re-prepending `mailto:`, preventing double-prefixing, and sanitizing against property injection.
- **Status**:
  Conforming specification boundary. Documented and pinned in `tests/event.rs`.

### 13.85 Divergence 85: Free/Busy Availability Whole-Component Refusal (`Option<String>`) vs Best-Effort Filtering and False Availability Prevention

- **Observed Behavior**:
  In general iCalendar and JSCalendar event mapping (`ical_to_event` / `event_to_ical`), unparseable properties or unrecognized values are dropped or skipped to preserve the remainder of the calendar event (a lost property is preferable to dropping an entire meeting). In contrast, `jmap-ical`'s `busy_periods_to_vfreebusy` returns `Option<String>`:
  1. Window validation: If either `utc_start` or `utc_end` of the requested search window cannot be parsed as a valid UTC instant, the function returns `None`.
  2. Period validation: If any `BusyPeriod` in the `periods` slice contains an unparseable `utc_start` or `utc_end` (such as invalid format, non-UTC timestamp, or nonexistent calendar dates like month 13 or hour 25), the function immediately returns `None`.
  3. Refusal vs best-effort: It does not drop the malformed period and emit the remaining valid periods; it refuses the entire component.
  In contrast, general-purpose iCalendar processors often perform best-effort filtering, dropping malformed entries and returning whatever remains.
- **Specification and Architectural Context**:
  1. In meeting scheduling (RFC 5546 iTIP and Evolution meeting scheduling dialogs), a scheduler queries availability to discover open time slots to book an appointment.
  2. If an unparseable busy period were silently dropped, the attendee would be presented to the scheduler as free during that time interval. The meeting organizer would then book a conflicting meeting into a slot where the attendee is actually busy.
  3. Refusing the entire component leaves the attendee's free/busy row blank in Evolution ("we do not know"), which truthfully reflects that availability could not be verified, preventing catastrophic double-booking.
- **Adjudication**:
  Deliberate mapping design and scheduling safety guarantee. Enforces all-or-nothing whole-component refusal upon unparseable windows or busy periods to prevent false availability reporting and schedule collisions.
- **Status**:
  Deliberate mapping design. Documented and pinned in `tests/event.rs`.

### 13.86 Divergence 86: Free/Busy Availability Status Vocabulary Mapping (`BusyPeriod.busyStatus` -> `FBTYPE`) and Fail-Safe `BUSY` Fallback

- **Observed Behavior**:
  In draft-ietf-jmap-calendars §2.2, `Principal/getAvailability` returns a list of `BusyPeriod` objects, where `busyStatus` takes `"confirmed"`, `"tentative"`, or `"unavailable"`. There is no `"free"` status in `getAvailability`. RFC 5545 §3.2.9 defines the `FBTYPE` parameter on `FREEBUSY` properties, supporting `BUSY`, `BUSY-UNAVAILABLE`, `BUSY-TENTATIVE`, and `FREE`. In `jmap-ical`:
  1. `"tentative"` maps to `FBTYPE=BUSY-TENTATIVE`.
  2. `"unavailable"` maps to `FBTYPE=BUSY-UNAVAILABLE`.
  3. `"confirmed"` maps to `FBTYPE=BUSY`.
  4. Any unrecognized status token (such as future draft extensions or unknown vendor values) or empty string maps safely to `FBTYPE=BUSY`.
  Stalwart v1.0.0 parses and translates these statuses between JMAP and CalDAV protocols.
- **Specification and Architectural Context**:
  1. Because `Principal/getAvailability` exclusively reports busy intervals, every record returned by the server represents attendee unavailability.
  2. If a future revision of the JMAP Calendars specification or an extended server implementation introduces a new status token (such as `"working-elsewhere"` or `"focus-time"`), treating an unknown status as anything other than busy could cause scheduling engines to assume the attendee is free.
  3. Defaulting all unknown statuses to `BUSY` provides fail-safe backward and forward compatibility, ensuring that attendee time remains protected against inadvertent overbooking.
- **Adjudication**:
  Conforming specification boundary and forward-compatible scheduling safety. Maps standard draft statuses to RFC 5545 `FBTYPE` tokens and clamps all unknown or empty statuses to `BUSY`.
- **Status**:
  Conforming specification boundary. Documented and pinned in `tests/event.rs`.

### 13.87 Divergence 87: Free/Busy Availability UTC Date-Time Fractional Seconds Truncation and RFC 3339 / RFC 5545 Harmonization

- **Observed Behavior**:
  RFC 3339 §5.6 and JMAP `UTCDate` admit fractional seconds (e.g. `2026-08-19T09:00:00.512Z`). RFC 5545 §3.3.5 explicitly specifies the `DATE-TIME` format as `YYYYMMDDTHHMMSSZ` and forbids fractional seconds. Stalwart v1.0.0 and CalDAV servers emit integer-second `DATE-TIME` values. In `jmap-ical`:
  1. Sub-second detection: `instant(&UtcDate)` checks for a decimal point (`split_once('.')`).
  2. Digit validation: It verifies that all characters between the decimal point and the terminating `'Z'` are ASCII digits. If non-digit characters appear, validation fails, returning `None`.
  3. Sub-second truncation: It strips the fractional digits and converts the integer second timestamp into RFC 5545 UTC format `YYYYMMDDTHHMMSSZ`.
  4. Format compliance: Emitted `FREEBUSY`, `DTSTART`, and `DTEND` properties strictly conform to RFC 5545 date-time syntax without fractional digits.
- **Specification and Architectural Context**:
  1. JMAP servers backed by SQL databases or high-resolution clocks often produce timestamps with millisecond or microsecond fractions.
  2. If `jmap-ical` strictly rejected fractional seconds, harmless sub-second precision from servers would trigger whole-component refusal under Divergence 85, breaking free/busy lookup.
  3. If `jmap-ical` emitted fractional seconds on `FREEBUSY` lines, RFC 5545 parsers and `libical` in Evolution would fail to parse the `VFREEBUSY` component.
  4. Validating and truncating fractional seconds harmonizes the RFC 3339 timestamp format with RFC 5545 while preserving availability data.
- **Adjudication**:
  Conforming specification boundary and protocol interoperability tolerance. Truncates valid fractional seconds from `UTCDate` timestamps while enforcing strict RFC 5545 integer-second syntax in emitted `VFREEBUSY` components.
- **Status**:
  Conforming specification boundary. Documented and pinned in `tests/event.rs`.






