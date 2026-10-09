<!--
SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
SPDX-License-Identifier: GPL-3.0-or-later
-->

# Coverage-Guided Fuzzing Architecture and Execution Log

## 1. Threat Model

In accordance with the architecture findings in `AUDIT-FFI.md`, the JMAP server is treated as untrusted:
- The network endpoint can deliver arbitrarily malformed, truncated, deeply nested, or hostile payloads.
- Deserialization and parsing code in client libraries must never panic, crash, abort, or hang on hostile input.
- A decoding failure returning `Err(...)` is standard and expected; unhandled unwraps or indexing panics are security bugs.

## 2. Toolchain and Environment

- Compiler: `rustc 1.101.0-nightly (a30aa9064 2026-10-08)`
- Fuzzing Engine: `cargo-fuzz 0.13.2` with LLVM `libFuzzer` and AddressSanitizer (ASan)
- Platform: `x86_64-unknown-linux-gnu`

## 3. Fuzz Target Surfaces

Three dedicated fuzz targets isolate the untrusted server parser boundaries. Each target lives in its own crate `fuzz/` directory with independent workspace isolation:

### Surface A: `jmap-proto` (`fuzz_proto_parsers`)
- Location: `rust/crates/jmap-proto/fuzz/fuzz_targets/fuzz_proto_parsers.rs`
- Coverage: Deserializes arbitrary bytes into top-level response envelopes and domain object models:
  - `jmap_proto::response::Response` (RFC 8620 Section 3.4)
  - `jmap_proto::session::Session` (RFC 8620 Section 2)
  - `jmap_proto::mail::Mailbox` (RFC 8621 Section 2)
  - `jmap_proto::mail::Email` (RFC 8621 Section 4)
  - `jmap_proto::contacts::ContactCard` (RFC 9610 / JSContact RFC 9553)
  - `jmap_proto::calendars::CalendarEvent` (JMAP Calendars draft / JSCalendar RFC 8984)
  - Re-serializes with `serde_json::to_vec` on successful parse to verify serializer safety.

### Surface B: `jmap-client` (`fuzz_client_response`)
- Location: `rust/crates/jmap-client/fuzz/fuzz_targets/fuzz_client_response.rs`
- Coverage: Connects `jmap_client::Client` with a synthetic `Transport` that feeds arbitrary bytes as HTTP response bodies for real requests built by the client:
  - `client.mailbox_get(&account_id)`
  - `client.email_get(&account_id, &[Id::new("e1")], None)`
  - `client.calendars(&account_id)`
  - `client.event_get(&account_id, &[Id::new("ev1")])`
  - `client.address_books(&account_id)`
  - `client.contact_get(&account_id, &[Id::new("c1")])`

### Surface C: `jmap-ical` (`fuzz_ical_roundtrip`)
- Location: `rust/crates/jmap-ical/fuzz/fuzz_targets/fuzz_ical_roundtrip.rs`
- Coverage: Ingests arbitrary bytes as iCalendar text via `ical_to_event`, maps to `CalendarEvent`, exports back via `event_to_ical`, and verifies the multi-stage fixed point invariant (`Export_2 == Export_3` and `Event_2 == Event_3`) documented in `docs/ICAL-MAPPING.md` Section 6.

## 4. Initial Corpus and Seeding

Corpora are seeded from real client exports and protocol fixtures:
- `jmap-proto`: Seeded with `session.json`, `response_with_error.json`, `mailbox.json`, `email.json`, `contact_card.json`, `calendar_event.json`, and edge case JSON tokens.
- `jmap-client`: Seeded with valid response envelope structures and session payloads.
- `jmap-ical`: Seeded with all 12 real calendar client exports from `jmap-ical/tests/fixtures/` (Apple Calendar, Google Calendar, Nextcloud, Evolution, Thunderbird, Outlook M365, SOGo, Cyrus).

## 5. Execution Summary and Findings

### Session 1 (2026-10-09): Batch 30
- Target `fuzz_proto_parsers` (`jmap-proto`):
  - Total executions: 184,388 runs during bounded slice (-max_total_time=60), plus 2,000 validation runs.
  - Speed: ~3,000 exec/s.
  - Peak RSS: 83 MB.
  - Crashes: 0.
  - Hangs: 0.
  - OOMs: 0.
  - Findings: Serde custom visitors and enum deserializers across RFC 8620, RFC 8621, RFC 9610, and RFC 8984 safely reject malformed byte streams without panics.
  - Minimized corpus: 200 files capturing 5,902 coverage branches and 9,972 features.
- Target `fuzz_client_response` (`jmap-client`):
  - Smoke execution: 100 runs.
  - Findings: Response dispatch and JSON envelope error handling return structured `Err(Error::...)` cleanly on malformed payloads.
- Target `fuzz_ical_roundtrip` (`jmap-ical`):
  - Smoke execution: 100 runs.
  - Findings: Fixed point stability confirmed across all seeded fixture permutations.

## 6. Commands to Resume and Run Fuzz Targets

All commands require the nightly toolchain and must be run from the respective crate directory.

### Running `jmap-proto`:
```bash
cd rust/crates/jmap-proto
cargo +nightly fuzz run fuzz_proto_parsers -- -max_total_time=2700
```

### Running `jmap-client`:
```bash
cd rust/crates/jmap-client
cargo +nightly fuzz run fuzz_client_response -- -max_total_time=2700
```

### Running `jmap-ical`:
```bash
cd rust/crates/jmap-ical
cargo +nightly fuzz run fuzz_ical_roundtrip -- -max_total_time=2700
```

### Minifying Corpus:
```bash
cd rust/crates/<crate>
cargo +nightly fuzz cmin <target_name>
```

### Checking Compilation:
```bash
cd rust/crates/<crate>
cargo +nightly fuzz check
```
