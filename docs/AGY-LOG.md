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

## 2026-08-18 — secondary polish: empty ORG name emission & Windows timezone refusal tests

- **AGY-TASKS sub-step:** SECONDARY — Empty-`ORG`-name emission (`jmap-vcard`) and Windows time-zone names refusal path (`jmap-ical`).
- **Changes:**
  - Added `an_organization_with_an_empty_name_string_behaves_consistently` in `rust/crates/jmap-vcard/tests/mapping.rs`, testing that an organisation with `name: Some("")` and no units emits no `ORG` line (states nothing) and round-trips to `None`, while an organisation with `name: Some("")` and units emits `ORG;X-JMAP-KEY=...:;Unit` preserving the empty leading component and round-trips to `name: None` with units intact.
  - Added `windows_time_zone_names_are_refused_as_unsendable_by_design` in `rust/crates/jmap-ical/tests/event.rs`, testing that Windows time zone names (e.g., `W. Europe Standard Time`, `Pacific Standard Time`, `Eastern Standard Time`, `GMT Standard Time`, `Tokyo Standard Time`, `Central European Standard Time`) are not recognized as IANA names by `names_time_zone`, cannot be defined via `defines_time_zone` due to lacking a leading solidus, and are refused by `maps_time_zone` as unsendable-by-design.
- **Calcard behaviour-difference findings:** None.
- **Gates ran:** `./ci/checks.sh` clean (REUSE 3.3 compliant, `cargo fmt`, `cargo clippy --all-targets --locked -- -D warnings`, `cargo test --locked`, `cargo deny check`).

## 2026-08-18 — IM online-service URI schemes mapping (`jmap-vcard`)

- **AGY-TASKS sub-step:** 2. IM / social online-service URI schemes: map the remaining schemes onto `onlineServices` (AIM, ICQ, MSN, Yahoo, GroupWise, Matrix).
- **Changes:**
  - Extended `SERVICE_SCHEMES` in `rust/crates/jmap-vcard/src/contact.rs` to support URI scheme aliases for all mapped instant-messaging services: `aim` (AIM), `icq` (ICQ), `msn` and `msnim` (MSN), `yahoo` and `ymsgr` (Yahoo), `groupwise` (GroupWise), and `matrix` (Matrix).
  - Updated `handle_in_uri` in `rust/crates/jmap-vcard/src/contact.rs` to validate incoming URI schemes against all registered aliases for the target service and extract bare handles.
  - Added TDD tests `bare_im_service_uris_are_drawn_and_roundtripped` and `online_service_uri_constructs_canonical_uri_for_all_supported_services`, and updated `action_query_im_uris_get_no_vcard_line` in `rust/crates/jmap-vcard/tests/mapping.rs`.
- **Calcard behaviour-difference findings:**
  1. `X-TWITTER` and `X-SIP` remain unmapped/unslotted in vCard 3.0 generation by design: EDS models them as multi-valued list attributes without `_HOME_1..3`/`_WORK_1..3` slots, and `jmap-book-sync` relies on them being preserved on the server without emitting lossy vCard lines.
  2. URI formats with actions/queries (e.g., `aim:goim?screenname=...`, `msnim:chat?contact=...`, `ymsgr:sendim?...`, `icq:message?uin=...`, `matrix:u/vera:...`) are safely rejected by `plain_handle` to prevent corrupting plain handle fields.
- **Gates ran:** `./ci/checks.sh` clean (REUSE 3.3 compliant, `cargo fmt`, `cargo clippy --all-targets --locked -- -D warnings`, `cargo test --locked`, `cargo deny check`).

## 2026-08-18 — Multi-TYPE phone numbers fidelity and characterization (`jmap-vcard`)

