# Roadmap

Goal: a **secure, easy to use, natively integrated** way to use JMAP from
GNOME Evolution — mail, contacts, and calendars — structured like
evolution-ews, written in Rust, developed test-first against the in-repo
mock server (`jmap-mockd`), and shipped as installable artifacts.

Round 1 (done): protocol crate, blocking client, stateful mock server,
42-test TDD suite, dual CI with reproducible builds and provenance. See
README.

## CURRENT PRIORITY: make it usable by a normal person (2026-08-16)

The four backends work end-to-end in real Evolution (M1–M6, M8 done) and
v0.1.0 is released — but a real user still cannot use it: there is no
account-setup UI, and it has never talked to a real JMAP server. Those two
gaps, not more polish, are what stand between "works for us, hand-configured,
against a mock" and "works for a user against their own account." So, until
they close, prioritise in this order:

1. **M7 — account-setup UI** (`module-jmap-configuration.so`). The single
   biggest blocker. Build it even though its GUI cannot be verified headless:
   implement, mark it *needs human verification in real Evolution* in the
   night log, and do NOT tag it COMPLETE until a human confirms it. This is
   the agent↔human loop — you scaffold, the maintainer verifies in the VM.
2. **Real-server readiness** — OAuth2 auth (real providers use it; the client
   only does Basic/Bearer today) via EDS's OAuth2 source support, plus a
   `--features live-server` integration harness and capability-negotiation
   robustness, all buildable/testable against the mock now so a real server
   (Stalwart, then Fastmail) is a config change, not a rewrite.
   - **FIXED 2026-08-18 (86fea00)** — the redirect-auth bug found by the first
     live-Stalwart test (see NIGHT-LOG "session-discovery redirect strips
     auth" and its "Fixed" follow-up): `UreqTransport::new` now sets
     `redirect_auth_headers = SameHost`, and `jmap-mock` grew a
     `session_via_redirect()` mode so the whole thing is covered by
     `jmap-client/tests/redirect_auth.rs` without a live server.
   - **FIXED 2026-08-18** — the *secondary* blocker the same finding flagged
     (root-caused in NIGHT-LOG "apiUrl's scheme is hardcoded https, not just
     the hostname"): Stalwart's session always advertises `apiUrl` as
     `https://<defaultHostname>/…` regardless of listener TLS config, and this
     VPC-reachable Stalwart only publishes plain HTTP on :8080, so no
     Stalwart *setting* makes the session's own `apiUrl` reachable from the
     runner. Rather than a TLS-trust or infra change, `jmap-client` grew an
     opt-in `ClientBuilder::rebase_urls_to_origin` (env
     `JMAP_LIVE_SERVER_REBASE_URLS` for the harness): after session discovery,
     rewrite `apiUrl`/`downloadUrl`/`uploadUrl`/`eventSourceUrl`'s
     scheme+authority to the origin actually connected through, keeping each
     URL's path/query as stated. Off by default; TDD'd against `jmap-mock`
     (`MockServerBuilder::advertise_origin`,
     `jmap-client/tests/rebase_urls.rs`) and confirmed against the live
     Stalwart: without it, 4 of 5 `live_server.rs` tests failed with `HTTP
     405` — `apiUrl`'s `https://example.com` is a real Internet domain this
     runner has ordinary egress to, so every method call was silently being
     sent there, not merely failing to connect; with
     `JMAP_LIVE_SERVER_REBASE_URLS=1`, all 5 pass against the real
     deployment. See NIGHT-LOG's "Delivered: opt-in apiUrl/downloadUrl/
     uploadUrl rebase to the connected origin".
   - **VERIFIED 2026-08-19 (operator, real Evolution → real Stalwart):** mail
     send/deliver/read, contacts, and calendar all round-trip and persist across
     a reboot (NIGHT-LOG "operator-verified real-server end-to-end"). The
     real-server dimension of M7 and of this item is human-confirmed.
   - **DEFERRED (maintainer, 2026-08-19): OAuth 2.0 real-server validation.**
     The client's RFC 8414 issuer check correctly rejects the throwaway Stalwart
     (plain HTTP, `defaultHostname = example.com`, reached via a forward) — see
     NIGHT-LOG "OAuth 2.0 discovery's issuer check". OAuth2 is fully mock-tested
     and behaving correctly; exercising it against a real server needs a
     TLS-proper deployment (real hostname + cert), parked for later. Do NOT
     relax the issuer match to make it pass — it is the mix-up-attack defence, a
     deliberate maintainer call, not a bug. Do not re-surface it as a blocker.
3. Then **M9** (functional + GUI-smoke CI) and **M10** (EDS version matrix).
4. **Maintainer-directed externalisation quick win.**
   The externalisation audit (`docs/EXTERNALISATION-AUDIT.md`) found the codebase
   already well-externalised; one clean win remained: unify the hand-rolled
   percent codec onto the `percent-encoding` crate. **DONE 2026-08-19** —
   `jmap-client/src/url.rs`'s `encode_template_value`/`hex_digit` and
   `jmap-mock/src/server.rs`'s `percent_decode`/`hex_value` now delegate to
   `percent_encoding::utf8_percent_encode`/`percent_decode_str`, promoted from a
   transitive (`ureq`) to a direct dependency of both crates — no new package in
   `Cargo.lock`. `hex_digit`/`hex_value` are gone; `parse_form_body`'s `+`↔space
   handling stayed hand-written (form-specific, not RFC 3986). Every existing
   `url.rs`/`oauth.rs`/`server.rs` test stayed green, unmodified.
5. **JMAP SRV autodiscovery — CLAIMABLE NOW (operator-found 2026-08-19).**
   Operator tested a real `muelli@fastmail.com` account through real Evolution:
   the password path fails with **HTTP 404**. Root cause (confirmed from the
   source and DNS): `client.rs:135` hardcodes session discovery to
   `https://{domain}/.well-known/jmap` with `{domain}` = the email domain
   (`fastmail.com`), but Fastmail serves JMAP at `api.fastmail.com`, published
   the RFC 8620 §2.2 way via a SRV record — the operator resolved
   `_jmap._tcp.fastmail.com` from the VM and got `0 1 443 api.fastmail.com.`,
   while `https://fastmail.com/.well-known/jmap` returns 404. The transport
   already follows redirects (incl. to `/jmap/session`), so the *only* missing
   step is the SRV lookup.
   **Do:** implement RFC 8620 §2.2 autodiscovery — before the bare-domain
   `.well-known/jmap`, look up `_jmap._tcp.{domain}`; if a target is returned,
   build the session URL from `https://{target-host}[:port]/.well-known/jmap`;
   fall back to today's bare-domain URL when there is no SRV record (preserves
   the Stalwart/self-hosted path and every current test).
   **Two sites share this bare-domain assumption — fix both, or you fix the
   password 404 while OAuth-detect stays broken:** (a) `client.rs:135` (session
   discovery, the password/Bearer path); and (b) the "Look Up Account Details"
   worker `config_lookup.rs::probe_host` (line ~143), which returns the bare
   email domain for its RFC 8414 / JMAP discovery. (b) is *why OAuth was not
   detected for the operator's Fastmail account and why the assistant fell back
   to suggesting imapx* — the worker probed `fastmail.com`, found no JMAP, and
   returned nothing, so Evolution's generic ISPDB autoconfig won. The OAuth
   machinery (discovery, dynamic registration, `EOAuth2Service`, the lookup
   worker) is otherwise built and mock-verified; this SRV gap is what keeps it
   from firing against a real SRV-published provider.
   **Design (pre-decided, to keep `jmap-client` dependency-lean — do NOT add a
   Rust DNS crate to it):** add a `Resolver` trait seam to the client; the pure
   crate's default resolver does no SRV (today's behaviour), and the EDS
   integration supplies a real one backed by GIO's `g_resolver_lookup_service()`
   (GResolver does SRV natively — no new dependency; needs a small `eds-sys`/glib
   binding). **TDD:** red test first in `jmap-client/tests/` — `connect("example.com")`
   with an injected fake resolver returning `_jmap._tcp.example.com → api.example.com:443`
   must request the session from `api.example.com`, asserted via the `Transport`
   fake / `jmap-mock`; plus a no-SRV test proving the bare-domain fallback is
   unchanged. **Cannot** be verified against Fastmail from the runner (no creds;
   Fastmail also needs an app-password/API token, not the login password — a
   separate 401 concern, not this 404); the operator confirms end-to-end in the
   VM. Reasonable to escalate if the `Resolver` seam or the GResolver binding
   proves gnarly. (The revise-path setup-UI prefill bug the same operator session
   found is already FIXED and operator-verified in `1afebc1`; the OAuth-2.0
   "can't be set up" the operator saw is the intended autodiscovery-only message
   from decision #1 below — not a bug, do not touch it.)
   - **PARTIAL 2026-08-19** — the `Resolver` trait seam landed in
     `jmap-client` (`resolver.rs`: `Resolver`/`SrvTarget`/`NoSrvResolver`;
     `ClientBuilder::resolver`/`connect_domain`), TDD'd in
     `jmap-client/tests/srv_discovery.rs` against a fake resolver and a fake
     in-memory transport — see NIGHT-LOG "Delivered: `Resolver` trait seam
     for JMAP SRV autodiscovery". **Still open, both named call sites
     unchanged:** (a) nothing yet calls `connect_domain` on the password
     path; (b) `config_lookup.rs::probe_host` still returns the bare email
     domain. Also unstarted: a real `Resolver` backed by
     `g_resolver_lookup_service()` in the EDS integration layer (FFI —
     likely escalation-worthy). Not claimable-complete; the next session on
     this thread should wire (a)/(b) into `connect_domain` and build the
     GResolver-backed implementation.

**Do NOT reopen completed backends (M1–M6, M8) to polish edge cases.** They
are closed. The contact-editor fidelity items, extra vCard/iCal corner
cases, and similar refinements are real but LOW-LEVERAGE right now — record
them in `docs/BACKLOG.md` for a later hardening pass and move on. Correctness
still governs *how* the priority work is done (TDD, honest verification); this
directive governs *what* to work on.

## ROUND 2 BACKLOG (2026-08-19) — hardening, observability, packaging, EDS parity

The CURRENT PRIORITY goal (usable end-to-end) is met. This is the maintainer's
next-mountain list, grouped into tracks. Status tags: **CLAIMABLE** = unblocked,
headless, no decision needed; **NEEDS-DECISION** = a design/product call is open
(do NOT start it; it is listed for visibility). Lane tag: `[claude]` heavier /
priority lane, `[agy]` in-lane polish (headless, test-gated). Log completions as
usual. Do not reopen closed backends except where an item names one.

**Lead order (maintainer, 2026-08-19):** once the two remaining CURRENT PRIORITY
items (M10's 3 tests, the percent-encoding win) land, the Claude lane leads
Round 2 with **Track D** (EDS parity — restores evolution-ews parity and has no
protocol gap). Track E is a **design spike first** (see Track E). The other
tracks follow; the maintainer may reorder anytime.

### Track A — Quality & security (CLAIMABLE)
- **A1 `[agy]` Mutation testing of the mapping crates.** Run `cargo-mutants`
  (stable; `cargo install` it if absent) on `jmap-vcard` and `jmap-ical`. For
  each *surviving* mutant that is a real behavioural gap, add a round-trip test
  that kills it; log deliberately-left equivalent mutants with a one-line why.
- **A2 `[claude]` Mutation testing of the protocol/client core.** Same tool on
  `jmap-proto` and `jmap-client` — they carry the untrusted-server wire
  contract. Strengthen tests to kill high-value survivors.
- **A3 `[agy]` Structure-aware fuzzing of the vCard/iCal round-trips.**
  `proptest` + `arbitrary` as dev-deps on **stable** (NOT `cargo-fuzz`: it needs
  nightly and breaks the pinned-stable reproducibility). Generate random
  JSContact/JSCalendar and random vCard/iCal; assert (a) no panic, (b) round-trip
  stability. Fix any panic found.
- **A4 `[claude]` Malicious-input hardening of the untrusted-server boundary.**
  proptest/arbitrary harness feeding hostile JMAP *responses* into `jmap-proto`
  deserialization + `jmap-client`, and hostile session `apiUrl`/redirect targets
  into `transport.rs`. The server is NOT trusted: assert no panic and no auth
  leak on redirect (same surface as the 86fea00 redirect-auth fix). Aligns with
  the standing "Recurring security re-audit" directive.
- **A5 `[claude]` FFI soundness audit.** Worth doing, and timely — Tracks D/B add
  FFI surface and M10 already exposed cross-version FFI drift. Scope: every vfunc
  trampoline `catch_unwind`-wrapped; transfer-full vs transfer-none ownership on
  every returned GObject/string (g_free correctness); nullability at each
  boundary; `GCancellable` honoured on the sync vfuncs. Deliverable:
  `docs/FFI-SOUNDNESS-AUDIT.md` + TDD fixes for findings. Escalation-worthy.
- **A6 `[claude]` Unsafe reduction / idiom audit.** Deliverable
  `docs/UNSAFE-AUDIT.md`: inventory every `unsafe` in `rust/` by category, tag
  each cluster **KEEP** (intrinsic, well-contained) / **IMPROVE** (a concrete
  safer or more idiomatic refactor — a pointer-owning newtype with `Drop`, a
  typed accessor, or an existing safe `glib`/`gobject` binding replacing a
  hand-rolled FFI call) / **INVESTIGATE**, then a short prioritized IMPROVE list
  with rough effort. Audit only — land improvements as separate follow-ups. Be
  balanced: "already solid, here's why" is a valid finding. **Complements A5, not
  a duplicate:** A5 is *soundness* (catch_unwind, transfer-full/none, nullability,
  GCancellable); A6 is *reduction / containment / idiom*. Do them together or
  A5-then-A6.
- **A7 `[claude]` Stale-comments audit.** Deliverable
  `docs/STALE-COMMENTS-AUDIT.md`: comments that no longer match the code —
  renamed/removed items, changed behaviour, done TODOs, resolved-milestone refs
  ("once M7 lands" when M7 is done), `calcard`/percent-codec leftovers. Precise
  and conservative; confidence-tag each and include a "looked suspicious but
  fine" section so the sweep is trustworthy. **Seed already found:**
  `jmap-config/src/textdomain.rs:17-18` says `insert_widgets` "is unwritten"
  while `lib.rs:125` says it is now written — verify and fix as part of the
  sweep. Fixing HIGH-confidence stale comments in the same pass is fine (comments
  don't affect tests); leave anything uncertain as a logged finding.

### Track B — Observability (NEEDS-DECISION on approach, then CLAIMABLE)
- **B1 `[claude]` journald structured logging, TRACE→ERROR.** Replace the ~54
  ad-hoc `println!`/`eprintln!`/`g_message` sites with the `tracing` crate.
  Recommended sink: `tracing-journald` (writes the journal native protocol
  directly — no libsystemd FFI, keeps the dep/repro posture clean; MIT/Apache,
  allowlist-ok) behind an env filter (default WARN, opt-in to TRACE via e.g.
  `EVOLUTION_JMAP_LOG`). Init ONCE per process via `OnceLock`/`try_init` — the
  backends load as cdylibs into shared EDS factory processes, so double-init must
  be a no-op. Structured fields: account id, JMAP method, object type, request
  id. DECISION: `tracing`+`tracing-journald` vs. routing through glib `g_log`
  (EDS already funnels g_log to the journal, but that loses Rust-side structured
  fields and TRACE granularity). Recommend the former.

### Track C — Packaging (mostly CLAIMABLE; official Debian is a human process)
- **C1 `[claude]` Lintian-clean .deb.** Run `lintian` on the CPack `.deb`; fix
  warnings (Section, Priority, extended description, Depends/Recommends). Add a
  CI check that lintian stays clean.
- **C2 `[claude]` Machine-readable DEP-5 `debian/copyright`** generated from the
  REUSE metadata we already maintain — the single biggest ease-of-packaging win.
- **C3 `[claude]` `debian/` skeleton** (control, rules using `dh` over the
  cmake/cargo build, watch file) so a Debian packager starts from a working tree.
  Document the Rust-in-Debian reality (dh-cargo wants every crate dep packaged /
  vendored) rather than pretend it away.
- **C4 NEEDS-DECISION (maintainer/social):** filing an ITP and uploading to
  Debian proper is a human process, not agent work. Flagged only.

### Track D — EDS parity features (CLAIMABLE; restores evolution-ews parity)
- **D1 `[claude]` Create/delete a calendar and an address book.** We mirror
  existing server collections but cannot create new ones (audit: no
  `AddressBook/set`/`Calendar/set`, and `create_resource`/`delete_resource` are
  left as EDS defaults — `jmap-backend-collection/src/backend.rs:110`, with
  `tests/backend.rs:340` asserting they are NOT overridden). Add `AddressBook/set`
  + `Calendar/set` create to jmap-proto/jmap-client + mock, then wire
  `create_resource_sync`/`delete_resource_sync` to call them and mint/remove the
  child ESource — mail folders already do exactly this via `Mailbox/set`
  (`jmap-mail/src/manage.rs:227`, `jmap-client/src/mail.rs:164`); mirror it.
  - **PARTIAL 2026-08-19** — the protocol/client/mock half landed:
    `AddressBook/set` and `Calendar/set` (create + destroy) in `jmap-mock`
    (`contacts.rs::address_book_set`, `calendars.rs::calendar_set`, both
    `simple_set` over the existing generic `SetRequest<T>`/`SetResponse<T>` —
    no `jmap-proto` changes needed, since `AddressBook`/`Calendar` are plain
    data types with no hierarchy like `Mailbox` has), wired into
    `dispatch.rs`, and `Client::address_book_create`/`address_book_destroy` +
    `Client::calendar_create`/`calendar_destroy` in `jmap-client`, mirroring
    `contact_create`/`contact_destroy` and `event_create`/`event_destroy`
    exactly. TDD'd in `jmap-client/tests/{contacts,calendars}.rs`. **Still
    open:** wiring `create_resource_sync`/`delete_resource_sync` on
    `ECollectionBackendClass` to call these and mint/remove the child
    `ESource` — GObject-vtable FFI work of the same kind as `child_added`'s,
    not attempted this session (kept out of an increment that is otherwise
    pure safe Rust). Not claimable-complete; the next session on this thread
    should do the FFI wiring.
- **D2 `[claude]` Calendar colour.** `Calendar.color` is parsed
  (`jmap-proto/src/calendars.rs:29`) then dropped — thread it Resource→Child and
  emit an ESourceSelectable `("Calendar","Color", …)` setting in
  `jmap-collection-sync/src/child_source.rs`; write-back rides on D1's
  `Calendar/set`.

### Track E — Sharing + scheduling (SPIKE DONE → NEEDS-DECISION on the doc)
Design spike **complete**: see `docs/PRINCIPALS-DESIGN.md` (commit 98c0576). It
corrected two premises after checking datatracker — the earlier framing here was
wrong:
- **Free/busy slot-picking is NOT in RFC 9670.** `Principal/getAvailability`
  lives in the **JMAP-for-Calendars draft** (this repo already pins draft -27);
  RFC 9670 supplies the shared `Principal` vocabulary + capability URNs
  (`urn:ietf:params:jmap:principals`, `…:principals:owner`) that the availability
  and sharing methods both build on.
- **Mail has permissions but not sharing.** `Mailbox` carries `myRights`
  (RFC 8621) but there is **no standard `Mailbox.shareWith`** — a *mailbox* cannot
  be shared under current specs, only its rights read. Calendar/AddressBook
  `shareWith` live in their respective drafts.

Today we advertise/consume none of it (`jmap-proto/src/session.rs:14-18`);
server-sent `myRights` lands unread in the serde `extra` bag, and read-only is an
account-wide heuristic (`jmap-collection-sync/src/children.rs:93`).
**Recommended plan (from the doc):** a small shared "Principal floor" (proto type
+ two capability constants + client/mock support), then **Path A (free/busy
availability) first** — it answers the scheduling ask soonest, is
read-only/low-blast-radius, and its very first step is a half-day Stalwart probe
to confirm the server actually answers `getAvailability` before committing to the
(unsafe-FFI, EDS-only) `ECalBackend` free/busy vfunc. Rough effort: **~2.5–3.5
weeks** for floor + Path A (scheduling); **~+1 week** for Path B (per-source
`myRights` correctness, mostly published-RFC surface, replaces the read-only
heuristic); write-side `shareWith`/`ShareNotification` deferred to a later phase
(least spec-stable, only destructive surface). OAuth (M7) does not block it.
**DECISION (maintainer): approve the doc's plan to start Path A, or adjust — no
implementation until then.**

### Not doing (protocol-gated)
- **Tasks (VTODO) / Memos (VJOURNAL).** BLOCKED upstream: draft-ietf-jmap-calendars
  models events only; there is no standardized JMAP task/note object (JSTask is a
  separate, less-mature draft). The cal factory registers VEVENT only, on purpose
  (`jmap-backend-cal/src/factory.rs:41-53`) — empty Task/Memo factories would look
  broken, not absent. Revisit if/when a JMAP task object standardizes. Recorded,
  not queued.

## MAINTAINER DECISIONS (2026-08-17) — resolves the three open items

The 338th night-log entry surfaced three items only the maintainer could
decide. All three are now answered:

1. **Auth UX → option (b): autodiscovery-only, by design.** Manual selection
   of OAuth 2.0 on the manual server-settings page stays blocked; there will
   be NO manual "register a client" affordance (option (a) is declined). The
   existing `complete.rs` status message (*"OAuth 2.0 needs \"Look Up Account
   Details\" to run first."*) is the intended documentation of this. This
   clears the last thing holding M7 back. **Action for the agent:** tag M7
   COMPLETE in `docs/MILESTONES.md` — its setup UI is human-verified working
   (two operator rounds, plus the round-2 entry confirming the status label,
   persistence across `--force-shutdown`, and graceful port handling). You may
   refine that message's wording for a first-time user, but add no new code
   path.

2. **M10 → DONE, both legs green.** Compile drift was fixed first (`c28adbb`
   eds-sys, `a8bb65e` jmap-mail); the 3 behavioral `eds-sys` `contacts`
   assertions that then failed on newer EDS
   (`contact_date_fields_are_structured_e_contact_date_types`,
   `e_contact_field_id_from_vcard_maps_x_lines`,
   `structured_name_geo_and_metadata_vcard_lines_and_modification_in_eds`)
   were made version-aware in `00271f9` the same day, per
   `docs/eds-version-matrix.md`'s "(B) Fixed 2026-08-17". That doc update
   never made it back to this section, so this text kept describing the
   pre-fix state and a later session (`1ce7237`) reasonably but wrongly
   un-tagged `M10 COMPLETE` from `docs/MILESTONES.md` on the strength of it —
   a docs-sync bug, not a code regression. **Re-verified 2026-08-19**: the
   `eds-version-matrix` job runs nothing GitHub-specific — it is
   `ci/eds-matrix.sh` inside the digest-pinned public
   `docker.io/library/fedora` image named in `ci.yml`, reproducible on any
   Docker host, no `gh`/dispatch needed. Pulled that exact digest on this
   runner (resolves to EDS 3.60.2, confirmed via `pkg-config`), ran
   `ci/eds-matrix.sh` against current `master` (`eb9f785`) fresh: **1132
   passed, 0 failed**, the 3 named assertions included. `M10 COMPLETE` is
   re-added to `docs/MILESTONES.md`.

3. **Stalwart → provisioned.** `stalwart-1` (europe-west3-c) is running the
   real JMAP server. IMPORTANT: its firewall admits only the *operator's* host
   IP, not the runner's egress — so the `--features live-server` harness is run
   **operator-side**, not from the night runner. Do NOT attempt to reach
   Stalwart from the runner; keep the harness mock-green as before and leave
   real-server runs to the operator.

## Milestones (in order)

### M1 — `eds-sys`: bindgen FFI layer
New crate `rust/crates/eds-sys`: bindgen at build time from the installed
EDS headers (found via pkg-config: `libebackend-1.2`, `libedata-book-1.2`,
`libedata-cal-2.0`, `camel-1.2`), depending on `glib-sys`/`gobject-sys`
for base GObject machinery. Allowlist only what the backends need
(`EBookMetaBackend*`, `ECalMetaBackend*`, `ESource*`, `Camel*` later).
Excluded from `default-members` (needs headers, like example-module).
Acceptance: `cargo build -p eds-sys` succeeds in the CI image; class
struct layouts spot-checked against `g_type_query` sizes in a unit test.

### M2 — `jmap-backend-core`: subclassing scaffold
rlib with the shared machinery: GObject subclass registration helpers,
`extern "C"` vfunc trampolines that `catch_unwind` (a Rust panic must
never cross into C), `GCancellable` → `CancelFlag` bridging, GError
mapping for `jmap_client::Error`. Acceptance: a trivial GObject subclass
registers and instantiates in a test binary linked against the system
GLib.

### M3 — Address book backend (`libebookbackendjmap.so`)
Subclass **EBookMetaBackend** (not raw EBookBackendSync — the meta
backend provides cache/offline for free). Implement the sync vfuncs:
`connect_sync`, `disconnect_sync`, `list_existing_sync`,
`load_contact_sync`, `save_contact_sync`, `remove_contact_sync`, and
`get_changes_sync` mapped 1:1 onto `ContactCard/changes` (client method
exists). JSContact ↔ vCard/EContact mapping, minimal set first: UID, FN,
N, EMAIL, TEL. Security: credentials come from EDS's ESourceAuthentication
(libsecret) — never from config files; TLS required for non-localhost.
CMake: `add_cargo_cdylib` + install into the libedata-book backend dir
(`pkg_check_variable`). Acceptance: mapping unit tests against fixtures;
protocol behaviour tested against `jmap-mockd`; documented manual test
recipe with a hand-written `.source` keyfile.

### M4 — Calendar backend (`libecalbackendjmap.so`)
Mirror of M3 on **ECalMetaBackend**. JSCalendar ↔ iCalendar mapping,
minimal set: UID, SUMMARY, DESCRIPTION, DTSTART (+timeZone), DURATION,
STATUS, RRULE (FREQ/INTERVAL/COUNT/UNTIL). Same acceptance pattern.

### M5 — Mail: Camel provider (`libcameljmap.so` + `.urls`)
The largest piece. `CamelJmapStore` (folder list from `Mailbox/get`),
`CamelJmapFolder` (summaries via `Email/query`+`Email/get`, bodies via
blob download), `CamelJmapTransport` (send via `EmailSubmission/set`,
reusing the client's `send_email` flow). Entry point is
`camel_provider_module_init` (not `e_module_load`). Offline/summary cache
can lean on CamelFolderSummary defaults initially.

### M6 — Collection backend (`module-jmap-backend.so`)
ECollectionBackend for `evolution-source-registry`: one JMAP account
fans out to mail + book + cal sources; autodiscovery via the session
object (`/.well-known/jmap`).

### M7 — Account setup UI (`module-jmap-configuration.so`)
Evolution config module (EExtension idiom, target Evolution 3.52 — note
3.56+ replaced GtkUIManager, so gate anything UI-XML-related).

### M8 — Installable artifacts
Every CI run already uploads the built `.so`s; add a `.deb` built from
the CMake install tree (CPack) so testing a nightly build is
`apt install ./evolution-jmap.deb`. Wire into release.yml with
attestation like the other artifacts.

### M9 — End-to-end tests through real EDS + a GUI smoke test
Keep this deliberately small; a full Evolution UI suite is out of scope
for this repo (see below). Two layers only:
- **Layer 1 — functional, headless (the priority).** Drive Evolution
  Data Server through its client API / D-Bus against `jmap-mockd`:
  create a contact via `e-book-client` and assert it reached the mock's
  store; the same for a calendar event via `e-cal-client`; list the
  inbox. Fast, deterministic, no display server. A gated CI job
  (`workflow_dispatch`/label) since it needs the EDS *runtime*, and a
  local recipe. Depends on M3/M4/M5.
- **Tier 2 — one GUI smoke test.** Under `Xvfb`, launch Evolution
  against the mock and assert it starts, the account appears, and the
  inbox is non-empty (driven via AT-SPI/dogtail). Capture artifacts
  ONLY on failure: record the X session to tmpfs with `ffmpeg
  -f x11grab` and keep the video only if the test failed; on failure
  also dump a screenshot, the AT-SPI tree, and the EDS + mock logs.
  Upload with `if: failure()` / `artifacts: when: on_failure`, so a
  green run stores nothing. One test — accept it will be a little flaky
  (retry once); it is a canary, not coverage.

Full scripted GUI flows (click through account setup, read message
lists, open contacts) with screenshots/video are explicitly deferred to
a **separate project** that UI-tests the latest *released* plugin build
— keeping this repo's test surface fast and this milestone cheap.

### M10 — FFI safety across EDS versions (a CI matrix)
The FFI layer is generated by bindgen at build time and guarded by
`eds-sys/tests/layout.rs`, which cross-checks every subclassed struct's
`size_of` against the running GObject type system's `g_type_query()`.
Today that runs against a single pinned EDS (3.52, in the CI container),
so it only proves self-consistency against *that* version. This
milestone extends it to a **matrix of EDS versions**, turning "the ABI
drifted on a newer EDS" from a latent runtime segfault into a red check.

Acceptance:
- A CI job builds `eds-sys` + the backend crates and runs the
  `eds-sys`/`jmap-backend-*` test suites (layout checks included) against
  each of several EDS releases — at minimum the pinned 3.52, the current
  stable, and (once it exists) a 3.56+ that crosses the GtkUIManager
  change M7 notes. Prefer distinct pinned container images per version
  (built like the main CI image, digest-pinned) over apt pinning.
- A build or layout mismatch on ANY version fails that matrix leg
  loudly, with the version and the offending type in the output.
- Document, in the job or a short `docs/`, which EDS versions are
  supported and that the plugin must be built against the EDS it is
  deployed on (the normal ABI contract for EDS modules).
- Out of scope: making the code actually *work* on every version — the
  matrix's job is to make breakage *visible*, not to auto-port. A leg
  that legitimately can't pass yet (e.g. 3.56 UI changes before M7 is
  ported) may be marked `allow_failure`/informational, but must still
  run so the breakage is on the record, not invisible.
- What this does NOT catch, and must say so plainly: *semantic* ABI
  drift — a signature and struct size that stay identical while the
  contract (ownership, enum meaning, vfunc pre/postconditions) changes
  underneath. Only behavioural tests (M9 Layer 1) and human changelog
  review catch that; the matrix is necessary, not sufficient.

## Standing directives

### Outsource iCalendar/vCard parsing to `calcard` (2026-08-08)
Replace the hand-rolled RFC 5545/6350 text layers — `jmap-ical`'s
lexer/emitter and `jmap-vcard`'s syntax module — with the
[`calcard`](https://github.com/stalwartlabs/calcard) crate (MIT, Stalwart
Labs): it parses and builds iCalendar, vCard, JSCalendar, and JSContact
and converts between the pairs, and is production-hardened by Stalwart's
CalDAV/CardDAV stack. Keep our semantic mapping decisions and ALL existing
fixture/round-trip tests — they are the acceptance suite for the
migration; a behaviour difference calcard introduces is a finding, not a
nuisance. Rationale: outsource parsing liability; our code should carry
only the JMAP/EDS integration it exists for.

### Mark UI strings translatable (2026-08-09)
Every user-facing string — account-setup labels and tooltips (M7),
Camel/EDS error and status text the user can see, folder/source display
names we originate — must be wrapped for translation via gettext the
moment it is introduced, not retrofitted later. Retrofitting means
hunting every literal after the fact and missing some; marking at
introduction is nearly free.
- C code: `_( )` / `N_( )` with the project's `GETTEXT_PACKAGE` (already
  set in the top-level `CMakeLists.txt`); `bindtextdomain` wired in each
  module's init. Rust code emitting user-visible text: `gettextrs` (or an
  FFI call to `g_dgettext`) against the same domain.
- Set up `po/` with a `POTFILES.in` listing every source that holds
  translatable strings, and a `LINGUAS`; a CI check (or the `reuse`-style
  lint) that flags a source added to a UI crate but absent from
  `POTFILES.in`. Do NOT translate protocol constants, JMAP property
  names, log/trace text, or developer-facing errors — only what an
  Evolution user reads.
- Not a blocker for landing a UI milestone, but a string shipped
  unmarked is a bug to be filed, not ignored. Applies from M6/M7 onward,
  where the first user-visible strings appear.

### Recurring security re-audit (2026-08-08)
The first FFI audit (`docs/AUDIT-FFI.md`, branch `audit/ffi`) found F1–F10;
F1–F4 are fixed on master with regression tests that run in CI, so those
specific issues cannot silently return. But new code lands continuously,
and CI only catches regressions of *known* bugs — not novel ones. So the
adversarial audit runs again periodically, as its own stream (NOT roadmap
feature work — the roadmap agent must not attempt it).

Each re-audit (driver `infra/night-shift/start-reaudit.sh`, prompt
`infra/night-shift/reaudit-prompt.md`) forks a fresh dated branch off
current master, and:
1. Re-verifies every prior finding's disposition still holds — the F1–F4
   regression tests exist and pass, and the F5–F10 "clean"/"info"
   judgements are still true of the current code.
2. Audits everything added since the last audit's base commit (recorded
   in the previous report) with the same hunt list as the original.
