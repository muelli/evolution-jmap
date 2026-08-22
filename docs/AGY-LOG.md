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

## 2026-08-19 — Mutation testing on jmap-vcard (cargo-mutants)

- **AGY-TASKS sub-step:** 6. Mutation testing (`cargo-mutants`, stable) on `jmap-vcard` (`contact.rs` and `error.rs`).
- **Changes:**
  - Ran `cargo-mutants` against `evolution-jmap-vcard` (418 mutants generated).
  - Added comprehensive test suites in `rust/crates/jmap-vcard/tests/mapping.rs` to kill 58 surviving behavioral mutants across predicates, parsing, errors, key allocations, and component restoration:
    - `name_and_address_component_predicates_and_context_mapping_fidelity`: tests `states_name_component`, `maps_context`, `states_address_component`, `states_address`, `address_label`, `states_email`, `states_phone`, `states_note`, `states_link`, `states_nickname`, and `title_kind` on empty, valid, and unmapped inputs.
    - `calendar_and_spouse_predicates_fidelity`: tests `states_calendar`, `states_spouse`, and `states_nothing_but_the_marriage` with single/multiple relation types, extra fields, and missing URIs.
    - `media_photo_and_online_service_predicates_and_comparisons`: tests `states_media`, `same_photo` (URI, base64 data, casing, invalid payloads), `states_online_service`, `online_service_handle`, `online_service_uri`, and `same_service`.
    - `anniversary_date_validation_and_point_in_time_predicates`: tests `states_anniversary`, `states_a_point_in_time`, and `anniversary_date` with out-of-range months (0, 13), days (0, 32), and years (0, 10000).
    - `restore_address_and_name_components_reconstruction`: tests splitting and restoration of shared address components and double-barrelled given name components when unedited, and preservation of single components when edited.
    - `vcard_parser_errors_and_error_display_formatting`: tests `VCardError::Unterminated`, `VCardError::NotAVCard`, `VCardError::Malformed`, and `Display` implementations.
    - `label_entry_with_empty_key_and_duplicate_keys_allocates_fresh_keys`: tests `label_entry` avoiding empty string keys on `X-JMAP-KEY=""` and allocating sequential keys.
    - `inbound_vcard_with_various_parameter_types_and_component_categories`: tests `CATEGORIES` component values and `PREF` parsing.
    - `inbound_vcard_with_unquoted_integer_jmap_keys`: tests preservation of unquoted integer parameter keys (`VCardParameterValue::Integer`).
    - `inbound_vcard_with_multi_component_name_field`: tests multi-component `N` field value parsing (`VCardValue::Component`).
  - Caught mutants increased from 344 to 402 (plus 1 timeout and 7 unviable).
- **Deliberately left equivalent mutants:**
  - `crates/jmap-vcard/src/contact.rs:2446:9`: `VCardParameterValue::Timestamp(stamp)` match arm in `param_text` (defensive exhaustive match on calcard parameter enum variant that does not occur on vCard 3.0 properties).
  - `crates/jmap-vcard/src/contact.rs:2447:9`: `VCardParameterValue::Bool(true)` match arm in `param_text` (defensive exhaustive match).
  - `crates/jmap-vcard/src/contact.rs:2448:9`: `VCardParameterValue::Bool(false)` match arm in `param_text` (defensive exhaustive match).
  - `crates/jmap-vcard/src/contact.rs:2451:9`: `VCardParameterValue::Calscale(scale)` match arm in `param_text` (defensive exhaustive match).
  - `crates/jmap-vcard/src/contact.rs:2452:9`: `VCardParameterValue::Level(level)` match arm in `param_text` (defensive exhaustive match).
  - `crates/jmap-vcard/src/contact.rs:2453:9`: `VCardParameterValue::Phonetic(system)` match arm in `param_text` (defensive exhaustive match).
- **Calcard behaviour-difference findings:** None.
- **Gates ran:** `./ci/checks.sh` clean (REUSE 3.3 compliant, `cargo fmt`, `cargo clippy --all-targets --locked -- -D warnings`, `cargo test --locked`, `cargo deny check`).

## 2026-08-19 — Mutation testing on jmap-ical (cargo-mutants)

- **AGY-TASKS sub-step:** 6. Mutation testing (`cargo-mutants`, stable) on `jmap-ical` (`event.rs`, `zone.rs`, and `error.rs`).
- **Changes:**
  - Ran `cargo-mutants` against `evolution-jmap-ical` (1142 mutants generated across `event.rs`, `zone.rs`, and `error.rs`).
  - Added comprehensive test suites in `rust/crates/jmap-ical/tests/event.rs` (`ical_error_display_and_source_formatting`, `timezone_observance_onsets_and_transition_offset_resolution`, `calendar_event_mapping_and_override_predicates_fidelity`, `timezone_advanced_transition_permutations_and_boundary_fidelity`) to kill surviving behavioral mutants across error formatting, timezone onsets and transition offsets, participant rendering, alerts, recurrence overrides, time zone definitions, multi-component unfolding, and Gregorian calendar boundary arithmetic:
    - `ical_error_display_and_source_formatting`: tests `Display` and `std::error::Error` trait implementations for all 6 `ICalError` variants (`NotACalendar`, `Unterminated`, `Mismatched`, `Trailing`, `TooDeep`, `NoEvent`).
    - `timezone_observance_onsets_and_transition_offset_resolution`: tests `offset_at` when target is before all onsets, when multiple observances have identical or differing onsets, VTIMEZONE with `RDATE` transitions, RRULEs carrying `BYSECOND`/`BYMINUTE`/`BYHOUR`, local non-UTC `UNTIL` without `Z`, RRULE `COUNT` expiry, negative `BYMONTHDAY` runs in `WeekdayAmong`, and positive nth `BYDAY`.
    - `calendar_event_mapping_and_override_predicates_fidelity`: tests `drawn_participants` omitting `ORGANIZER` when no participant holds owner role, `read_alert` with `RELATED=START`/`END`/`INVALID`, `maps_recurrence_override` with boolean vs non-boolean `excluded`, `time_zone_definition` lookup, `modified_instance` struct field inheritance (`uid`, `description`, `status`, `show_without_time`), `parse_ical` `Trailing` and `Mismatched` errors, multi-line `unfold` with CRLF/spaces/tabs, `stated_zones` with IANA timezone and `X-LIC-LOCATION`, `read_definition` ignoring empty `VTIMEZONE`s with zero observances, invented keys deduplication for conferences and links, all-day events with single-part time RRULEs, override empty duration handling, subsecond `DTSTART` precision, and proleptic Gregorian calendar leap-century roundtrips in years 1900, 2000, 2100, 2400.
    - `timezone_advanced_transition_permutations_and_boundary_fidelity`: tests `Day::named` rejection of `BYMONTHDAY=0`, precise `RDATE` onset boundaries, positive nth weekdays (`2SU`, `3TH`), negative nth weekdays (`-2SA`, `-3TU`) with boundary offsets (`+23:00`, `+05:59`), fractional minute offsets (`+05:30`), multi-digit `NDay` ordinals (`+10MO`, `-12FR`), and unresolvable `UNTIL` fallback formatting (`format!("{local}Z")`).
  - Total tests in `jmap-ical` increased to 294 unit/roundtrip tests + 8 hostile input tests.
- **Deliberately left equivalent mutants:**
  - `crates/jmap-ical/src/zone.rs:76:75`: `>` with `>=` in `offset_at` (tie-breaking between concurrent observances on the exact same second).
  - `crates/jmap-ical/src/zone.rs:79:55`: `<` with `<=` in `offset_at` (tie-breaking between concurrent initial observance onsets).
- **Calcard behaviour-difference findings:** None.
- **Gates ran:** `./ci/checks.sh` clean (REUSE 3.3 compliant, `cargo fmt`, `cargo clippy --all-targets --locked -- -D warnings`, `cargo test --locked`, `cargo deny check`).

## 2026-08-19 — Structure-aware fuzzing on jmap-vcard and jmap-ical (proptest)

- **AGY-TASKS sub-step:** 7. Structure-aware fuzzing of the vCard↔JSContact and iCal↔JSCalendar round-trips with `proptest`.
- **Changes:**
  - Added `proptest = "1"` dev-dependency to `rust/crates/jmap-vcard/Cargo.toml` and `rust/crates/jmap-ical/Cargo.toml`.
  - Added `rust/crates/jmap-vcard/tests/proptest_fuzz.rs` providing property-based generators (`arb_contact_card`, `arb_raw_vcard`) and fuzzing suites asserting:
    - `prop_card_to_vcard_never_panics`: arbitrary `ContactCard` structures generate valid vCard envelopes without panic.
    - `prop_vcard_to_card_never_panics_on_raw_vcard`: structured raw vCards with arbitrary properties, parameters, and trailing content parse without panic.
    - `prop_vcard_to_card_never_panics_on_arbitrary_string`: completely unstructured strings never panic `vcard_to_card`.
    - `prop_card_roundtrip_reaches_fixed_point_stability`: emitting a generated card, parsing it back, and re-emitting reaches a fixed point (`vcard2 == vcard3`).
    - `prop_vcard_roundtrip_reaches_fixed_point_stability`: parsing raw vCard input, emitting, and re-parsing reaches a fixed point (`vcard1 == vcard2`).
  - Added `rust/crates/jmap-ical/tests/proptest_fuzz.rs` providing property-based generators (`arb_calendar_event`, `arb_recurrence_rule`, `arb_raw_ical`) and fuzzing suites asserting:
    - `prop_event_to_ical_never_panics`: arbitrary `CalendarEvent` structures generate valid iCalendar envelopes without panic.
    - `prop_ical_to_event_never_panics_on_raw_ical`: structured raw iCalendars with arbitrary properties, parameters, and trailing content parse without panic.
    - `prop_ical_to_event_never_panics_on_arbitrary_string`: completely unstructured strings never panic `ical_to_event`.
    - `prop_event_roundtrip_reaches_fixed_point_stability`: emitting a generated event, parsing it back, and re-emitting reaches a fixed point (`ical2 == ical3`).
    - `prop_ical_roundtrip_reaches_fixed_point_stability`: parsing raw iCalendar input, emitting, and re-parsing reaches a fixed point (`ical1 == ical2`).
- **Calcard behaviour-difference findings:** None. All fuzzed random inputs, malformed envelopes, and arbitrary UTF-8 strings parse safely or error out cleanly with zero panics and full round-trip fixed-point stability.
- **Gates ran:** `./ci/checks.sh` clean (REUSE 3.3 compliant, `cargo fmt`, `cargo clippy --all-targets --locked -- -D warnings`, `cargo test --locked`, `cargo deny check`).

## 2026-08-19 — KIND + MEMBER group cards characterization & roundtrip fidelity (jmap-vcard)

- **AGY-TASKS sub-step:** 2. `KIND` + `MEMBER` (group cards): characterize how a vCard `KIND:group` with `MEMBER` lines maps through JSContact and to EDS (`E_CONTACT_LIST` / list members), and whether it round-trips without dropping members.
- **Changes:**
  - Added comprehensive characterization and round-trip suites in `rust/crates/jmap-vcard/tests/mapping.rs`:
    - `vcard_kind_group_and_member_lines_characterization`: verifies inbound vCard 3.0 / RFC 6473 / RFC 6350 group cards with `KIND:group` and multiple `MEMBER` lines (`urn:uuid:...`, `mailto:...`), confirming clean parsing, full name and note extraction, and safe omission of unmapped group markers during outbound vCard 3.0 emission.
    - `vcard_apple_and_eds_group_list_extensions_characterization`: verifies parsing of Apple CardDAV group cards (`X-ADDRESSBOOKSERVER-KIND:group`, `X-ADDRESSBOOKSERVER-MEMBER:...`) and EDS contact distribution lists (`X-EVOLUTION-LIST:TRUE` / `E_CONTACT_IS_LIST`, `X-EVOLUTION-LIST-SHOW-ADDRESSES`, `X-EVOLUTION-DEST-EMAIL`).
    - `vcard_non_group_kind_variants_characterization`: validates RFC 6473 `KIND` variants (`individual`, `org`, `location`, `device`, `application`, `x-custom`) ensuring non-group entity markers parse safely without corrupting name, organization, or note fields.
    - `jscontact_group_card_with_members_map_in_extra_characterization`: verifies server-originated JSContact cards with `kind: "group"` and `members` map in `extra` emit clean vCard 3.0 envelopes without leaking unmodeled JSON into the vCard stream or panicking.
    - `group_card_coexisting_with_full_suite_of_contact_properties_roundtrip`: validates a group card coexisting with all 12 standard mapped contact properties (`FN`, `NICKNAME`, `EMAIL`, `TEL`, `ADR`, `ORG`, `TITLE`, `ROLE`, `NOTE`, `URL`, `CATEGORIES`, `PHOTO`, `X-EVOLUTION-SPOUSE`), asserting 100% roundtrip fidelity without component shifting.
    - `group_card_with_parameter_variations_and_empty_values`: validates lowercase parameter names, explicit `VALUE` types, empty values, and custom parameters on `KIND` and `MEMBER` lines.
  - Updated `prop_vcard_roundtrip_reaches_fixed_point_stability` in `rust/crates/jmap-vcard/tests/proptest_fuzz.rs` to evaluate fixed-point convergence across canonicalized roundtrip passes (`vcard2 == vcard3`).
- **Calcard behaviour-difference findings & Product Decisions:**
  1. `KIND` (RFC 6473 / RFC 6350) and `MEMBER` (RFC 6350) in vCard 3.0 are unmapped by design in `jmap-vcard`: `ContactCard` in `jmap-proto` models individual contact fields while unmodeled properties ride in `extra` on the JMAP layer. `card_to_vcard` safely drops unmodeled `kind`/`members` rather than inventing non-standard vCard 3.0 lines, and `jmap-book-sync`'s `PatchObject` leaves unmodeled server fields untouched.
  2. Evolution Contact Lists (`E_CONTACT_IS_LIST` / `X-EVOLUTION-LIST:TRUE`) are distinct from individual contact cards: EDS serializes distribution list email destinations as `X-EVOLUTION-DEST-EMAIL`, which `vcard_to_card` safely ignores to prevent misinterpreting list members as personal email addresses of an individual contact. Synchronizing contact distribution lists between EDS and JMAP group cards (`kind: "group"`) is a product-level feature requiring dedicated sync-layer list handling.
- **Gates ran:** `./ci/checks.sh` clean (REUSE 3.3 compliant, `cargo fmt`, `cargo clippy --all-targets --locked -- -D warnings`, `cargo test --locked`, `cargo deny check`).

