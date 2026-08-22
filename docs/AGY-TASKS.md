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

## Batch 4 (2026-08-22) — filed bugs + real unmapped EDS fields
History: batches 1–3 are complete and merged to master (`f4c1ae7`, `87b0856`,
`036b46f`); see git log for scope. Same standing rules: TDD (red first),
whole-crate green at every push, one increment per session, log to
`docs/AGY-LOG.md`, **check the current code first** — if an item turns out
already covered, write that finding and move on. Log genuine product decisions
as findings rather than guessing. Do NOT edit `docs/BACKLOG.md` (outside the
lane — the maintainer prunes it); reference its entries from `docs/AGY-LOG.md`
instead.

1. **Fix the filed round-trip fixpoint bug.** `docs/BACKLOG.md` records a
   `jmap-vcard` proptest fixed-point failure that reproduces on unmodified
   master — a trailing-whitespace round-trip nit, with its minimal input
   recorded in the entry. Reproduce with that input (red), fix, and pin the
   minimal input as a named regression test (not just the proptest seed).
2. **`TEL;TYPE=WORK,VOICE,FAX` dual-role mapping** (from BACKLOG's fidelity
   notes): a combined voice+fax work number currently picks one winner and
   loses the distinction. Decide a deterministic mapping (e.g. fill both
   `E_CONTACT_PHONE_BUSINESS` and `_BUSINESS_FAX`, export re-merging or
   keeping them split — document which) and extend batch 3's 19-field matrix
   tests to cover multi-role TYPEs.
3. **`X-TWITTER`, `X-SIP`, and IM URI-scheme long tail** (from BACKLOG's
   fidelity notes): audit the current IM/social scheme table (batch 1 added
   many); add the missing ones EDS/Evolution recognize, with round-trip tests
   and a `VCARD-MAPPING.md` note per scheme.
4. **`FILE-AS` mapping.** EDS has `E_CONTACT_FILE_AS`; jmap-vcard maps nothing
   to it (grep: no `FILE-AS`/`X-EVOLUTION-FILE-AS` handling). Map it, and
   document its relationship to the batch-3-preserved `SORT-STRING` (vCard 3.0
   twin of the same concept) in `VCARD-MAPPING.md` — one must not clobber the
   other on round-trip.
5. **Promote `LOGO` and `KEY` from preserved blobs to real fields.** Batch 3
   characterized them as preserved-unmapped; EDS has first-class
   `E_CONTACT_LOGO` and `E_CONTACT_X509_CERT` (check the exact field semantics
   first — audit-first; if the EDS shape doesn't fit, log the finding and keep
   preservation). `LOGO` should reuse the `PHOTO` base64/URI machinery.
6. **Apple-style property groups** (`item1.TEL` + `item1.X-ABLabel`, as iCloud
   and macOS exporters emit): audit what the parser currently does with a
   group prefix (corrupt? drop? pass through?). Minimum bar: grouped
   properties import without loss and round-trip stably; stretch: map
   `X-ABLabel` values to the closest TYPE/EDS slot with the label preserved.
   Fixture-driven tests with representative iCloud-shaped vCards.

**With no active items left, the agy shift reports `AGY-SHIFT: BLOCKED` and
pauses** — which is correct; it costs nothing and auto-resumes the moment this
file changes.

## To re-arm the lane (when batch 3 drains)
Add a numbered list of fresh **`jmap-vcard`-only, headless, test-gated** tasks
above (same rules). Changing this file auto-clears the driver's blocked-pause,
so agy picks the new batch up on its next boot. Do not wander into the frozen
crates to stay busy; a clean `BLOCKED` beats a merge conflict.
