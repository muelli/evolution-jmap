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
- **`jmap-ical` round trip is not a fixed point for a whitespace-only
  `CATEGORIES` value (found 2026-08-19).** Same shape as the `jmap-vcard`
  entry below, different crate: `evolution-jmap-ical/tests/proptest_fuzz.rs`'s
  `prop_ical_roundtrip_reaches_fixed_point_stability` fails on a random seed —
  reproduces on unmodified `master` (`ab35cde`), so not a regression from any
  work in flight. Minimal input:
  `BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//Example//NONSGML//EN\r\nBEGIN:VEVENT\r\nUID:evt1\r\nDTSTART:20260115T130000Z\r\nCATEGORIES:\ \r\nEND:VEVENT\r\nEND:VCALENDAR\r\n`
  (a `CATEGORIES` line whose sole category is a single space). First emit
  keeps a `CATEGORIES: ` line with the space; parsing that and re-emitting
  drops the property entirely — so the parse side treats the space-only
  category as empty/absent while the first emit did not, and one round trip
  too few hides it. Low severity (a closed M4 backend, costs a category no
  one can see rather than a panic), so filed per the current ROADMAP
  priority rather than fixed. `.proptest-regressions` deliberately not
  committed, same reasoning as the `jmap-vcard` entry.
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

## Upstream: GLib 2.80's `g_resolver_lookup_service()` leaks ~1 kB per call
Found 2026-08-19 while building `jmap-backend-core/src/resolver.rs`
(`SystemResolver`, the real `_jmap._tcp` SRV lookup). Measured on this VM
(GLib/GIO 2.80.0): RSS grows linearly at ~1 kB per lookup, over 6000
consecutive lookups of the *same* domain, on **both** the found and the
not-found path.

**It is not ours.** A minimal C reference program doing exactly the canonical
GLib sequence — `g_resolver_get_default()`, `g_resolver_lookup_service()`,
`g_resolver_free_targets()` on a non-NULL list, `g_error_free()` on a set
error, `g_object_unref()` on the resolver — leaks at the same ~1 kB/call rate.
For contrast, the same shape around `g_resolver_lookup_by_name()` /
`g_resolver_free_addresses()` is flat (delta 0 kB after warm-up), so this is
specific to the SRV/records path rather than to `GResolver` generally, and it
is not a bounded DNS cache (it would have plateaued for a repeated domain).

**Not worth working around today, on frequency grounds.** `lookup_srv` runs
once per `ConnectTarget::Domain` connect — a backend `connect_sync`, a
collection fan-out authentication, or a click of "Look Up Account Details" —
not once per sync poll and not per JMAP method call. A long-running EDS
factory process therefore loses on the order of tens to hundreds of kB, and
only for accounts set up from a bare email domain (an explicit host:port
endpoint is `ConnectTarget::Origin` and never resolved).

**If it ever does matter,** the options in order of preference are: (a) report
it upstream against GLib and pick up the fix, (b) memoize the per-domain
answer in the resolver — cheap, but it would have to ignore the record's DNS
TTL, which is a real correctness cost for a process that lives for days, and
(c) `g_resolver_lookup_records(…, G_RESOLVER_RECORD_SRV, …)`, which is *not*
an improvement: same GLib code path, so probably the same leak, plus it
returns raw `GVariant` tuples and drops GLib's RFC 2782 sorting, i.e. more
hand-rolled code for no fix. Reproduction is small enough to rebuild from the
description above; the throwaway harness was deliberately not committed
(a network- and allocator-dependent RSS assertion is not a test that belongs
in CI).

## `jmap-vcard` round trip is not a fixed point for a value with trailing whitespace (found 2026-08-19)

Found by Track A3's own `proptest` fuzzer, on a random seed, while running the
full suite for an unrelated increment — confirmed to reproduce on unmodified
`master` (`6ba07a9`), so it is not a regression from that work.

`prop_vcard_roundtrip_reaches_fixed_point_stability` in
`rust/crates/jmap-vcard/tests/proptest_fuzz.rs` asserts that re-emitting an
already-emitted vCard changes nothing. It does not hold when a property value
ends in a space. Minimal input:

```
BEGIN:VCARD\r\nVERSION:3.0\r\nNICKNAME;ENCODING=b:! \r\nEND:VCARD\r\n
```

First emit keeps the trailing space (`NICKNAME;X-JMAP-KEY=k1:! `); parsing
*that* and emitting again drops it (`NICKNAME;X-JMAP-KEY=k1:!`). So the parse
side strips trailing whitespace from a value and the emit side does not, and
one round trip too few hides it.

