# Antigravity task list (polish lane)

Headless, in-lane work for the Antigravity polish shift (driven by
`infra/agy-night-shift/agy-prompt.md`). Claude works the priority milestones on
`master`; these are things agy can do with **no GUI and no maintainer decision**,
gated entirely by the existing test suite. Record completions in
`docs/AGY-LOG.md`; do **not** prune this file (the maintainer removes finished
items when merging `antigravity → master`).

## ⚠ COLLISION AVOIDANCE — read first (2026-08-19)
To keep the `antigravity → master` merge trivial, **stay inside `jmap-vcard`**
(its `src/` and `tests/`) for every task below. Claude (master) is actively
editing these — do **NOT** touch them from this lane:
- `jmap-ical`, `jmap-cal-sync`, `jmap-backend-cal` — Claude adds the
  `BusyPeriod → VFREEBUSY` free/busy marshaller and the `get_free_busy_sync`
  vfunc here for Track E Path A (scheduling).
- `jmap-proto`, `jmap-client`, `jmap-mock` — Track E Phase 0 (`principals.rs`),
  Track D (create/delete book+calendar), and SRV autodiscovery live here.
- `jmap-collection-sync`, `jmap-backend-{book,collection,core}`, `jmap-config`,
  `jmap-mail*` — Track D wiring, SRV call site (b), the unsafe-audit helpers.

If a task would need a file outside `jmap-vcard`, **log it as a finding in
`docs/AGY-LOG.md` and skip it** — do not reach into Claude's lane. `jmap-ical`
fidelity (former item 5/6) is **frozen for this lane** until Path A lands; Claude
owns that crate for now. Adding a dev-dep touches the shared `Cargo.lock`; that
is fine (it merges), but keep `Cargo.toml` edits limited to `jmap-vcard`.

## Done and pruned (2026-08-19)
Items 1–6 of the previous list are **complete** (see `docs/AGY-LOG.md`):
multi-`TYPE` phones, IM/social schemes, multi-component `ORG`/`TITLE`, bare-year
dates, `merge_units` empty-name, and mutation testing of both crates
(`jmap-vcard` caught-mutants 344→402; `jmap-ical` to 294 tests). The new items
are all **`jmap-vcard`-only** so they never collide with the Path A / Track D
work on the calendar and protocol crates.

## Contact / vCard fidelity & robustness (`jmap-vcard` ONLY) — HEADLESS
Same rules as before: TDD, whole-crate tests green at every push, one
self-contained increment per session, log completions in `docs/AGY-LOG.md`,
**check the current code first** (some may be partly done — do the undone parts).
A behaviour choice that is a genuine product decision is a **finding to log**,
not something to guess at.

1. **Structure-aware round-trip fuzzing of `jmap-vcard`** with `proptest` +
   `arbitrary` (dev-deps, **stable** — never `cargo-fuzz`, it needs nightly and
   breaks the pinned-stable reproducibility). Generate random JSContact and random
   vCard; assert (a) no panic and (b) round-trip stability (vCard→JSContact→vCard
   and JSContact→vCard→JSContact converge). Fix any panic found, or log it as a
   finding if the input is genuinely out of contract. Keep generators shrinking so
   failures are minimal. **Scope: `jmap-vcard` only** — the iCal side is frozen
   for this lane (see collision note).
2. **`KIND` + `MEMBER` (group cards).** Characterize how a vCard `KIND:group`
   with `MEMBER` lines maps through JSContact and to EDS (`E_CONTACT_LIST` /
   list members), and whether it round-trips without dropping members. Pin with
   round-trip tests; improve the mapping where the right answer is unambiguous,
   else log a finding.
3. **`ALTID` / `LANGUAGE` alternate representations.** vCards can carry the same
   property in several languages grouped by `ALTID`. Characterize whether our
   mapping preserves the alternates (or deterministically picks one) rather than
   dropping or duplicating them; add round-trip tests pinning the behaviour.
4. **`PREF` → primary selection.** Beyond parsing `PREF` (already done), verify
   that among multiple `EMAIL`/`TEL`/`ADR` the `PREF`-lowest becomes EDS's primary
   field, and that the ordering round-trips. Test the tie-break and the
   no-`PREF`-present fallback; log a finding if the "which is primary" rule is a
   product decision.
5. **Full structured `ADR` + `LABEL`.** Confirm all seven `ADR` components
   (po-box, ext, street, locality, region, postcode, country) plus a `LABEL`
   param round-trip vCard↔JSContact↔EDS without loss; test empty-component and
   multi-value cases.
6. **Unknown `X-` property preservation.** Verify that `X-` properties we do not
   explicitly map survive a round-trip (carried through, not silently dropped);
   add tests. If they are dropped by design, log that as a finding with the
   rationale rather than forcing preservation.

## Documentation (zero-collision, `jmap-vcard` scope)
7. **`docs/VCARD-MAPPING.md`** — a reference table: each vCard property/param →
   its JSContact representation → the EDS `E_CONTACT_*` field it lands in, with a
   note on any lossy or product-decision cases (cross-reference the findings in
   `docs/AGY-LOG.md`). A brand-new file only this lane touches, so it never
   collides. Keep it accurate to the current `contact.rs`; cite function names.

Only when you exhaust these AND find no further in-lane `jmap-vcard` sub-step
should you report `AGY-SHIFT: BLOCKED` — the maintainer refills this file then.
Do not wander into the frozen crates to stay busy; a clean `BLOCKED` is better
than a merge conflict.
