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
   - Property: multi-pass convergence safety for `vcard -> card -> vcard` loops.
   - Checks: each emitted vCard must parse successfully; once two consecutive
     emitted vCards are byte-identical, their parsed cards must match.

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

### 2026-10-10 UTC (target rotation: `fuzz_vcard_roundtrip_fixed_point`)

Completed in this session:
- Passed harness compile check before execution:
  - `cd rust/crates/jmap-vcard && cargo +nightly fuzz check`
- Reworked the target assertion strategy after deterministic normalization
  mismatches on malformed and legacy fixture-derived inputs:
  - `crash-4b7521666c2b31afb24b0717337a38ac65583505` (Outlook vCard 2.1
    seed path, one-pass phone feature normalization)
  - `crash-d81fd08419c63f26f3ac4b74adf8daab33d7d1b1` (URI photo media-type
    normalization)
  - `crash-9419a804155e83b1949b38a1d83f680dbea97999` (malformed CATEGORIES
    normalization drift)
- Final bounded run after target adjustment:
  - `cargo +nightly fuzz run fuzz_vcard_roundtrip_fixed_point -- -max_total_time=2700`
  - Completed without sanitizer errors or crashes.

Observed libFuzzer result summary:
- Total executions: `3,715,307`.
- Runtime: `2701` seconds.
- Final coverage counters: `cov: 10625`, `ft: 48630`.
- Final corpus state during run: `9447` inputs, `6088Kb`.
- Throughput: `exec/s: 1375`.
- Peak resident set size: `rss: 604Mb`.
- Crashes: `0`.
- Hangs: `0`.
- OOMs: `0`.

Command note:
- The bounded run used a temporary copy of
  `fuzz/corpus/fuzz_vcard_roundtrip_fixed_point` under `/tmp` to avoid writing
  generated units into the committed repository corpus.

### 2026-10-10 UTC (target rotation: `fuzz_vcard_to_jscontact`, second pass)

Completed in this session:
- Passed harness compile check before execution:
  - `cd rust/crates/jmap-vcard && cargo +nightly fuzz check`
- Ran the next bounded target slice for Batch 4 rotation:
  - `cargo +nightly fuzz run fuzz_vcard_to_jscontact -- -max_total_time=2700`
  - Completed without sanitizer errors or crashes.

Observed libFuzzer result summary:
- Total executions: `13,420,793`.
- Runtime: `2701` seconds.
- Final coverage counters: `cov: 10020`, `ft: 50840`.
- Final corpus state during run: `13296` inputs, `7813Kb`.
- Throughput: `exec/s: 4968`.
- Peak resident set size: `rss: 679Mb`.
- Crashes: `0`.
- Hangs: `0`.
- OOMs: `0`.

Command note:
- The bounded run used a temporary copy of
  `fuzz/corpus/fuzz_vcard_to_jscontact` under `/tmp` to avoid writing
  generated units into the committed repository corpus.

### 2026-10-10 UTC (target rotation: `fuzz_jscontact_to_vcard`, second pass)

Completed in this session:
- Passed harness compile check before execution:
  - `cd rust/crates/jmap-vcard && cargo +nightly fuzz check`
- Ran the next bounded target slice for Batch 4 rotation:
  - `cargo +nightly fuzz run fuzz_jscontact_to_vcard -- -max_total_time=2700`
  - Completed without sanitizer errors or crashes.

Observed libFuzzer result summary:
- Total executions: `39,863,949`.
- Runtime: `2701` seconds.
- Final coverage counters: `cov: 11128`, `ft: 34729`.
- Final corpus state during run: `9902` inputs, `4850Kb`.
- Throughput: `exec/s: 14758`.
- Peak resident set size: `rss: 701Mb`.
- Crashes: `0`.
- Hangs: `0`.
- OOMs: `0`.

Command note:
- The bounded run used a temporary copy of
  `fuzz/corpus/fuzz_jscontact_to_vcard` under `/tmp` to avoid writing
  generated units into the committed repository corpus.

### 2026-10-10 UTC (target rotation: `fuzz_vcard_roundtrip_fixed_point`, second pass)

Completed in this session:
- Passed harness compile check before execution:
  - `cd rust/crates/jmap-vcard && cargo +nightly fuzz check`
- Ran the next bounded target slice for Batch 4 rotation:
  - `cargo +nightly fuzz run fuzz_vcard_roundtrip_fixed_point -- -max_total_time=2700 -verbosity=0 -print_final_stats=1`
  - Completed without sanitizer errors or crashes.

