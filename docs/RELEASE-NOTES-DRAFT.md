# Release notes draft — next version after v0.2.0 (prepared 2026-08-29)

Prepared as release-notes preparation for the next version after v0.2.0.
Covers `git log v0.2.0..HEAD`. **The maintainer chooses the version number
and edits this down** — nothing here is final copy, and nothing bumps
`rust/Cargo.toml`, `CMakeLists.txt`, or `docs/packaging/changelog`. Every
bullet below cites the commit(s) it is based on; verify by inspection
rather than trusting the summary if anything looks surprising.

## Fixed

- **Hourly OAuth2 re-consent (the most-reported bug) — fixed and
  operator-validated.** A live backend connection kept using its cached
  access token after it expired (~1h lifetime) and turned the resulting 401
  straight into a consent popup instead of silently refreshing via the
  stored refresh token, interrupting the operator roughly once an hour.
  Every account-facing path (calendar, address book, mail store, mail
  transport/send) now retries once after a silent token refresh on a 401,
  and only escalates to consent if the *refreshed* token also fails.
  Verified live against real Fastmail over a multi-hour session: zero
  consent windows, seven observed 401s all resolved by silent refresh.
  (`71886ba`, `60b5f0f`, `b2cf10b`, `26f420b`, `41cdd20`, `7885526`,
  `6d79870` — the last of which fixed a related cause: child address-book/
  calendar backends and the mail transport had no OAuth2 client
  registration of their own to refresh with, only the parent collection
  did.)

- **Mail body downloads against real servers.** `download_blob` now refuses
  to treat a cross-origin redirect's answer as the blob, returning a clear
  error instead of silently caching garbage (`31919a4`); the blob-download
  request now declares `Accept: */*` per RFC 8620 §6.2 instead of
  `Accept: application/json`, matching two independent reference JMAP
  clients (`059c543`); a rebased-then-refused download's error now names
  `JMAP_LIVE_SERVER_REBASE_URLS` for diagnosability (`414919f`).

- **Inbox showing "no messages" on first open**, until switching to
  another folder and back — folder summary is now populated on initial
  open (`2db1cbc`).

- **Stale OAuth2 D-Bus proxy misclassified as a keyring failure.** After an
  `evolution-source-registry` restart, a dead D-Bus peer caused every token
  fetch to fail; it is now reported naming the dead peer instead of blaming
  the keyring (`68926fa`), and OAuth2 token-fetch failures are classified
  by real GError domain — secret-store failure vs. authorization failure
  (`9cecb1a`, `2475685`), locked keyring told apart from missing consent
  (`18afce3`). This is a diagnosability fix, not a recovery path — no
  retry-through-the-registry was implemented (evaluated and rejected as
  unreachable from a backend factory process).

- **Account setup and OAuth2, especially Fastmail.**
  - JMAP `_jmap._tcp` SRV autodiscovery via `GResolver` (RFC 8620 §2.2), so
    providers that don't serve `.well-known/jmap` at the bare email domain
    now work from just an email address (`a07f1a6`, `aa711bf`, `bdca950`,
    `2881ac5`).
  - Default HTTPS/HTTP ports no longer serialized into the connection
    origin, which was silently breaking Fastmail's OAuth issuer-identity
    check (`91c0f25`).
  - OAuth2 redirect URI changed to a dotted reverse-DNS scheme
    (`org.gnome.evolution.jmap:/redirect`), required by providers that
    reject non-dotted private-use schemes (`d597c0f`).
  - Added RFC 7636 PKCE support — EDS 3.52 has none, and providers mandate
    it (`a1e5782`); added RFC 8707 resource-indicator support end to end,
    required by Fastmail (`52db331`); request exactly the scopes the client
    uses instead of every advertised scope or none (`b2ff852`, `0df34c8`).
  - Register the OAuth2 service type in every process that needs it,
    fixing a fallback to an unusable password prompt (`f83e04b`,
    `86bd41c`); rank the discovered JMAP collection above ISPDB's generic
    imap/smtp guesses (`c85e916`); override `auto_configure` so the account
    assistant's "Look Up" can offer JMAP at all (`8936d12`).
  - Added an API-token (Bearer) authentication method alongside Password
    and OAuth 2.0, needed for Fastmail app passwords (`db03372`,
    `d318dc2`, `e21f97d`).
  - A post-fetch 401 on a bearer token EDS itself just handed over is no
    longer misclassified as a wrong password, which had discarded the
    valid refresh token and forced fresh consent (`d6b26aa`, `e6af829`).
  - An OAuth2 account is now authenticated silently instead of
    unconditionally popping the consent window, both at startup and at
    mail-send time (`005b980`, `cc64da1`).
  - A rejected calendar/contact write now names the offending properties
    instead of failing with no detail (`8c32c9b`).

