# Backlog — deferred hardening

Real but low-leverage items, parked until the usability priorities in
`docs/ROADMAP.md` ("CURRENT PRIORITY") are done. **Do not implement these
now** — add to the list when you notice one and would otherwise be tempted
to polish a completed backend. A later hardening pass works through them.

## EDS 3.60+ compatibility (M10 area, found by the version matrix)
- ~~`jmap-backend-book/src/marshal.rs`'s `e_vcard_to_string` call, and
  `eds-sys`/`jmap-mail`'s `CamelFolderSearch`/summary-record surface~~ —
  **fixed 2026-08-17** via version-conditional FFI (`eds-sys::compat`,
  `EDS_FEATURES` cfg markers detected from the installed headers) and the
  `jmap-mail` Camel port onto 3.60's base-class `folder_search_sync`. Both
  the pinned-3.52 and 3.60.2 legs now build and pass their full suites.
  Detail and the docker repro recipe: `docs/eds-version-matrix.md`.
- **(B) Remaining — contact-model semantics needs a maintainer decision.**
  Three `eds-sys/tests/contacts.rs` assertions still fail on 3.60: it's not
  an FFI bug, EDS just answers differently there (`X-JABBER`/`X-AIM`/
  `X-GADUGADU` resolve to the first home slot instead of the multi-valued IM
  field; `ANNIVERSARY`/`X-EVOLUTION-ANNIVERSARY` swap places;
  `E_CONTACT_NAME_OR_ORG`'s derivation changes). What the plugin should
  emit/read on a newer EDS is a `jmap-vcard` mapping call, not one to guess
  here — the three questions are listed in `docs/eds-version-matrix.md`'s
  section (B).
- **(C) Remaining — clippy can't gate the 3.60 leg yet.** `ci/eds-matrix.sh`
  only runs `cargo test`, not clippy; adding `-D warnings` there today would
  trip on five `unnecessary_transmute` warnings in bindgen's output for
  glibc's `_IO_FILE` bitfield accessor (a container/rustc artifact, nothing
  of ours). Low-leverage hardening, not a regression — the pinned-3.52 leg
  is already clippy-clean.
- None of this affects the pinned-3.52 leg the plugin actually ships
  against; parked here rather than fixed now per M10's explicit
  make-it-visible-not-auto-port scope.

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
- ~~**Whitespace in the identity address slips through setup.**~~
  **Settled 2026-08-16, no code change needed.** The open question this entry
  asked — does a space typed into Evolution's (lenient) identity page survive
  into the committed account, or is it stopped? — is answerable from
  Evolution's own call order, not just this crate's source: `GtkAssistant`'s
  `prepare` vfunc
  (`e-mail-config-assistant.c:969`, `mail_config_assistant_prepare`) calls
  `e_mail_config_page_setup_defaults` synchronously the first time the JMAP
  server-settings page is visited, before the user can interact with it;
  `mail_config_service_page_setup_defaults`
  (`e-mail-config-service-page.c:585-613`) runs every candidate backend's
  `setup_defaults` (this project's `backend.rs:873`, which writes the
  identity string — space included — via `apply()`) and then activates the
  page's combo box, whose `"changed"` handler
  (`e-mail-config-service-page.c:576`) fires `e_mail_config_page_changed` →
  `mail_config_assistant_page_changed`
  (`e-mail-config-assistant.c:279-285`) → `check_complete`
  (`backend.rs:990`), all inside that one `prepare` call. So by the time the
  JMAP page is interactive, `complete::check`'s `is_address` has already seen
  the space-containing identity and refused it — `check_complete` returns
  `FALSE`, *Next*/*Apply* stays insensitive, and there is no path through the
  assistant or the account editor that commits an account with a space in its
  identity. The space is stopped, not stripped, but the practical answer is
  the same as "stripped": benign, nothing to fix. (Verified against the
  upstream Evolution 3.52.3 source, not by running the GUI — the call chain
  above is deterministic and does not depend on timing.)

## Cross-cutting, noticed while wiring OAuth 2.0 onto the connect path
- ~~**`ConnectError`'s own messages are not marked for translation.**~~
  **Closed 2026-08-16.** `CredentialsRequired`, `NoSuchCollection`,
  `NoDefaultCollection`, `Collection::noun`, `no_source_gerror`, and
  `access_token`'s two fallback messages now go through
  `translate`/`translate_with`; `jmap-backend-core/src/{connect,oauth2}.rs`
  are in `po/POTFILES.in` and `po/evolution-jmap.pot` is regenerated.
  `ConnectError::OAuth2(message)` stays untouched on purpose — that string is
  EDS's own, not this project's to translate.
