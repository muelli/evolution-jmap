# Backlog — deferred hardening

Real but low-leverage items, parked until the usability priorities in
`docs/ROADMAP.md` ("CURRENT PRIORITY") are done. **Do not implement these
now** — add to the list when you notice one and would otherwise be tempted
to polish a completed backend. A later hardening pass works through them.

## Real-server readiness — OAuth 2.0 discovery vs. a misconfigured issuer
- **Maintainer's call, not code to guess at.** Found 2026-08-18 (see
  `docs/NIGHT-LOG.md` "REAL-SERVER FINDING: OAuth 2.0 discovery's issuer
  check rejects this Stalwart"): `jmap_client::oauth::discover` enforces RFC
  8414 §3.3 (the metadata document's stated `issuer` must equal the one
  asked for) and the disposable Stalwart test deployment fails it —
  `SystemSettings.defaultHostname=example.com` plus a hardcoded `https://`
  scheme (the same root cause already on record for the `apiUrl` finding)
  means its `/.well-known/oauth-authorization-server` document names issuer
  `https://example.com` no matter which reachable address it was fetched
  from. `discover_and_register` (`jmap-config/src/oauth2_setup.rs`) builds
  its issuer from exactly the host/port a user types into account setup, so
  any self-hosted deployment with this same mismatch cannot use OAuth 2.0
  through this client today.
- Unlike the `apiUrl` fix (`ClientBuilder::rebase_urls_to_origin`, shipped
  unilaterally because it only changes which reachable address
  already-authenticated requests target), relaxing the §3.3 issuer check —
  even behind an opt-in flag — changes what a client trusts a self-hosted
  deployment's own metadata to assert about *itself* before any
  authentication has happened. That is the mix-up defence RFC 8414 §3.3
  exists for, not a routing convenience, so it needs the maintainer's
  explicit sign-off rather than an agent's guess. Candidate shapes, for
  whichever the maintainer prefers: (a) leave it strict and document that a
  self-hosted deployment's `defaultHostname`/public URL must actually match
  how clients reach it — the normal ABI-style contract EDS modules already
  hold servers to elsewhere; (b) an opt-in analogous to
  `rebase_urls_to_origin` that trusts the connected origin over the
  document's stated issuer, on the reasoning that this call site is
  first-party discovery from user-typed input, not delegated/redirected
  discovery from an untrusted source — but explicitly flagged as a trust
  decision, not a reachability one.

## EDS 3.60+ compatibility (M10 area, found by the version matrix)
- ~~`jmap-backend-book/src/marshal.rs`'s `e_vcard_to_string` call, and
  `eds-sys`/`jmap-mail`'s `CamelFolderSearch`/summary-record surface~~ —
  **fixed 2026-08-17** via version-conditional FFI (`eds-sys::compat`,
  `EDS_FEATURES` cfg markers detected from the installed headers) and the
  `jmap-mail` Camel port onto 3.60's base-class `folder_search_sync`. Both
  the pinned-3.52 and 3.60.2 legs now build and pass their full suites.
  Detail and the docker repro recipe: `docs/eds-version-matrix.md`.
- ~~**(B) — the 3 `eds-sys/tests/contacts.rs` failures on 3.60.**~~
  **Test-level fix landed 2026-08-17.** They characterized EDS's own C
  behaviour, not a `jmap-vcard` mapping choice, so the assertions are now
  version-aware (`eds_death_date_field` cfg in `eds-sys/build.rs`) rather than
  guessed; `ci/eds-matrix.sh` passes with 0 failures on both legs, verified
  locally in the pinned 3.60.2 container. Detail:
  `docs/eds-version-matrix.md` (B).
- **(B′) Still open — a `jmap-vcard` mapping decision, not a test fix.**
  Whether the plugin's *own* mapping should change on a newer EDS: should a
  JMAP contact's chat handle be read from/written to the multi-valued IM
  field or the first home slot (now that EDS 3.60 prefers the latter);
  should the plugin write `ANNIVERSARY` or `X-EVOLUTION-ANNIVERSARY`; does
  anything rely on `E_CONTACT_NAME_OR_ORG`'s sort-order shape. Maintainer's
  call, not code to guess at — `docs/eds-version-matrix.md` (B) has the
  measured facts these questions turn on.
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