3. Writes a dated report `docs/AUDIT-FFI-<date>.md` ending in
   `AUDIT COMPLETE`, fixing clear-cut bugs (each with a red-first test)
   and documenting design concerns.

Cadence: launched automatically after the mail surface settles. A VM-side
watcher (`infra/night-shift/reaudit-trigger.sh`) polls `docs/MILESTONES.md`
and fires the re-audit exactly once, when `M5 COMPLETE` appears (and folds
in the calcard surface whether or not `CALCARD COMPLETE` has landed yet —
the prompt audits calcard code if present). Further passes are launched
per-milestone by hand or via the documented weekly cron. Same isolation
rules as the original audit stream (own clone, own branch, never master,
never touch other streams' checkouts).

### Open audit recommendations to action
Findings the re-audits raised as recommendations rather than fixing in
place. Close them like any other work — red test first, then fix.
- ~~**F14 (from AUDIT-FFI-20260810) — URI-encode server-chosen values in
  blob URLs.**~~ **Closed 2026-08-10.** `jmap-client/src/url.rs`
  percent-encodes every value substituted into the `downloadUrl`/
  `uploadUrl` templates, down to RFC 3986 §2.3's unreserved set so one
  encoder is correct in a path segment and in a query value alike; the
  mock decodes path segments after splitting them, and
  `jmap-client/tests/blob_urls.rs` drives a hostile `blobId`, `name` and
  `accountId` end to end. The alternative the report offered — a grammar
  check in `Id`'s constructor — was not taken: it constrains ids the RFC
  allows, and encoding is what §6.2 actually asks for.
- ~~**F15 (from AUDIT-FFI-20260810) — every response is capped at 10 MiB by a
  limit nobody here chose.**~~ **Closed 2026-08-11.** `HttpRequest` now carries
  `max_response_bytes`, every caller states one, and `UreqTransport` applies it
  rather than letting `ureq`'s `read_to_vec` impose its own `MAX_BODY_SIZE`.
  The numbers are `jmap-client/src/limits.rs`'s, with their reasoning written
  down: a JSON answer is held to `MAX_API_RESPONSE_BYTES`, and a blob download
  to what its caller asks for — for mail, the row's own `size` widened by
  `jmap_mail_sync::download_ceiling`, so the bound is the account's rather than
  a constant this repository guessed, with `MAX_BLOB_BYTES` as the fallback for
  a server that reports no size. Over the ceiling is
  `Error::ResponseTooLarge`, abandoned at the limit rather than buffered and
  then judged. `jmap-client/tests/response_size.rs` and
  `jmap-mail-sync/tests/source.rs` drive an 11 MiB message end to end, the
  refusal, and the boundary — `ureq`'s limiting reader fails on the read
  *after* the last octet allowed, so a body of exactly the ceiling needs a
  limit one higher.

## Integration testing (parallel track)
Once M3 exists: gated CI job + local recipe against a real
[Stalwart](https://stalw.art/) server (full JMAP mail/contacts/calendars)
— `infra/gcp/create-stalwart.sh` provisions one. The mock stays the
default test target.

## Rules for autonomous work sessions

**Correctness over progress — the overriding principle.** The maintainer
would rather this repository advance one milestone slowly and *correctly*
than three milestones quickly and dirtily. A session that ends with no
new commit because the honest state was "blocked" or "needs human
verification" is a *good* session, not a failure. Concretely:
- Never weaken, skip, `#[ignore]`, or delete a test to make something
  pass; never stub a function and present it as implemented; never paper
  over a failing check. If the real thing is hard, do the real thing or
  stop and log why.
- **Do not claim what you cannot verify.** If a milestone's behaviour
  can't be checked here — most importantly GUI/config code (M7, M9's GUI
  tier), which needs a real Evolution session and a display this VM
  lacks — implement it conservatively, mark it in `docs/NIGHT-LOG.md` as
  *needs human verification in real Evolution*, and do NOT tag it
  `COMPLETE`. Compiling is not working.
- Prefer a small, fully-tested increment over a large, partly-verified
  one. When unsure whether something is right, stop and write down the
  uncertainty rather than pushing on it.

- Work only inside this repository; never force-push; never rewrite
  history; do not modify `infra/` or `.github/workflows/ci-image.yml`
  unless a milestone requires it.
- TDD: red test first (against fixtures or `jmap-mockd`), then green.
  Before every push, run **`ci/checks.sh`** and make sure it passes — it
  is the single source of truth for the gate (rustfmt, clippy
  `-D warnings`, tests, `cargo deny`, `reuse lint`), the very same script
  CI runs, so a green local run means a green pipeline. Crates needing
  EDS headers stay out of `default-members`.
- Every source file: SPDX header, `GPL-3.0-or-later` (checked by
  `ci/checks.sh`).
- **Commit messages a tired reviewer can skim.** Author
  `Tobias Mueller <muelli@cryptobitch.de>`, **no Co-Authored-By
  trailers**. The subject line states, in the imperative, *what the
  commit does* — someone reading `git log --oneline` must understand it
  without opening the diff. Shape it `crate: do the thing` (≤ ~70 chars;
  a leading `Mn ` milestone tag is fine). Do **not** write oblique
  noun-phrase subjects: they read as riddles.
    - ✗ `M6: jmap-collection-sync, the address books an account is not one of`
      → ✓ `jmap-collection-sync: skip address books the account doesn't own`
    - ✗ `M5: the message that leaves the account`
      → ✓ `jmap-mail: send a message via EmailSubmission`
  The body is concise — *what* changed and *why*, a few lines (wrap ~72),
  not an essay. The deep design narrative (the "here is exactly why this
  was subtle" prose) belongs in `docs/NIGHT-LOG.md`, which is its right
  home; a commit body is 2–6 lines, and repeating the diary there is
  wasted words.
- Push after each green increment (deploy key is configured).
- Keep a running log in `docs/NIGHT-LOG.md`: what was done, decisions
  taken, blockers hit. If blocked on a milestone, log it and take the
  next tractable item instead of spinning.
- **Signal milestone completion.** When a milestone's acceptance
  criteria are fully met (or a standing directive is fully carried out),
  append one line to `docs/MILESTONES.md` and commit it with the work:
  `<TAG> COMPLETE <short-sha> <ISO-date>` — e.g. `M5 COMPLETE a1b2c3d
  2026-08-10`, or `CALCARD COMPLETE …` for the calcard directive. This
  file is a machine-readable trigger (the re-audit watcher watches it);
  write a tag only when you would defend the milestone as genuinely
  done, and never remove or edit prior lines.
