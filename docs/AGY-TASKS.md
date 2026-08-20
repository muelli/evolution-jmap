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

## ✅ LANE DRAINED (2026-08-20) — no active tasks
Both vCard-fidelity batches are complete and merged to `master`:
- **Batch 1** (`f4c1ae7`): multi-`TYPE` phones, IM/social schemes, multi-component
  `ORG`/`TITLE`, bare-year dates, `merge_units` empty-name, mutation testing,
  and the first `KIND`/`MEMBER`, `ALTID`/`LANGUAGE`, `PREF`, `ADR`+`LABEL`,
  unknown-`X-`, and `VCARD-MAPPING.md` work.
- **Batch 2** (`87b0856`): line folding/unfolding (RFC 2426 §2.6), value escaping,
  `CATEGORIES` ↔ `E_CONTACT_CATEGORY_LIST`, `NICKNAME`/`URL`, non-ASCII +
  `CHARSET`/`ENCODING`, and inline `PHOTO` (base64 + URI) — each with round-trip
  tests and proptest fuzzing, plus the `VCARD-MAPPING.md` refresh.

The known vCard 3.0 fidelity corners are covered. **With no active items below,
the agy shift will report `AGY-SHIFT: BLOCKED` and pause** — which is correct;
it costs nothing and auto-resumes the moment this file changes.

## To re-arm the lane
Add a numbered list of fresh **`jmap-vcard`-only, headless, test-gated** tasks
below this line (same rules: TDD, whole-crate green at every push, one increment
per session, log to `docs/AGY-LOG.md`, check current code first, log genuine
product decisions as findings rather than guessing). Changing this file
auto-clears the driver's blocked-pause, so agy picks the new batch up on its next
boot. Do not wander into the frozen crates to stay busy; a clean `BLOCKED` beats a
merge conflict.

<!-- (no active tasks — add a numbered batch here to re-arm) -->
