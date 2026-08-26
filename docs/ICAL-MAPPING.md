<!--
SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
SPDX-License-Identifier: GPL-3.0-or-later
-->

# iCalendar ↔ JSCalendar ↔ EDS Calendar Mapping Reference

This document is the authoritative reference specification for calendar data translation across:
1. **iCalendar 2.0** (RFC 5545, RFC 7986) as parsed and emitted via `calcard`.
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

---

## 2. VTIMEZONE and TZID Resolution Architecture

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

### 2.1 Accepted Time Zone Identifier Forms

`jmap-ical` accepts and classifies time zone identifiers into four distinct tiers:

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

---

## 3. Recurrence & UNTIL Instant Calculation with Timezones

RFC 5545 §3.3.10 states recurrence rule `UNTIL` as a UTC instant (`YYYYMMDDTHHMMSSZ`) whenever `DTSTART` specifies a timezone. Conversely, RFC 8984 §4.3.3 / jscalendarbis states `until` as a local date-time string (`YYYY-MM-DDTHH:MM:SS`) in the event's own timezone.

`jmap-ical`'s `zone.rs` module evaluates transition offsets (`TZOFFSETTO` / `TZOFFSETFROM`) directly from the document's `VTIMEZONE` observances:
- When a Windows TZID (e.g. `W. Europe Standard Time`) is present, `read_vevent` resolves the timezone to its canonical IANA name while looking up observance rules from the matching `VTIMEZONE` in the document.
- Transitions across standard and daylight savings time (e.g. +0100 to +0200) are applied accurately at the instant of the `UNTIL` timestamp.

---

## 4. Multi-Stage Fixed-Point Stability

Every calendar transformation in `jmap-ical` adheres to strict fixed-point stability:
1. **Pass 1 (Import & Normalization)**: Raw iCalendar input with non-standard TZIDs (e.g. `DTSTART;TZID=W. Europe Standard Time:...` or `DTSTART;TZID=/mozilla.org/...`) is normalized to canonical JSCalendar (`timeZone: "Europe/Berlin"`).
2. **Pass 2 (Canonical Outbound)**: Serializing back emits canonical RFC 5545 (`DTSTART;TZID=Europe/Berlin:...`).
3. **Pass 3 (Fixed Point Convergence)**: Re-importing and re-exporting produces byte-identical iCalendar streams:
   $$\text{Export}_2 \equiv \text{Export}_3 \quad \text{and} \quad \text{Event}_2 \equiv \text{Event}_3$$

---

## 5. Alerts & VALARM Mapping Architecture (RFC 8984 §4.5 ↔ RFC 5545 §3.6.6 / RFC 9074)

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

### 5.1 Trigger Formats, Signs & Normalization

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

### 5.2 Action Types Decision Matrix

| iCalendar Action | JSCalendar Action | Inbound Handling | Outbound `maps_alerts` | Design Rationale |
| :--- | :--- | :--- | :--- | :--- |
| `ACTION:DISPLAY` | `"display"` (or absent) | Accepted → `Alert` | `true` (Emits `VALARM`) | Full bidirectional fidelity with Evolution and CalDAV clients. |
| `ACTION:AUDIO` | `"audio"` | Dropped (`None`) | `false` (Refused) | RFC 8984 lacks dedicated audio alarm actions; prevents lossy edits. |
| `ACTION:EMAIL` | `"email"` | Dropped (`None`) | `false` (Refused) | RFC 5545 requires `ATTENDEE` and `SUMMARY` which JSCalendar Alert does not model. |
| `ACTION:PROCEDURE` | `"procedure"` | Dropped (`None`) | `false` (Refused) | Unsupported program execution alarm type. |

### 5.3 UID Allocation & Key Collision Avoidance

