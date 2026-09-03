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
| **`RRULE`** | Recurrence parameters | `event.recurrence_rules` (`RecurrenceRule`) | `RRULE` / Recurrence | `read_rrule`, `drawn_rrule`, [`maps_recurrence_rule`], [`unstateable_until`] | Full RFC 5545 recurrence grammar: `FREQ`, `INTERVAL`, `COUNT`, `UNTIL` (local in series timezone), `BYSECOND`, `BYMINUTE`, `BYHOUR`, `BYDAY` / `NDay`, `BYMONTHDAY`, `BYYEARDAY`, `BYWEEKNO`, `BYMONTH`, `BYSETPOS`, `WKST`. |
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
| **`DTSTAMP`** | — | — | `DTSTAMP` / Timestamp | — | Envelope generation timestamp. Strictly managed by server/exporter. |
| **`CREATED`** | — | `event.created` (RFC 8984 §4.1.3) | `CREATED` / Created | `read_vevent` | Creation timestamp in UTC. Read on import; server owns authoritative creation timestamp. |
| **`LAST-MODIFIED`** | — | `event.updated` (RFC 8984 §4.1.4) | `LAST-MODIFIED` / Updated | `read_vevent` | Modification timestamp in UTC. Read on import; server owns authoritative update timestamp. |
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

### 3.9 Recurrence Rules (`RRULE` ↔ `recurrenceRules`)
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

---

## 5. Recurrence & UNTIL Instant Calculation with Timezones

RFC 5545 §3.3.10 states recurrence rule `UNTIL` as a UTC instant (`YYYYMMDDTHHMMSSZ`) whenever `DTSTART` specifies a timezone. Conversely, RFC 8984 §4.3.3 / jscalendarbis states `until` as a local date-time string (`YYYY-MM-DDTHH:MM:SS`) in the event's own timezone.

`jmap-ical`'s `zone.rs` module evaluates transition offsets (`TZOFFSETTO` / `TZOFFSETFROM`) directly from the document's `VTIMEZONE` observances:
- When a Windows TZID (e.g. `W. Europe Standard Time`) is present, `read_vevent` resolves the timezone to its canonical IANA name while looking up observance rules from the matching `VTIMEZONE` in the document.
- Transitions across standard and daylight savings time (e.g. +0100 to +0200) are applied accurately at the instant of the `UNTIL` timestamp.

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

The real-world exporter corpus (`google_calendar_export.ics`, `outlook_m365_export.ics`, `apple_calendar_export.ics`, `thunderbird_calendar_export.ics`, `sogo_calendar_export.ics`, `evolution_calendar_export.ics`, `nextcloud_calendar_export.ics`) characterizes how alarms emitted by major platforms behave on the bidirectional round-trip:

1. **Google Calendar (`google_calendar_export.ics`)**:
   - **Shapes Emitted**: Multiple display alarms at standard offsets (`-P1D`, `-PT15M`), email notification alarms (`ACTION:EMAIL` with `ATTENDEE` and `SUMMARY`), and absolute trigger alarms (`TRIGGER;VALUE=DATE-TIME`).
   - **Mapping Fidelity**: Display alarms map to JSCalendar `Alert` records (`a1`, `a2`). Email and absolute alarms are safely ignored on import and dropped on export without polluting `event.extra` or corrupting appointment properties.

2. **Microsoft Outlook / M365 (`outlook_m365_export.ics`)**:
   - **Shapes Emitted**: Display alarms with `ACTION:DISPLAY`, `DESCRIPTION:REMINDER` (all caps), trigger offsets (`-PT15M`, `-PT30M`), RFC 9074 `UID` and `X-WR-ALARMUID` vendor properties, and enterprise email alarms.
   - **Mapping Fidelity**: Explicit UIDs and positional keys (`a1`) are faithfully preserved. Generic `DESCRIPTION:REMINDER` is replaced on outbound serialization with the event's summary according to RFC 5545 §3.6.6. `X-WR-ALARMUID` and `ACTION:EMAIL` are dropped cleanly on export. Long UIDs (e.g. 94 octets) fold and unfold cleanly at the RFC 5545 75-octet boundary.

3. **Apple Calendar / macOS (`apple_calendar_export.ics`)**:
   - **Shapes Emitted**: Multi-alarm sequences (`-P1D`, `-PT2H`, `-PT15M`), Apple `ACKNOWLEDGED` snoozed timestamps (RFC 9074 §6.1), `X-WR-ALARMUID` paired with `UID`, `ACTION:AUDIO` with sound attachments (`ATTACH;VALUE=URI:Basso`), and absolute date-time triggers.
   - **Mapping Fidelity**: Display alarms with explicit UUID keys are preserved. `ACKNOWLEDGED` timestamps and `X-WR-ALARMUID` properties are ignored on parse to avoid setting `event.extra`. Refused audio and absolute triggers are dropped on export without data loss.

4. **Mozilla Thunderbird (`thunderbird_calendar_export.ics`)**:
   - **Shapes Emitted**: Display alarms with `ACTION:DISPLAY`, bi-weekly recurrence, timezone-aware `EXDATE`s, conference URIs, and PDF attachments.
   - **Mapping Fidelity**: Display alarms map cleanly to JSCalendar `Alert` records and roundtrip with fixed-point stability.

5. **SOGo Connector / Radicale (`sogo_calendar_export.ics`)**:
   - **Shapes Emitted**: Multiple display alarms at `-PT15M` and `-P1D`, French location strings, conference chat endpoints, and monthly ordinal recurrence.
   - **Mapping Fidelity**: Preserved and roundtripped losslessly.

6. **GNOME Evolution (`evolution_calendar_export.ics`)**:
   - **Shapes Emitted**: Native `X-EVOLUTION-ALARM-UID` parameters and explicit `VALUE=DURATION` trigger parameters.
   - **Mapping Fidelity**: Positional keys (`a1`, `a2`) map cleanly to JSCalendar map IDs and roundtrip with fixed-point stability.

7. **Nextcloud / SabreDAV (`nextcloud_calendar_export.ics`)**:
   - **Shapes Emitted**: Multi-day display offsets (`-P2D`).
   - **Mapping Fidelity**: Preserved and roundtripped losslessly.

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

