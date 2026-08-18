# Antigravity task list (polish lane)

Headless, in-lane work for the Antigravity polish shift (driven by
`infra/agy-night-shift/agy-prompt.md`). Claude works the priority milestones on
`master`; these are things agy can do with **no GUI and no maintainer decision**,
gated entirely by the existing test suite. Record completions in
`docs/AGY-LOG.md`; do **not** prune this file (the maintainer removes finished
items when merging `antigravity → master`).

## PRIMARY — calcard migration (ROADMAP: "Outsource iCalendar/vCard parsing to calcard")

Replace the hand-rolled RFC 5545/6350 text layers with the `calcard` crate
(already a workspace dependency: `calcard = "0.3.9"` in `rust/Cargo.toml`).

**Targets to remove/replace:**
- `rust/crates/jmap-vcard/src/syntax.rs` (~410 lines) — vCard lexer/emitter
- `rust/crates/jmap-ical/src/syntax.rs` (~628 lines) — iCal lexer/emitter
  (mind `jmap-ical/src/zone.rs` for time zones)

**Keep, do not rewrite:** the semantic mapping in `jmap-vcard/src/contact.rs`
and `jmap-ical/src/event.rs` (JSContact↔vCard, JSCalendar↔iCal). Adapt them to
consume/produce calcard's parsed form instead of the hand-rolled syntax types.

**Acceptance suite = ALL existing fixture/round-trip tests in both crates.** Keep
them green at every push. A behaviour difference calcard introduces is a
**finding to log in `docs/AGY-LOG.md`**, never a test to weaken or delete.

**This is multi-session — progress it incrementally**, one coherent sub-layer per
session, whole-crate tests green each time. Do NOT report BLOCKED just because it
won't fit one session. Suggested order:
1. jmap-vcard: route **parsing** through calcard; adapt `contact.rs`; tests green.
2. jmap-vcard: route **emitting** through calcard; delete the dead syntax code; tests green.
3. jmap-ical: **parsing** through calcard (handle `zone.rs` zones); tests green.
4. jmap-ical: **emitting** through calcard; delete the dead syntax code; tests green.

When both hand-rolled `syntax.rs` layers are gone and all tests pass, append
`CALCARD COMPLETE <UTC date>` to `docs/MILESTONES.md` (the ROADMAP tag mechanism).

## SECONDARY — small headless items (only if the calcard step is blocked)
- Empty-`ORG`-name emission: an organisation whose `name` is `""` rather than
  absent — decide the `ORG` line output and add a round-trip test (`jmap-vcard`).
- Windows time-zone names: add a test confirming the refusal path is correct
  (`jmap-ical`), unsendable-by-design.
