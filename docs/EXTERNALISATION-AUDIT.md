<!--
SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
SPDX-License-Identifier: GPL-3.0-or-later
-->

# Externalisation audit — hand-rolled code a maintained crate could replace

Read-only survey of `rust/crates/*/src`, 2026-08-19, in the spirit of the
completed `calcard` migration (which retired ~1000 lines of hand-rolled
vCard/iCal parsers). Goal: find hand-rolled code a well-maintained external
crate could own instead, so the project maintains less of its own.

## Headline

This workspace has **already externalised aggressively**: base64 (`base64`),
SHA-256 (`sha2`), OS randomness (`getrandom`), JSON (`serde`/`serde_json`),
the blocking HTTP client (`ureq`), the mock's HTTP server (`tiny_http`), and
the whole iCalendar/vCard text layer (`calcard`). What is left hand-rolled is
overwhelmingly one of three things a crate *should not* replace: deliberate,
comment-justified choices; FFI/GObject glue; or the JMAP-side semantic mapping
that is the project's own reason to exist.

The single genuinely clean win is **unifying the hand-rolled percent
encode/decode onto `percent-encoding`** — a crate that is *already compiled in
the dependency tree* (pulled in transitively by `ureq`), so adopting it adds no
new dependency at all. Everything else is a justified KEEP. "Nothing else worth
externalising" is the honest finding, and it is a good sign for the codebase.

A dependency-cost note that recurs below: `chrono` **and** `chrono-tz` are
already in the tree (pulled in by `calcard`), and so are `mail-parser` /
`mail-builder`. So for several KEEP items the "avoid a dependency" half of the
original rationale is now moot — those crates are already built — and the
reason to keep the code is purely behavioural. Each such case is called out.

---

## Candidates, most-worthwhile first

### 1. Percent encode/decode — unify onto `percent-encoding` (already in tree)

1. **The hand-rolled code.** The same RFC 3986 percent codec is hand-written in
   three places:
   - `jmap-client/src/url.rs` — `encode_template_value` + `hex_digit`, ~25
     lines: percent-encode everything outside the RFC 3986 §2.3 unreserved set,
     by octet. Used for URL-template substitution *and* reused by
     `oauth.rs::form_body` to build `application/x-www-form-urlencoded` bodies.
   - `jmap-mock/src/server.rs` — `percent_decode` + `hex_value` + the decode
     half of `parse_form_body`, ~55 lines: undo percent-encoding leniently
     (a bad `%XX` is kept verbatim), plus `+`→space for form bodies.
   - (`jmap-config/src/config_lookup.rs::parse_target` is *structural* URL
     splitting, not percent-coding — see KEEP list.)

2. **The crate.** [`percent-encoding`](https://crates.io/crates/percent-encoding)
   (servo/rust-url), **already at v2.3.2 in `Cargo.lock`** via `ureq`. It gives
   `utf8_percent_encode(value, SET)` and `percent_decode_str(s).decode_utf8_lossy()`.
   The strict unreserved set is one const:
   `const UNRESERVED: &AsciiSet = &NON_ALPHANUMERIC.remove(b'-').remove(b'.').remove(b'_').remove(b'~');`
   License MIT OR Apache-2.0 — both on `deny.toml`'s allowlist. **No new
   dependency:** it is compiled already.

3. **Risk/effort.** Small and low-risk. The encoder is behaviour-preserving
   (same unreserved set, uppercase hex, per-octet). The mock's *lenient* decode
   (keep a malformed `%` as-is) matches `percent-encoding`'s own behaviour, so
   the mock is preserved too. None of this is FFI-bound. Net removal ~80 lines
   across two crates; `hex_digit`/`hex_value` disappear entirely. The one thing
   to keep hand-rolled is the `&`/`=` *splitting* in `parse_form_body` (or pull
   in the tiny `form_urlencoded` crate, MIT/Apache-2.0, for that half too — but
   that one *is* a new crate, so weigh it separately).

4. **Verdict: WORTH IT** (the only clear win). Because the crate is already in
   the tree, this is pure code-reduction with no supply-chain cost. The heavy
   doc-comments in `url.rs` explaining *why* strict encoding is required stay
   valuable and can move onto the `AsciiSet` const.

### 2. `po-compile` — msgfmt subset — mostly KEEP, one externalisable half

1. **The hand-rolled code.** `po-compile/src/lib.rs`, ~400 lines: a `.po`
   parser plus a `.mo` binary emitter (little-endian header, sorted
   originals/translations tables, no hash table). It is `msgfmt` restricted to
   this project's catalogues, with deliberate *refusals* (stops on `msgctxt`,
   plural forms, unknown escapes, duplicates, non-UTF-8) rather than silent
   drops.

