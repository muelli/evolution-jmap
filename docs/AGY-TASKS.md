# Antigravity task list (polish lane)

Headless, in-lane work for the Antigravity polish shift (driven by
`infra/agy-night-shift/agy-prompt.md`). Claude works the priority milestones on
`master`; these are things agy can do with **no GUI and no maintainer decision**,
gated entirely by the existing test suite. Record completions in
`docs/AGY-LOG.md`; do **not** prune this file (the maintainer removes finished
items when merging `antigravity → master`).

## ⚠ COLLISION AVOIDANCE — read first (2026-08-19)
To keep the `antigravity → master` merge trivial, **stay inside `jmap-vcard`**
(its `src/` and `tests/`, plus `docs/VCARD-MAPPING.md`) for every task below.
Claude (master) is actively editing these — do **NOT** touch them from this lane:
- `jmap-ical`, `jmap-cal-sync`, `jmap-backend-cal` — free/busy marshaller and the
  `get_free_busy_sync` vfunc (Track E Path A). `jmap-ical` fidelity stays **frozen
  for this lane** until Path A lands; Claude owns that crate for now.
- `jmap-proto`, `jmap-client`, `jmap-mock` — Track E principals, Track D
  create/delete book+calendar, SRV autodiscovery.
- `jmap-collection-sync`, `jmap-backend-{book,collection,core}`, `jmap-config`,
  `jmap-mail*`, and the packaging/CI files — collection discovery + the auth-retry
  loop (roadmap item 7) and the lintian/RUNPATH CI fix (item 8) live here.

If a task would need a file outside `jmap-vcard`, **log it as a finding in
`docs/AGY-LOG.md` and skip it** — do not reach into Claude's lane. Adding a
dev-dep touches the shared `Cargo.lock` (fine, it merges), but keep `Cargo.toml`
edits limited to `jmap-vcard`.

## Done and pruned (2026-08-19)
Complete (see `docs/AGY-LOG.md`), pruned on the `antigravity → master` merge
(`f4c1ae7`):
- **First batch:** multi-`TYPE` phones, IM/social schemes, multi-component
  `ORG`/`TITLE`, bare-year dates, `merge_units` empty-name, mutation testing of
  both crates (`jmap-vcard` caught-mutants 344→402; `jmap-ical` to 294 tests).
- **Second batch:** structure-aware round-trip fuzzing (`proptest` — zero panics,
  fixed-point stable); `KIND`+`MEMBER` group cards; `ALTID`/`LANGUAGE`
  alternates; `PREF` → primary selection + ordering; full structured `ADR`+`LABEL`;
  unknown `X-` property preservation (dropped-by-design, sync-safe); and the
  `docs/VCARD-MAPPING.md` reference. All `jmap-vcard`-only.

## Contact / vCard 3.0 fidelity & robustness (`jmap-vcard` ONLY) — HEADLESS (refill 2026-08-19)
`jmap-vcard` maps JSContact ↔ **vCard 3.0** (RFC 2426), the format EDS/Evolution
3.52 uses — so these are 3.0 concerns, not 4.0 conversions. Same rules as before:
TDD, whole-crate tests green at every push, one self-contained increment per
session, log completions in `docs/AGY-LOG.md`, **check the current code first**
(some may be partly done — do the undone parts). A behaviour choice that is a
genuine product decision is a **finding to log**, not something to guess at.

1. **Line folding / unfolding (RFC 2426 §2.6).** A value longer than 75 octets
   must fold on write and unfold losslessly on read. Test round-trip of a long
   `NOTE` and an inline base64 `PHOTO`; test pre-folded input (CRLF + leading
   space/tab continuations); and confirm a multi-byte UTF-8 sequence is never
   split across a fold. Fix or log a finding if any of these lose data.
2. **Value escaping (RFC 2426 §2).** `\n`, `\,`, `\;`, `\\` in text values must
   escape on write and unescape on read with no loss and no double-escaping.
   Test a `NOTE` containing all four, a comma inside an `ORG` unit, and a
   semicolon inside an `ADR` component; assert fixed-point convergence.
3. **`CATEGORIES` ↔ `E_CONTACT_CATEGORY_LIST`.** Comma-separated categories
   round-trip — order preserved, commas within a category escaped — for empty,
   single, and multiple; pin with tests, else log a finding.
4. **`NICKNAME` and `URL`.** Characterize `NICKNAME` (single and multiple) and
   one-or-more `URL` properties into their EDS fields (`E_CONTACT_NICKNAME`,
   homepage/blog/etc.); pin round-trips; log a finding where the slotting is a
   product decision rather than obvious.
5. **Non-ASCII and `CHARSET`/`ENCODING` params.** Evolution and older clients
   export vCard 3.0 with `;CHARSET=UTF-8` and sometimes
   `;ENCODING=QUOTED-PRINTABLE`. Verify non-ASCII names/values round-trip; decide
   (and pin, or log as a finding) whether a QUOTED-PRINTABLE-encoded value is in
   contract or cleanly rejected.
6. **Inline `PHOTO` (base64) vs URI.** A `PHOTO;ENCODING=b;TYPE=JPEG:` inline
   image round-trips to the EDS photo field and back with its media type intact;
   test the URI-valued `PHOTO` variant too. Log a finding if either is lossy by
   design.

## Documentation (zero-collision, `jmap-vcard` scope)
7. **Keep `docs/VCARD-MAPPING.md` current.** As items 1–6 are characterized,
   extend the reference table and the product-decision/lossy-case notes so the
   doc stays accurate to `contact.rs` (cite function names). This file is only
   touched by this lane, so it never collides.

Only when you exhaust these AND find no further in-lane `jmap-vcard` sub-step
should you report `AGY-SHIFT: BLOCKED` — the maintainer refills this file then.
Do not wander into the frozen crates to stay busy; a clean `BLOCKED` is better
than a merge conflict.