- **RFC 9074 `UID`**: Named `VALARM` components with valid IDs preserve their server-assigned key (`UID:k1` → `"k1"`).
- **Nameless & Evolution Alarms**: Exporters omitting RFC 9074 `UID` (such as Evolution's internal `X-EVOLUTION-ALARM-UID` or Apple Calendar) are assigned deterministic positional keys (`"a1"`, `"a2"`, …).
- **Collision Avoidance**: Invented positional keys automatically skip keys already claimed by explicit `UID` lines, guaranteeing 100% uniqueness without entry collapsing.
- **Duplicate UIDs**: If an incoming stream contains duplicate UIDs, RFC 9074 §6 uniqueness rules apply and duplicate entries collapse into a single map entry.

### 5.4 Safety and Whole-Property Replacement

- **Event Title in `DESCRIPTION`**: RFC 5545 §3.6.6 mandates `DESCRIPTION` on `ACTION:DISPLAY`. `event_to_ical` populates `DESCRIPTION` with `event.title` (omitted if title is empty or `None`).
- **Custom `description` on `Alert`**: If a server-side `Alert` contains a custom `description` field, `maps_alerts` returns `false` because `VALARM` description is derived from event title, preventing silent deletion of custom alert descriptions.
- **`acknowledged` Timestamps (RFC 9074 §6.1)**: Dismissed/snoozed alert timestamps in JSCalendar are refused by `maps_alerts` (`maps_alerts(&event) == false`) to prevent whole-property replacement from un-dismissing snoozed alarms.
- **`useDefaultAlerts`**: When `useDefaultAlerts: true` (RFC 8984 §4.5.1), `event_to_ical` emits 0 `VALARM`s and `maps_alerts` returns `false`. Recurrence overrides also inherit `useDefaultAlerts` from the master series.

### 5.5 Real-Exporter Alarm Corpus Fidelity & Refused Shapes Isolation

The real-world exporter corpus (`google_calendar_export.ics`, `outlook_m365_export.ics`, `apple_calendar_export.ics`, `evolution_calendar_export.ics`, `nextcloud_calendar_export.ics`) characterizes how alarms emitted by major platforms behave on the bidirectional round-trip:

1. **Google Calendar (`google_calendar_export.ics`)**:
   - **Shapes Emitted**: Multiple display alarms at standard offsets (`-P1D`, `-PT15M`), email notification alarms (`ACTION:EMAIL` with `ATTENDEE` and `SUMMARY`), and absolute trigger alarms (`TRIGGER;VALUE=DATE-TIME`).
   - **Mapping Fidelity**: Display alarms map to JSCalendar `Alert` records (`a1`, `a2`). Email and absolute alarms are safely ignored on import and dropped on export without polluting `event.extra` or corrupting appointment properties.

2. **Microsoft Outlook / M365 (`outlook_m365_export.ics`)**:
   - **Shapes Emitted**: Display alarms with `ACTION:DISPLAY`, `DESCRIPTION:REMINDER` (all caps), trigger offsets (`-PT15M`, `-PT30M`), RFC 9074 `UID` and `X-WR-ALARMUID` vendor properties, and enterprise email alarms.
   - **Mapping Fidelity**: Explicit UIDs and positional keys (`a1`) are faithfully preserved. Generic `DESCRIPTION:REMINDER` is replaced on outbound serialization with the event's summary according to RFC 5545 §3.6.6. `X-WR-ALARMUID` and `ACTION:EMAIL` are dropped cleanly on export. Long UIDs (e.g. 94 octets) fold and unfold cleanly at the RFC 5545 75-octet boundary.

3. **Apple Calendar / macOS (`apple_calendar_export.ics`)**:
   - **Shapes Emitted**: Multi-alarm sequences (`-P1D`, `-PT2H`, `-PT15M`), Apple `ACKNOWLEDGED` snoozed timestamps (RFC 9074 §6.1), `X-WR-ALARMUID` paired with `UID`, `ACTION:AUDIO` with sound attachments (`ATTACH;VALUE=URI:Basso`), and absolute date-time triggers.
   - **Mapping Fidelity**: Display alarms with explicit UUID keys are preserved. `ACKNOWLEDGED` timestamps and `X-WR-ALARMUID` properties are ignored on parse to avoid setting `event.extra`. Refused audio and absolute triggers are dropped on export without data loss.

4. **GNOME Evolution (`evolution_calendar_export.ics`)**:
   - **Shapes Emitted**: Native `X-EVOLUTION-ALARM-UID` parameters and explicit `VALUE=DURATION` trigger parameters.
   - **Mapping Fidelity**: Positional keys (`a1`, `a2`) map cleanly to JSCalendar map IDs and roundtrip with fixed-point stability.

5. **Nextcloud / SabreDAV (`nextcloud_calendar_export.ics`)**:
   - **Shapes Emitted**: Multi-day display offsets (`-P2D`).
   - **Mapping Fidelity**: Preserved and roundtripped losslessly.

---

## 6. Recurrence Overrides & RECURRENCE-ID Mapping Architecture (RFC 8984 §4.3.4 ↔ RFC 5545 §3.8.4.4 / §3.8.5)

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

### 6.1 Three Representation Categories

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

### 6.2 Precedence & Conflict Resolution Matrix

When an iCalendar stream contains multiple statements about the same occurrence instant:
1. **Detached `VEVENT` vs `RDATE`**: The detached `VEVENT` takes precedence. It provides specific property overrides while the `RDATE` only states that the occurrence happens.
2. **Detached `VEVENT` vs `EXDATE`**: The detached `VEVENT` takes precedence. The specific modification resurrects/redefines the occurrence.
3. **`EXDATE` vs `RDATE`**: `EXDATE` takes precedence (`excluded: true`), following RFC 5545 §3.8.5.1 to prevent generating unwanted appointments.
4. **`RANGE=THISANDFUTURE` (RFC 5545 §3.2.13)**: Detached components with `RANGE=THISANDFUTURE` are safely skipped by `read_overrides` to prevent silently corrupting subsequent occurrences in the series.
5. **Out-of-Order Components**: Documents where detached `VEVENT` occurrences precede the master series in physical line order are correctly associated by `UID`.

### 6.3 Restatable Properties Decision Matrix (`OVERRIDE_PROPERTIES`)

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

### 6.4 Clocks and Time Zone Separation

When an occurrence moves to a different time zone:
- **`RECURRENCE-ID`**: Evaluated on the **master series clock** (`series_zone`), identifying the original generated occurrence instant (RFC 5545 §3.8.4.4).
- **`DTSTART`**: Evaluated on the **instance's own clock** (`instance.time_zone`), placing the rescheduled occurrence at its actual local start time.
- **Windows & Globally-Unique TZIDs**: TZIDs on `RECURRENCE-ID` and instance `DTSTART` resolve through the canonical resolution pipeline (Section 2), tolerating real-world exporter formats across providers.