- **Calendar and address-book management.**
  - "New Address Book" / "New Calendar" now actually work against a JMAP
    account — create and delete on the server were previously unimplemented
    (`03994b7`, `6254f51`, `f3a521b`).
  - A calendar's color is now read from and written back to the server,
    previously parsed and dropped (`0d04613`, `4a6bd37`).
  - The meeting scheduler's free/busy query is now answered
    (`55e8699`, via a new `Principal/getAvailability` client slice,
    `efea5f4`, `7f1fea9`).
  - `get_destination_address` implemented, letting EDS track host-specific
    network reachability instead of only generic online/offline (`d10c20b`).
  - Fixed `CalendarEvent`'s recurrence rule field being modeled as plural
    where JSCalendar defines it as singular (`2545d17`); JSCalendar events
    now stamp the required `version: "2.0"` (`900b52a`, `f24e3f5`).

- **Contacts and calendar field-mapping fidelity** (a large batch of real
  vCard/iCalendar ↔ EDS round-trip fixes, not just added tests): IM URI
  schemes for AIM/ICQ/MSN/Yahoo/GroupWise/Matrix/Twitter/SIP (`676e082`,
  `b89f567`, `d5cc4c6`); `PREF` mapped to EDS's primary-field slot with
  ordering preserved (`583e11a`); full structured `ADR`/`LABEL` roundtrips
  (`3a25473`); `MOBILE` phone TYPE synonym support (`a9ce9c3`); remaining
  `X-EVOLUTION-*` fields — manager, assistant, blog/video URLs, FILE-AS
  (`50f22b8`, `5a280dd`); vCard 2.1 and 4.0 legacy/modern import tolerance
  (`7efc94d`, `b62e98d`); Apple property groups and `X-ABLabel` mapping
  (`105f260`); an empty stated full name treated as absent on emit instead
  of emitting a bogus name (`d6ab1f3`); `CATEGORIES` whitespace trimmed to
  a stable fixed point (`fbeee29`); line-folding correctness against a
  known calcard overshoot bug (`5e2364e`, `83985d3`); hyphenated vCard
  dates pre-normalized on import (`0ca31d4`); Windows/globally-unique
  `TZID` tolerance for calendar timezones (`a05061f`).

## Internal (no direct user-facing change, listed for transparency)

- **FFI/memory-safety hardening**, found by audit rather than a bug report:
  a confirmed use-after-free in OAuth2 string handling fixed (`dab9348`); a
  GObject-resurrection race in the `EOAuth2Services` singleton lookup fixed
  (`290c961`); GObject references wrapped in an owning type with `Drop`
  across the calendar, collection and mail backends (`a362578`, `32c3e0b`,
  `f16ca55`); several smaller UNSAFE-AUDIT remediations.
- **Mutation testing and fuzzing** of `jmap-vcard`, `jmap-ical`,
  `jmap-proto`, `jmap-client`: surviving mutants killed with new tests
  (`6206f31`, `8c3c5b3`, `d5cf563`, `c0480ac`); proptest-based fuzzing of
  session/request/response deserialization and structure-aware vCard/iCal
  round-trips added (`1332dbd`, `6a32d7c`, `3a92425`).
  Two fuzz-found issues are logged but not fixed in this window (a
  `jmap-vcard` trailing-whitespace round-trip nit, and a real `jmap-ical`
  panic on a non-ASCII byte in a DATE-TIME value).
