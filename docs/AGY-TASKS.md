# Antigravity task list (polish lane)

Headless, in-lane work for the Antigravity polish shift (driven by
`infra/agy-night-shift/agy-prompt.md`). Claude works the priority milestones on
`master`; these are things agy can do with **no GUI and no maintainer decision**,
gated entirely by the existing test suite. Record completions in
`docs/AGY-LOG.md`; do **not** prune this file (the maintainer removes finished
items when merging `antigravity → master`).

## ⚠ COLLISION AVOIDANCE — read first
To keep the `antigravity → master` merge trivial, **stay inside `jmap-vcard`**
(its `src/` and `tests/`, plus `docs/VCARD-MAPPING.md`). Claude (master) owns
everything else — `jmap-ical`, `jmap-cal-sync`, `jmap-backend-cal` (free/busy),
`jmap-proto`, `jmap-client`, `jmap-mock` (principals, Track D, SRV),
`jmap-collection-sync`, `jmap-backend-{book,collection,core}`, `jmap-config`,
`jmap-mail*`, and the packaging/CI files (collection discovery + auth loop,
item 7; the lintian/RUNPATH CI fix, item 8). If a task would need a file outside
`jmap-vcard`, **log it as a finding in `docs/AGY-LOG.md` and skip it**.

## Batch 3 (2026-08-22) — fidelity long tail, audit-first
History: **Batch 1** (`f4c1ae7`) and **Batch 2** (`87b0856`) are merged; see git
log for their scope (multi-`TYPE` phones, IM schemes, folding/escaping,
`CATEGORIES`, `PHOTO`, proptest fuzzing, `VCARD-MAPPING.md`).

Standing rules, unchanged: TDD (red first), whole-crate green at every push, one
increment per session, log to `docs/AGY-LOG.md`, **check the current code first**
— several items below are phrased as audits because the maintainer grepped, not
proved, the gap. If an item turns out already covered, write that finding to
`docs/AGY-LOG.md` and move to the next. Log genuine product decisions as
findings rather than guessing.

1. **Standard-property preservation audit (GEO, TZ, MAILER, PRODID, REV,
   SORT-STRING, CLASS, SOUND, LOGO).** The unknown-property round-trip
   mechanism preserves unknown `X-` properties (`X-EVOLUTION-UNKNOWN`); check
   what happens to *standard* vCard 3.0 properties Evolution has no
   `E_CONTACT_*` field for. If they are dropped on import→export, extend the
   preservation mechanism to carry them (same shape as the `X-` path), or — if
   preservation is architecturally wrong for some (e.g. `REV`, which the
   exporter should own) — document the deliberate drop per property in
   `docs/VCARD-MAPPING.md`. Round-trip tests either way.
2. **Phone-TYPE completeness vs EDS.** The mapping references
   `E_CONTACT_PHONE_{BUSINESS,BUSINESS_FAX,HOME,OTHER,OTHER_FAX}`; EDS also has
   MOBILE/CELL, PAGER, CAR, ISDN, CALLBACK, COMPANY, PRIMARY, RADIO, TELEX,
   TTYTDD, HOME_FAX, BUSINESS_2, HOME_2, and ASSISTANT phones. Audit which
   `TEL;TYPE=` combinations map to which fields today, close the real gaps
   (CELL is the most common TYPE in the wild), and pin the full matrix with
   round-trip tests + a `VCARD-MAPPING.md` table.
3. **Remaining X-EVOLUTION-* fields.** `X-EVOLUTION-{ANNIVERSARY,SPOUSE}` are
   mapped; EDS also round-trips `X-EVOLUTION-{MANAGER,ASSISTANT,BLOG-URL,
   VIDEO-URL}` (and `FBURL`/`CALURI` exist as `E_CONTACT_{FREEBUSY_URL,
   CALENDAR_URI}` — grep says those two ARE referenced, so audit first). Map
   the missing ones to/from JSContact sensibly (JSContact has `relatedTo` /
   `links`; if no clean JSContact home exists, preserve as vendor properties)
   with tests.
4. **EMAIL and ADR slot completeness.** Only `E_CONTACT_EMAIL_1` and
   `E_CONTACT_ADDRESS_LABEL_HOME` show up in a grep; EDS has `EMAIL_2..4` (and
   the `E_CONTACT_EMAIL` attribute list) plus `ADDRESS_LABEL_{WORK,OTHER}`.
   Audit how multiple EMAILs and the WORK/OTHER address labels round-trip;
   close gaps with tests covering 3+ emails and all three label slots,
   including `PREF` interplay.
5. **vCard 2.1 legacy import tolerance.** Real exporters (old Outlook, feature
   phones) still emit 2.1: `ENCODING=QUOTED-PRINTABLE` values, bare type words
   (`TEL;HOME:`), `CHARSET=` params. Import-side tolerance only — export stays
   strictly 3.0. Tests with representative 2.1 fixtures; document accepted
   subset in `VCARD-MAPPING.md`.
6. **Round-trip fixpoint property test.** Add a proptest that for arbitrary
   generated contacts, vCard→EContact→vCard→EContact→vCard reaches a fixpoint
   by the second export (export₂ == export₃ byte-identical). Reuse the
   batch-1/2 generators; shrinkage on failure should name the property that
   oscillates. This is the lane's standing regression net for everything above.

**With no active items left, the agy shift reports `AGY-SHIFT: BLOCKED` and
pauses** — which is correct; it costs nothing and auto-resumes the moment this
file changes.

## To re-arm the lane (when batch 3 drains)
Add a numbered list of fresh **`jmap-vcard`-only, headless, test-gated** tasks
above (same rules). Changing this file auto-clears the driver's blocked-pause,
so agy picks the new batch up on its next boot. Do not wander into the frozen
crates to stay busy; a clean `BLOCKED` beats a merge conflict.
