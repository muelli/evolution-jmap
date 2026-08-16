# Backlog — deferred hardening

Real but low-leverage items, parked until the usability priorities in
`docs/ROADMAP.md` ("CURRENT PRIORITY") are done. **Do not implement these
now** — add to the list when you notice one and would otherwise be tempted
to polish a completed backend. A later hardening pass works through them.

## Contact / vCard fidelity (M3 area, backend already works)
- Multi-`ORG`/`TITLE` and multi-component field behaviour vs Evolution's
  contact editor (which components it shows, how it round-trips a 4th `ORG`
  component, `E_CONTACT_OFFICE`).
- `TEL;TYPE=WORK,VOICE,FAX` filling both business and business-fax fields
  (picking a winner loses the voice/fax distinction).
- `X-TWITTER`, `X-SIP`, and IM URI schemes (AIM, ICQ, MSN, Yahoo, …):
  mapping and contact-editor behaviour unmeasured.
- Photo handling: `VALUE=uri` rendering, what the editor writes for a
  replaced or cleared photo (currently inferred, not measured).
- Birthday/deathday/anniversary stated as a bare year (EDS clamps).
- An organisation whose `name` is `""` rather than absent: the `ORG` line
  writes an empty first component, the reader reads back no name, and the
  save patches `name: null` on every save of that entry. Loses nothing a
  user can see — normalising `""` to absent may be right — but it writes a
  needless patch. Maintainer's call which.

## Calendar / iCal fidelity (M4 area, backend already works)
- `UNTIL` values the parser itself refuses (invisible to `jmap-ical`).
- Windows time-zone names (unsendable by design — confirm the refusal path).
- ~~`merge_units` degenerate case: a unit with an empty name is dropped.~~
  Fixed 2026-08-16 (`jmap-book-sync: keep an org unit the ORG line has no
  name to state`) — the work was finished and green before this file landed.

## Cross-cutting
- calcard migration (ROADMAP standing directive) — replace the hand-rolled
  iCal/vCard text layers; robustness/liability, not a functional gap.
- Contact-editor behavioural unknowns generally: many "needs human
  verification in real Evolution" notes in `docs/NIGHT-LOG.md`.