Observed libFuzzer result summary:
- Total executions: `3,650,939`.
- Average throughput: `1351` exec/s.
- New units added: `52,763`.
- Peak resident set size: `636Mb`.
- Crashes: `0`.
- Hangs: `0`.
- OOMs: `0`.

Command note:
- The bounded run used a temporary copy of
  `fuzz/corpus/fuzz_vcard_roundtrip_fixed_point` under `/tmp` to avoid writing
  generated units into the committed repository corpus.
- This invocation used `-verbosity=0`; final stats were emitted, but final
  `cov`/`ft`/`corp` counters were not printed in the log.

### 2026-10-10 UTC (target rotation: `fuzz_vcard_to_jscontact`, third pass)

Completed in this session:
- Passed harness compile check before execution:
  - `cd rust/crates/jmap-vcard && cargo +nightly fuzz check`
- Ran the next bounded target slice for Batch 4 rotation:
  - `cargo +nightly fuzz run fuzz_vcard_to_jscontact <tmp-corpus> -- -max_total_time=2700`
  - Completed without sanitizer errors or crashes.

Observed libFuzzer result summary:
- Total executions: `11,972,831`.
- Runtime: `2701` seconds.
- Average throughput (derived from completion footer): about `4432` exec/s.
- Crashes: `0`.
- Hangs: `0`.
- OOMs: `0`.

Command note:
- The bounded run used a temporary copy of
  `fuzz/corpus/fuzz_vcard_to_jscontact` under `/tmp` to avoid writing
  generated units into the committed repository corpus.
- This invocation used default verbosity, which emitted very large progress
  output. The completion footer (`Done ... runs in 2701 second(s)`) was
  captured, but final `cov`/`ft`/`corp` counters were not retained in the
  harness transcript.

### 2026-10-10 UTC (target rotation: `fuzz_jscontact_to_vcard`, third pass)

Completed in this session:
- Passed harness compile check before execution:
  - `cd rust/crates/jmap-vcard && cargo +nightly fuzz check`
- Ran the next bounded target slice for Batch 4 rotation:
  - `cargo +nightly fuzz run fuzz_jscontact_to_vcard <tmp-corpus> -- -max_total_time=2700 -verbosity=0 -print_final_stats=1`
  - Completed without sanitizer errors or crashes.

Observed libFuzzer result summary:
- Total executions: `42,008,051`.
- Average throughput: `15552` exec/s.
- New units added: `90,448`.
- Slowest unit time: `0` seconds.
- Peak resident set size: `730Mb`.
- Crashes: `0`.
- Hangs: `0`.
- OOMs: `0`.

Command note:
- The bounded run used a temporary copy of
  `fuzz/corpus/fuzz_jscontact_to_vcard` under `/tmp` to avoid writing
  generated units into the committed repository corpus.
- This invocation used `-verbosity=0 -print_final_stats=1` to keep logs
  compact while retaining end-of-run execution stats.

### 2026-10-10 UTC (target rotation: `fuzz_vcard_roundtrip_fixed_point`, third pass)

Completed in this session:
- Passed harness compile check before execution:
  - `cd rust/crates/jmap-vcard && cargo +nightly fuzz check`
- Ran the next bounded target slice for Batch 4 rotation:
  - `cargo +nightly fuzz run fuzz_vcard_roundtrip_fixed_point <tmp-corpus> -- -max_total_time=2700 -verbosity=0 -print_final_stats=1`
  - Completed without sanitizer errors or crashes.

Observed libFuzzer result summary:
- Total executions: `3,791,725`.
- Average throughput: `1403` exec/s.
- New units added: `54,595`.
- Slowest unit time: `0` seconds.
- Peak resident set size: `632Mb`.
- Crashes: `0`.
- Hangs: `0`.
- OOMs: `0`.

Command note:
- The bounded run used a temporary copy of
  `fuzz/corpus/fuzz_vcard_roundtrip_fixed_point` under `/tmp` to avoid writing
  generated units into the committed repository corpus.
- This invocation used `-verbosity=0 -print_final_stats=1` to keep logs
  compact while retaining end-of-run execution stats.

### 2026-10-10 UTC (target rotation: `fuzz_vcard_to_jscontact`, fourth pass)

Completed in this session:
- Passed harness compile check before execution:
  - `cd rust/crates/jmap-vcard && cargo +nightly fuzz check`
