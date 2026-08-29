# calcard semantic-conversion spike (item 27) — measured 2026-08-29

Report only, per the item's own instruction. **Nothing in `rust/` changed as
a result of this spike** — every measurement below was taken with a scratch
adapter that was written, run, and discarded in the same session (not
committed; see "Method" for exactly what it was). `jmap-vcard`'s 327 tests
and `jmap-ical`'s 334 tests are confirmed passing unmodified on `master`
before and after.

## Answer, up front

**Keep ours.** The pass rate against calcard's default `jmap`-feature
converter, run through our own acceptance suites unmodified, is **17.4% for
contacts and 12.9% for calendars** — and it is **0 of 21** on the tests that
matter most, the real-exporter round-trip fixtures (Google, Apple, Outlook,
Thunderbird, Nextcloud, SOGo). The gap is not a handful of missing properties
that an adapter could paper over; it is structural, in three ways
independent of each other (§b): calcard's JSContact→vCard side targets vCard
**4.0**, while this project's EDS integration is committed to vCard **3.0**
(`EContact`'s wire format); calcard has no notion of Evolution's `X-*`
extension properties (IM handles, file-as, spouse/manager/assistant) at all,
where our 39,000 lines of mapping tests exist substantially *because of*
those; and the `ContactCard`/`CalendarEvent` types this project already
ships mix RFC 8620 JMAP-object properties (`id`, `addressBookIds`/
`calendarIds`) into the same struct as the RFC 9553/8984 semantic
properties, which calcard's parser does not expect and mishandles by default.
None of these are calcard bugs — calcard is doing exactly what a
general-purpose JSContact/JSCalendar library should do; they are the exact
"Evolution-specific residue" the standing directive asked this spike to
quantify, and it is larger than a grep for the literal string `Evolution`
suggested.

## Method

A scratch adapter (`spike_calcard.rs`, added to each crate's `src/` for the
duration of the run, `calcard`'s `jmap` feature turned on via `features =
["jmap"]` on `jmap-vcard`'s and `jmap-ical`'s `calcard` dependency — the
main `Cargo.toml`'s workspace dependency stays `default-features = false`,
unchanged) implements drop-in replacements for `card_to_vcard`/
`vcard_to_card` (contacts) and `event_to_ical`/`ical_to_event` (calendar)
with matching signatures:

- `card_to_vcard`: `serde_json::to_value(&ContactCard)`, strip the two
  JMAP-wrapper keys (`id`, `addressBookIds`) calcard's `JSContact` does not
  model, `calcard::jscontact::JSContact::<String, String>::parse`, then
  `.into_vcard()` and `Display`.
- `vcard_to_card`: `calcard::vcard::VCard::parse`, `.into_jscontact()`,
  `to_string_pretty()`, `serde_json::from_str` back into `ContactCard`.
- The calendar pair is the same shape with `calcard::jscalendar::JSCalendar`
  and `ICalendar`, stripping `id`/`calendarIds` and requiring a `VEVENT`
  component (mirroring `parse_ical`'s own `ICalError::NoEvent`).

A duplicate of each test file (`tests/mapping_calcard.rs`,
`tests/event_calcard.rs`) then imports these instead of the real functions
— everything else in the ~21,000-line contacts suite and the ~13,000-line
calendar suite is byte-for-byte the original file. `cargo test` runs it and
reports pass/fail per named test, exactly as it does for the real suite.
Four vCard tests and two iCalendar tests assert the *specific* error enum
variant (`VCardError`/`ICalError`) our own parser raises on malformed input;
the adapter maps calcard's own `Entry` parse-failure shape onto the closest
matching variant so those four/two compile and run rather than being
excluded, though the mapping is necessarily approximate (calcard's error
taxonomy is its own, not ours — see §b). Nothing else in either test file
was touched. The two `features = ["jmap"]` edits, the two `spike_calcard.rs`
files, and the two `*_calcard.rs` test files were reverted at the end of the
session; `git status` is clean of them.

## (a) Pass rate

| | tests | passed unchanged | failed | real-exporter fixtures passed |
|---|---:|---:|---:|---:|
| JSContact ↔ vCard (`jmap-vcard`) | 327 | 57 (17.4%) | 270 | 0 / 11 |
| JSCalendar ↔ iCalendar (`jmap-ical`) | 334 | 43 (12.9%) | 291 | 0 / 10 |
| **Combined** | **661** | **100 (15.1%)** | **561** | **0 / 21** |

Failure classes, contacts (270 failures, classified by first panic line):

| class | count | example |
|---|---:|---|
| generic content mismatch (`assert_eq!`) | 101 | structured value differs from ours |
| vCard text mismatch (mostly VERSION 3.0-vs-4.0 and syntax) | 55 | `emits_a_vcard_30_envelope` |
| other (mostly targeted `assert!`s on specific behaviour) | 50 | `a_keyword_set_to_anything_but_true_gets_no_line` |
| keyed-map lookup miss (calcard's `PROP-ID` scheme names keys `k1`, `k2`, …; ours uses meaningful keys like `work`, `mobile`, `a1`) | 33 | `a_label_without_a_key_joins_the_address_of_the_same_type`: expected key `a1`, got `k1` |
| Evolution `X-*` property entirely absent from calcard's output | 14 | `a_handle_holding_the_separators_is_escaped_and_comes_back_whole`: no `X-JABBER` line; calcard wrote vCard 4.0's `SOCIALPROFILE` instead |
| unwrap-on-`None` (expected structured field absent) | 13 | `a_vcard_from_evolution_has_no_jmap_id_yet` |
| round-trip leaves residue in `extra` calcard could not classify | 4 | all 4 are `real_exporter_fixture_*` |

Failure classes, calendar (291 failures):

| class | count | example |
|---|---:|---|
| iCalendar text mismatch | 155 | e.g. calcard emits `NAME:` for the event title where RFC 5545 `VEVENT` wants `SUMMARY:` |
| other (targeted `assert!`s) | 63 | — |
| generic content mismatch | 54 | — |
| unwrap-on-`None` | 16 | recurrence-rule edge cases a hand-written rule can express and calcard's exporter drops |
| round-trip residue in `extra` | 3 | all 3 are `real_exporter_fixture_*` |

Both directions confirm the same point from independent evidence: the
`SUMMARY`/`NAME` divergence was not investigated further (out of scope for a
report-only spike — plausibly a jscalendarbis-vs-8984 property-name drift,
plausibly a default-object-type quirk on a bare `Event` fed without a
wrapping component; either way it is calcard's call to make, not a bug
report against it) but it alone accounts for a large share of the 155.

## (b) Evolution-specific residue — larger than the grep floor

The item text's own grep (`~82` lines of `contact.rs`, `~25` of `jmap-ical`
naming Evolution literally) is confirmed as a floor, not a measure, and this
spike's failures show concretely why:

1. **The vCard version itself.** `jmap-vcard`'s whole reason to exist opens
   with "JSContact ↔ vCard **3.0**" (`contact.rs:4`) — a choice `EContact`,
   the type EDS's address book vfuncs actually pass, forces (not stated as
   Evolution's name anywhere the grep would catch it, but it is Evolution's
   decision as surely as an `X-EVOLUTION-*` line is). calcard's JSContact
   exporter targets vCard 4.0 unconditionally — there is no version
   parameter on `into_vcard()`. Adopting it as-is means every contact
   Evolution reads or writes changes vCard major version, which is exactly
   the kind of behaviour change item 27's own directive says counts as a
   finding, not a detail to smooth over.
2. **The `X-*` extension table.** IM handles (`X-JABBER`, `X-AIM`,
   `X-GADUGADU`, `X-MATRIX`, …), `X-EVOLUTION-FILE-AS`, `X-EVOLUTION-SPOUSE`/
   `-MANAGER`/`-ASSISTANT`, `X-EVOLUTION-ANNIVERSARY` are simply not in
   calcard's vocabulary — not wrong, not mismapped, absent. `docs/`'s own
   count of ~82 lines under-states this because most of that logic is not
   *mentioning* Evolution, it is silently *encoding* Evolution's field
   layout: which vCard property a JSContact `onlineServices` entry becomes
   is an EDS UI decision (which field the contact editor reads), not a
   naming choice a grep for the word "Evolution" would catch.
3. **The key-naming contract.** `ContactCard`'s `BTreeMap<String, T>` keys
   (`work`, `mobile`, `a1`, …) are meaningful identifiers this project
   invented for stable round-tripping; calcard invents its own (`k1`, `k2`,
   sequential, per-property-not-per-card). Nothing downstream of
   `jmap-vcard` in this codebase currently depends on the *values* of those
   keys surviving a round-trip unchanged (the tests assert it because it is
   the crate's contract, not because a caller reaches in), but the 33
   `keyed-map-lookup-miss` failures show that adopting calcard's exporter
   verbatim would also mean adopting calcard's key-naming scheme.

## (c) Type-boundary cost

`ContactCard`/`CalendarEvent` live in `jmap-proto` and are deserialised
directly off the wire (`jmap-client`) and serialised directly onto it
(every backend's `Set`/`Get` calls); every one of the four sync crates
depends on their exact shape. calcard's `JSContact<I, B>`/`JSCalendar<I, B>`
are a different representation entirely — a generic `jmap_tools::Value` tree
keyed by `JSContactProperty`/`JSCalendarProperty` enums, not a `#[derive
(Serialize, Deserialize)]` struct — so there is no direct substitution
available. Two real options, both with a real cost:

- **Replace `ContactCard`/`CalendarEvent` with calcard's types.** Ripples
  into `jmap-client` (deserialising `ContactCard`/`CalendarEvent` off
  `ContactCard/get` and `CalendarEvent/get` responses) and all four sync
  crates (`jmap-backend-book`, `-cal`, and the two `with_connection`
  call sites item 25 just finished hardening) — every place that reads a
  field off these structs today (dozens, going by `grep -rn
  "\.name\b\|\.emails\b\|\.start\b" rust/crates/jmap-backend-*` alone) would
  need to learn `jmap_tools::Value`'s API instead of a typed field access.
  Size: large, workspace-wide, and not attempted or estimated further in
  this report-only spike.
- **Write an adapter.** This spike *is* that adapter's first draft, and its
  own 15.1% raw pass rate is the evidence for the claim in the item text
  that "an adapter may eat the savings": stripping two JMAP-wrapper keys
  moved the contact number from 16.5% to 17.4%, i.e. the adapter's easy 20%
  of the work bought back essentially none of the gap. Closing the vCard
  3.0-vs-4.0 divergence and the whole `X-*` table (§b) is not adapter work
  at that boundary at all — it is re-implementing large parts of
  `jmap-vcard`/`jmap-ical`'s existing hand-written mapping *on top of*
  calcard's types, which is most of the 39,000 lines of mapping tests this
  project already has, not less of them.

Either path costs more than it saves, on the evidence actually measured
here — see (e).

## (d) Dependency cost

Enabling `jmap` on `calcard` (still `default-features = false` at the
workspace level — this spike enabled it per-crate only, and reverted that)
pulls in three crates not currently in `Cargo.lock`: `jmap-tools` 0.1.8
(Apache-2.0 OR MIT), `uuid` 1.26.0 (Apache-2.0 OR MIT), `sha1_smol` 1.0.1
(BSD-3-Clause, a `uuid` v5 transitive dependency). All three are on
`deny.toml`'s license allowlist already (`MIT`, `Apache-2.0`,
`BSD-3-Clause` are all listed) — checked by inspecting each crate's
`Cargo.toml` `license` field directly, since `cargo-deny` itself is not
runnable on this VM (`[[checks-sh-blocked-on-vm]]`). No duplicate-version
conflicts: `serde`/`serde_json` are already workspace dependencies at
compatible versions, so `jmap`'s `serde`/`serde_json` features add no
second copy. Reproducible-build posture is unaffected in kind — three more
pinned crates.io dependencies, no new build-time codegen or `build.rs`
beyond what `calcard` itself already has. The Debian third-party-notices
generator (`tools/generate-debian-copyright.py --third-party-notices`,
Track C2) would need three more entries; mechanical, not investigated
further here. In short: dependency cost is small and not a blocker either
way — it does not move the recommendation.

## (e) Recommendation

**Keep our hand-written mapping. Do not adopt calcard's `jmap`-feature
converter, on either side, in whole or in part**, for reasons independent
enough that fixing any one would still leave the other two:

1. The measured pass rate against our own acceptance suite — the one this
   project chose as the bar because it encodes years of real-exporter and
   real-server findings — is 15.1% combined and **0% on every real-exporter
   fixture**, the single number this report was scoped to produce.
2. The gap is structural (vCard version, the whole Evolution `X-*` table,
   the JMAP/JSContact type boundary), not a list of individually fixable
   omissions calcard could plausibly close in a future release aimed at
   this project's use case — calcard has no reason to target vCard 3.0 or
   Evolution's field layout, and should not.
3. Our 39,000 lines of mapping tests (`jmap-vcard` + `jmap-ical`) already
   encode exactly the residue in (b); "keep the tests, swap the
   implementation under them" — the framing the standing directive offered
   as the best case for adoption — does not hold here, because the
   implementation *is* what encodes the residue the tests check. There is
   no thinner implementation underneath that still passes them.

This does not reopen the 2026-08-18 decision to use calcard for the *text*
layer (lexer/writer) — that migration's numbers (327/334 passing, unchanged,
before and after) were completely different because the text layer has no
Evolution-specific semantics to lose. This spike is the semantic layer, and
the numbers there say keep ours.