- **AGY-TASKS sub-step:** 1. Multi-`TYPE` phone numbers (`TEL;TYPE=WORK,VOICE,FAX` and friends): characterize and pin EDS slotting, feature prioritization, context selection, and predicates.
- **Changes:**
  - Added comprehensive characterization and round-trip tests in `rust/crates/jmap-vcard/tests/mapping.rs`:
    - `multi_type_phone_numbers_characterization_and_roundtrip`: verifies inbound parsing of 14 permutations of multi-token `TYPE` attributes (e.g. `WORK,VOICE,FAX`, `HOME,VOICE,FAX`, bare `VOICE,FAX`, `WORK,CELL,VOICE`, `HOME,PAGER,VOICE`, `WORK,VOICE,VIDEO`, `HOME,CELL,PAGER,FAX,VOICE,VIDEO`, `PREF,WORK,VOICE,FAX`, separate `TYPE` parameters, mixed-case parameter names/values, unmapped types like `ISDN`/`CAR`, and plain untyped phone numbers), outbound vCard 3.0 generation under EDS slot constraints, and accurate evaluation of `states_phone_feature` and `states_context` predicates.
    - `maps_phone_feature_predicate_characterization`: characterizes `maps_phone_feature` coverage for all supported JSContact keys (`mobile`, `pager`, `fax`, `voice`, `video`) and confirms rejection of invalid or unmapped tokens.
    - `phone_feature_slot_resolution_order_is_fully_determined`: pins the complete precedence ranking (`mobile` > `pager` > `fax` > `voice` > `video`) and pairwise narrowing rules against EDS editor collision requirements.
- **Calcard behaviour-difference findings:**
  1. `calcard` correctly groups multiple `VCardParameterName::Type` parameters into comma-delimited `TYPE=...` lists upon serialization while parsing both comma-delimited tokens (`TYPE=WORK,VOICE`) and repeated parameters (`TYPE=WORK;TYPE=VOICE`) into individual type entries.
  2. `calcard` safely ignores unrecognized `TYPE` parameter tokens without failing the parse or corrupting companion properties.
- **Gates ran:** `./ci/checks.sh` clean (REUSE 3.3 compliant, `cargo fmt`, `cargo clippy --all-targets --locked -- -D warnings`, `cargo test --locked`, `cargo deny check`).

## 2026-08-18 — Multi-component ORG / TITLE round-trip fidelity (`jmap-vcard`)

- **AGY-TASKS sub-step:** 3. Multi-component `ORG` / `TITLE` round-trip: ensure a vCard `ORG` with 3+ components (incl. a 4th mapping to `E_CONTACT_OFFICE`) round-trips vCard↔JSContact without dropping components.
- **Changes:**
  - Added `unfolded` helper and comprehensive round-trip tests in `rust/crates/jmap-vcard/tests/mapping.rs`:
    - `multi_component_org_with_three_or_more_units_and_office_roundtrips_faithfully`: verifies 4-component `ORG` values (`Acme Ltd;Research;Optics;Lenses`) mapping components across `E_CONTACT_ORG` (name), `E_CONTACT_ORG_UNIT` (department), `E_CONTACT_OFFICE` (office), and unmapped 4th units (`Lenses`), as well as inbound unkeyed vCard parsing into `o1`.
    - `multi_component_org_with_deep_hierarchy_and_trailing_or_intermediate_units_roundtrip`: tests 6-component deep hierarchy (`Global Tech;Engineering;Infrastructure;Storage Systems;Flash Division;Team Beta`), nameless organisations with multiple units (preserving structured leading semicolon), and intermediate empty components (e.g. EDS clearing `E_CONTACT_OFFICE` in place) round-tripping cleanly without component shifting.
    - `multi_component_org_and_multiple_titles_roles_coexist_and_roundtrip`: tests multi-component `ORG` coexisting with multiple `TITLE` and `ROLE` entries and unmapped vendor kinds (`x-honour`) in the same card, validating `states_title` predicates and canonical kind normalization.
    - `multi_component_org_with_escaped_punctuation_roundtrips`: validates structured value delimiter escaping (`\,`, `\;`) across multi-component units and roundtrips without corruption.
- **Calcard behaviour-difference findings:**
  1. `calcard` automatically folds long lines (such as deep multi-component `ORG` values and escaped punctuation) at 75 octets with CRLF-space folding, while unfolding them losslessly on parse.
  2. `calcard` parses structured values containing escaped delimiters (`\;`, `\,`) into discrete component tokens with unescaped text content and re-escapes them on emission according to RFC 2426 §2.4.2.
- **Gates ran:** `./ci/checks.sh` clean (REUSE 3.3 compliant, `cargo fmt`, `cargo clippy --all-targets --locked -- -D warnings`, `cargo test --locked`, `cargo deny check`).