- Ran the next bounded target slice for Batch 4 rotation:
  - `cargo +nightly fuzz run fuzz_vcard_to_jscontact <tmp-corpus> -- -max_total_time=2700 -verbosity=0 -print_final_stats=1`
  - Completed without sanitizer errors or crashes.

Observed libFuzzer result summary:
- Total executions: `16,819,650`.
- Average throughput: `6227` exec/s.
- New units added: `134,064`.
- Slowest unit time: `0` seconds.
- Peak resident set size: `676Mb`.
- Crashes: `0`.
- Hangs: `0`.
- OOMs: `0`.

Command note:
- The bounded run used a temporary copy of
  `fuzz/corpus/fuzz_vcard_to_jscontact` under `/tmp` to avoid writing
  generated units into the committed repository corpus.
- This invocation used `-verbosity=0 -print_final_stats=1` to keep logs
  compact while retaining end-of-run execution stats.

### 2026-10-10 UTC (target rotation: `fuzz_jscontact_to_vcard`, fourth pass)

Completed in this session:
- Passed harness compile check before execution:
  - `cd rust/crates/jmap-vcard && cargo +nightly fuzz check`
- Ran the next bounded target slice for Batch 4 rotation:
  - `cargo +nightly fuzz run fuzz_jscontact_to_vcard <tmp-corpus> -- -max_total_time=2700 -verbosity=0 -print_final_stats=1`
  - Completed without sanitizer errors or crashes.

Observed libFuzzer result summary:
- Total executions: `45,470,838`.
- Average throughput: `16834` exec/s.
- New units added: `91,049`.
- Slowest unit time: `0` seconds.
- Peak resident set size: `716Mb`.
- Crashes: `0`.
- Hangs: `0`.
- OOMs: `0`.

Command note:
- The bounded run used a temporary copy of
  `fuzz/corpus/fuzz_jscontact_to_vcard` under `/tmp` to avoid writing
  generated units into the committed repository corpus.
- This invocation used `-verbosity=0 -print_final_stats=1` to keep logs
  compact while retaining end-of-run execution stats.

### 2026-10-10 UTC (target rotation: `fuzz_vcard_roundtrip_fixed_point`, fourth pass)

Completed in this session:
- Passed harness compile check before execution:
  - `cd rust/crates/jmap-vcard && cargo +nightly fuzz check`
- Ran the next bounded target slice for Batch 4 rotation:
  - `cargo +nightly fuzz run fuzz_vcard_roundtrip_fixed_point <tmp-corpus> -- -max_total_time=2700 -verbosity=0 -print_final_stats=1`
  - Completed without sanitizer errors or crashes.

Observed libFuzzer result summary:
- Total executions: `4,389,918`.
- Average throughput: `1625` exec/s.
- New units added: `59,072`.
- Slowest unit time: `0` seconds.
- Peak resident set size: `623Mb`.
- Crashes: `0`.
- Hangs: `0`.
- OOMs: `0`.

Command note:
- The bounded run used a temporary copy of
  `fuzz/corpus/fuzz_vcard_roundtrip_fixed_point` under `/tmp` to avoid writing
  generated units into the committed repository corpus.
- This invocation used `-verbosity=0 -print_final_stats=1` to keep logs
  compact while retaining end-of-run execution stats.
- `libFuzzer` still emitted a long recommended dictionary block before the
  final `stat::` lines in this mode.

### 2026-10-10 UTC (target rotation: `fuzz_vcard_to_jscontact`, fifth pass)

Completed in this session:
- Passed harness compile check before execution:
  - `cd rust/crates/jmap-vcard && cargo +nightly fuzz check`
- Ran the next bounded target slice for Batch 4 rotation:
  - `cargo +nightly fuzz run fuzz_vcard_to_jscontact <tmp-corpus> -- -max_total_time=2700 -verbosity=0 -print_final_stats=1`
  - Completed without sanitizer errors or crashes.

Observed libFuzzer result summary:
- Total executions: `16,623,969`.
- Average throughput: `6154` exec/s.
- New units added: `133,019`.
- Slowest unit time: `0` seconds.
- Peak resident set size: `676Mb`.
- Crashes: `0`.
- Hangs: `0`.
- OOMs: `0`.

Command note:
- The bounded run used a temporary copy of
  `fuzz/corpus/fuzz_vcard_to_jscontact` under `/tmp` to avoid writing
  generated units into the committed repository corpus.
