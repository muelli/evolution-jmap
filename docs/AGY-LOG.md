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

