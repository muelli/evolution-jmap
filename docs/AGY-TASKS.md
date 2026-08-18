# Antigravity task list (polish lane)

Headless, in-lane work for the Antigravity polish shift (driven by
`infra/agy-night-shift/agy-prompt.md`). Claude works the priority milestones on
`master`; these are things agy can do with **no GUI and no maintainer decision**,
gated entirely by the existing test suite. Record completions in
`docs/AGY-LOG.md`; do **not** prune this file (the maintainer removes finished
items when merging `antigravity → master`).

The calcard migration (former PRIMARY) is **DONE and merged to master
(2026-08-19)** — both hand-rolled `syntax.rs` layers are gone. The items below
are the next fidelity polish: all **semantic-mapping** work in
`jmap-vcard/src/contact.rs` or `jmap-ical/src/event.rs`, gated by the test suite.
Same rules: TDD, whole-crate tests green at every push, log completions in
`docs/AGY-LOG.md`. **Check the current code first** — some of these may be partly
done; do the undone parts and add round-trip tests that pin the behaviour. A
behaviour choice that is genuinely a product decision is a **finding to log**,
not something to guess at.

## Contact / vCard fidelity (`jmap-vcard/src/contact.rs`) — HEADLESS only
1. **Multi-`TYPE` phone numbers** (`TEL;TYPE=WORK,VOICE,FAX` and friends):
   characterize how a number carrying several TYPE tokens maps to EDS's
   business / business-fax fields — today picking one winner loses the
   voice/fax distinction. Add round-trip tests pinning the behaviour; improve the
   mapping where the right answer is unambiguous. Do NOT try to verify the
   contact editor's display — that is the maintainer's GUI check.
2. **IM / social online-service URI schemes**: map the remaining schemes onto
   `onlineServices` — `X-TWITTER`, `X-SIP`, and the IM protocols not already
   handled (MSN, Yahoo, AIM/ICQ if not done, …). Read `contact.rs` for what is
   already mapped; add the undone ones with round-trip tests.
3. **Multi-component `ORG` / `TITLE`** round-trip: ensure a vCard `ORG` with 3+
   components (incl. a 4th mapping to `E_CONTACT_OFFICE`) round-trips
   vCard↔JSContact without dropping components. Headless round-trip only.
4. **Bare-year dates** (`BDAY`/anniversary stated as a year only): characterize
   and test how a year-only date maps (EDS clamps); pin it with a round-trip test.

## Calendar / iCal fidelity (`jmap-ical/src/event.rs`) — HEADLESS only
5. **`merge_units` empty-name edge case**: a unit with an empty name is currently
   dropped — characterize, add a test, and fix if the drop is wrong.

Work one increment per session, each self-contained so the periodic
`antigravity → master` merge stays trivial. Only when you exhaust these AND find
no further in-lane headless sub-step should you report `AGY-SHIFT: BLOCKED` — the
maintainer refills this file then.