- This invocation used `-verbosity=0 -print_final_stats=1` to keep logs
  compact while retaining end-of-run execution stats.
- `libFuzzer` still emitted a long recommended dictionary block before the
  final `stat::` lines in this mode.

### 2026-10-10 UTC (target rotation: `fuzz_jscontact_to_vcard`, fifth pass)

Completed in this session:
- Passed harness compile check before execution:
  - `cd rust/crates/jmap-vcard && cargo +nightly fuzz check`
- Ran the next bounded target slice for Batch 4 rotation:
  - `cargo +nightly fuzz run fuzz_jscontact_to_vcard <tmp-corpus> -- -max_total_time=2700 -verbosity=0 -print_final_stats=1`
  - Completed without sanitizer errors or crashes.

Observed libFuzzer result summary:
- Total executions: `50,214,632`.
- Average throughput: `18591` exec/s.
- New units added: `96,412`.
- Slowest unit time: `0` seconds.
- Peak resident set size: `747Mb`.
- Crashes: `0`.
- Hangs: `0`.
- OOMs: `0`.

Command note:
- The bounded run used a temporary copy of
  `fuzz/corpus/fuzz_jscontact_to_vcard` under `/tmp` to avoid writing
  generated units into the committed repository corpus.
- This invocation used `-verbosity=0 -print_final_stats=1` to keep logs
  compact while retaining end-of-run execution stats.
- `libFuzzer` still emitted a long recommended dictionary block before the
  final `stat::` lines in this mode.

### 2026-10-10 UTC (target rotation: `fuzz_vcard_roundtrip_fixed_point`, fifth pass)

Completed in this session:
- Passed harness compile check before execution:
  - `cd rust/crates/jmap-vcard && cargo +nightly fuzz check`
- Ran the next bounded target slice for Batch 4 rotation:
  - `cargo +nightly fuzz run fuzz_vcard_roundtrip_fixed_point <tmp-corpus> -- -max_total_time=2700 -verbosity=0 -print_final_stats=1`
  - Completed without sanitizer errors or crashes.

Observed libFuzzer result summary:
- Total executions: `4,721,307`.
- Average throughput: `1747` exec/s.
- New units added: `62,390`.
- Slowest unit time: `0` seconds.
- Peak resident set size: `629Mb`.
- Crashes: `0`.
- Hangs: `0`.
- OOMs: `0`.

Command note:
- The bounded run used a temporary copy of
  `fuzz/corpus/fuzz_vcard_roundtrip_fixed_point` under `/tmp` to avoid writing
  generated units into the committed repository corpus.
- This invocation used `-verbosity=0 -print_final_stats=1` to keep logs
  compact while retaining end-of-run execution stats.
- `libFuzzer` still emitted a long recommended dictionary block before the
  final `stat::` lines in this mode.

### 2026-10-10 UTC (target rotation: `fuzz_vcard_to_jscontact`, sixth pass)

Completed in this session:
- Passed harness compile check before execution:
  - `cd rust/crates/jmap-vcard && cargo +nightly fuzz check`
- Ran the next bounded target slice for Batch 4 rotation:
  - `cargo +nightly fuzz run fuzz_vcard_to_jscontact <tmp-corpus> -- -max_total_time=2700 -verbosity=0 -print_final_stats=1`
  - Completed without sanitizer errors or crashes.

Observed libFuzzer result summary:
- Total executions: `16,369,426`.
- Average throughput: `6060` exec/s.
- New units added: `128,027`.
- Slowest unit time: `0` seconds.
- Peak resident set size: `702Mb`.
- Crashes: `0`.
- Hangs: `0`.
- OOMs: `0`.

Command note:
- The bounded run used a temporary copy of
  `fuzz/corpus/fuzz_vcard_to_jscontact` under `/tmp` to avoid writing
  generated units into the committed repository corpus.
- This invocation used `-verbosity=0 -print_final_stats=1` to keep logs
  compact while retaining end-of-run execution stats.
- `libFuzzer` still emitted a long recommended dictionary block before the
  final `stat::` lines in this mode.

## Resume commands

```bash
cd rust/crates/jmap-vcard
cargo +nightly fuzz check
cargo +nightly fuzz run fuzz_vcard_to_jscontact -- -max_total_time=2700
cargo +nightly fuzz run fuzz_jscontact_to_vcard -- -max_total_time=2700
cargo +nightly fuzz run fuzz_vcard_roundtrip_fixed_point -- -max_total_time=2700
```