## 2026-08-18 — Bare-year dates fidelity & EDS clamping protection (`jmap-vcard`)

- **AGY-TASKS sub-step:** 4. Bare-year dates (`BDAY`/anniversary stated as a year only): characterize and test how a year-only date maps (EDS clamps); pin it with a round-trip test.
- **Changes:**
  - Added comprehensive characterization and round-trip tests in `rust/crates/jmap-vcard/tests/mapping.rs`:
    - `bare_year_dates_characterization_and_eds_clamping_roundtrip`: verifies that `PartialDate` anniversaries stating only a year (`birth`, `wedding`, `death`, and unmapped kinds) emit no `BDAY` or `X-EVOLUTION-ANNIVERSARY` vCard lines, ensuring `diff_entries` leaves the server-side date untouched rather than exposing it to EDS's `e_contact_date_to_string` clamping (`1000..=9999` for year, `1..=12` for month, `1..=31` for day) which would corrupt a bare year into a January 1 date; confirms `states_anniversary`, `anniversary_date`, and `states_a_point_in_time` predicates evaluate to false/None; validates that truncated inbound vCard date lines (`BDAY:1984`, `BDAY:1984-06`, `BDAY:--06-21`, `X-EVOLUTION-ANNIVERSARY:1996`, explicit `VALUE=date`/`VALUE=text`) are safely dropped by `read_anniversary` rather than parsed into invalid dates; and tests roundtrip preservation of cards with coexisting bare-year and full-date anniversaries.
    - `bare_year_and_partial_dates_with_custom_attributes_roundtrip`: validates that `PartialDate` instances specifying custom calendar scales (`buddhist`, `hebrew`) with bare years are safely handled and round-trip without emitting malformed properties or corrupting other fields.
- **Calcard behaviour-difference findings:**
  1. `calcard` correctly parses `VCardProperty::Bday` entries into discrete components and passes raw value strings intact to mapping helpers without implicit date conversion or field guessing.
  2. Inbound vCards with incomplete or non-standard date values (such as bare years or truncated ISO-8601 strings) are safely handed to `read_anniversary`, which safely rejects them without panicking or producing partial records.
- **Gates ran:** `./ci/checks.sh` clean (REUSE 3.3 compliant, `cargo fmt`, `cargo clippy --all-targets --locked -- -D warnings`, `cargo test --locked`, `cargo deny check`).

## 2026-08-18 — Org unit empty-name edge case & unstated predicate fidelity (`jmap-vcard`)

- **AGY-TASKS sub-step:** 5. `merge_units` empty-name edge case: characterize `states_org_unit` unstated predicate fidelity, empty-name unit omission, and structured ORG roundtrip.
- **Changes:**
  - Added comprehensive characterization and round-trip tests in `rust/crates/jmap-vcard/tests/mapping.rs`:
    - `org_unit_empty_name_characterization_and_unstated_predicate_fidelity`: characterizes `states_org_unit` predicate across empty names (`OrgUnit::new("")`), empty names with `sortAs` (`extra["sortAs"]`), standard named units, and whitespace-only units; characterizes `states_organization` predicate with combinations of empty units and employer names.
    - `org_with_empty_name_units_and_sort_as_emission_and_roundtrip`: tests emission and roundtrip of organizations with only empty-named units (which emit only the employer name and omit unit components from wire format, preventing bogus empty components), organizations with intermediate or trailing empty units, nameless organizations with empty units (retaining leading semicolon for department position while omitting empty units), and inbound vCards with multiple empty components (`ORG:;;;`, `ORG:Acme;;Research;;Development;`).
- **Calcard behaviour-difference findings:**
  1. `calcard` correctly writes structured `ORG` entries with semicolon delimiters for non-empty components and avoids writing trailing empty delimiters when all units are empty.
  2. `calcard` parses multiple consecutive semicolons in structured values into empty string slices without collapsing delimiters, allowing `read_organization` to cleanly filter out intermediate empty components without shifting trailing units.
- **Gates ran:** `./ci/checks.sh` clean (REUSE 3.3 compliant, `cargo fmt`, `cargo clippy --all-targets --locked -- -D warnings`, `cargo test --locked`, `cargo deny check`).

