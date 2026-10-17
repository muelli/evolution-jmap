<!--
SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
SPDX-License-Identifier: GPL-3.0-or-later
-->

# Fuzzing

How to run the coverage-guided fuzz targets under `rust/crates/*/fuzz/`.

## Threat model

The JMAP server, and any vCard or iCalendar text a server or an import hands
us, is untrusted. It can send arbitrarily malformed, truncated, deeply nested
or deliberately hostile payloads. Parsing and mapping code may reject such
input with `Err(...)`; it must never panic, abort, hang or run out of memory.
A panic on input is a bug, and in the EDS and Camel processes it takes the
whole process down.

## Targets

Each fuzzed crate has its own `fuzz/` directory, a standalone Cargo workspace
(not a member of `rust/Cargo.toml`), so the nightly-only build never touches
the main workspace.

| Crate | Target | What it covers |
|---|---|---|
| `jmap-proto` | `fuzz_proto_parsers` | Deserialises the bytes as `response::Response`, `session::Session`, `mail::Mailbox`, `mail::Email`, `contacts::ContactCard` and `calendars::CalendarEvent`, and re-serialises whatever parsed. |
| `jmap-client` | `fuzz_client_response` | Feeds the bytes to a real `Client` through a fake `Transport`: once as the session document on connect, then as the body of every method response the client issues (mailboxes, email, calendars, events, address books, contacts, blobs, principals, identities, vacation response, threads, notifications). |
| `jmap-ical` | `fuzz_ical_roundtrip` | Parses the bytes as iCalendar with `ical_to_event`, then exports and re-imports, asserting the export and the event reach a fixed point (see `docs/ICAL-MAPPING.md`). |
| `jmap-vcard` | `fuzz_vcard_to_jscontact` | Parses the bytes as vCard with `vcard_to_card`. |
| `jmap-vcard` | `fuzz_jscontact_to_vcard` | Deserialises the bytes as a JSContact `ContactCard`, renders it with `card_to_vcard` and parses the result back. |
| `jmap-vcard` | `fuzz_vcard_roundtrip_fixed_point` | Runs `vcard -> card -> vcard` several times; every emitted vCard must parse, and once two consecutive emitted vCards are byte-identical their parsed cards must be equal. |

Stable-toolchain counterparts run in `cargo test`: `tests/fuzz_smoke.rs` in
`jmap-proto`, `jmap-client` and `jmap-ical` replay hostile inputs and the
regressions earlier fuzzing found; `jmap-vcard` has `tests/hostile.rs` and
`tests/proptest_fuzz.rs`.

## Seed corpora

The committed corpus for a target is `fuzz/corpus/<target>/` in its crate.

- `jmap-proto` and `jmap-client`: seeded from the JSON fixtures in
  `jmap-proto/tests/fixtures/` (sessions, response envelopes, domain objects)
  plus edge-case tokens, then minimised. Files are named by content hash.
- `jmap-ical`: every real client export in `jmap-ical/tests/fixtures/*.ics`
  under its own name, plus minimised inputs found by fuzzing.
- `jmap-vcard`: the `*.vcf` exports in `jmap-vcard/tests/fixtures/` for the two
  vCard-input targets, and `contact_card.json` for `fuzz_jscontact_to_vcard`.

Every corpus file needs a `.license` sidecar, or `reuse lint` fails.

## Running

You need a nightly toolchain and `cargo-fuzz`:

```sh
rustup toolchain install nightly
cargo install cargo-fuzz
```

Run every command from the crate directory (the one containing `fuzz/`).
libFuzzer writes new inputs into the first corpus directory it is given, so
fuzz into a scratch copy and keep the committed corpus clean:

```sh
cd rust/crates/jmap-proto        # or jmap-client, jmap-ical, jmap-vcard
T=fuzz_proto_parsers             # a target name from the table above
work=$(mktemp -d)
find fuzz/corpus/$T -type f ! -name '*.license' -exec cp -t "$work" {} +
cargo +nightly fuzz run $T "$work" -- -max_total_time=2700
```

`cargo +nightly fuzz list` prints the crate's target names.

### Crashes

A failing input is saved as `fuzz/artifacts/<target>/crash-<hash>` (or
`timeout-`, `oom-`). Reproduce and shrink it with:

```sh
cargo +nightly fuzz run $T fuzz/artifacts/$T/crash-<hash>
cargo +nightly fuzz tmin $T fuzz/artifacts/$T/crash-<hash>
```

Fix the bug, then add the minimised input as a stable regression test (the
crate's `tests/fuzz_smoke.rs`, or `tests/hostile.rs` for `jmap-vcard`), so
`cargo test` keeps it fixed without nightly.

### Minimising the corpus

```sh
cargo +nightly fuzz cmin $T "$work"
```

To commit the result, replace `fuzz/corpus/$T/` with the contents of
`$work`, restore any named fixture seeds `cmin` dropped, and give each new
file a sidecar with the same two lines the existing ones carry:

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

### Checking the targets build

```sh
cargo +nightly fuzz check
```