- **A large batch of test-only fidelity characterization** (pins existing
  behavior; no production code changed) across ALTID/LANGUAGE, CATEGORIES,
  non-ASCII/CHARSET/ENCODING, NICKNAME/URL, LOGO/KEY, multi-TYPE phones,
  ORG/TITLE, bare-year dates, timezone/UNTIL/VALARM handling, plus
  real-exporter fixture corpora from Google Contacts, Thunderbird, SOGo,
  Apple and Evolution's own editors.
- **Functional test coverage expanded to real servers/objects** (~40
  commits): mail, calendar, address-book and collection backend operations
  now exercised against a real Stalwart JMAP server and/or real EDS/Camel
  objects, not only the mock.
- **Structured tracing/observability**: `journald` logging via `tracing`
  (`4314ba3`) plus structured fields (message/resource/account IDs, method
  and call IDs, credential method, HTTP bodies) across the client and every
  backend crate.
- **CI/build**: disk-exhaustion fixes (`352b296`, `cec7cbd`), a CI red
  streak fixed by a calcard bump (`e0d7675`), an intermittent collection-
  backend SIGSEGV fixed by serializing/bounding tests (`09e1ffd`,
  `996e8ba`, `264870b`), `sbom.sh` fix (`768c2bb`).
- **Packaging**: the CPack `.deb` made lintian-clean (`6efcd34`);
  `debian/copyright`'s own-file stanzas now generated from `REUSE.toml`
  (`7e9605f`); a `debian/` skeleton added for `dpkg-buildpackage`
  (`55a8e3a`); a third-party-notices appendix generated (`e23f9ce`); a
  packager-onboarding guide added (`087b373`).
- **Dependencies/refactors**: percent-encoding unified onto the
  `percent-encoding` crate (`0382402`); calcard bumped to 0.3.13, retiring
  a fold-workaround (`3dde547`).
- **Research with no shipped code change**: a "stale source UID" consent
  scare investigated and found harmless — the UID belonged to an ordinary
  collection child, not dangling dconf debris
  (`9e95a56`); externalising JSContact/JSCalendar conversion onto the
  `calcard` crate's own converter (item 27) was measured (15% pass rate
  against our acceptance suite) and rejected — see
  `docs/CALCARD-SEMANTIC-SPIKE.md` (`e6b42ef`); crate-extraction/
  publication-to-crates.io potential assessed in
  `docs/CRATE-EXTRACTION.md` (`7bfcd50`).

Omitted from both lists: ~26 `agy:`/`night-shift:`/`gcp:`/`drivers:` commits
and ~6 `upstream:` commits are autonomous-agent operational tooling and
draft GitLab issue comments (GNOME/evolution#374 snooze, #411 scheduled
send) — process artifacts of how this project is developed, not part of the
shipped plugin.

## Maintainer TODO before tagging

Version currently reads `0.2.0` everywhere (workspace `rust/Cargo.toml`,
`CMakeLists.txt`'s `VERSION`, `docs/packaging/changelog` — `debian/changelog`
is a symlink to the latter, so it's one edit) with `v0.1.0`/`v0.2.0` already
tagged. To cut the next release:
1. Pick the next version number and set it in `rust/Cargo.toml`'s
   `[workspace.package] version` and `CMakeLists.txt`'s `set(VERSION ...)`
   (kept in lockstep by convention — every crate uses `version.workspace =
   true` except `example-module`, pinned at `0.0.1`, which isn't packaged
   and is unaffected).
2. Add a `docs/packaging/changelog` entry (Debian changelog format) —
   trim/adjust this draft's "Fixed" list into it.
3. Tag `vX.Y.Z`.

Packaging was re-verified clean at this tree's HEAD (`ca188315a0c4`):
`ninja -C build` and `ctest --test-dir build -R
'package-deb-lintian|debian-copyright-in-sync'` both pass, so nothing here
blocks a release.