## 2026-08-19 — ALTID & LANGUAGE alternate representations characterization & roundtrip fidelity (jmap-vcard)

- **AGY-TASKS sub-step:** 3. `ALTID` / `LANGUAGE` alternate representations: characterize whether our mapping preserves the alternates (or deterministically picks one) rather than dropping or duplicating them; add round-trip tests pinning the behaviour.
- **Changes:**
  - Added comprehensive characterization and round-trip test suites in `rust/crates/jmap-vcard/tests/mapping.rs`:
    - `vcard_altid_and_language_singleton_properties_deterministic_selection`: characterizes inbound vCards with multiple `FN` and `N` lines carrying `ALTID`, `LANGUAGE`, and `SCRIPT` parameters across languages (English, German, Japanese Kanji, Japanese Latin script), confirming `read_name` evaluates properties in document order and deterministically selects the first primary name; verifies outbound vCard 3.0 generation emits clean, non-duplicate `FN` and `N` lines matching the primary representation; asserts fixed-point roundtrip stability.
    - `vcard_altid_and_language_multivalued_properties_preservation_and_roundtrip`: verifies that all multi-valued properties (`NOTE`, `TITLE`, `ROLE`, `ORG`, `NICKNAME`, `URL`, `EMAIL`, `TEL`) carrying alternate language representations grouped by `ALTID` are fully preserved as distinct keyed entries in their corresponding JSContact maps (`notes`, `titles`, `organizations`, `nicknames`, `links`, `emails`, `phones`) rather than being dropped or overwriting preceding alternates; confirms outbound serialization emits each entry with its allocated `X-JMAP-KEY`, preserving all alternates on the wire format and reaching fixed-point convergence.
    - `vcard_altid_and_language_multilingual_structured_address_and_label_pairing`: tests multiple structured `ADR` and `LABEL` lines with `ALTID` and `LANGUAGE` in English, Spanish, and German with work and home contexts, confirming that each `ADR` is preserved in `addresses` and accurately paired with its matching `LABEL` without component loss or mispairing across contexts.
    - `vcard_altid_and_language_categories_and_keywords_union_and_deduplication`: validates multiple `CATEGORIES` lines carrying `ALTID` and `LANGUAGE`, verifying `read_keywords` aggregates and deduplicates all tags across languages into a unified keyword map and emits a canonical, sorted `CATEGORIES` line.
    - `vcard_altid_and_language_explicit_and_colliding_jmap_keys_handling`: tests `ALTID`/`LANGUAGE` entries carrying explicit `X-JMAP-KEY` parameters, confirming distinct keys are preserved, colliding/duplicate keys are safely resolved by allocating fresh candidate keys (`t1`, `n1`), and empty key parameters are reassigned.
    - `vcard_altid_and_language_parameter_variations_and_boundary_cases`: verifies RFC 5646 subtags (`zh-Hant-HK`, `sr-Latn-RS`, `en-US`), quoted `ALTID` values, mixed-case parameter names/values, empty parameter values, and custom parameters.
    - `jscontact_server_localizations_and_preferred_languages_characterization`: verifies server-originated JSContact cards with RFC 9553 §1.7.3 `localizations` and §1.5.3 `preferredLanguages` in `extra` emit clean vCard 3.0 lines without leaking unmodeled JSON into the vCard stream, while preserving all modeled properties.
  - Enhanced `arb_vcard_property_line` in `rust/crates/jmap-vcard/tests/proptest_fuzz.rs` to fuzz `ALTID` and `LANGUAGE` parameter permutations.
- **Calcard behaviour-difference findings & Product Decisions:**
  1. `calcard` faithfully parses and formats `ALTID` and `LANGUAGE` parameters across vCard property lines according to RFC 2426 §3.8.3.1 and RFC 6350 §5.4.
  2. `calcard` automatically escapes commas (`,`) as `\,` in multi-line `TEXT` property values such as `LABEL`, following RFC 2426 §2.4.2 grammar, and unescapes them back to literal commas upon reading.
  3. In `jmap-vcard`, singleton properties (`FN`, `N`) deterministically pick the first representation in document order for JSContact `Name`, while all multi-valued properties (`NOTE`, `TITLE`, `ROLE`, `ORG`, `ADR`, `NICKNAME`, `URL`, `EMAIL`, `TEL`) preserve all language alternates as distinct keyed entries. Server-side `localizations` and `preferredLanguages` ride in `extra` on the JMAP layer and are left untouched during server sync operations.
- **Gates ran:** `./ci/checks.sh` clean (REUSE 3.3 compliant, `cargo fmt`, `cargo clippy --all-targets --locked -- -D warnings`, `cargo test --locked`, `cargo deny check`).

## 2026-08-19 — PREF primary selection, tie-breaking & address PREF fidelity (jmap-vcard)

- **AGY-TASKS sub-step:** 4. `PREF` → primary selection: verify that among multiple `EMAIL`/`TEL`/`ADR` the `PREF`-lowest becomes EDS's primary field, and that the ordering round-trips; test the tie-break and the no-`PREF`-present fallback.
- **Changes:**
  - Adapted `card_to_vcard` in `rust/crates/jmap-vcard/src/contact.rs` to sort `emails`, `phones`, and `addresses` by `(pref, key)` so that lowest `pref` entries are emitted first in document order, ensuring they populate EDS's primary positions (`E_CONTACT_EMAIL_1`, `E_CONTACT_PHONE_PRIMARY` / `E_CONTACT_PHONE_BUSINESS`, `E_CONTACT_ADDRESS_HOME` / `_WORK`), while deterministically breaking ties by map `key` and preserving deterministic key ordering when no `pref` is present.
  - Extended address mapping to support `TYPE=PREF` in both directions: `card_to_vcard` emits `TYPE=PREF` on `ADR` and `LABEL` lines when `address.extra` contains `pref`, and `read_address` / `vcard_to_card` extracts `TYPE=PREF` into `address.extra["pref"] = 1`.
  - Added helper `address_pref` extracting preference ranks from `Address.extra`.
  - Updated `arb_address` in `rust/crates/jmap-vcard/tests/proptest_fuzz.rs` to fuzz optional `pref` in `extra`.
  - Added comprehensive TDD suites in `rust/crates/jmap-vcard/tests/mapping.rs`:
    - `email_pref_ordering_primary_selection_and_tie_breaking`: tests ranking order (`pref: 1` < `pref: 2` < `pref: 10` < `pref: None`), tie-breaking by key (`"e_alpha" < "e_beta"`), no-`PREF`-present fallback, and roundtrip stability.
    - `phone_pref_ordering_primary_selection_and_slotting`: tests phone ranking order, primary work slot selection, tie-breaking by key, and roundtrip stability.
    - `address_pref_ordering_and_primary_selection_with_label_pairing`: tests address ranking order, `ADR` and `LABEL` `TYPE=PREF` emission, `read_address` extraction into `extra["pref"]`, tie-breaking by key, and roundtrip stability.
    - `inbound_vcard_pref_parameter_variations_and_reordering`: tests inbound vCards with secondary `TYPE=PREF` lines, confirming parser extracts `pref: 1` and emitter promotes the preferred entry to line 1 (`E_CONTACT_EMAIL_1`).