2. **The crate.** [`rspolib`](https://crates.io/crates/rspolib) parses and
   writes both `.po` and `.mo`. It would own the byte-level `.mo` format and
   the `.po` tokeniser — the mechanical ~55-line `emit`/`record` half in
   particular. (Verify its license before adopting; it is not in the tree, so
   this *is* a new dependency and a new advisory-surface.)

3. **Risk/effort.** Moderate. The refusal semantics (§"What it refuses, and why
   refusing is the point") are load-bearing design, not an accident — a
   general-purpose library would *accept* `msgctxt`/plurals and silently do the
   wrong thing for this project's build, which is exactly the failure the module
   was written to prevent. So a migration would keep the validation layer and
   externalise only the format I/O underneath it.

4. **Verdict: KEEP (lean).** The module comment already justifies *not shelling
   out to `msgfmt`* (CI has no gettext tools; a `.mo` must be built on every
   machine). It does not address a Rust crate — but the externalisable half is
   only ~55 lines, it would add a new dependency to a build-time-only tool, and
   the refusals that are the point stay hand-written regardless. Low leverage;
   revisit only if the `.mo` emitter ever needs to grow (plurals, contexts).

### 3. `config_lookup::parse_target` — `scheme://host:port` split — KEEP

`jmap-config/src/config_lookup.rs::parse_target`, ~20 lines, splits an optional
`http(s)://host:port` override, deliberately leaving a bracketed IPv6 literal
intact. The `url` crate could parse this, but `url` is a heavyweight dependency
(drags in `idna`, `form_urlencoded`) **not currently in the tree**, for 20 lines
that intentionally do *less* than a general URL parser (it must hand an
un-split IPv6 host to `jmap_backend_core::source`). **Verdict: KEEP** — the
crate is bigger than the problem and would change behaviour at the IPv6 edge.

---

## KEEP (deliberately hand-rolled) — with the reason

- **`jmap-mail-sync/src/date.rs`** (RFC 3339 ↔ epoch seconds, ~220 non-test
  lines; Howard Hinnant civil-date arithmetic). The module comment explicitly
  justifies hand-rolling it. Note honestly: `chrono` is **already in the tree**
  (via calcard), so the "avoid a dependency" argument is moot — but the
  substantive reasons stand and are behavioural. The code deliberately accepts a
  *superset* of RFC 3339 (`+HHMM` without a colon, per RFC 5322 `Date`
  headers), *clamps* leap-second `:60`→`:59` rather than rejecting, and returns
  `None` (not `Err`) so one bad `Date` can't hide a mailbox. `chrono`/`time`
  would *reject* several inputs this accepts, i.e. not behaviour-preserving.
  **KEEP** — replacement changes observable behaviour on real-world dates.

- **`jmap-ical/src/zone.rs`** (offset in force from an embedded `VTIMEZONE`,
  ~430 lines). Looks like a job for `chrono-tz` (which *is* in the tree via
  calcard) — but it is the opposite job. RFC 5545 §3.6.5 says a `TZID` is
  defined by the `VTIMEZONE` *in the same document*; this reads the offset out
  of that document, not out of a system zone database. `chrono-tz` would give
  the wrong answer for a self-defined or non-IANA zone (Exchange/Zimbra/Lotus
  invitations). **KEEP** — no zone-database crate can do this.

- **`jmap-mail/src/mime.rs`** (message → upload octets, ~160 non-test lines,
  mostly FFI; plus a 4-line `crlf`). Delegates RFC 5322 emission to Camel's own
  writer *on purpose* — the comment explains that a provider emitting headers
  itself would be "a second, disagreeing MIME implementation inside the same
  process." Notably `mail-builder` is already in the tree (via calcard) and is
  *still* the wrong choice for exactly this reason. **KEEP** — FFI-bound and
  comment-justified; the 4-line CRLF filter mirrors `CamelMimeFilterCrlf`.

- **`jmap-backend-core/src/i18n.rs`** (~120 lines). FFI to the *host process's*
  system gettext (`bindtextdomain`/`dgettext`). A Rust gettext crate
  (`gettext-rs`, `gettext`) would defeat the point: the module must integrate
  with the gettext the host (Evolution/Camel) already set up, and must never
  call `textdomain()`. **KEEP** — FFI to system libc, comment-justified.

- **`jmap-client/src/transport.rs`** (`UreqTransport` + `CancelFlag`/
  `CancelScope`, ~265 lines). The HTTP work is *already* externalised to
  `ureq`; what remains hand-rolled is the `Transport` trait seam and the
  thread-local cancellation bridge to `GCancellable`. Blocking-`ureq` was
  chosen deliberately to avoid `tokio`. **KEEP** — the abstraction seam and the
  cancellation model are the project's own and FFI-shaped.

- **`jmap-client/src/oauth.rs`** (RFC 8414 discovery, RFC 7591 registration,
  PKCE, token exchange, ~1000 lines with tests). This is thin `serde` structs
  plus RFC-8414-§3.3 issuer validation; PKCE already uses `getrandom` + `sha2`
  + `base64`, and `form_body` reuses the URL encoder (see candidate 1). The
  [`oauth2`](https://crates.io/crates/oauth2) crate exists (MIT/Apache-2.0) but
  is built around a full authorization-code browser flow and its own type-state
  client; it does not fit the EDS `EOAuth2Service` split where EDS owns the
  browser/consent step and this code owns only discovery+exchange. **KEEP** —
  poor impedance match; the validated types are the security surface.

- **`jmap-proto/*`** (wire types: RFC 8620 core, 8621 mail, 9610 contacts,
  Calendars draft). Thin `serde` `#[derive]` structs. These are trivial and
  deliberately not a dependency — a JMAP-protocol crate would be a large new
  surface for what is a handful of `Deserialize`/`Serialize` structs the
  project fully controls. **KEEP.**

- **The JMAP-side mapping: `jmap-ical/src/event.rs` (~4700 lines),
  `jmap-vcard/src/contact.rs` (~2500), `jmap-book-sync/src/patch.rs` (~1180),
  `jmap-cal-sync/src/patch.rs`.** These do JSCalendar↔iCalendar and
  JSContact↔vCard *semantic* mapping and PatchObject construction. The text
  layer is *already* calcard's; this is explicitly "the JMAP side of the
  mapping is ours" (see `rust/Cargo.toml`). No crate covers it. **KEEP.**

- **FFI / GObject glue** — `jmap-backend-core/src/{subclass,trampoline,
  connect,source,marshal}.rs`, every `*-module` cdylib, `jmap-mail/src/
  {message_info,summary,store,folders,envelope,folder,…}.rs`,
  `jmap-backend-*/src/*`. GObject subclassing, vfunc trampolines, `GError`
  marshalling and Camel summary rows. These MUST be hand-rolled (and several
  are already excluded from scope). **KEEP.**

- **`jmap-mock/*`** beyond the percent-codec of candidate 1 — stateful
  in-memory JMAP server. Test-only; HTTP is already `tiny_http`. The routing
  and state machine are the test fixture and have no external equivalent.
  **KEEP.**

## Excluded from scope (per the brief)

`eds-sys` / `evo-sys` (bindgen-generated FFI), `example-module`, the
GObject/FFI trampoline glue, and anything a code comment already justifies as a
deliberate choice.

## Bottom line

One clean, zero-new-dependency win (candidate 1: `percent-encoding`, already
compiled via `ureq`). Everything else is a defensible KEEP. The codebase is
already close to the "externalise as much as is sensible" target — the
remaining hand-rolled code is deliberate design, FFI, or the project's own JMAP
mapping, none of which a crate should own.
