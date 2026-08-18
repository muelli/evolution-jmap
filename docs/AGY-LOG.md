<!--
SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
SPDX-License-Identifier: GPL-3.0-or-later
-->

# Antigravity Log

Running record of headless polish increments on the `antigravity` branch.

## 2026-08-18 — calcard migration step 1: jmap-vcard parsing through calcard

- **AGY-TASKS sub-step:** 1. jmap-vcard: route parsing through calcard; adapt `contact.rs`; tests green.
- **Changes:**
  - Adapted `jmap-vcard/src/contact.rs` to consume `calcard`'s parsed types (`VCardEntry`, `VCardValue`, `VCardParameterValue`) directly in `vcard_to_card` and its reading helper functions (`read_name`, `read_photo`, `read_title`, `spouse_named`, `read_anniversary`, `read_address`, `read_organization`, `label_entry`, `entry_key`, `read_flags`, `read_keywords`), bypassing the intermediate hand-rolled `Property` wrapper for parsing.
  - Added TDD test `reads_a_vcard_with_mixed_case_property_names_and_parameters` in `tests/mapping.rs` verifying mixed-case property/parameter parsing and multi-line category collection.
- **Calcard behaviour-difference findings:** None. All 139 mapping unit tests, syntax tests, server roundtrip tests, and hostile input tests pass with identical semantics.
- **Gates ran:** `./ci/checks.sh` clean (REUSE 3.3 compliant, `cargo fmt`, `cargo clippy --all-targets --locked -- -D warnings`, `cargo test --locked`, `cargo deny check`).

## 2026-08-18 — calcard migration step 2: jmap-vcard emitting through calcard & syntax cleanup

- **AGY-TASKS sub-step:** 2. jmap-vcard: route emitting through calcard; delete the dead syntax code; tests green.
- **Changes:**
  - Adapted `card_to_vcard` in `rust/crates/jmap-vcard/src/contact.rs` to construct `calcard::vcard::VCardEntry` objects and emit them using `entry.write_to` wrapped in standard vCard 3.0 envelopes.
  - Deleted the dead hand-rolled lexer/emitter `rust/crates/jmap-vcard/src/syntax.rs` (~410 lines) and internal tests `rust/crates/jmap-vcard/tests/syntax.rs`; removed `pub mod syntax;` from `rust/crates/jmap-vcard/src/lib.rs`.
  - Updated `tests/hostile.rs` to test calcard parameter quoting and escaped-newline roundtrips without depending on `syntax.rs`.
  - Added comprehensive TDD round-trip test `emits_a_comprehensive_vcard_via_calcard_and_roundtrips` in `rust/crates/jmap-vcard/tests/mapping.rs`.
- **Calcard behaviour-difference findings:**
  1. `calcard` formats URI properties (such as `URL`) without backslash-escaping URI query punctuation (`;` and `,`), following RFC 2426 §3.6.8 and RFC 3986.
  2. `calcard` escapes quotes in parameter values as `\"`, faithfully round-tripping keys with embedded quotes while preventing parameter injection.
  3. `calcard` escapes carriage returns as `\r` and newlines as `\n`, preserving CRLF values losslessly rather than stripping `\r`.
- **Gates ran:** `./ci/checks.sh` clean (REUSE 3.3 compliant, `cargo fmt`, `cargo clippy --all-targets --locked -- -D warnings`, `cargo test --locked`, `cargo deny check`).

## 2026-08-18 — calcard migration step 3: jmap-ical parsing through calcard

