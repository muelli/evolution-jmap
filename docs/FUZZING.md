<!--
SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
SPDX-License-Identifier: GPL-3.0-or-later
-->

# Fuzzing

This document tells you how to run the coverage-guided fuzz targets under
`rust/crates/*/fuzz/`.

## Threat model

The JMAP server is untrusted. Any vCard or iCalendar text that a server or an
import gives to us is also untrusted. Such input can be malformed, truncated,
deeply nested or hostile. The parsing and mapping code can reject it with
`Err(...)`. It must never panic, abort, hang or run out of memory. A panic on
input is a bug. In the EDS and Camel processes, a panic stops the whole
process.

## Targets

Each fuzzed crate has its own `fuzz/` directory. This directory is a
standalone Cargo workspace. It is not a member of `rust/Cargo.toml`. The
nightly-only build therefore does not touch the main workspace.

| Crate | Target | What it covers |
|---|---|---|
| `jmap-proto` | `fuzz_proto_parsers` | Deserialises the bytes as `response::Response`, `session::Session`, `mail::Mailbox`, `mail::Email`, `contacts::ContactCard` and `calendars::CalendarEvent`. Serialises again everything that parsed. |
| `jmap-client` | `fuzz_client_response` | Gives the bytes to a real `Client` through a fake `Transport`. First, the bytes are the session document on connect. Then they are the body of every method response that the client issues (mailboxes, email, calendars, events, address books, contacts, blobs, principals, identities, vacation response, threads, notifications). |
| `jmap-ical` | `fuzz_ical_roundtrip` | Parses the bytes as iCalendar with `ical_to_event`. Then exports and imports again. Asserts that the export and the event reach a fixed point (see `docs/ICAL-MAPPING.md`). |
| `jmap-vcard` | `fuzz_vcard_to_jscontact` | Parses the bytes as vCard with `vcard_to_card`. |
| `jmap-vcard` | `fuzz_jscontact_to_vcard` | Deserialises the bytes as a JSContact `ContactCard`. Renders it with `card_to_vcard` and parses the result again. |
| `jmap-vcard` | `fuzz_vcard_roundtrip_fixed_point` | Runs `vcard -> card -> vcard` several times. Every emitted vCard must parse. If two consecutive emitted vCards are byte-identical, their parsed cards must be equal. |

`cargo test` runs the counterparts for the stable toolchain:

- `tests/fuzz_smoke.rs` in `jmap-proto`, `jmap-client` and `jmap-ical` replays
  hostile inputs and the regressions that earlier fuzzing found.
- `jmap-vcard` has `tests/hostile.rs` and `tests/proptest_fuzz.rs`.

## Seed corpora

The committed corpus for a target is `fuzz/corpus/<target>/` in its crate.

- `jmap-proto` and `jmap-client`: the seeds come from the JSON fixtures in
  `jmap-proto/tests/fixtures/` (sessions, response envelopes, domain objects)
  and from edge-case tokens. The corpus is minimised. The file names are the
  content hashes.
- `jmap-ical`: every real client export in `jmap-ical/tests/fixtures/*.ics`
  under its own name. The corpus also contains the minimised inputs that
  fuzzing found.
- `jmap-vcard`: the `*.vcf` exports in `jmap-vcard/tests/fixtures/` for the two
  vCard-input targets. `contact_card.json` is for `fuzz_jscontact_to_vcard`.

Every corpus file needs a `.license` sidecar. If a file has no sidecar,
`reuse lint` fails.

## Running

You need a nightly toolchain and `cargo-fuzz`:

```sh
rustup toolchain install nightly
cargo install cargo-fuzz
```

Run every command from the crate directory (the directory that contains
`fuzz/`). libFuzzer writes new inputs into the first corpus directory that you
give to it. Fuzz into a scratch copy. This keeps the committed corpus clean:

```sh
cd rust/crates/jmap-proto        # or jmap-client, jmap-ical, jmap-vcard
T=fuzz_proto_parsers             # a target name from the table above
work=$(mktemp -d)
find fuzz/corpus/$T -type f ! -name '*.license' -exec cp -t "$work" {} +
cargo +nightly fuzz run $T "$work" -- -max_total_time=2700
```

`cargo +nightly fuzz list` prints the target names of the crate.

### Crashes

The fuzzer saves a failing input as `fuzz/artifacts/<target>/crash-<hash>`
(or `timeout-`, `oom-`). To reproduce and shrink the input, run these
commands:

```sh
cargo +nightly fuzz run $T fuzz/artifacts/$T/crash-<hash>
cargo +nightly fuzz tmin $T fuzz/artifacts/$T/crash-<hash>
```

To fix the bug, do these steps:

1. Fix the bug.
2. Add the minimised input as a stable regression test. Use `tests/fuzz_smoke.rs`
   of the crate. For `jmap-vcard`, use `tests/hostile.rs`.

`cargo test` then keeps the bug fixed without nightly.

### Minimising the corpus

```sh
cargo +nightly fuzz cmin $T "$work"
```

To commit the result, do these steps:

1. Replace `fuzz/corpus/$T/` with the contents of `$work`.
2. Restore the named fixture seeds that `cmin` removed.
3. Give each new file a sidecar. The sidecar has the same two lines as the
   existing sidecars.

<!-- REUSE-IgnoreStart -->
```sh
for f in fuzz/corpus/$T/*; do
    case $f in *.license) continue ;; esac
    [ -e "$f.license" ] || printf '%s\n' \
        'SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>' \
        'SPDX-License-Identifier: GPL-3.0-or-later' > "$f.license"
done
```
<!-- REUSE-IgnoreEnd -->

### Checking that the targets build

```sh
cargo +nightly fuzz check
```
