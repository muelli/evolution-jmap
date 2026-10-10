<!--
SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
SPDX-License-Identifier: GPL-3.0-or-later
-->

# jmap-vcard Fuzzing Plan and Execution Log

## Threat surface

`jmap-vcard` sits on an untrusted-input boundary in both directions:
- vCard text from remote address book servers and imported contact files.
- JSContact JSON that can arrive from network responses or persisted state.

The fuzzing goal is strict: malformed or hostile input may return `Err`, but must not panic, hang, or OOM.

## Tooling and setup

- Engine: `cargo-fuzz` with LLVM libFuzzer and ASan.
- Toolchain: nightly Rust, because `cargo fuzz` does not run on stable.
- Crate-local harness: `rust/crates/jmap-vcard/fuzz/` (standalone workspace, not a root workspace member).
- Installed for this lane session:
  - `rustc 1.101.0-nightly (a30aa9064 2026-10-08)`
  - `cargo-fuzz 0.13.2`

## Targets

1. `fuzz_vcard_to_jscontact`
   - Input: arbitrary bytes interpreted as UTF-8 vCard text.
   - Surface: `jmap_vcard::vcard_to_card`.
2. `fuzz_jscontact_to_vcard`
   - Input: arbitrary bytes interpreted as JSON.
   - Surface: `serde_json::from_slice::<ContactCard>`, then `jmap_vcard::card_to_vcard`, then parse back with `vcard_to_card`.
3. `fuzz_vcard_roundtrip_fixed_point`
   - Input: arbitrary bytes interpreted as UTF-8 vCard text.
   - Property: `vcard -> card1 -> vcard2 -> card2` keeps `card1 == card2`.

## Seed corpus

Seed files are copied from `rust/crates/jmap-vcard/tests/fixtures/`:
- `*.vcf` into `fuzz_vcard_to_jscontact` and `fuzz_vcard_roundtrip_fixed_point`.
- `contact_card.json` into `fuzz_jscontact_to_vcard`.

## Execution log

### 2026-10-10 UTC

Completed in this session:
- Installed `nightly-x86_64-unknown-linux-gnu` and `cargo-fuzz 0.13.2`.
- Passed harness compile check:
  - `cd rust/crates/jmap-vcard && cargo +nightly fuzz check`
- Ran one bounded fuzz target slice for Batch 4 rotation:
  - `cargo +nightly fuzz run fuzz_vcard_to_jscontact -- -max_total_time=2700`
  - Completed without sanitizer errors or crashes.

Observed libFuzzer result summary:
- Total executions: `13,990,976`.
- Runtime: `2701` seconds.
- Final coverage counters: `cov: 10337`, `ft: 52346`.
- Final corpus state during run: `13755` inputs, `8068Kb`.
- Throughput: `exec/s: 5179`.
- Peak resident set size: `rss: 681Mb`.
- Crashes: `0`.
- Hangs: `0`.
- OOMs: `0`.

Command note:
- The bounded run used a temporary copy of the seed corpus under `/tmp` to avoid writing generated units into the committed repository corpus.

### 2026-10-10 UTC (target rotation: `fuzz_jscontact_to_vcard`)

Completed in this session:
- Passed harness compile check before execution:
  - `cd rust/crates/jmap-vcard && cargo +nightly fuzz check`
- Ran the second bounded fuzz target slice for Batch 4 rotation:
  - `cargo +nightly fuzz run fuzz_jscontact_to_vcard -- -max_total_time=2700`
  - Completed without sanitizer errors or crashes.

Observed libFuzzer result summary:
- Total executions: `40,887,929`.
- Runtime: `2701` seconds.
- Final coverage counters: `cov: 10998`, `ft: 33559`.
- Final corpus state during run: `9641` inputs, `4344Kb`.
- Throughput: `exec/s: 15138`.
- Peak resident set size: `rss: 716Mb`.
- Crashes: `0`.
- Hangs: `0`.
- OOMs: `0`.

Command note:
- The bounded run used a temporary copy of `fuzz/corpus/fuzz_jscontact_to_vcard` under `/tmp` to avoid writing generated units into the committed repository corpus.

## Resume commands

```bash
cd rust/crates/jmap-vcard
cargo +nightly fuzz check
cargo +nightly fuzz run fuzz_vcard_to_jscontact -- -max_total_time=2700
cargo +nightly fuzz run fuzz_jscontact_to_vcard -- -max_total_time=2700
cargo +nightly fuzz run fuzz_vcard_roundtrip_fixed_point -- -max_total_time=2700
```