**Severity: low, and the reason it is filed rather than fixed.** RFC 6350 §3.3
makes trailing whitespace in a value significant, so this is a real fidelity
loss — but it costs a trailing space on a contact field, it is not a panic, and
`jmap-vcard` is part of a closed backend (M3) that the current ROADMAP
priority explicitly says not to reopen for corner cases. Fixing it means
picking a side (strip on both, or preserve on both) and re-running the vCard
fixture suite, which is a hardening-pass increment rather than a priority one.

**Note for whoever takes it:** the failure is seed-dependent, so a green run
proves nothing. `proptest` persists the failing seed to
`crates/jmap-vcard/tests/proptest_fuzz.proptest-regressions`; that file was
deliberately **not** committed, because doing so would turn an intermittent
red into a permanent one on `master` and block every other lane's gate for a
low-severity nit. Recreate it by pasting the minimal input above into a
`#[test]` that asserts the fixed point directly — that is the red test to
start from, and it is deterministic.

## ~~`jmap-ical` panics on a DATE-TIME value with a non-ASCII byte before offset 6~~ (found 2026-08-19)

**Fixed** — `to_local_date_time` (`jmap-ical/src/event.rs`) now checks
`date.is_char_boundary(8)`/`time.is_char_boundary(6)` before slicing (landed
as part of the Antigravity/agy-lane vCard-fidelity merge, `3a25473`/`f4c1ae7`,
2026-08-19 — this entry was written the same day from an independent
`master` checkout and never got word). Confirmed by reverting the two
`is_char_boundary` checks locally and rerunning: the exact minimal input
below panics again at the same site, so this is genuinely what fixed it, not
a coincidental change elsewhere. Pinned with a permanent regression test,
`jmap-ical/tests/hostile.rs::a_dtend_with_a_multibyte_character_at_the_slice_boundary_does_not_panic`,
asserting the event still parses (DTEND dropped, not invented) rather than
panicking. Left below for its original findings/repro value, not because
anything is still open here.

Found incidentally while gating an unrelated OAuth increment: `cargo test
--locked` (workspace, unmodified test selection) failed on
`jmap-ical/tests/proptest_fuzz.rs`'s `prop_ical_to_event_never_panics_on_raw_ical`
— a property whose entire job is asserting no panic on arbitrary/hostile
iCalendar text. Confirmed to reproduce on unmodified `master` (`57ec2ea`), so
not a regression from this session's work; the regenerated
`proptest-regressions` file was deliberately not committed, same reasoning as
the `jmap-vcard` entry above (an intermittent red must not become a permanent
one on `master`).

**Severity: higher than the vCard nit above — this is a real panic, not a
fidelity loss**, reachable from a hostile or merely malformed `DTEND`/similar
DATE-TIME value in server-supplied or imported iCalendar text (the untrusted-
server/untrusted-file boundary Track A3/A4 exist to harden). Minimal input
(from the fuzzer):

```
BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//Example//NONSGML//EN\r\nBEGIN:VEVENT\r\nUID:evt1\r\nDTSTART:20260115T130000Z\r\nDTEND: Aက ®T𐎟￼\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n
```

**Root cause, read not guessed:** `jmap-ical/src/event.rs` (around line 3876,
the DATE-TIME parsing helper) does:

```rust
if date.len() != 8 || time.len() < 6 {
    return None;
}
let time = &time[..6];                 // byte-slices before checking bytes 0..6 are ASCII
if !date.bytes().chain(time.bytes()).all(|b| b.is_ascii_digit()) {
    return None;                       // ASCII-digit check happens AFTER the slice above
}
```

`time.len()` is a byte length, so `time.len() >= 6` does not mean byte offset
6 is a char boundary — a multi-byte UTF-8 character straddling it panics on
the slice, before the ASCII-digit check three lines down ever gets to reject
the value cleanly. The fix is mechanical (check `time.is_char_boundary(6)`
before slicing, or slice on `.as_bytes()` and validate ASCII-digit-ness first,
then convert), but is real work: a red test first (the input above, asserted
to return `None` rather than panic), then the fix, then confirming the
existing proptest properties (which is what caught this) stay green.

**Was not fixed by the session that filed this entry, on purpose (see the
"Fixed" note above for what actually closed it):** `jmap-ical` is part of the closed M4 calendar
backend (`docs/ROADMAP.md` CURRENT PRIORITY says not to reopen M1–M6/M8 for
this), and Track A3 (structure-aware vCard/iCal fuzzing, where a survivor like
this belongs) is tagged `[agy]` lane, not `[claude]`. Logged with full
reproduction and root cause so whichever lane picks it up next does not have
to re-derive either.