- **AGY-TASKS sub-step:** 3. jmap-ical: parsing through calcard (handle `zone.rs` zones); tests green.
- **Changes:**
  - Added `parse_ical` and ICalendar entry / component accessors (`component_entry`, `component_entries`, `component_text`, `entry_text`, `entry_texts`, `entry_raw_value`, `entry_param`, `entry_param_values`, `value_text_str`) in `rust/crates/jmap-ical/src/syntax.rs`.
  - Adapted `ical_to_event` and all reading helpers in `rust/crates/jmap-ical/src/event.rs` (`read_vevent`, `stated_zones`, `read_time_zones`, `read_definition`, `read_observance`, `read_locations`, `read_virtual_locations`, `read_links`, `read_keywords`, `read_start`, `read_overrides`, `read_duration`, `read_privacy`, `read_priority`, `read_transparency`, `read_alerts`, `read_alert`) to consume `calcard::icalendar::ICalendarComponent` and `ICalendarEntry` directly.
  - Adapted `rust/crates/jmap-ical/src/zone.rs` (`offset_at`, `onsets`) to evaluate timezone observances directly from `ICalendarComponent` slices without intermediate conversion.
  - Updated `Zoned` in `rust/crates/jmap-ical/src/event.rs` to hold observances slice for calculating UTC offsets via `zone::offset_at`.
  - Added TDD test `reads_an_icalendar_with_mixed_case_properties_and_parameters_and_parses_faithfully` in `rust/crates/jmap-ical/tests/event.rs`.
- **Calcard behaviour-difference findings:** None. All 288 event tests, 8 hostile input tests, and 14 syntax tests pass with identical semantics.
## 2026-08-18 — calcard migration step 4: jmap-ical emitting through calcard & syntax cleanup (calcard migration complete)

- **AGY-TASKS sub-step:** 4. jmap-ical: emitting through calcard; delete the dead syntax code; tests green.
- **Changes:**
  - Adapted `event_to_ical` and drawing helpers (`drawn_alert`, `drawn_alarms`, `drawn_participants`, `drawn_conferences`, `drawn_conference`, `drawn_links`, `drawn_link`, `vtimezone_of`, `observance`, `vevent_of`, `dated`) in `rust/crates/jmap-ical/src/event.rs` to construct `calcard::icalendar::ICalendarEntry` and `Component` emitter wrappers delegating line writing, TEXT escaping, parameter quoting, and RFC 5545 line folding (75 octets) directly to `entry.write_to`.
  - Routed `RRULE` emission through `calcard::Parser` into `ICalendarValue::RecurrenceRule` to preserve unescaped recurrence rule formatting.
  - Deleted the dead hand-rolled lexer/emitter `rust/crates/jmap-ical/src/syntax.rs` (~628 lines) and internal tests `rust/crates/jmap-ical/tests/syntax.rs`; removed `pub mod syntax;` from `rust/crates/jmap-ical/src/lib.rs`.
  - Moved ICalendar reading accessors and `MAX_DEPTH` validation directly into `event.rs` and updated `zone.rs` and `error.rs` to import from `crate::event`.
  - Updated `tests/hostile.rs` nesting depth and escaped newline roundtrip tests to test through `parse_ical` / `ical_to_event` and `MAX_DEPTH`.
  - Added comprehensive TDD round-trip test `emits_a_comprehensive_icalendar_via_calcard_and_roundtrips` in `rust/crates/jmap-ical/tests/event.rs`.
  - Appended `CALCARD COMPLETE 2026-08-18` to `docs/MILESTONES.md` (both hand-rolled `syntax.rs` layers in `jmap-vcard` and `jmap-ical` are gone and all tests pass).
- **Calcard behaviour-difference findings:**
  1. `calcard` automatically quotes parameter values that contain whitespace (such as `CN="Alice Example"` and `LABEL="Team room"`) in accordance with RFC 5545 `quoted-string` grammar.
  2. `calcard` automatically handles RFC 5545 75-octet line folding without splitting multi-byte UTF-8 code points.
  3. `calcard` writes `RecurrenceRule` values using standard unescaped semicolon delimiters when parsed into `ICalendarValue::RecurrenceRule`.
- **Gates ran:** `./ci/checks.sh` clean (REUSE 3.3 compliant, `cargo fmt`, `cargo clippy --all-targets --locked -- -D warnings`, `cargo test --locked`, `cargo deny check`).


