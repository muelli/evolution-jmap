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

## M7 setup UI (account assistant)
- **Whitespace in the identity address slips through setup.** Evolution's own
  identity page accepts `alice@ example.com` (embedded space) and lets the
  assistant advance; on the JMAP server-settings page our `check_complete` does
  not block it either, so setup can complete. `complete::is_address` *does*
  reject embedded whitespace (unit-tested), so the malformed string is not
  reaching it — Evolution normalises the address, or we read it post-
  normalisation, before our guard runs. **First settle the open question**:
  does the space actually survive into the created account's identity (the
  `From:` address), or is it stripped? If it survives it is a real defect (a
  `From:` header containing a space); if stripped it is benign and needs
  nothing. If a safety net is added in `check_complete`, comment it as
  compensating for Evolution's lenient identity page and file an upstream bug
  against Evolution (its identity page should reject an address with
  whitespace). Deferred edge case, not a release blocker.

## Cross-cutting, noticed while wiring OAuth 2.0 onto the connect path
- ~~**`ConnectError`'s own messages are not marked for translation.**~~
  **Closed 2026-08-16.** `CredentialsRequired`, `NoSuchCollection`,
  `NoDefaultCollection`, `Collection::noun`, `no_source_gerror`, and
  `access_token`'s two fallback messages now go through
  `translate`/`translate_with`; `jmap-backend-core/src/{connect,oauth2}.rs`
  are in `po/POTFILES.in` and `po/evolution-jmap.pot` is regenerated.
  `ConnectError::OAuth2(message)` stays untouched on purpose — that string is
  EDS's own, not this project's to translate.