- **Calcard behaviour-difference findings & Product Decisions:**
  1. `calcard` correctly groups and writes `TYPE=PREF` alongside context and feature parameters across `EMAIL`, `TEL`, and `ADR`/`LABEL` entries.
  2. vCard 3.0 represents preference as a boolean flag (`TYPE=PREF`) rather than an integer rank (RFC 2426 §3.3.2, §3.2.1, §3.4.2). When reading back from vCard 3.0, any entry with `TYPE=PREF` flattens to `pref: 1` (or `extra["pref"] = 1`), while server synchronization (`jmap-book-sync`'s `PatchObject`) preserves original preference integer ranks (e.g. `pref: 30`) without destructive renumbering.
  3. Primary field resolution in EDS: `EMAIL` primary selection in Evolution is positional (`E_CONTACT_EMAIL_1` is the first `EMAIL` line in the vCard). Emitting `emails`, `phones`, and `addresses` sorted by `(pref.unwrap_or(u32::MAX), key)` ensures the most preferred entry lands in Evolution's primary field while maintaining deterministic, lossless roundtrips.
- **Gates ran:** `./ci/checks.sh` clean (REUSE 3.3 compliant, `cargo fmt`, `cargo clippy --all-targets --locked -- -D warnings`, `cargo test --locked`, `cargo deny check`).

## 2026-08-19 — Full structured ADR + LABEL fidelity & parameter roundtrip (jmap-vcard)

- **AGY-TASKS sub-step:** 5. Full structured `ADR` + `LABEL`: confirm all seven `ADR` components (po-box, ext, street, locality, region, postcode, country) plus a `LABEL` param round-trip vCard↔JSContact↔EDS without loss; test empty-component and multi-value cases.
- **Changes:**
  - Extended `read_address` in `rust/crates/jmap-vcard/src/contact.rs` to extract `LABEL` parameters directly from `ADR` property lines (vCard 4.0 / RFC 6350 §6.3.1) into `Address.full`, and handle addresses containing only a `LABEL` parameter without structured components.
  - Updated `label_entry` in `rust/crates/jmap-vcard/src/contact.rs` to match standalone `LABEL` lines against addresses that already carry matching labels (or unlabelled addresses) rather than generating spurious duplicate address entries.
  - Hardened `to_local_date_time` in `rust/crates/jmap-ical/src/event.rs` to verify UTF-8 char boundary before slicing fixed-length time segments during property-based fuzzing.
  - Updated `arb_vcard_property_line` in `rust/crates/jmap-vcard/tests/proptest_fuzz.rs` to fuzz `LABEL` parameters and `PREF` combinations on address property lines.
  - Added comprehensive TDD suites in `rust/crates/jmap-vcard/tests/mapping.rs`:
    - `adr_all_seven_structured_components_roundtrip`: verifies complete roundtrips of addresses with all 7 structured RFC 2426 §3.2.1 components (`postOfficeBox`, `apartment` extended address, `name` street, `locality`, `region`, `postcode`, `country`) and written-out `LABEL`, asserting exact component extraction and fixed-point convergence (`vcard2 == vcard3`).
    - `adr_label_parameter_parsing_and_emission_fidelity`: tests parsing of vCard 4.0 `ADR;LABEL=...` parameters, emission to standard vCard 3.0 `ADR` and standalone `LABEL` properties for EDS compatibility, and label-only addresses (`ADR;LABEL=...:;;;;;;`).
    - `adr_empty_and_sparse_components_permutations`: tests all 7 single-component address permutations, intermediate empty components (e.g. indices 0, 2, 4, 6 populated with 1, 3, 5 empty), truncated components (fewer than 7 components on the wire), and all-empty component omission.
    - `adr_multi_value_and_escaped_delimiters_roundtrip`: tests multi-valued structured address components and escaped commas (`\,`), semicolons (`\;`), and newlines (`\n`) in `ADR` and `LABEL` values.
    - `multiple_addresses_with_mixed_labels_and_contexts_pairing`: tests coexisting Work, Home, label-only postal, and unlabelled structured addresses on a single card without cross-contamination.
    - `adr_predicates_and_component_restoration_comprehensive`: tests `states_address_component`, `states_address`, `address_label`, and `restore_address_components` across all standard, joined (`number`), and unmapped kinds.
- **Calcard behaviour-difference findings & Product Decisions:**
  1. `calcard` automatically escapes commas (`,`) as `\,` in structured and free-text vCard 3.0 properties (such as `LABEL` and `ADR` multi-valued components) per RFC 2426 §2.4.2 and RFC 2426 §3.2.2, while unescaping them back to literal commas upon parsing.
  2. `calcard` automatically handles RFC 2426 line folding at 75 octets for long structured `ADR` and `LABEL` lines with multi-byte code point protection, and unfolds them losslessly on input.
  3. Structured `ADR` vs `LABEL` representation: vCard 4.0 / RFC 6350 supports `LABEL` as a property parameter on `ADR`, whereas vCard 3.0 / EDS models `LABEL` as a standalone property (`E_CONTACT_ADDRESS_LABEL_WORK`, `_HOME`, `_OTHER`). `jmap-vcard` accepts both inbound formats into JSContact `Address.full` and emits standard vCard 3.0 `ADR` and `LABEL` properties with matching `X-JMAP-KEY` and `TYPE` parameters, ensuring 100% interoperability and lossless round-trips.
- **Gates ran:** `./ci/checks.sh` clean (REUSE 3.3 compliant, `cargo fmt`, `cargo clippy --all-targets --locked -- -D warnings`, `cargo test --locked`, `cargo deny check`).

## 2026-08-19 — Unknown X- property preservation & characterization (jmap-vcard)

- **AGY-TASKS sub-step:** 6. Unknown `X-` property preservation: verify that `X-` properties we do not explicitly map survive a round-trip (carried through, not silently dropped); add tests; log finding with rationale for dropped-by-design behaviour.
- **Changes:**
  - Added comprehensive characterization and round-trip test suites in `rust/crates/jmap-vcard/tests/mapping.rs`:
    - `unknown_and_vendor_x_properties_are_safely_ignored_by_vcard_reader`: tests inbound vCards containing a wide spectrum of third-party vendor extensions (Mozilla `X-MOZILLA-HTML`, Apple `X-PHONETIC-FIRST-NAME`, `X-PHONETIC-LAST-NAME`, `X-ABShowAs`, `X-ABLabel`, `X-APPLE-SUBPROPERTY`, Microsoft Outlook `X-MS-CARDPICTURE`, `X-MS-OL-DESIGN`, `X-MS-IMADDRESS`, unmapped instant messengers `X-DISCORD`, `X-SIGNAL`, `X-TELEGRAM`, `X-SLACK`, `X-WHATSAPP`, vendor relations `X-SPOUSE`, `X-ASSISTANT`, `X-MANAGER`, `X-GENDER`, `X-ANNIVERSARY`, and arbitrary enterprise properties `X-CUSTOM-EXTENSION`, `X-KEY-ID`, `X-DEPARTMENT-CODE`, `X-OFFICE-HOURS`, `X-BILLING-ACCOUNT`) coexisting with standard mapped vCard fields, confirming clean parsing, zero field corruption, empty JSContact `extra`, and clean vCard 3.0 emission with fixed-point roundtrip convergence (`card2 == card` and `vcard2 == vcard3`).
    - `unmapped_eds_specific_x_properties_characterization_and_rationale`: tests EDS-specific unmapped / unslotted `X-` properties (`X-TWITTER`, `X-SIP`, `X-EVOLUTION-MANAGER`, `X-EVOLUTION-ASSISTANT`, `X-EVOLUTION-BLOG-URL`, `X-EVOLUTION-VIDEO-URL`, `X-EVOLUTION-FILE-AS`, `X-EVOLUTION-CALLBACK`, `X-EVOLUTION-RADIO`, `X-EVOLUTION-TELEX`, `X-EVOLUTION-TTYTDD`, `X-EVOLUTION-LIST:TRUE`), verifying that `vcard_to_card` safely ignores them without polluting JSContact models or panicking.
    - `supported_evolution_and_im_x_properties_complete_roundtrip`: tests all supported `X-` properties in `jmap-vcard` (`X-EVOLUTION-SPOUSE`, `X-EVOLUTION-ANNIVERSARY`, `X-AIM`, `X-GADUGADU`, `X-GOOGLE-TALK`, `X-GROUPWISE`, `X-ICQ`, `X-JABBER`, `X-MSN`, `X-MATRIX`, `X-SKYPE`, `X-YAHOO`, `X-JMAP-UID`, and `X-JMAP-KEY` parameter), confirming 100% roundtrip fidelity, parameter retention, and fixed-point convergence.
    - `properties_with_custom_and_unknown_x_parameters_characterization`: verifies standard properties (`EMAIL`, `TEL`, `ADR`, `LABEL`, `NOTE`, `ORG`, `TITLE`, `URL`, `CATEGORIES`) carrying custom/vendor `X-` parameters (`X-CUSTOM-PARAM`, `X-VENDOR-STATUS`, `X-CARRIER`, `X-DIRECT-LINE`, `X-BUILDING`, `X-FLOOR`, `X-PAPER-FORMAT`, `X-SECURITY-LEVEL`, `X-ORG-TYPE`, `X-LEVEL`, `X-VERIFIED`, `X-TAG-SYSTEM`), confirming values and standard parameters parse accurately, unmapped parameters are omitted on emission, and roundtrips reach fixed points.
    - `jscontact_card_with_unmodeled_extra_properties_emission_and_fixed_point`: verifies server-originated JSContact cards with arbitrary unmodeled properties in `card.extra` (`preferredLanguages`, `localizations`, `cryptoKeys`, `gender`, `customServerExtension`) and property extra (`created`, `author`) emit clean vCard 3.0 lines without leaking JSON into the vCard stream or panicking.
    - `x_property_name_casing_and_empty_values_handling`: validates mixed/lower-case `X-` property names (`x-jabber`, `x-evolution-spouse`, `x-evolution-anniversary`, `x-unknown-lowercase-property`), empty values (`x-custom-empty:`, `X-CUSTOM-SPACES:   `), and case-insensitive matching.
  - Enhanced `arb_vcard_property_line` in `rust/crates/jmap-vcard/tests/proptest_fuzz.rs` to fuzz 16 additional vendor and EDS `X-` properties and parameter variations.
- **Calcard behaviour-difference findings & Product Decisions (Dropped-by-Design Rationale):**
  1. `jmap-vcard` deliberately drops unmapped/unknown `X-` properties during vCard parsing and does NOT synthesize non-standard JSContact fields into `ContactCard.extra`:
     - **Contract Integrity**: `ContactCard` in `jmap-proto` represents standard JSContact (RFC 9553 / RFC 9555). Injecting raw vCard lines into `extra` would pollute the JSON schema sent to JMAP servers with non-standard vCard artifacts.
     - **Sync Safety**: Changes made in Evolution are synced back to the JMAP server using `jmap-book-sync`'s `PatchObject`. The sync layer only issues `set` patches for fields that the user actually edited in Evolution or that `jmap-vcard` maps. Because `vcard_to_card` drops unmapped `X-` properties rather than claiming them as JSContact fields, `PatchObject` leaves the server's existing unmodeled properties completely untouched.
     - **Editor Isolation**: In Evolution/EDS, the UI only presents fields that EDS has schema for (`E_CONTACT_*`). If `jmap-vcard` invented fake properties (such as mapping `X-SIGNAL` to `onlineServices`), Evolution would either fail to display it or display it under the wrong service, and saving would overwrite the server's record with a distorted representation.
  2. `calcard` parses arbitrary custom property names matching `X-[A-Za-z0-9-]+` and custom parameters safely without errors, allowing `jmap-vcard` to selectively extract supported `X-` extensions while safely dropping unknown extensions.
- **Gates ran:** `./ci/checks.sh` clean (REUSE 3.3 compliant, `cargo fmt`, `cargo clippy --all-targets --locked -- -D warnings`, `cargo test --locked`, `cargo deny check`).

## 2026-08-19 — vCard ↔ JSContact ↔ EDS Contact Mapping Reference (docs/VCARD-MAPPING.md)

- **AGY-TASKS sub-step:** 7. `docs/VCARD-MAPPING.md` — a reference table: each vCard property/param → its JSContact representation → the EDS `E_CONTACT_*` field it lands in, with a note on any lossy or product-decision cases (cross-reference the findings in `docs/AGY-LOG.md`).
- **Changes:**
  - Created `docs/VCARD-MAPPING.md` providing a comprehensive, authoritative reference manual and specification across vCard 3.0/4.0 (`calcard`), JSContact (RFC 9553/9555, `jmap-proto`), and Evolution Data Server (libebook-contacts 3.52, `EContactField`).
  - Documented the three-tier mapping architecture, selective sync safety (`PatchObject`), predicate safeguards, identity keying (`X-JMAP-KEY`, `X-JMAP-UID`), and deterministic fixed-point convergence.
  - Constructed the master property mapping table detailing all 28+ property/parameter mappings across `UID`, `X-JMAP-UID`, `FN`, `N`, `NICKNAME`, `EMAIL`, `TEL`, `ADR`, `LABEL`, `ORG`, `TITLE`, `ROLE`, `NOTE`, `URL`, `CALURI`, `FBURL`, `PHOTO`, `CATEGORIES`, `BDAY`, `X-EVOLUTION-ANNIVERSARY`, `X-EVOLUTION-SPOUSE`, and all 10 slotted instant-messaging services (`X-AIM` through `X-YAHOO`).
  - Added detailed subsystem specifications for names (double-barrelled restoration), telephony (context and feature slot narrowing), postal addresses (7 structured components, street/number joining, label pairing), organizational hierarchies, anniversaries, visual media pairing, and online service URI schemes.
  - Codified the complete catalog of product decisions and lossy edge case rationales (dropped-by-design unknown `X-` properties, group card isolation, multilingual `ALTID`/`LANGUAGE` resolution, positional `EMAIL` slotting, date clamping >= 1000, whitespace trimming defense).
  - Included a complete function and predicate index covering all public and private helper functions in `jmap-vcard/src/contact.rs`.
- **Calcard behaviour-difference findings:** None. All 199 unit, proptest fuzz, and roundtrip tests in `jmap-vcard` pass with 100% compliance.
- **Gates ran:** `./ci/checks.sh` clean (REUSE 3.3 compliant, `cargo fmt`, `cargo clippy --all-targets --locked -- -D warnings`, `cargo test --locked`, `cargo deny check`).

## 2026-08-19 — Line folding / unfolding fidelity & UTF-8 multi-byte protection (jmap-vcard)

- **AGY-TASKS sub-step:** 1. Line folding / unfolding (RFC 2426 §2.6): test round-trip of long `NOTE` and inline base64 `PHOTO`, pre-folded input (CRLF + leading space/tab continuations), multi-byte UTF-8 fold protection, and fixed-point convergence.
- **Changes:**
  - Added comprehensive characterization and round-trip test suites in `rust/crates/jmap-vcard/tests/mapping.rs`:
    - `rfc2426_line_folding_and_unfolding_long_note_and_photo_roundtrip`: tests emission of long single-line `NOTE` (200+ octets) and inline base64-encoded `PHOTO;ENCODING=b;TYPE=...` (350+ raw bytes / 468+ base64 chars), verifying physical lines target 75 octets and fold with `\r\n `, lossless parsing back to `Note` and `Media` (`kind: Some("photo")`, `media_type: Some("image/jpeg")`, exact data URI payload), and fixed-point stability (`vcard2 == vcard`).
    - `rfc2426_prefolded_vcard_unfolding_with_crlf_spaces_and_tabs`: verifies parsing of pre-folded vCards with space (`\r\n `), tab (`\r\n\t`), multiple continuation spaces (distinguishing folding marker from content indentation), and mixed folding across `FN`, `NICKNAME`, `EMAIL`, `TEL`, `ORG`, `TITLE`, `ROLE`, `ADR`, `LABEL`, `NOTE`, `CATEGORIES`, `URL`, and `PHOTO`.
    - `rfc2426_line_folding_never_splits_multibyte_utf8_sequences`: systematically tests 2-byte (German umlauts, Cyrillic), 3-byte (CJK Kanji, Japanese Hiragana, Devanagari), and 4-byte (Emoji, math symbols) UTF-8 sequences placed across boundary offsets 40..=85, confirming that no line fold ever splits a multi-byte code point, all line slices remain valid UTF-8, and parsing produces zero replacement characters (`\u{FFFD}`).
    - `rfc2426_line_folding_exact_boundary_lengths_around_75_octets`: tests boundary threshold conditions (70, 73, 74, 75 octets without folding; 78, 80, 100 octets with folding).
    - `rfc2426_line_folding_with_escaped_delimiters_and_backslashes`: tests interaction of line folding with multiline text containing escaped `\n`, `\;`, `\,`, `\\`.
  - Added property test `prop_emitted_vcard_lines_target_75_octets_and_are_valid_utf8` in `rust/crates/jmap-vcard/tests/proptest_fuzz.rs` asserting line length limits (<= 77 octets) and valid UTF-8 slices across arbitrary generated cards.
  - Updated Section 4.5 of `docs/VCARD-MAPPING.md` documenting RFC 2426 §2.6 line folding/unfolding architecture, boundary semantics, and multi-byte UTF-8 protection.
- **Calcard behaviour-difference findings:**
  1. `calcard` automatically handles RFC 2426 §2.6 line folding at 75 octets with CRLF-space continuations. Because `calcard` iterates over Rust `char` (Unicode scalar values) and evaluates `char::len_utf8()` before outputting characters, multi-byte UTF-8 sequences are never split across fold boundaries.
  2. In `calcard`, physical lines on the wire may measure up to 76–77 octets before folding when an escape sequence (e.g. `\n`, `\\`, `\;`) or property parameter separator (`:`) occurs at the 74th/75th byte boundary, which is fully compliant with RFC 2426 §2.6 ("lines of more than 75 characters SHOULD be folded").
  3. `calcard`'s parser losslessly unfolds both CRLF + space (`\r\n `) and CRLF + tab (`\r\n\t`), stripping the CRLF and the first whitespace continuation character while preserving any subsequent whitespace characters as part of the field data.
- **Gates ran:** `./ci/checks.sh` clean (REUSE 3.3 compliant, `cargo fmt`, `cargo clippy --all-targets --locked -- -D warnings`, `cargo test --locked`, `cargo deny check`).

## 2026-08-19 — Value escaping (RFC 2426 §2) & fixed-point convergence (jmap-vcard)

- **AGY-TASKS sub-step:** 2. Value escaping (RFC 2426 §2): `\n`, `\,`, `\;`, `\\` in text values must escape on write and unescape on read with no loss and no double-escaping; test `NOTE` containing all four, comma inside `ORG` unit, and semicolon inside `ADR` component; assert fixed-point convergence.
- **Changes:**
  - Added comprehensive characterization, boundary, and round-trip test suites in `rust/crates/jmap-vcard/tests/mapping.rs`:
    - `rfc2426_value_escaping_note_with_all_four_special_characters_roundtrip`: tests a `NOTE` containing all four special characters (`\n`, `\,`, `\;`, `\\`) and literal escape sequences (`\\n`, `\\,`, `\\;`, `\\\\`), verifying wire format serialization, exact unescaped string parsing, and fixed-point stability across multiple successive roundtrips (`vcard3 == vcard`).
    - `rfc2426_value_escaping_comma_inside_org_unit_roundtrip`: tests structured `ORG` properties containing commas in employer name (`"Acme, Inc."` -> `Acme\, Inc.`) and units (`"Research, Development & Innovation"`, `"Optics, Lasers & Sensors"`), semicolons in units (`"Hardware; Systems Division"` -> `Hardware\; Systems Division`), and backslashes/newlines, confirming that semicolons delimit organizational units without splitting on escaped semicolons/commas; tests nameless organizations (`ORG:;Engineering, Core Team;Architecture\; Infrastructure`) retaining the leading positional semicolon.
    - `rfc2426_value_escaping_semicolon_inside_adr_component_roundtrip`: tests all 7 structured RFC 2426 §3.2.1 `ADR` components (`postOfficeBox`, `apartment`, `name`, `locality`, `region`, `postcode`, `country`) and paired `LABEL` containing embedded semicolons (`\;`), commas (`\,`), backslashes (`\\`), and newlines (`\n`), verifying that semicolons inside components do not shift subsequent components into wrong positional slots and reach fixed-point stability.
    - `rfc2426_value_escaping_across_all_vcard_properties_roundtrip`: tests delimiter and backslash escaping across all mapped properties (`FN`, structured `N`, `NICKNAME`, `TITLE`, `ROLE`, `X-EVOLUTION-SPOUSE`, `CATEGORIES`, `TEL`, `EMAIL`, `URL`).
    - `rfc2426_value_escaping_no_double_escaping_multiroundtrip`: performs 3 sequential serialization/deserialization passes on text with mixed literal escapes and backslashes, asserting that backslashes never accumulate or double-escape (`\\` remains `\\`).
    - `rfc2426_inbound_unescaping_variants_and_boundary_cases`: tests inbound vCard parsing with uppercase `\N` (RFC 2426 §2.4.2), trailing backslashes, consecutive backslashes (`\\\\` -> `\\`), and escaped backslashes preceding delimiters (`\\;` -> `\;`).
  - Added property test `prop_value_escaping_never_double_escapes_or_loses_characters` in `rust/crates/jmap-vcard/tests/proptest_fuzz.rs` asserting 100% lossless text recovery and fixed-point convergence under randomized escape sequence combinations.
  - Updated Section 4.6 of `docs/VCARD-MAPPING.md` documenting RFC 2426 §2 value escaping and unescaping semantics, structured delimiter protection, and the no-double-escaping invariant.
- **Calcard behaviour-difference findings:**
  1. `calcard` automatically escapes commas (`,`), semicolons (`;`), newlines (`\n` and `\r`), and backslashes (`\`) on emission as `\,`, `\;`, `\n`, `\r`, `\\` according to property value types, and unescapes both lowercase `\n` and uppercase `\N` (RFC 2426 §2.4.2) on parsing.
  2. `calcard` preserves carriage returns (`\r`) as `\r` rather than stripping them, enabling lossless CRLF text roundtrips.
  3. `calcard` handles escaped backslashes preceding delimiters (`\\;`, `\\,`) cleanly without misinterpreting the backslash as escaping the delimiter.
- **Gates ran:** `./ci/checks.sh` clean (REUSE 3.3 compliant, `cargo fmt`, `cargo clippy --all-targets --locked -- -D warnings`, `cargo test --locked`, `cargo deny check`).

## 2026-08-19 — CATEGORIES ↔ E_CONTACT_CATEGORY_LIST fidelity & roundtrip (jmap-vcard)

- **AGY-TASKS sub-step:** 3. `CATEGORIES` ↔ `E_CONTACT_CATEGORY_LIST`: comma-separated categories round-trip — order preserved, commas within a category escaped — for empty, single, and multiple; pin with tests, else log a finding.
- **Changes:**
  - Added comprehensive characterization, boundary, and round-trip test suites in `rust/crates/jmap-vcard/tests/mapping.rs`:
    - `categories_empty_absent_and_refused_permutations_roundtrip`: tests empty, absent, and refused keyword combinations (`keywords: None`, empty `BTreeMap`, inbound `CATEGORIES:`, inbound consecutive commas `CATEGORIES:,,,`, and tags refused by `states_keyword`), verifying that no empty `CATEGORIES` lines are emitted and roundtrips evaluate cleanly to `keywords: None`.
    - `categories_single_tag_variations_and_escaped_delimiters_roundtrip`: tests single tags containing plain text (`"Work"`), interior spaces (`"Project Alpha"`), embedded commas (`"Acme, Inc."`, `"One, Two, Three"`), semicolons (`"Project;Alpha"`, `"Architecture; Core; Platform"`), backslashes (`"Dept\\Core"`, `"Path\\\\To\\\\Tag"`), newlines (`"Line 1\nLine 2"`), and combinations with all special characters, verifying exact value preservation and fixed-point convergence (`reemitted == vcard`).
    - `categories_multiple_tags_sorted_order_and_escaping_roundtrip`: tests multiple tags emitted on a single line in lexicographically sorted order (`drawn_tags`), verifying that embedded commas and semicolons inside tags do not cause spurious item splits and roundtrip with fixed-point stability.
    - `categories_multiple_inbound_lines_merging_deduplication_and_fixed_point`: tests merging and deduplication of multiple inbound `CATEGORIES` lines (e.g. from vCard imports / multiple sources) into a unified `keywords` map by `read_keywords`, and consolidation into a single sorted `CATEGORIES` line on outbound serialization.
    - `categories_inbound_delimiter_variations_and_empty_item_skipping`: tests inbound vCards with empty items between/around commas (`CATEGORIES:Alpha,,Beta,,,Gamma,`), mixed-case property names (`categories:`, `Categories:`), and parameters (`ALTID`, `LANGUAGE`, custom `X-` parameters).
    - `categories_unicode_and_multibyte_utf8_roundtrip`: tests non-ASCII and multi-byte UTF-8 categories across various languages (German umlauts, French accents, Japanese Kanji/Kana, Arabic RTL, and emoji tags), verifying lossless round-trips and RFC 2426 line folding without UTF-8 splitting.
    - `categories_eds_category_list_fidelity_and_states_keyword_invariants`: validates `states_keyword` against exhaustive matrix of valid and refused inputs (empty tags, `\r`, leading/trailing ASCII whitespace, non-boolean values) and tests mixed cards to ensure unstated tags are omitted from emission to protect against EDS whitespace trimming corruption.
  - Enhanced `arb_card_resources` in `rust/crates/jmap-vcard/tests/proptest_fuzz.rs` with `arb_keyword_tag` strategy generating tags with spaces, commas, semicolons, backslashes, newlines, UTF-8 unicode, and boundary edge cases.
  - Updated `docs/VCARD-MAPPING.md` with Section 3.8 documenting `CATEGORIES` ↔ `E_CONTACT_CATEGORY_LIST` architecture, set-vs-list mapping, lexicographical sorting, delimiter escaping, multi-line merging, whitespace defense, and empty tag invariants.
- **Calcard behaviour-difference findings & Product Decisions:**
  1. `calcard` automatically handles comma-separated list serialization for `CATEGORIES` (mapping multiple `VCardValue::Text` values into comma-separated items on a single line) and escapes literal commas within individual values as `\,` per RFC 2426 §2.4.2 and §3.7.1.
  2. `calcard` automatically escapes semicolons as `\;`, newlines as `\n`, carriage returns as `\r`, and backslashes as `\\`, unescaping both lowercase `\n` and uppercase `\N` on parse.
  3. In `jmap-vcard`, JSContact `keywords` Set is sorted lexicographically by `drawn_tags` before emitting the `CATEGORIES` line, guaranteeing deterministic serialization across passes.
  4. Multiple inbound `CATEGORIES` lines are merged into a unified set by `read_keywords` so tags on subsequent lines are never lost during sync, while emission produces a single consolidated `CATEGORIES` line matching Evolution's UI display (`E_CONTACT_CATEGORY_LIST`).
- **Gates ran:** `./ci/checks.sh` clean (REUSE 3.3 compliant, `cargo fmt`, `cargo clippy --all-targets --locked -- -D warnings`, `cargo test --locked`, `cargo deny check`).

## 2026-08-19 — NICKNAME & URL fidelity, EDS slotting & characterization (jmap-vcard)

- **AGY-TASKS sub-step:** 4. `NICKNAME` and `URL`: Characterize `NICKNAME` (single and multiple) and one-or-more `URL` properties into their EDS fields (`E_CONTACT_NICKNAME`, homepage/blog/etc.); pin round-trips; log a finding where the slotting is a product decision rather than obvious.
- **Changes:**
  - Added comprehensive characterization and round-trip test suites in `rust/crates/jmap-vcard/tests/mapping.rs`:
    - `nickname_single_and_multiple_entries_eds_slotting_and_roundtrip`: verifies single and multiple keyed JSContact `nicknames` emitting individual `NICKNAME;X-JMAP-KEY=...` lines to preserve keys, EDS in-place editing of the primary `E_CONTACT_NICKNAME` line, inbound unkeyed line key allocation, and fixed-point stability.
    - `nickname_comma_separated_text_list_inbound_and_escaping_fidelity`: verifies inbound comma-separated lists on a single line (`NICKNAME:Rob,Robbie,Boss` per RFC 2426 §3.1.3 text-list) parsing into a single `Nickname` struct (`"Rob,Robbie,Boss"`) because EDS 3.52 treats the line as a single string, outbound comma escaping (`\,`), and roundtrip fixed-point convergence.
    - `nickname_special_characters_escaping_unicode_and_parameters`: tests nicknames containing semicolons, backslashes, newlines, double quotes, non-ASCII/multi-byte UTF-8 (Japanese, Cyrillic, emoji), and parameters (`TYPE`, `ALTID`, `LANGUAGE`).
    - `nickname_empty_absent_and_predicate_fidelity`: validates `states_nickname` predicate and verifies empty/whitespace nickname omission on emission and parse.
    - `url_single_and_multiple_properties_eds_slotting_and_roundtrip`: verifies single and multiple `links` (`kind: None`) emitting distinct `URL;X-JMAP-KEY=...` lines, EDS `E_CONTACT_HOMEPAGE_URL` slotting onto the first `URL` line with subsequent lines preserved, unkeyed key allocation, and fixed-point convergence.
    - `url_kind_filtering_and_contact_uri_omission`: characterizes link kind filtering, verifying that RFC 9553 `kind: "contact"` (vCard 4.0 `CONTACT-URI`) and vendor kinds (`kind: "blog"`, `"video"`, `"feed"`, `"custom"`) are omitted from vCard 3.0 `URL` lines to prevent populating EDS `E_CONTACT_HOMEPAGE_URL` with non-homepage URLs.
    - `url_eds_blog_video_and_custom_extensions_characterization`: characterizes unmapped EDS `X-EVOLUTION-BLOG-URL` (`E_CONTACT_BLOG_URL`) and `X-EVOLUTION-VIDEO-URL` (`E_CONTACT_VIDEO_URL`), verifying they are safely ignored on parse without corrupting JSContact models.
    - `url_query_parameters_punctuation_and_encoding_fidelity`: tests complex URIs containing query semicolons, commas, hashes, credentials, ports, IPv6 literals, and percent-encodings without backslash escaping per RFC 3986 and RFC 2426 §3.6.8.
    - `url_empty_absent_and_predicate_fidelity`: validates `states_link` and `maps_link_kind` predicates, empty URL line skipping, and unmodeled field preservation in `Link.extra`.
    - `url_and_calendar_properties_coexistence_and_slotting`: tests coexistence of `URL` (`E_CONTACT_HOMEPAGE_URL`), `CALURI` (`E_CONTACT_CALENDAR_URI`), and `FBURL` (`E_CONTACT_FREEBUSY_URL`) on the same card with distinct keys and clean roundtrips.
  - Enhanced property-based fuzzing strategies `arb_nickname` and `arb_link` in `rust/crates/jmap-vcard/tests/proptest_fuzz.rs`.
  - Updated `docs/VCARD-MAPPING.md` with Section 3.9 detailing `NICKNAME` and `URL` mapping architecture, cardinality decision, comma escaping vs text-list parsing, EDS slotting, and link kind filtering.
- **Calcard behaviour-difference findings & Product Decisions:**
  1. `NICKNAME` Cardinality & Comma Handling:
     - RFC 2426 §3.1.3 defines `NICKNAME` as a comma-separated text-list, but JSContact (RFC 9553 §2.2.2) models nicknames as a keyed map. `jmap-vcard` emits one line per entry so each entry carries its `X-JMAP-KEY`.
     - Inbound comma-separated lists (`NICKNAME:Rob,Robbie,Boss`) are parsed via `entry_text_list` into a single `Nickname` struct (`"Rob,Robbie,Boss"`). This matches EDS 3.52's behavior, which hands the whole value back as one string (`E_CONTACT_NICKNAME`) without splitting on commas. Re-emission escapes commas as `\,`, converging to a fixed point.
  2. `URL` (Links) Slotting & Kind Filtering:
     - In EDS, `E_CONTACT_HOMEPAGE_URL` maps to the first `URL` line in document order. Subsequent `URL` lines pass through intact in the raw vCard.
     - `jmap-vcard` strictly restricts vCard 3.0 `URL` emission to `kind: None` (plain website). RFC 9553 `kind: "contact"` (vCard 4.0 `CONTACT-URI`) and vendor kinds (`kind: "blog"`, `"video"`, `"feed"`) emit no `URL` line by design to prevent misrepresenting them as the contact's homepage in Evolution's UI.
     - `X-EVOLUTION-BLOG-URL` (`E_CONTACT_BLOG_URL`) and `X-EVOLUTION-VIDEO-URL` (`E_CONTACT_VIDEO_URL`) are EDS-specific extensions and remain unmapped by design in `jmap-vcard`.
  3. `calcard` preserves raw RFC 3986 URI punctuation (e.g. `;`, `,` in query parameters) without backslash-escaping, ensuring valid URIs on the wire format.
- **Gates ran:** `./ci/checks.sh` clean (REUSE 3.3 compliant, `cargo fmt`, `cargo clippy --all-targets --locked -- -D warnings`, `cargo test --locked`, `cargo deny check`).

## 2026-08-19 — Non-ASCII and CHARSET / ENCODING params fidelity & characterization (jmap-vcard)

- **AGY-TASKS sub-step:** 5. Non-ASCII and `CHARSET`/`ENCODING` params: verify non-ASCII names/values round-trip; characterize and pin `CHARSET=UTF-8` and legacy `ENCODING=QUOTED-PRINTABLE` values; log contract findings.
- **Changes:**
  - Added comprehensive characterization and round-trip test suites in `rust/crates/jmap-vcard/tests/mapping.rs`:
    - `non_ascii_multilingual_names_and_components_roundtrip`: verifies round-trip fidelity across diverse world writing systems and scripts (French accents, German umlauts/eszett, Spanish tildes, Icelandic thorn/eth, Polish crossed-L, Russian Cyrillic, Greek, Hebrew, Arabic RTL, Chinese Hanzi, Japanese Kanji/Kana, Korean Hangul, Hindi Devanagari, Vietnamese, and Emoji/symbols), confirming exact component extraction, valid line folding without UTF-8 code point splitting, and fixed-point roundtrip stability (`vcard2 == vcard3`).
    - `non_ascii_multilingual_organization_title_and_role_roundtrip`: tests multi-component `ORG` and `TITLE`/`ROLE` with non-ASCII text across French, German, Russian, and Japanese, verifying unit retention and default title kind normalization.
    - `non_ascii_structured_addresses_and_labels_roundtrip`: tests all 7 structured RFC 2426 `ADR` components and multi-line `LABEL` with non-ASCII characters across French, German, and Japanese addresses, verifying delimiter escaping and label pairing.
    - `non_ascii_notes_nicknames_categories_and_spouse_roundtrip`: tests multilingual paragraphs with special mathematical symbols (`∀x ∈ ℝ`), non-ASCII single/multiple nicknames, keyword tags, and spouse relations.
    - `inbound_vcard_charset_parameter_variations_and_normalization`: tests inbound vCards carrying `;CHARSET=UTF-8`, `;CHARSET=utf-8`, `;charset=UTF-8` across all properties, verifying accurate extraction into JSContact fields, outbound normalization to clean vCard 3.0 without redundant `CHARSET` parameters, and fixed-point convergence.
    - `inbound_vcard_quoted_printable_encoding_with_charset_utf8_and_latin1`: tests legacy vCard 2.1 / 3.0 `ENCODING=QUOTED-PRINTABLE` with `CHARSET=UTF-8`, `CHARSET=ISO-8859-1`, `CHARSET=WINDOWS-1252`, and without `CHARSET` (default Latin-1), verifying lossless hex octet decoding into JSContact fields, outbound normalization to clean vCard 3.0 UTF-8 format, and fixed-point stability.
    - `inbound_vcard_quoted_printable_soft_line_breaks_and_escaped_delimiters`: tests QP soft line breaks (`=\r\n` and `=\n`) and encoded delimiters (`=3D`, `=3B`, `=2C`, `=0D=0A`).
    - `inbound_vcard_encoding_parameter_8bit_7bit_and_base64_fidelity`: tests `ENCODING=8BIT` and `ENCODING=7BIT` text properties and `PHOTO;ENCODING=b;TYPE=JPEG:...` inline images.
  - Enhanced `rust/crates/jmap-vcard/tests/proptest_fuzz.rs`:
    - Added `CHARSET` (`UTF-8`, `utf-8`, `ISO-8859-1`, `WINDOWS-1252`) and `ENCODING` (`QUOTED-PRINTABLE`, `8BIT`, `7BIT`, `BASE64`) parameter variations to `arb_vcard_property_line`.
    - Added `prop_non_ascii_unicode_card_roundtrips_without_corruption` property test verifying fuzzing roundtrips of arbitrary non-ASCII names and notes.
  - Updated `docs/VCARD-MAPPING.md` with Section 4.7 documenting RFC 2426 §2.1.2 & §2.1.3 character set and transport contracts, Postel's law legacy compatibility, QUOTED-PRINTABLE decoding rules, and outbound normalization.
- **Calcard behaviour-difference findings & Product Decisions:**
  1. **vCard 3.0 Standard Contract (RFC 2426 §2.1.2 & §2.1.3)**:
     - RFC 2426 §2.1.2 mandates that vCard 3.0 is unconditionally UTF-8; the `CHARSET` parameter is not supported / deprecated for text properties.
     - RFC 2426 §2.1.3 mandates that vCard 3.0 uses 8-bit MIME transport encoding; `ENCODING=QUOTED-PRINTABLE`, `ENCODING=8BIT`, and `ENCODING=7BIT` are not supported on text properties. Binary properties (`PHOTO`) use `ENCODING=b` (or `b`).
     - `card_to_vcard` strictly adheres to RFC 2426 by emitting native UTF-8 strings directly without redundant `CHARSET` or `ENCODING` parameters on text properties.
  2. **Inbound Compatibility & Postel's Law**:
     - `calcard` automatically recognizes and accepts `CHARSET` (case-insensitively, including `UTF-8`, `ISO-8859-1`, `WINDOWS-1252`) on input.
     - `calcard` automatically decodes `ENCODING=QUOTED-PRINTABLE` byte sequences according to the specified `CHARSET` (or ISO-8859-1 default per RFC 2045) and unfolds soft line breaks (`=\r\n`), translating legacy vCard 2.1 data losslessly into standard JSContact strings.
  3. **Outbound Normalization & Convergence**:
     - Cards parsed from legacy `CHARSET` or `ENCODING=QUOTED-PRINTABLE` inputs are normalized on output into standard vCard 3.0 UTF-8 format, achieving fixed-point convergence (`vcard2 == vcard3`) on subsequent passes.
- **Gates ran:** `./ci/checks.sh` clean (REUSE 3.3 compliant, `cargo fmt`, `cargo clippy --all-targets --locked -- -D warnings`, `cargo test --locked`, `cargo deny check`).

## 2026-08-22 — Standard vCard 3.0 property preservation audit & characterization (jmap-vcard)

- **AGY-TASKS sub-step:** Batch 3, Item 1. Standard-property preservation audit (GEO, TZ, MAILER, PRODID, REV, SORT-STRING, CLASS, SOUND, LOGO).
- **Changes:**
  - Added comprehensive audit, characterization, and round-trip test suites in `rust/crates/jmap-vcard/tests/mapping.rs`:
    - `standard_vcard_properties_dropped_by_design_characterization_and_rationale`: tests inbound vCards containing all 9 standard unmapped vCard 3.0 properties (`GEO`, `TZ`, `MAILER`, `PRODID`, `REV`, `SORT-STRING`, `CLASS`, `SOUND`, `LOGO`) alongside all standard mapped contact fields (`UID`, `X-JMAP-UID`, `FN`, `N`, `NICKNAME`, `EMAIL`, `TEL`, `ADR`, `LABEL`, `ORG`, `TITLE`, `ROLE`, `NOTE`, `URL`, `CALURI`, `FBURL`, `PHOTO`, `CATEGORIES`, `BDAY`, `X-EVOLUTION-ANNIVERSARY`, `X-EVOLUTION-SPOUSE`, `X-JABBER`), confirming 100% field extraction for mapped properties, exclusion of unmapped properties from JSContact models, clean outbound vCard 3.0 emission, and fixed-point convergence (`card2 == card` and `vcard2 == vcard3`).
    - `standard_properties_individual_variations_and_parameters`: tests each of the 9 standard properties across individual variations: `GEO` decimal/signed/high-precision coordinates, `TZ` UTC offsets and IANA/abbreviated text names, `MAILER` client agent strings, `PRODID` FPI product identifiers, `REV` ISO-8601 timestamps, `SORT-STRING` text and delimiter-escaped strings, `CLASS` access classifications, `SOUND` inline base64 and remote URI audio, and `LOGO` inline base64 and remote URI graphics.
    - `jscontact_sound_and_logo_media_entries_server_preservation`: tests `ContactCard` instances with multi-entry `media` maps containing photos, sounds, logos, and documents, asserting that `states_media` strictly filters for `kind: Some("photo")`, `card_to_vcard` emits exactly one `PHOTO` line, and non-photo media entries remain preserved on the JMAP server without triggering destructive diffs during sync (`PatchObject`).
    - `standard_properties_case_insensitivity_and_empty_values`: tests lowercase/mixed-case property names (`geo:`, `tz:`, `mailer:`, `prodid:`, `rev:`, `sort-string:`, `class:`, `sound:`, `logo:`) and empty/whitespace-only values.
  - Enhanced `arb_vcard_property_line` in `rust/crates/jmap-vcard/tests/proptest_fuzz.rs` to generate all 9 standard properties during property-based fuzzing.
  - Updated `docs/VCARD-MAPPING.md` with:
    - Section 2 Master Property Mapping Table rows for `GEO`, `TZ`, `MAILER`, `PRODID`, `REV`, `SORT-STRING`, `CLASS`, `SOUND`, and `LOGO`.
    - Section 4.9 documenting the architectural justification, EDS UI state, JSContact representation, and sync safety rationale for each dropped property.
- **Calcard behaviour-difference findings & Product Decisions (Dropped-by-Design Rationale for Standard vCard 3.0 Properties):**
  1. `GEO` (RFC 2426 §3.4.2): Evolution contact editor lacks UI for coordinates; JSContact scopes coordinates to `Address.coordinates`. Dropping top-level `GEO` prevents creating bogus address associations; server `Address.coordinates` values are untouched by `PatchObject`.
  2. `TZ` (RFC 2426 §3.4.1): Evolution has no per-contact timezone UI; JSContact uses IANA names (`card.time_zone`) while vCard 3.0 uses offsets/abbreviations. Server-side `time_zone` is preserved untouched by `PatchObject`.
  3. `MAILER` (RFC 2426 §3.6.3): Deprecated/removed in vCard 4.0; legacy client metadata with no Evolution UI or JSContact mapping.
  4. `PRODID` (RFC 2426 §3.6.4): Generator metadata is owned by the exporting serializer. Foreign `PRODID` strings are not preserved across saves to prevent misattribution.
  5. `REV` (RFC 2426 §3.6.5): Revision timestamp is strictly owned by the authoritative JMAP server (`updated`). Emitting stale client timestamps would corrupt server revision tracking.
  6. `SORT-STRING` (RFC 2426 §3.6.7): Replaced in vCard 4.0 by `SORT-AS`. Evolution uses `X-EVOLUTION-FILE-AS`. JSContact `sortAs` properties ride in `extra` on the JMAP layer and are left untouched by `PatchObject`.
  7. `CLASS` (RFC 2426 §3.7.2): Deprecated access flag with no Evolution editor UI. Server privacy settings are preserved untouched by `PatchObject`.
  8. `SOUND` (RFC 2426 §3.6.6) & `LOGO` (RFC 2426 §3.5.3): Evolution contact editor only supports personal photo display/editing (`E_CONTACT_PHOTO`). `states_media` strictly filters for `kind: Some("photo")` on `PHOTO` lines. Inbound `SOUND`/`LOGO` lines are ignored to prevent misinterpreting them as personal photos. Server-side `sound` and `logo` entries in `card.media` remain safely preserved by `PatchObject`.
- **Gates ran:** `./ci/checks.sh` clean (REUSE 3.3 compliant, `cargo fmt`, `cargo clippy --all-targets --locked -- -D warnings`, `cargo test --locked`, `cargo deny check`, .deb package ctest).

## 2026-08-22 — Phone-TYPE completeness, MOBILE synonym & 19-field EDS matrix (jmap-vcard)

- **AGY-TASKS sub-step:** Batch 3, Item 2. Phone-TYPE completeness vs EDS.
- **Changes:**
  - Added `PHONE_FEATURE_TYPES` and adapted `read_phone_flags` in `rust/crates/jmap-vcard/src/contact.rs` to recognize both `TYPE=CELL` and real-world synonym `TYPE=MOBILE` (case-insensitively) as JSContact `features: {"mobile": true}`, while outbound emission normalizes to canonical RFC 2426 §3.3.1 `TYPE=CELL`.
  - Added comprehensive characterization and round-trip test suites in `rust/crates/jmap-vcard/tests/mapping.rs`:
    - `phone_mobile_type_synonym_and_permutations_characterization`: tests inbound `TEL;TYPE=MOBILE` across bare, lowercase, mixed-case, work/home contexts, preference parameters, and multi-feature permutations (`MOBILE,VOICE`, `MOBILE,FAX`), verifying `mobile` feature extraction, outbound normalization to `TYPE=CELL`, and fixed-point roundtrip stability (`vcard2 == vcard3`).
    - `phone_nineteen_eds_fields_complete_matrix_and_roundtrip`: verifies the full matrix of all 19 EDS phone fields from `libebook-contacts` 3.52 (`PRIMARY`, `BUSINESS`, `BUSINESS_2`, `BUSINESS_FAX`, `HOME`, `HOME_2`, `HOME_FAX`, `MOBILE` [both `CELL` and `MOBILE`], `PAGER`, `OTHER`, `OTHER_FAX`, `CAR`, `ISDN`, `CALLBACK`, `COMPANY`, `RADIO`, `TELEX`, `TTYTDD`, `ASSISTANT`), confirming accurate context/feature/pref extraction, preference-first sorting on emission (`TEL;TYPE=PREF`), and 100% roundtrip convergence.
    - `phone_whitespace_punctuation_and_uri_schemes_handling`: tests formatted numbers with spaces, dashes, dots, parentheses, `tel:` URIs (`tel:+1-555-0123;ext=100`), raw string preservation, and empty number omission.
    - `phone_multi_token_and_case_insensitive_type_matrix_roundtrip`: tests comma-separated vs repeated `TYPE` parameters, mixed casing, and feature narrowing precedence order (`CELL`/`MOBILE` > `PAGER` > `FAX` > `VOICE` > `VIDEO`).
  - Enhanced `arb_vcard_property_line` in `rust/crates/jmap-vcard/tests/proptest_fuzz.rs` to generate all phone type parameter variations (`CELL`, `MOBILE`, `PAGER`, `VOICE`, `FAX`, `VIDEO`, `CAR`, `ISDN`, `TTYTDD`, `WORK,CELL`, `HOME,MOBILE`).
  - Updated `docs/VCARD-MAPPING.md` with:
    - Section 2 Master Property Mapping Table row for `TEL` covering all 19 EDS fields and `MOBILE` synonym normalization.
    - Section 3.3 Master EDS Phone Mapping Matrix (19 Fields) detailing field IDs, UI slots, wire types, JSContact representations, outbound types, and resolution rules.
- **Calcard behaviour-difference findings & Product Decisions:**
  1. `calcard` automatically escapes semicolons in structured `TEL` values (such as `tel:+1-555-0123;ext=100` -> `tel:+1-555-0123\;ext=100`) on emission per RFC 2426 §2.4.2, and unescapes `\;` back to literal `;` upon reading.
  2. `calcard` serializes multiple parameters on `TEL` lines with comma-delimited `TYPE=...` grouping while parsing both comma-separated and repeated `TYPE` parameters cleanly.
  3. `MOBILE` vs `CELL` normalization: `TYPE=MOBILE` is widely used in the wild by Android, iOS, and Outlook vCard exporters. `jmap-vcard` accepts `MOBILE` on import as JSContact `mobile` feature and normalizes to standard RFC 2426 `TYPE=CELL` on export, ensuring lossless round-trips and fixed-point convergence.
- **Gates ran:** `./ci/checks.sh` clean (REUSE 3.3 compliant, `cargo fmt`, `cargo clippy --all-targets --locked -- -D warnings`, `cargo test --locked`, `cargo deny check`, .deb package ctest).

## 2026-08-22 — Remaining X-EVOLUTION-* fields mapping & CALURI/FBURL audit (jmap-vcard)

- **AGY-TASKS sub-step:** Batch 3, Item 3. Remaining X-EVOLUTION-* fields (`X-EVOLUTION-{MANAGER, ASSISTANT, BLOG-URL, VIDEO-URL}`) and `CALURI`/`FBURL` audit.
- **Changes:**
  - Audited `FBURL` and `CALURI`: Confirmed they are already fully mapped and tested in `jmap-vcard` to/from `card.calendars` with `kind: "calendar"` (`CALURI` ↔ `E_CONTACT_CALENDAR_URI`) and `kind: "freeBusy"` (`FBURL` ↔ `E_CONTACT_FREEBUSY_URL`).
  - Added constants `X_EVOLUTION_MANAGER`, `X_EVOLUTION_ASSISTANT`, `MANAGER_RELATION = "manager"`, `ASSISTANT_RELATION = "assistant"`, `X_EVOLUTION_BLOG_URL`, `X_EVOLUTION_VIDEO_URL` in `rust/crates/jmap-vcard/src/contact.rs`.
  - Added public predicates `states_manager` and `states_assistant` and exported them in `rust/crates/jmap-vcard/src/lib.rs`.
  - Updated `states_link` and `maps_link_kind` in `rust/crates/jmap-vcard/src/contact.rs` to support `None` (`URL`), `Some("blog")` (`X-EVOLUTION-BLOG-URL`), and `Some("video")` (`X-EVOLUTION-VIDEO-URL`), with leading/trailing whitespace and CR defense.
  - Adapted `card_to_vcard` in `rust/crates/jmap-vcard/src/contact.rs` to:
    - Emit `URL` (`kind: None`), `X-EVOLUTION-BLOG-URL` (`kind: Some("blog")`), and `X-EVOLUTION-VIDEO-URL` (`kind: Some("video")`) for links carrying `X-JMAP-KEY`.
    - Emit `X-EVOLUTION-SPOUSE`, `X-EVOLUTION-MANAGER`, and `X-EVOLUTION-ASSISTANT` lines for entries in `related_to` matching `states_spouse`, `states_manager`, and `states_assistant`.
  - Adapted `vcard_to_card` in `rust/crates/jmap-vcard/src/contact.rs` to:
    - Parse `"URL" | X_EVOLUTION_BLOG_URL | X_EVOLUTION_VIDEO_URL` into `card.links` with respective `kind` (`None`, `Some("blog")`, `Some("video")`).
    - Parse `X_EVOLUTION_SPOUSE | X_EVOLUTION_MANAGER | X_EVOLUTION_ASSISTANT` into `card.related_to[name]` with `relation: {"spouse": true}`, `{"manager": true}`, or `{"assistant": true}` when `names_a_person` holds. Multiple roles on a single person merge cleanly into one `Relation.relation` map.
  - Added comprehensive characterization, predicate, and round-trip test suites in `rust/crates/jmap-vcard/tests/mapping.rs`:
    - `evolution_manager_and_assistant_relations_roundtrip`: tests roundtrips of cards with spouse, manager, and assistant relations.
    - `evolution_blog_and_video_urls_links_roundtrip`: tests roundtrips of website, blog, and video links.
    - `evolution_remaining_x_properties_coexistence_and_predicates`: validates `states_manager`, `states_assistant`, `states_spouse`, `states_link` against valid person names (including non-ASCII), rejected names (empty, whitespace-padded, URIs, CR), and relation types.
    - `multiple_relations_on_single_person_and_multi_relation_cards`: tests individuals holding multiple simultaneous relations (e.g. manager + assistant, spouse + manager) merging on parse and roundtripping with 100% fidelity.
    - `evolution_links_and_relations_case_insensitivity_and_whitespace`: tests mixed-case property names (`x-evolution-manager:`, `X-Evolution-Blog-Url:`) and empty/whitespace line filtering.
  - Enhanced `rust/crates/jmap-vcard/tests/proptest_fuzz.rs`:
    - Updated `arb_link` to generate `video` link kinds.
    - Updated `arb_relation` to generate `manager` and `assistant` relation types.
    - Updated `arb_vcard_property_line` to generate `X-EVOLUTION-VIDEO-URL`.
  - Updated `docs/VCARD-MAPPING.md`:
    - Section 2 Master Property Mapping Table rows for `URL`, `X-EVOLUTION-BLOG-URL`, `X-EVOLUTION-VIDEO-URL`, `X-EVOLUTION-SPOUSE`, `X-EVOLUTION-MANAGER`, `X-EVOLUTION-ASSISTANT`.
    - Section 3.9 & Section 3.11 documenting relationship and link mappings, multiple-role merging, and URI identifier defenses.
    - Section 4.1 updating dropped-by-design unknown property list.
    - Section 5 Function Index with `states_manager` and `states_assistant`.
- **Calcard behaviour-difference findings & Product Decisions:**
  1. `calcard` correctly passes through custom `X-EVOLUTION-*` properties and parameter keys (`X-JMAP-KEY`) without mangling or dropping values.
  2. `X-EVOLUTION-BLOG-URL` and `X-EVOLUTION-VIDEO-URL` map naturally to JSContact `links` with standard kinds `"blog"` and `"video"` (RFC 9553 §2.6.3), preserving separation between personal homepages (`URL` / `E_CONTACT_HOMEPAGE_URL`) and multimedia links. Generic vendor properties (`X-BLOG-URL`, `X-VIDEO-URL`) without `X-EVOLUTION-` prefix remain safely ignored to avoid vendor drift.
  3. `X-EVOLUTION-MANAGER` and `X-EVOLUTION-ASSISTANT` map cleanly to JSContact `related_to` with standard relation types `"manager"` and `"assistant"` (RFC 9553 §2.1.8). The entity name serves as the map key (RFC 9555 §2.9.5). `names_a_person` prevents URN/URI identifiers from being rendered into Evolution's text fields. Multiple relations for the same person round-trip into separate lines and re-merge deterministically.
- **Gates ran:** `./ci/checks.sh` clean (REUSE 3.3 compliant, `cargo fmt`, `cargo clippy --all-targets --locked -- -D warnings`, `cargo test --locked`, `cargo deny check`, .deb package ctest).

## 2026-08-22 — EMAIL and ADR slot completeness & PREF interplay (jmap-vcard)

- **AGY-TASKS sub-step:** Batch 3, Item 4. EMAIL and ADR slot completeness vs EDS (EMAIL 1..4 + attribute list, ADR/LABEL 3 slots WORK/HOME/OTHER, PREF interplay).
- **Changes:**
  - Audited and characterized EDS `EMAIL` slotting: Evolution exposes 4 discrete string fields (`E_CONTACT_EMAIL_1` through `E_CONTACT_EMAIL_4`, fields 8..11) plus the full `E_CONTACT_EMAIL` (field 97) `GList` attribute list containing all `EMAIL` lines (1..=4 and 5+).
  - Audited and characterized EDS `ADR` and synthetic `LABEL` 3-slot matrix: `E_CONTACT_ADDRESS_WORK` / `E_CONTACT_ADDRESS_LABEL_WORK` (work slot), `E_CONTACT_ADDRESS_HOME` / `E_CONTACT_ADDRESS_LABEL_HOME` (home slot), and `E_CONTACT_ADDRESS_OTHER` / `E_CONTACT_ADDRESS_LABEL_OTHER` (other/unslotted).
  - Added comprehensive characterization and round-trip test suites in `rust/crates/jmap-vcard/tests/mapping.rs`:
    - `email_four_slots_and_attribute_list_matrix_roundtrip`: tests cards with 1..=6 emails, verifying promotion of preferred email (`pref: 1`) to `E_CONTACT_EMAIL_1`, positional slotting across `E_CONTACT_EMAIL_1..4`, preservation of lines 5+ in `E_CONTACT_EMAIL` attribute list, unranked email sorting by key, unkeyed vCard key allocation (`e1..e5`), and fixed-point roundtrip stability (`card2 == card` and `emitted2 == emitted`).
    - `address_three_label_slots_work_home_other_and_adr_pairing_matrix`: tests the full matrix across WORK (`TYPE=WORK`), HOME (`TYPE=HOME`), and OTHER (`TYPE=OTHER` / bare) slots with structured components and standalone labels, label-only addresses, ADR-only addresses, mixed slotting, in-place synthetic label modifications, preference-first sorting with `TYPE=PREF`, and unkeyed context pairing fallback in `label_entry`.
    - `email_and_address_label_edge_cases_and_parameter_permutations`: tests escaped delimiters (`\n`, `\,`, `\;`, `\\`), mixed-case property/parameter names (`email;type=work,pref:`, `adr;type=work,pref:`, `label;type=other:`), and empty/whitespace line omission (`EMAIL:`, `ADR:;;;;;;`, `LABEL:`).
  - Updated `docs/VCARD-MAPPING.md` with:
    - Section 3.3 Master EDS Email Mapping Matrix (4 Slots + Attribute List) detailing field IDs, UI slots, wire types, JSContact representations, outbound types, and resolution rules.
    - Section 3.4 Master EDS Address & Label Mapping Matrix (3 Slots + 3 Synthetic Labels) covering WORK, HOME, and OTHER slots, 21 subfields, and synthetic label pairing mechanics.
- **Calcard behaviour-difference findings & Product Decisions:**
  1. **EMAIL Positional Slotting & Attribute List in EDS**:
     - EDS maps incoming `EMAIL` lines 1..4 to `E_CONTACT_EMAIL_1..4` and all lines (1..=4 and 5+) to `E_CONTACT_EMAIL`.
     - `card_to_vcard` sorts emails by `(pref.unwrap_or(u32::MAX), key)` on emission so the lowest `pref` rank lands in `E_CONTACT_EMAIL_1` (primary email), followed by unranked emails in deterministic key order.
  2. **Address & Synthetic Label 3-Slot Pairing**:
     - EDS models address labels as synthetic string fields (`E_CONTACT_ADDRESS_LABEL_WORK`, `_HOME`, `_OTHER`). On import, `label_entry` matches standalone `LABEL` lines to preceding `ADR` entries using `X-JMAP-KEY` or context matching (`TYPE=WORK` -> work ADR, `TYPE=HOME` -> home ADR, `TYPE=OTHER`/bare -> other ADR).
     - This context-matching fallback prevents spurious duplicate address entries when EDS rebuilds synthetic label fields and strips custom parameters.
     - Outbound emission pairs each structured `ADR` with its matching standalone `LABEL` in sorted `(address_pref, key)` order, ensuring primary preferred addresses and labels land in the appropriate Evolution editor slots.
- **Gates ran:** `./ci/checks.sh` clean (REUSE 3.3 compliant, `cargo fmt`, `cargo clippy --all-targets --locked -- -D warnings`, `cargo test --locked`, `cargo deny check`, .deb package ctest).

## 2026-08-22 — vCard 2.1 legacy import tolerance & asymmetric compatibility (jmap-vcard)

- **AGY-TASKS sub-step:** Batch 3, Item 5. vCard 2.1 legacy import tolerance (ENCODING=QUOTED-PRINTABLE values, bare type words, CHARSET= params, import tolerance vs strict 3.0 export).
- **Changes:**
  - Enhanced `entry_has_type` in `rust/crates/jmap-vcard/src/contact.rs` to recognize both standard `TYPE=value` parameters and bare vCard 2.1 type parameter names (e.g. bare `WORK`, `HOME`, `CELL`, `MOBILE`, `VOICE`, `FAX`, `PAGER`, `PREF`, `POSTAL`, etc.) matching the token case-insensitively.
  - Enhanced `read_photo` in `rust/crates/jmap-vcard/src/contact.rs` with `is_known_image_subtype` to infer image format subtypes (`JPEG`, `GIF`, `PNG`, `BMP`, `TIFF`, `WEBP`) from bare parameter names when explicit `TYPE=` is omitted (e.g. `PHOTO;JPEG;ENCODING=BASE64:`), constructing standard `data:image/<subtype>;base64,...` data URIs.
  - Extended `arb_vcard_property_line` and `arb_raw_vcard` in `rust/crates/jmap-vcard/tests/proptest_fuzz.rs` to fuzz vCard 2.1 bare parameters and `VERSION:2.1` envelopes during property-based fuzzing.
  - Added comprehensive characterization and round-trip test suites in `rust/crates/jmap-vcard/tests/mapping.rs`:
    - `vcard_21_outlook_representative_fixture_import_and_normalization`: tests representative real-world vCard 2.1 exported by legacy Microsoft Outlook (`VERSION:2.1`, bare `TEL;WORK;VOICE`, `TEL;HOME;VOICE`, `TEL;CELL;VOICE`, `TEL;WORK;FAX`, `EMAIL;PREF;INTERNET`, `ADR;WORK;PREF` / `LABEL;WORK;PREF` in QP, `NOTE;CHARSET=UTF-8;ENCODING=QUOTED-PRINTABLE` with German umlauts and soft breaks, `PHOTO;JPEG;ENCODING=BASE64`, `BDAY`, `URL`, `REV`), verifying 100% extraction, outbound normalization to strict vCard 3.0 UTF-8, and fixed-point roundtrip stability (`export2 == export3` and `card2 == card3`).
    - `vcard_21_feature_phone_nokia_sony_ericsson_fixtures_import`: tests feature phone fixtures with bare `TEL` types (`HOME`, `WORK`, `CELL`, `MOBILE`, `FAX`, `PAGER`), `EMAIL;INTERNET`, soft-wrapped QP notes, and safe omission of unmapped `SOUND;WAVE;BASE64` audio entries.
    - `vcard_21_legacy_charsets_iso_8859_1_and_windows_1252_import`: tests legacy character set decoding across German (`CHARSET=ISO-8859-1`) and French (`CHARSET=WINDOWS-1252` with Euro sign `=80`) into native UTF-8 strings, verifying outbound normalization to clean vCard 3.0 with no redundant `CHARSET` or `ENCODING` parameters.
    - `vcard_21_bare_type_words_and_combinations_matrix`: tests exhaustive matrix of bare parameter type combinations across telephony, email, and postal address contexts, features, and preference flags.
    - `vcard_21_photo_formats_and_encoding_permutations`: tests `PHOTO;JPEG;ENCODING=BASE64`, `PHOTO;GIF;BASE64`, `PHOTO;PNG;ENCODING=BASE64`, and `PHOTO;TYPE=JPEG;ENCODING=BASE64`, verifying subtype inference and standard vCard 3.0 `PHOTO;ENCODING=b;TYPE=...` re-emission.
    - `vcard_21_quoted_printable_soft_line_breaks_and_continuation`: tests QP soft line wrapping (`=\r\n`, `=\n`) and hex byte decoding (`=3D`, `=3B`, `=2C`, `=0D=0A`) across multi-line notes and structured name/organization fields.
  - Updated `docs/VCARD-MAPPING.md` with Section 4.10 documenting the accepted vCard 2.1 subset, Postel's law asymmetric compatibility contract, and fixed-point stability invariants.
- **Calcard behaviour-difference findings & Product Decisions:**
  1. **vCard 2.1 Asymmetric Import Tolerance Contract**:
     - Inbound parsing (`vcard_to_card`) adopts Postel's Law: accepts legacy `VERSION:2.1`, bare type parameter words (`TEL;HOME:`), legacy character sets (`CHARSET=ISO-8859-1`, `CHARSET=WINDOWS-1252`), and `ENCODING=QUOTED-PRINTABLE` byte sequences.
     - Outbound serialization (`card_to_vcard`) is strictly canonical RFC 2426 vCard 3.0 in native UTF-8, with 75-octet folding, standard `TYPE=` parameter grouping, and RFC 2426 §2.4.2 delimiter escaping (`\n`, `\,`, `\;`, `\\`).
     - Legacy parameters (`CHARSET`, `ENCODING=QUOTED-PRINTABLE`, `INTERNET`) are never emitted.
  2. `calcard` automatically decodes Quoted-Printable byte sequences according to the declared `CHARSET` (or ISO-8859-1 default) and unfolds QP soft line breaks (`=\r\n`) seamlessly.
  3. `entry_has_type` evaluates both `TYPE=value` parameters and bare parameter names matching the target type name, enabling seamless interoperability with 2.1 exporters without breaking standard 3.0 parameter matching.
- **Gates ran:** `./ci/checks.sh` clean (REUSE 3.3 compliant, `cargo fmt`, `cargo clippy --all-targets --locked -- -D warnings`, `cargo test --locked`, `cargo deny check`, .deb package ctest).

## 2026-08-22 — Round-trip fixpoint property test & oscillation regression net (jmap-vcard)

- **AGY-TASKS sub-step:** Batch 3, Item 6. Round-trip fixpoint property test (vCard→EContact→vCard→EContact→vCard reaches fixpoint by second export: export₂ == export₃ byte-identical, proptest shrinkage oscillation namer).
- **Changes:**
  - Added multi-pass fixpoint roundtrip characterization and oscillation diagnostic tests in `rust/crates/jmap-vcard/tests/mapping.rs`:
    - `fixpoint_roundtrip_characterization_and_oscillation_diagnostics`: tests multi-stage translation lifecycle (vCard₁ -> Card₁ -> vCard₂ [Export₁] -> Card₂ [EContact₂] -> vCard₃ [Export₂] -> Card₃ [EContact₃] -> vCard₄ [Export₃]) on a comprehensive multi-property fixture contact covering all 19 phone fields, 3 address slots, multi-component orgs, multi-role relations, custom links, anniversaries, IM services, and dropped-by-design standard properties, asserting `Export₂ == Export₃` byte-identity and `Card₂ == Card₃` structural identity.
    - `fixpoint_convergence_across_all_contact_property_domains_matrix`: tests discrete property domains in isolation and in combination (names, nicknames, emails with 4 slots & attribute list, phones with 19 types, addresses with 3 slots & synthetic labels, organizations with 4+ units, titles/roles, notes with escapes, anniversaries with partial dates, links/blogs/videos, calendars/freeBusy, photos, online services, relations spouse/manager/assistant, categories/keywords).
  - Added oscillation diagnosis helpers in `rust/crates/jmap-vcard/tests/proptest_fuzz.rs`:
    - `identify_oscillating_vcard_property`: isolates exact property name and differing lines when vCard exports oscillate (`Export₂ != Export₃`).
    - `identify_oscillating_card_field`: isolates exact JSContact field when deserialized cards oscillate (`Card₂ != Card₃`).
    - `assert_vcard_fixpoint` and `assert_card_fixpoint`: custom proptest assertion helpers providing detailed oscillation failure diagnostics.
  - Enhanced `arb_phone` in `rust/crates/jmap-vcard/tests/proptest_fuzz.rs` to fuzz all 19 EDS phone feature combinations (`car`, `isdn`, `ttytdd`, `voice+mobile`, `voice+fax`, `cell+video`, `other` context).
  - Added 9 domain-focused fixpoint proptests in `rust/crates/jmap-vcard/tests/proptest_fuzz.rs`: `prop_fixpoint_telephony_domain`, `prop_fixpoint_email_domain`, `prop_fixpoint_address_and_label_domain`, `prop_fixpoint_organization_domain`, `prop_fixpoint_relation_domain`, `prop_fixpoint_anniversary_domain`, `prop_fixpoint_categories_domain`, `prop_fixpoint_notes_escaping_domain`, `prop_fixpoint_online_services_domain`.
  - Updated `docs/VCARD-MAPPING.md` with Section 6 documenting the multi-stage roundtrip contract, standing fixpoint invariants (`Export₂ == Export₃`, `Card₂ == Card₃`), and the proptest regression harness.
- **Calcard behaviour-difference findings & Product Decisions:**
  1. **Multi-Stage Fixpoint Stability Invariant**:
     - `vCard₁ -> Card₁ -> vCard₂ (Export₁) -> Card₂ (EContact₂) -> vCard₃ (Export₂) -> Card₃ (EContact₃) -> vCard₄ (Export₃)`.
     - Export₁ normalizes legacy formats (vCard 2.1, QP encoding, bare parameters, foreign keying) into standard RFC 2426 vCard 3.0 with allocated `X-JMAP-KEY` parameters.
     - By Export₂ (pass 2), every property representation is canonicalized and stabilized: Export₂ is byte-identical to Export₃ (`Export₂ == Export₃`), and Card₂ is structurally identical to Card₃ (`Card₂ == Card₃`).
  2. **Oscillation Diagnostic Net**:
     - The `identify_oscillating_vcard_property` / `identify_oscillating_card_field` helpers inspect line-by-line diffs during proptest shrinkage, immediately naming the property name (`TEL`, `ADR`, `ORG`, `CATEGORIES`, `NOTE`, etc.) that oscillates.
- **Gates ran:** `./ci/checks.sh` clean (REUSE 3.3 compliant, `cargo fmt`, `cargo clippy --all-targets --locked -- -D warnings`, `cargo test --locked`, `cargo deny check`, .deb package ctest).

## 2026-08-22 — Round-trip fixpoint bug fix & trailing whitespace regression suite (jmap-vcard)

- **AGY-TASKS sub-step:** Batch 4, Item 1. Fix the filed round-trip fixpoint bug (`docs/BACKLOG.md` trailing-whitespace round-trip nit reproduction & pin minimal input regression).
- **Changes:**
  - Fixed full name emission oscillation in `rust/crates/jmap-vcard/src/contact.rs`: filtered empty string `name.full` in `card_to_vcard` before calling `derive_full`, preventing oscillation between `full: None` and `full: Some("...")` across roundtrip passes when structured components are present alongside an empty full name string.
  - Added comprehensive characterization and named regression test suites in `rust/crates/jmap-vcard/tests/mapping.rs`:
    - `trailing_whitespace_filed_bug_minimal_input_named_regression`: pins the exact minimal reproduction input recorded in `docs/BACKLOG.md` (`BEGIN:VCARD\r\nVERSION:3.0\r\nNICKNAME;ENCODING=b:! \r\nEND:VCARD\r\n`), asserting 100% fixed-point convergence (`Export₂ == Export₃` and `Card₂ == Card₃`).
    - `trailing_whitespace_on_property_values_across_all_domains_fixpoint`: tests trailing whitespace across all property domains (`FN`, structured `N`, `NICKNAME`, `EMAIL`, `TEL`, `ADR`, `LABEL`, `ORG`, `TITLE`, `ROLE`, `NOTE`, `URL`, `CATEGORIES`, `X-EVOLUTION-SPOUSE`, `X-EVOLUTION-MANAGER`, `X-EVOLUTION-ASSISTANT`, `X-EVOLUTION-BLOG-URL`, `X-EVOLUTION-VIDEO-URL`), validating fixed-point stability without loss or whitespace oscillation.
    - `trailing_whitespace_only_property_values_fixpoint_and_filtering`: tests cards with whitespace-only values across all mapped properties, ensuring clean parsing, safe emission without property corruption, and fixed-point convergence.
    - `trailing_whitespace_with_vcard_21_and_quoted_printable_fixpoint`: tests vCard 2.1 envelopes and Quoted-Printable values with trailing whitespace, verifying outbound normalization to vCard 3.0 and fixed-point stability.
    - `name_with_empty_full_string_and_components_reaches_fixed_point`: verifies that a contact card containing an empty `name.full` string (`Some("")`) alongside structured name components derives the full name consistently on export without oscillating.
  - Updated `docs/VCARD-MAPPING.md` with Section 6.3 documenting trailing whitespace significance, whitespace-only filtering, and legacy parameter handling.
- **Calcard behaviour-difference findings & Product Decisions:**
  1. **Trailing Whitespace Significance (RFC 6350 §3.3 / RFC 2426 §2)**:
     - Trailing whitespace within text property values is preserved verbatim during vCard parsing and emission, preventing data truncation while reaching exact byte-identical fixpoints (`Export₂ == Export₃`).
  2. **Empty String Full Name Normalization**:
     - JSContact cards with `name.full: Some("")` and structured components normalize to the derived full name on initial export, stabilizing immediately on Pass 1 and guaranteeing structural fixed-point equality (`Card₂ == Card₃`).
  3. **Minimal BACKLOG Bug Resolution**:
     - The minimal input recorded in `docs/BACKLOG.md` (`NICKNAME;ENCODING=b:! `) is parsed safely: binary text values on non-binary properties are filtered out by `entry_text_list`, producing a normalized vCard on Export₁ that reaches byte-identical fixed-point stability on Export₂ (`Export₂ == Export₃`).
- **Gates ran:** `./ci/checks.sh` clean (REUSE 3.3 compliant, `cargo fmt`, `cargo clippy --all-targets --locked -- -D warnings`, `cargo test --locked`, `cargo deny check`, .deb package ctest).

## 2026-08-22 — FILE-AS and X-EVOLUTION-FILE-AS mapping & SORT-STRING relationship (jmap-vcard)

- **AGY-TASKS sub-step:** Batch 4, Item 4. `FILE-AS` mapping (EDS `E_CONTACT_FILE_AS` / `X-EVOLUTION-FILE-AS`, inbound `FILE-AS` and `X-FILE-AS` synonyms, `states_file_as` predicate, and documented relationship to `SORT-STRING` without round-trip clobbering).
- **Changes:**
  - Implemented `X-EVOLUTION-FILE-AS` mapping in `rust/crates/jmap-vcard/src/contact.rs`:
    - Inbound: `read_name` extracts `X-EVOLUTION-FILE-AS`, `FILE-AS`, or `X-FILE-AS` text into `Name.extra["fileAs"]` (or creates `Name` if absent).
    - Outbound: `card_to_vcard` emits `X-EVOLUTION-FILE-AS: <fileAs>` if `fileAs` / `file_as` is present on `Name.extra` or `ContactCard.extra` and non-empty.
    - Predicate: Added and exported `states_file_as(name: Option<&Name>) -> bool` to check if a valid, non-empty file-as string is stated.
  - Re-exported `states_file_as` in `rust/crates/jmap-vcard/src/lib.rs`.
  - Added comprehensive unit and round-trip test suites in `rust/crates/jmap-vcard/tests/mapping.rs`:
    - `file_as_basic_evolution_x_property_roundtrip`: verifies `X-EVOLUTION-FILE-AS` parsing, emission, predicate evaluations, and multi-pass fixed-point convergence (`Export₁ == Export₂`, `Export₂ == Export₃`, `Card₂ == Card₃`).
    - `file_as_inbound_synonyms_file_as_and_x_file_as`: tests `FILE-AS`, `X-FILE-AS`, and case-insensitive property names normalizing outbound to `X-EVOLUTION-FILE-AS`.
    - `file_as_and_sort_string_coexistence_without_clobbering`: validates coexistence of `fileAs` (`X-EVOLUTION-FILE-AS`) and `sortAs` (`SORT-STRING`) without mutual clobbering.
    - `file_as_escaping_special_characters_and_unicode`: asserts delimiter escaping (commas, semicolons, backslashes) and Unicode preservation on round-trips.
    - `file_as_card_level_and_name_level_emission`: tests name-level and card-level `fileAs`/`file_as`, whitespace filtering, and standalone cards with only file-as.
  - Updated `docs/VCARD-MAPPING.md`:
    - Section 2 Master Property Mapping Table row for `X-EVOLUTION-FILE-AS` / `FILE-AS` / `X-FILE-AS`.
    - Section 4.9 Item 6 clarifying `SORT-STRING` vs `X-EVOLUTION-FILE-AS` storage and round-trip coexistence.
- **Calcard behaviour-difference findings & Product Decisions:**
  1. **Batch 4 Item 2 Lane Collision Discovery**:
     - Attempting to change `TEL;TYPE=WORK,VOICE,FAX` emission in `jmap-vcard` revealed a direct coupling in `jmap-book-sync/tests/save.rs` (`editing_preserves_the_feature_a_line_could_state_only_one_of` and `moving_a_number_to_another_kind_of_phone_field_reclassifies_it`), which explicitly assert `vcard.contains("TEL;X-JMAP-KEY=p1;TYPE=WORK,FAX:")`.
     - Because `jmap-book-sync` is outside the Antigravity lane (owned by Claude on master), modifying `jmap-vcard` to emit dual-role `WORK,VOICE,FAX` causes cross-crate test breakage unless `jmap-book-sync` is modified. In accordance with the lane collision rules ("If a task would need a file outside jmap-vcard, log it as a finding in docs/AGY-LOG.md and skip it"), Item 2 is logged and deferred to a coordinated master merge, and Item 4 was worked instead.
  2. **`FILE-AS` and `SORT-STRING` Coexistence**:
     - Evolution's "File Under" field maps to `E_CONTACT_FILE_AS` and is serialized as `X-EVOLUTION-FILE-AS`.
     - In JSContact, `fileAs` is stored on `Name.extra["fileAs"]`. Standard vCard 3.0 `SORT-STRING` (family sort string) corresponds to JSContact `sortAs` (`Name.extra["sortAs"]`).
     - Because they reside under distinct keys on the JSContact layer (`fileAs` vs `sortAs`), neither clobbers the other during synchronization or vCard import/export.
## 2026-08-22 — Apple-style property groups & X-ABLabel semantic mapping (jmap-vcard)

- **AGY-TASKS sub-step:** Batch 4, Item 6. Apple-style property groups (`item1.TEL` + `item1.X-ABLabel`, as iCloud and macOS exporters emit): grouped properties import without loss, labels map to the closest TYPE/EDS slot or extra["label"], extended relations (X-ABRELATEDNAMES) and dates (X-ABDATE) map cleanly, and round-trips reach fixed-point stability.
- **Changes:**
  - Added helper `clean_apple_label(raw: &str) -> &str` in `rust/crates/jmap-vcard/src/contact.rs` to unwrap Apple-style `_$!<LabelName>!$_` markers into clean label names or trim custom labels.
  - Adapted `vcard_to_card` in `rust/crates/jmap-vcard/src/contact.rs`:
    - Collects group labels from `entry.group` with `X-ABLabel` properties in an initial scan.
    - `EMAIL`: maps group labels (`Work`/`School` -> `contexts: {"work": true}`, `Home` -> `contexts: {"private": true}`, custom -> `email.extra["label"]`).
    - `TEL`: maps group labels (`Mobile`/`Cell`/`iPhone` -> `mobile`, `Pager` -> `pager`, `WorkFAX`/`HomeFAX`/`Fax` -> `fax`, `Main` -> `voice` + `work`, `Work`/`Home` -> contexts, custom -> `phone.extra["label"]`).
    - `ADR`: updated `read_address` to consume `group_label` and map `Work`/`Home` contexts and custom `extra["label"]`.
    - `URL`: maps group labels (`HomePage` -> `kind: None`, `Blog` -> `kind: Some("blog")`, `Work`/`Home` -> contexts, custom -> `link.extra["label"]`).
    - Extended relations: added support for `X-ABRELATEDNAMES` / `X-AB-RELATED-NAMES` with companion group label mapping to `spouse`/`partner`, `manager`, `assistant`, or custom relations in `card.related_to`. Ungrouped/unlabelled `X-ABRelatedNames` are safely skipped.
    - Extended dates: added support for `X-ABDATE` / `X-AB-DATE` with companion group label mapping to `wedding` (anniversary), `birth`, or custom anniversary kinds in `card.anniversaries`.
  - Added comprehensive fixture-driven unit and round-trip test suites in `rust/crates/jmap-vcard/tests/mapping.rs`:
    - `apple_property_groups_representative_icloud_fixture_import_and_roundtrip`: verifies full representative iCloud/macOS contact card with multi-type emails, mobile/work/fax/main phones, work/home addresses, homepage link, spouse/manager/assistant relations, anniversary dates, notes, and categories.
    - `apple_property_groups_custom_labels_and_extended_relations`: verifies custom label preservation in `extra["label"]`, WorkFAX, Pager, custom relation types (`Partner`, `Colleague`), custom date kinds (`First Met`).
    - `apple_property_groups_variations_and_boundary_cases`: verifies case-insensitivity in labels (`x-ablabel`, `X-ABLABEL`), unescaped labels, orphaned `X-ABLabel` lines, and fixed-point roundtrip stability.
  - Enhanced proptest structure-aware fuzzing in `rust/crates/jmap-vcard/tests/proptest_fuzz.rs`:
    - Added group prefixes (`item1.`, `item2.`, `itemA.`) and Apple properties (`X-ABLabel`, `X-ABRELATEDNAMES`, `X-ABDATE`) to `arb_vcard_property_line`.
  - Updated `docs/VCARD-MAPPING.md`:
    - Added Master Property Mapping Table rows for `itemN.PROPERTY`, `X-ABLabel`, `X-ABRELATEDNAMES`, `X-ABDATE`.
    - Added Section 4.11 detailing Apple Property Groups & `X-ABLabel` Semantic Mapping.
- **Calcard behaviour-difference findings & Product Decisions:**
  1. **Apple Group Grammar (RFC 2426 §2.1.1)**:
     - `calcard::vcard::parser` parses property group prefixes (`itemN.PROPERTY`) into `entry.group: Some("itemN")` and sets `entry.name` to the standard property name (e.g. `VCardProperty::Tel`, `VCardProperty::Email`, `VCardProperty::Other("X-ABLabel")`).
     - This clean separation allows `jmap-vcard` to collect `X-ABLabel` annotations indexed by group name and pair them with standard properties, extended relations (`X-ABRELATEDNAMES`), and extended dates (`X-ABDATE`).
  2. **Standard vs. Custom Label Mapping**:
     - Standard Apple markers (`_$!<Work>!$_`, `_$!<Home>!$_`, `_$!<Mobile>!$_`, `_$!<WorkFAX>!$_`, `_$!<Main>!$_`, `_$!<HomePage>!$_`, `_$!<Spouse>!$_`, `_$!<Anniversary>!$_`) map directly to native JSContact context/feature/relation fields and EDS slots.
     - Custom/user-defined labels (e.g. `item1.X-ABLabel:Direct Line`) are preserved in `extra["label"]` on JSContact `ContactPhone`, `ContactEmail`, `Address`, `Link`, and as named relation/anniversary types.
  3. **Outbound Normalization & Round-Trip Fixpoint**:
     - Outbound vCard serialization emits canonical RFC 2426 vCard 3.0 properties (`TYPE=WORK,CELL`, `X-EVOLUTION-SPOUSE`, `X-EVOLUTION-ANNIVERSARY`).
     - Re-parsing emitted vCards achieves exact byte-identical and structural fixed-point convergence (`Export₂ == Export₃` and `Card₂ == Card₃`).
- **Gates ran:** `./ci/checks.sh` clean (REUSE 3.3 compliant, `cargo fmt`, `cargo clippy --all-targets --locked -- -D warnings`, `cargo test --locked`, `cargo deny check`, .deb package ctest).

## 2026-08-22 — X-TWITTER, X-SIP & IM URI-scheme long tail mapping (jmap-vcard)

- **AGY-TASKS sub-step:** Batch 4, Item 3. `X-TWITTER`, `X-SIP`, and IM URI-scheme long tail (audit scheme table, add missing scheme aliases for mapped IM services, canonical URI resolution, action query rejection, and X-TWITTER / X-SIP unslotted attribute list characterization and server-side preservation).
- **Changes:**
  - Extended `SERVICE_SCHEMES` in `rust/crates/jmap-vcard/src/contact.rs` with recognized URI scheme aliases for mapped IM services: `Jabber` (`xmpp`, `jabber`), `Google Talk` (`xmpp`, `gtalk`), `AIM` (`aim`, `aol`), `Gadu-Gadu` (`gg`, `gadugadu`, `gadu`), `GroupWise` (`groupwise`, `novell`), `ICQ` (`icq`), `MSN` (`msn`, `msnim`), `Matrix` (`matrix`), `Skype` (`skype`), `Yahoo` (`yahoo`, `ymsgr`).
  - Audited `online_service_uri` to ensure canonical URI scheme selection for all 10 mapped services.
  - Hardened and characterized `plain_handle` action/query URI rejection (`aim:goim?screenname=...`, `msnim:chat?contact=...`, `ymsgr:sendim?...`, `icq:message?uin=...`, `skype:echo123?call`, `gtalk:chat?jid=...`, `matrix:u/vera:...`).
  - Added characterization and TDD test suites in `rust/crates/jmap-vcard/tests/mapping.rs`:
    - `im_scheme_long_tail_aliases_and_canonical_uri_resolution`: verifies canonical URI generation across all 10 services and handle extraction across all 18 URI scheme aliases with 100% roundtrip fidelity.
    - `im_scheme_action_query_and_invalid_handle_rejection`: validates refusal of action/query URIs across all services.
    - `twitter_sip_and_unslotted_social_services_characterization_and_rationale`: characterizes `X-TWITTER` and `X-SIP` unslotted attribute list semantics in EDS (`E_CONTACT_IM_TWITTER` 135 and `E_CONTACT_SIP` 127) alongside unslotted social platforms (Telegram, Discord, Signal, WhatsApp, Mastodon, IRC), verifying they are safely omitted on vCard 3.0 emission to protect server-side state via `PatchObject`, and safely ignored on vCard import without corrupting mapped services.
  - Enhanced `arb_online_service` in `rust/crates/jmap-vcard/tests/proptest_fuzz.rs` to fuzz all 18 URI scheme aliases during property testing.
  - Updated `docs/VCARD-MAPPING.md`:
    - Section 3.7 Master EDS Instant Messaging Mapping Matrix with 10 services, 60 slots, 18 URI schemes, handle grammar, and resolution notes.
    - Section 4.12 detailing IM, social networks, `X-TWITTER`/`X-SIP` unslotted architecture, and action rejection.
- **Calcard behaviour-difference findings & Product Decisions:**
  1. **EDS IM Slotting Architecture vs Unslotted Lists (`EContactAttrList`)**:
     - Slotted IM services (`AIM`, `GADUGADU`, `GOOGLE_TALK`, `GROUPWISE`, `ICQ`, `JABBER`, `MSN`, `MATRIX`, `SKYPE`, `YAHOO`) map onto 60 discrete per-slot string fields in EDS (`_HOME_1..3`, `_WORK_1..3`).
     - `E_CONTACT_IM_TWITTER` (135) and `E_CONTACT_SIP` (127) are modeled in EDS as `EContactAttrList` (`GList*` of `char*`) without `HOME`/`WORK` slotting.
     - Evolution's contact editor provides UI exclusively for slotted services. In `jmap-vcard`, `Twitter` and `SIP` are deliberately unmapped on vCard 3.0 emission, ensuring they remain safely preserved on the server via `jmap-book-sync`'s `PatchObject` (verified by `unmapped_or_unslotted_services_are_preserved_across_saves`).
  2. **Canonical Schemes & URI Scheme Aliases**:
     - `online_service_uri` deterministically selects the primary IANA/RFC scheme for each service (`xmpp:`, `aim:`, `gg:`, `groupwise:`, `icq:`, `msn:`, `matrix:`, `skype:`, `yahoo:`).
     - `online_service_handle` recognizes legacy and proprietary aliases (`jabber:`, `gtalk:`, `aol:`, `gadugadu:`, `gadu:`, `novell:`, `msnim:`, `ymsgr:`) to extract bare handles seamlessly from third-party vCards and JMAP `uri` fields.
  3. **Action and Query URI Rejection**:
     - URIs containing action verbs or parameters (`?call`, `?chat`, `?screenname=...`, `/u/...`) are rejected by `plain_handle` to prevent corrupting handle fields or creating fake handles.
- **Gates ran:** `./ci/checks.sh` clean (REUSE 3.3 compliant, `cargo fmt`, `cargo clippy --all-targets --locked -- -D warnings`, `cargo test --locked`, `cargo deny check`, .deb package ctest).

## 2026-08-22 — LOGO & KEY field audit, characterization & server preservation (jmap-vcard)

- **AGY-TASKS sub-step:** Batch 4, Item 5. Promote `LOGO` and `KEY` from preserved blobs to real fields (audit exact EDS field semantics first — `E_CONTACT_LOGO` and `E_CONTACT_X509_CERT`/`E_CONTACT_PGP_CERT`; evaluate UI presence, sync layer interaction, and lane boundaries; characterization tests and server preservation).
- **Changes:**
  - Audited `LOGO` (RFC 2426 §3.5.3 / `E_CONTACT_LOGO` ID 95) and `KEY` (RFC 2426 §3.7.2 / `E_CONTACT_X509_CERT` ID 109 / `E_CONTACT_PGP_CERT` ID 110) in EDS (`eds-sys/tests/contacts.rs`), JSContact (RFC 9553 §2.6.4 `media` and §2.7.1 `cryptoKeys`), and `jmap-book-sync` (`patch.rs` / `save.rs`).
  - Added comprehensive characterization, boundary, and fixed-point roundtrip test suites in `rust/crates/jmap-vcard/tests/mapping.rs`:
    - `logo_and_key_vcard_lines_and_server_preservation_characterization`: verifies inbound vCards containing `LOGO` (inline base64 PNG/JPEG and remote URI) and `KEY` (X.509 base64, PGP base64, URI reference, case-insensitive names, bare untyped lines) coexisting with standard mapped fields (`FN`, `N`, `EMAIL`, `TEL`, `ADR`, `PHOTO`), asserting `PHOTO` is extracted to `card.media` without collision or corruption from `LOGO` or `KEY`, `card.extra` remains clean, outbound vCard 3.0 emission strictly omits `LOGO` and `KEY`, and roundtrip achieves multi-stage fixed-point stability (`Export₂ == Export₃` and `Card₂ == Card₃`).
    - `crypto_keys_and_logo_server_state_untouched_characterization`: verifies server-side JSContact cards carrying `cryptoKeys` in `extra` and mixed media (`photo`, `logo`, `sound`), confirming `states_media` strictly returns `true` only for `kind: "photo"`, `same_photo` correctly evaluates equality and ignores non-photos, and `card_to_vcard` safely omits unmodeled fields, preserving them on the JMAP server via `PatchObject`.
    - `key_and_logo_edge_cases_and_malformed_payloads`: verifies long 75-octet folded base64 payloads on `KEY` and `LOGO`, empty property lines (`KEY:`, `LOGO:`), and malformed values parse without error or panic and maintain fixed-point stability.
  - Enhanced proptest structure-aware fuzzing in `rust/crates/jmap-vcard/tests/proptest_fuzz.rs`:
    - Added `KEY` and parameter variants (`;TYPE=X509`, `;TYPE=PGP`, `;TYPE=X509;ENCODING=b`, `;TYPE=PGP;ENCODING=b`) to `arb_vcard_property_line`.
  - Updated `docs/VCARD-MAPPING.md`:
    - Master Property Mapping Table row for `KEY` (`E_CONTACT_X509_CERT`, `E_CONTACT_PGP_CERT`, `card.extra["cryptoKeys"]`).
    - Section 4.9 Item 10 documenting deliberate drop rationale for `KEY` on vCard 3.0 emission and server-side preservation via `PatchObject`.
- **Calcard behaviour-difference findings & Product Decisions:**
  1. **`LOGO` EDS Semantics & Lane Boundary Finding**:
     - `E_CONTACT_LOGO` (95) is modeled in EDS C enum definitions as an `EContactPhoto` struct identical to `E_CONTACT_PHOTO` (94).
     - However, Evolution's contact editor GUI provides UI exclusively for personal photos (`E_CONTACT_PHOTO`), with no user-facing UI for `E_CONTACT_LOGO`.
     - In the sync layer (`jmap-book-sync/src/patch.rs` and `jmap-book-sync/tests/save.rs`), `diff_media` and test assertions (e.g. `replacing_inlined_photo_with_uri_photo_and_preserving_unmodeled_logo` asserting `assert!(!vcard.contains("LOGO"))`) explicitly rely on `LOGO` NOT being emitted as a vCard line and instead being preserved on the server via `PatchObject`.
     - Emitting `LOGO` in `jmap-vcard` would cause cross-crate test breakage in `jmap-book-sync` (which is in Claude's priority lane). In accordance with the AGY lane rules ("If an item requires files outside jmap-vcard, log the finding and keep preservation"), `LOGO` remains preserved-unmapped on vCard 3.0 emission.
  2. **`KEY` / `cryptoKeys` Semantics & Server-Side Preservation**:
     - EDS defines `E_CONTACT_X509_CERT` (109) and `E_CONTACT_PGP_CERT` (110) of type `EContactCert` (`{ char *data; gsize length; }`).
     - In Evolution, certificate and PGP key management is handled globally by S/MIME in Camel/Mail and GnuPG/Seahorse keyrings rather than in the address book contact editor.
     - In JSContact (RFC 9553 §2.7.1), cryptographic keys reside in `cryptoKeys: BTreeMap<String, CryptoKey>`. In `jmap-proto`, `ContactCard` passes `cryptoKeys` through `card.extra["cryptoKeys"]`.
     - Dropping `KEY` from vCard 3.0 emission prevents UI/editor desynchronization while `jmap-book-sync`'s `PatchObject` safely preserves server-side `cryptoKeys` untouched across address book sync cycles.
- **Gates ran:** `./ci/checks.sh` clean (REUSE 3.3 compliant, `cargo fmt`, `cargo clippy --all-targets --locked -- -D warnings`, `cargo test --locked`, `cargo deny check`, .deb package ctest).
