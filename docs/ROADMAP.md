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
   - **PARTIAL 2026-08-19 (later same day) — call site (b) wired.**
     `config_lookup.rs::probe_host` now takes a `&dyn jmap_client::resolver::
     Resolver` and consults it for `_jmap._tcp.<domain>` before falling back
     to the bare domain, exactly the seam and fallback order
     `ClientBuilder::connect_domain` already uses; an SRV target renders as
     `host:port`, which `parse_target` already reads as a bare, secure host
     with that port. `run()` passes `NoSrvResolver` — behaviour is
     unchanged until a real resolver exists — with a comment marking that as
     the one line the future GResolver-backed resolver replaces. TDD'd with
     the same `FakeResolver` shape as `srv_discovery.rs`. **Still open:**
     call site (a) — `fan_out.rs`/the backend `connect.rs` files all call
     `Client::connect(&config.origin, …)` where `origin` is a fully-assembled
     `scheme://host:port` string from `jmap_backend_core::source::origin`;
     reconciling that structured host/port/secure model with
     `connect_domain`'s bare-`https`-only argument is a real design question
     (every backend test wires an explicit mock-server port through `origin`)
     and was deliberately not attempted in the same increment as (b). The
     GResolver-backed real `Resolver` (FFI) is still unstarted and still the
     escalation candidate.
   - **DONE 2026-08-19 (later still) — call site (a) wired, both sites now
     covered.** A research agent first traced the actual production path a
     plain email+password Fastmail-style setup takes, to confirm this really
     is the remaining gap: `jmap-config/src/defaults.rs::from_identity`
     correctly writes the bare email domain into `Authentication:Host` with
     no port, per RFC 8620 §2.2 — no UI autodiscovery step was missing there.
     The design: `jmap_backend_core::source` gained `ConnectTarget`
     (`Origin(String)` for an explicit endpoint or an IP literal, `Domain
     (String)` for a port-unset+secure+non-IP-literal host — exactly the
     shape `from_identity` produces, and exactly RFC 8620 §2.2's "the domain
     is the entry point" case) plus `connect_target()`, sharing `origin()`'s
     existing host validation and TLS rule (`origin()` is now a thin wrapper
     over it, so `jmap-backend-collection`'s `Server::origin` display value
     — asserted verbatim in several tests — keeps its exact old string). A
     new `jmap_backend_core::source::connect(target, credentials)` dispatches
     `Origin` to today's `Client::connect` and `Domain` to `ClientBuilder::
     connect_domain`; `jmap-backend-book`/`cal`'s `connect.rs`, `jmap-mail/
     src/connect.rs`, and `jmap-backend-collection/src/fan_out.rs` all switch
     to it. `SourceConfig`, `jmap-mail`'s `ServerConfig`, and `jmap-backend-
     collection`'s `Server` all carry `target: ConnectTarget` instead of a
     collapsed `origin: String` (`Server` keeps a separate `origin: String`
     display field, still built via the `origin()` wrapper, since fan_out's
     own tests construct `Server::connection` — the fields repeated to
     children — independently of the address actually dialled, and `.target`
     is what the connect call now reads instead). `jmap_client::Client`
     gained a shared `rebase_urls_from_env()` (extracted from `Client::
     connect`'s existing env-var check) so the `Domain` branch honours
     `JMAP_LIVE_SERVER_REBASE_URLS` identically to the `Origin` one. TDD:
     new `connect_target` unit tests in `jmap-backend-core/src/source.rs`
     (bare domain → `Domain`, explicit port → `Origin`, IP literal even with
     no port → `Origin`, insecure/missing-host refusals unchanged); every
     existing `SourceConfig`/`ServerConfig`/`Server`-driven test updated to
     assert the right `ConnectTarget` variant instead of a bare string —
     several (`jmap-config`'s `the_defaults_are_the_account_the_address_
     names`, `the_default_and_the_registry_agree_about_the_server`, and
     siblings) flip from `Origin("https://example.com")` to
     `Domain("example.com")`, which is the actual behaviour change this
     closes: a plain email+password setup's session discovery now tries
     `_jmap._tcp.<domain>` before the bare-domain `.well-known/jmap`
     fallback. Behaviour is unchanged today (`NoSrvResolver`, the only
     resolver anything constructs, never finds a record) until the
     GResolver-backed real `Resolver` lands — FFI, unstarted, still the
     escalation candidate for this thread, and the last piece needed before
     Fastmail's password path can be verified end-to-end.
     Full gate: `cargo fmt --check` clean; `cargo clippy --all-targets
     --locked -- -D warnings` (default-members) and `cargo clippy -p
     evolution-jmap-client -p jmap-backend-core -p jmap-backend-book -p
     jmap-backend-cal -p jmap-mail -p jmap-backend-collection -p jmap-config
     --all-targets -- -D warnings` (the EDS-gated crates directly, this VM
     having the headers) both clean; `cargo test --locked` and the same
     per-crate `cargo test` both green, every `test result: ok`, 0 failed.
     Disk filled mid-session (`rust/target/debug` at 24G, "No space left on
     device" on `jmap-mail`'s test build) — `cargo clean --profile dev`
     recovered it, consistent with prior sessions' standing note.
   - **CLAIMABLE NOW — build the real SRV `Resolver` (the last piece before a
     Fastmail end-to-end test).** Both call sites now route through the seam but
     still construct `NoSrvResolver`, so no real SRV happens yet and Fastmail's
     password path still 404s. Build a resolver that actually looks up
     `_jmap._tcp.<domain>` and inject it where `NoSrvResolver` is constructed
     today (`jmap_backend_core::source::connect` and `config_lookup::run`).
     **Maintainer preference (2026-08-19): reuse a library over hand-rolled code
     — a crate if one fits, else a system library, and only "own code" if
     neither works (rather own code than no code).** Given the workspace is
     deliberately blocking / tokio-free and the resolver is built in the
     EDS-integration layer where GLib is already linked, the recommended answer
     is **GLib's `GResolver` via `g_resolver_lookup_service()`**, and the binding
     is **already in the tree**: `gio-sys` 0.22 is already in `Cargo.lock`
     (sibling of the `glib-sys`/`gobject-sys` this project already uses, MIT,
     `deny.toml`-allowlisted) and exposes `g_resolver_get_default`,
     `g_resolver_lookup_service`, and the `GSrvTarget` accessors
     (`g_srv_target_get_hostname/_port/_priority/_weight`). So there is **no new
     dependency and no hand-written FFI to maintain** — just a `Resolver` impl
     that calls them and maps the returned `GSrvTarget` `GList` (RFC 2782 order:
     lowest priority, then highest weight) to `SrvTarget`, freeing the list after.
     `g_resolver_lookup_service` is frozen GLib API (since 2.22, 2009), so drift
     risk is minimal — this is the opposite of the EDS-vCard surface that drifted
     on 3.60. A pure-Rust SRV crate is acceptable *only* if it is
     blocking, license-allowlisted (`deny.toml`), and drags no async runtime into
     the lean tree — most (e.g. hickory) are tokio-based, so evaluate before
     adding; hand-rolled DNS packet parsing is the last resort, not the default.
     Test against a fake as `srv_discovery.rs` does; a live
     `_jmap._tcp.fastmail.com` lookup is network-dependent, so leave true
     end-to-end confirmation to the operator in the VM (real Fastmail, using an
     **app password / API token**, not the login password — that 401s, a
     separate concern from this 404). FFI — reasonable to escalate.
   - **DONE 2026-08-19 (on opus, per the escalation at `a4533d1`) — the real
     resolver landed; item 5's code side is complete, pending operator
     confirmation.** `jmap-backend-core/src/resolver.rs` adds
     `SystemResolver`, a `jmap_client::resolver::Resolver` implementation
     backed by `g_resolver_lookup_service()` — already bound in `gio-sys`
     0.22, so **no new dependency and no hand-written DNS**, exactly as the
     maintainer preference above directed. It lives in `jmap-backend-core`
     because that is the one EDS-integration crate *both* call sites can
     reach (`jmap-config` already depends on it), and it is now installed at
     both, replacing `NoSrvResolver`: `jmap_backend_core::source::connect`'s
     `Domain` branch (via `ClientBuilder::resolver`) and
     `jmap_config::config_lookup::run`. Two GLib guarantees, checked against
     the installed `Gio-2.0.gir` doc text rather than assumed, are what keep
     the implementation small: the returned `GSrvTarget` list is **already
     sorted into RFC 2782 preference order** (so the first node is the answer
     — no hand-rolled priority/weight sort) and it is **NULL on failure,
     non-empty on success**, freed as a whole by `g_resolver_free_targets`.
     Every unresolvable case — no record, a failed lookup, a domain Rust
     cannot even hand to C (interior NUL), a `.` target (RFC 2782's "no
     service here"), a zero port — answers `None`, which is the direction
     that matters: an SRV record can only *redirect* discovery, never break
     the deployments that answer at their own domain (Stalwart, self-hosted,
     the in-repo mock). A fully-qualified target loses its trailing dot.
     **Deliberate limit:** the lookup is not cancellable, because the
     `Resolver` trait passes no `GCancellable` and storing a raw one in a
     `Send + Sync` value is the worse trade for a lookup this short; the
     system resolver's own timeout bounds it. TDD: red test first
     (`jmap-backend-core/tests/resolver.rs` failed to compile on the missing
     module), then the deterministic cases — a `.invalid` domain (RFC 6761
     guarantees it never resolves, so the test holds with or without DNS
     egress), the interior-NUL refusal, and 64 repetitions of the failing
     path so an unbalanced ref/free has somewhere to show — plus pure-helper
     unit tests for the host normalisation. The success path cannot be
     hermetic, so it is an `#[ignore]`d live test against
     `_jmap._tcp.fastmail.com`; **it was run, and passes** —
     `api.fastmail.com:443`, which is the one thing no fake can prove (that
     the `GSrvTarget` list is walked and read correctly). Full gate green:
     `cargo fmt --check`; `cargo clippy --all-targets --locked -- -D
     warnings` (default-members) and the seven-crate EDS-gated clippy both
     clean; `cargo test --locked` (84 binaries, 0 failed) and the
     seven-crate `cargo test` (1178 passed, 0 failed) both green.
     **Finding, logged not worked around:** GLib 2.80's
     `g_resolver_lookup_service()` leaks ~1 kB per call — proven upstream, not
     ours, by a minimal C reference doing the identical canonical sequence
     (while `g_resolver_lookup_by_name` is flat). Once per connect is a
     frequency that makes it not worth a TTL-ignoring cache; see
     `docs/BACKLOG.md`. **Still open, and now the only thing left on item 5:**
     operator end-to-end confirmation against real Fastmail in the VM, using
     an **app password / API token** (the login password 401s — a separate
     concern from the 404 this closes).

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
  - **PARTIAL 2026-08-19** — the `jmap-proto` deserialization half landed:
    `jmap-proto/tests/malicious_input.rs` feeds a bounded-depth, bounded-
    breadth arbitrary-JSON `proptest` strategy into `Session`, `Request`,
    `Response`, `MethodError`, and `RequestError` deserialization, asserting
    only "no panic" (`Ok`/`Err` both pass). No `arbitrary` crate needed — a
    hand-rolled recursive JSON strategy in `proptest` alone was enough,
    keeping the new dev-dependency to one crate. All five properties pass on
    the first run; a manual read of the deserialization paths first
    (`Invocation`'s hand-rolled `Deserialize` delegates to serde's own
    length-checked tuple machinery, not custom indexing) found no panic
    site, so this is a regression net rather than a bug fix, consistent with
    A3's "fix any panic found" wording allowing for none. **Still open:**
    the `jmap-client` half (hostile `apiUrl`/redirect targets into
    `transport.rs`/`url::rebase_origin`) — a manual read of `rebase_origin`
    during scoping found every slice index it uses comes from `str::find`,
    which only ever returns valid char-boundary offsets, so it looks
    panic-safe by inspection; a proptest harness proving that (rather than
    inspecting it) is the next session's increment on this thread.
  - **DONE 2026-08-19** — the `jmap-client` half landed:
    `jmap-client/src/url.rs`'s `#[cfg(test)]` module gained a `proptest!`
    block with two properties over an unconstrained `.*` (arbitrary Unicode)
    string strategy — `rebase_origin_never_panics_on_hostile_input(url,
    origin)`, standing in for a malicious/buggy server's fully-controlled
    `apiUrl`/`downloadUrl`/`uploadUrl`/`eventSourceUrl`, and
    `encode_template_value_never_panics_on_hostile_input(value)`, standing in
    for a server-supplied `blobId`/`accountId` substituted into a blob-URL
    template. Both had to live inside `url.rs` itself rather than a `tests/`
    integration binary, since both functions are `pub(crate)`. Both
    properties pass on the first run (256 cases each), confirming rather
    than resting on the prior session's by-inspection reasoning — another
    regression net, no panic found, consistent with A3's wording. The
    "no auth leak on redirect" half of A4's acceptance criterion is separate
    coverage, not proptest: `UreqTransport::new`'s `redirect_auth_headers =
    SameHost` and `jmap-client/tests/redirect_auth.rs` (from the 86fea00 fix)
    already assert a cross-host redirect drops the `Authorization` header;
    this session did not add fuzzing over arbitrary redirect targets on top
    of that fixed-case test, so treat that as the one narrower thing left on
    this thread if a future session wants to broaden it — Track A4 itself is
    otherwise complete (both named halves, `jmap-proto` and `jmap-client`,
    done).
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
  - **DONE 2026-08-19** — `lintian --pedantic` was clean on Section/Priority/
    extended description/Depends already; the four real findings — unstripped
    binaries, a missing `changelog`/`copyright` under `/usr/share/doc/`, and
    group-writable (0775) directories from the builder's umask — are fixed,
    and a `package-deb-lintian` CTest keeps it that way. See NIGHT-LOG.
- **C2 `[claude]` Machine-readable DEP-5 `debian/copyright`** generated from the
  REUSE metadata we already maintain — the single biggest ease-of-packaging win.
  - **PARTIAL 2026-08-19** — the own-source half is done:
    `tools/generate-debian-copyright.py` renders `docs/packaging/copyright`
    from `REUSE.toml`'s `[[annotations]]` (fixing a real inaccuracy — the
    hand-written file omitted the `LGPL-2.1-or-later` example-module
    override entirely), kept in sync by the `debian-copyright-in-sync` CTest.
    See NIGHT-LOG "Delivered: Track C2 (own-file slice)". **Still open, and
    NEEDS-DECISION, not a guess to make headlessly:** the ~140 third-party
    Cargo crates statically linked into the shipped `.so`s have no honest
    DEP-5 `Files:` pattern (their sources are neither vendored here nor
    shipped in the `.deb`) — pick (a) a non-`Files` third-party-notices
    appendix, or (b) full `dh-cargo` vendoring under Track C3, before a
    future session attempts the enumeration.
- **C3 `[claude]` `debian/` skeleton** (control, rules using `dh` over the
  cmake/cargo build, watch file) so a Debian packager starts from a working tree.
  Document the Rust-in-Debian reality (dh-cargo wants every crate dep packaged /
  vendored) rather than pretend it away.
  - **DONE 2026-08-19 (skeleton only — not upload-ready, see below)** —
    `debian/control` (Build-Depends mirroring `ci/install-deps.sh` plus
    `debhelper-compat (= 13)`/`cargo`/`rustc`), `debian/rules` (dh over the
    existing `cmake/{Rust,Backends,Packaging}.cmake` tree, `-G Ninja`, an
    `override_dh_auto_install` looping `cmake --install --component` over
    the same five components Track C1's CPack path names, so the demo
    `src/` module stays excluded the same way), `debian/watch` (GitHub tags,
    matching the `vX.Y.Z` tags already pushed), `debian/source/format`
    (`3.0 (quilt)`, empty patch queue), and `debian/README.source`.
    `debian/changelog`/`debian/copyright` are symlinks to
    `docs/packaging/{changelog,copyright}` rather than second copies, since
    those are already in the right formats and already kept current (by
    hand, and by Track C2's generator, respectively). Verified for real,
    not just written: installed `debhelper` locally (`ci/install-deps.sh`
    never needed it — Track C1's CPack path doesn't use `dh`) and ran
    `dpkg-buildpackage -us -uc -b -d` end to end; needed one fix
    `dh_shlibdeps` doesn't get for free — Evolution's private libdir
    (`pkg-config --variable=privlibdir evolution-shell-3.0`), the same one
    `cmake/Packaging.cmake` already passes to CPack's `dpkg-shlibdeps` —
    passed via an `override_dh_shlibdeps`. Resulting `.deb`: `lintian
    --pedantic` clean, same standard as Track C1's package.
    **Explicitly not solved, and said so in `debian/README.source`:** the
    build only succeeds because `~/.cargo/registry` already holds the ~140
    crates from ordinary `cargo build`/`cargo test` use on this VM — a real
    Debian buildd has no network and no such cache, so this is a stress-free
    local demo of the skeleton, not proof it survives an official build.
    Two ways to actually close that gap are recorded in
    `debian/README.source` (mirroring Track C2's own copyright-file note of
    the identical problem, since it's the same missing vendoring
    decision from both directions): `cargo vendor` into the source package,
    or `debcargo`-generated `librust-*-dev` packages per dependency —
    neither attempted here, a maintainer call plus real effort either way.
    `debian/watch` is unverified against the live GitHub tags page (no
    `uscan`/`devscripts` on this VM). `dh_auto_test` is a deliberate no-op
    (same crates.io-access reasoning; the Rust suite already runs via
    `ci/checks.sh`/CTest against this exact source). `.gitignore` gained the
    dh/dpkg build-product entries (`/obj-debian/`, `/debian/evolution-jmap/`,
    etc.) and `REUSE.toml` a `debian/**` entry — `control`/`rules`/`watch`
    carry in-file SPDX headers already, but reuse's comment-style lookup is
    by filename/extension and none of these extensionless Debian-packaging
    names are in its recognized list; `changelog`/`copyright` are symlinks
    needing their own annotated path; `source/format`'s content is fixed by
    dpkg-source's own format string, leaving no room for a comment.
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
  - **PARTIAL 2026-08-19 (on opus, per the escalation at `7dd8a00`) — the
    `create_resource_sync` half of the vtable wiring landed; only
    `delete_resource_sync` is left.** `ECollectionBackendClass::
    create_resource_sync` is now overridden in
    `jmap-backend-collection/src/backend.rs`, and the account source is made
    `remote-creatable` from `populate` (through a new
    `Populating::offer_creation`, so *whether* it is offered is a tested
    decision rather than an untestable line in the vfunc) — without which the
    vfunc is unreachable dead code, since `server_side_source_remote_create_sync`
    refuses outright for a collection source that lacks the flag. The decisions
    are split the way the crates are: `jmap-collection-sync/src/create.rs`
    (`Requested`, `CreateFailure`, `create_collection`) resolves the JMAP account
    through the existing `CollectionLayout`, calls `AddressBook/set`/
    `Calendar/set`, and derives the `Child` through a new `Child::for_resource`
    that `Fanout::children()` now also uses — so a created child cannot drift
    from a discovered one; `jmap-backend-collection/src/create_resource.rs` holds
    the EDS ends (`requested_of`, `adopt_created`, `stored_password_of`).
    **Three findings from reading the EDS 3.52.4 and evolution-ews 3.52.4
    sources rather than inferring:** (a) the parent's `create_resource_sync` is a
    `G_IO_ERROR_NOT_SUPPORTED` refusal, so this is the one override in the crate
    that must *not* chain up; (b) the scratch `ESource` is a real
    `EServerSideSource` EDS builds in the *user* source directory and
    deliberately does not add to the registry, so the backend has to finish it —
    `parent`/`write_directory`/`writable`, which is
    `collection_backend_new_source()`'s own set minus the `removable = FALSE`
    that `child_added` supplies; (c) credentials need not be cached the way
    evolution-ews caches them, because
    `e_source_registry_server_ref_credentials_provider()` +
    `e_source_credentials_provider_lookup_sync()` were already in `eds-sys`'s
    generated bindings — so the password is looked up on demand: no secret held
    for the life of the account, no instance state, and a create works in a
    process where no `authenticate_sync` has run yet. `authenticate.rs` grew a
    shared `login_of`/`LoginError` so the OAuth2-vs-password rule stays written
    once. **Deliberately NOT done, and not an omission:** `delete_resource_sync`
    and the `remote-deletable` flag that would reach it. It is the destructive
    half — a wrong kind or id there costs a server-side collection with no undo
    — and setting `remote-deletable` before the vfunc exists would make
    Evolution offer "Delete" and answer the click with EDS's "does not support
    deleting remote resources". `tests/backend.rs` and
    `tests/create_resource.rs` both assert the current state on purpose, so the
    gap is visible rather than forgotten. *(Closed the same day — see the
    `delete_resource_sync` entry below.)* TDD: the pre-existing
    `tests/backend.rs:340` assertion (that `create_resource_sync` is NOT
    overridden) was the red test and was flipped; new
    `jmap-collection-sync/tests/create.rs` (6 tests against `jmap-mockd`,
    including "the created child equals the one the next discovery writes") and
    `jmap-backend-collection/tests/create_resource.rs` (9 tests against a real
    `EServerSideSource` built the way EDS builds one), plus 2 new `populate`
    tests for the creatable flag. New user-facing strings are gettext-marked and
    `po/POTFILES.in`/`po/evolution-jmap.pot` regenerated. Full gate green:
    `cargo fmt --check`; `cargo clippy --all-targets --locked -- -D warnings`
    (default-members) and the seven-crate EDS-gated clippy both clean; `cargo
    test --locked` (85 binaries) and the seven-crate `cargo test` (1190 passed)
    both green, 0 failed. **NEEDS HUMAN VERIFICATION in real Evolution** — no
    headless test can drive "New Address Book" against a live registry, so do
    not tag D1 complete on this.
  - **DONE 2026-08-19 (on opus, per the escalation at `1ff28e7`) —
    `delete_resource_sync` landed; D1's code side is complete, pending
    operator confirmation.** `ECollectionBackendClass::delete_resource_sync` is
    now overridden in `jmap-backend-collection/src/backend.rs`, and every child
    of a JMAP collection is made `remote-deletable` — without which the vfunc is
    unreachable, since `server_side_source_remote_delete_sync()` refuses on the
    child's own flag before it ever looks for a backend. The split mirrors the
    create side exactly: `jmap-collection-sync/src/delete.rs` (`Doomed`,
    `DeleteFailure`, `delete_collection`) resolves the JMAP account through the
    existing `CollectionLayout` and calls `AddressBook/set`/`Calendar/set`
    destroy; `jmap-backend-collection/src/delete_resource.rs` holds the EDS ends
    (`DeleteError`, `doomed_of`, `offer_deletion`, `delete_on_server`).
    **Four decisions taken from the EDS 3.52.4 and evolution-ews 3.52.4 sources
    rather than inferred:** (a) the parent's `delete_resource` is the same
    `G_IO_ERROR_NOT_SUPPORTED` refusal `create_resource` is, so this override
    must not chain up either; (b) `remote-deletable` goes on the **child**
    source, the opposite of `remote-creatable`'s account source; (c) the order
    is destroy-then-`e_source_remove_sync`, which is EDS's documented "the
    implementor must also remove @source from the backend's server" and also
    the only recoverable order — a source removed first and a destroy that then
    failed is a collection still on the server that the next populate puts back
    under a *new* uid, losing the old child's offline cache for nothing; (d)
    `e_source_remove_sync` on a server-side source *is* the registry removal
    (`server_side_source_remove()` calls `e_source_registry_server_remove_source`
    and deletes the key file), so `crate::removal` grew a shared `remove_source`
    and the vfunc reuses the populate's call rather than writing a second one.
    **One deliberate deviation from evolution-ews:** EWS sets `remote-deletable`
    at each of the three sites that mint a child; this backend sets it once, in
    the `child_added` vfunc — EDS's own funnel for every child of a collection
    (fanned-out, cached-and-exported, or just published by a create) and the very
    place EDS writes `removable = FALSE`. One funnel is the same behaviour with
    no site left to forget, and it is gated on `doomed_of` answering `Some`, so
    the account's mail sources and anything this backend did not write are never
    offered. That gate is the whole safety of the feature: the vfunc is handed
    whichever source the user clicked on, and a guess there is not a wrong error
    message, it is a destroy sent to a JMAP server naming an id read out of
    somebody else's keyfile. The kind is carried in `Doomed` and never inferred
    from the id, because an `AddressBook` and a `Calendar` may share one (RFC
    8620 §1.2) and picking the `/set` call wrong destroys the other object and
    reports success — `tests/delete.rs` has that case explicitly. Also factored:
    `backend.rs::login_for`, the account→password→`Login` prelude both resource
    vfuncs need, so the OAuth2-vs-password rule stays written once (and
    `stored_password_of` took a `context` so its criticals name the right vfunc).
    TDD: the pre-existing `tests/backend.rs` assertion that `delete_resource_sync`
    is *not* overridden was the red test and was flipped (with `delete_resource`/
    `delete_resource_finish` now pinning the slot from the far side); new
    `jmap-collection-sync/tests/delete.rs` (7 tests against `jmap-mockd`) and
    `jmap-backend-collection/tests/delete_resource.rs` (8 tests against real
    `ESource`s, a real `EServerSideSource` and a real mock). New user-facing
    strings are gettext-marked and `po/POTFILES.in`/`po/evolution-jmap.pot`
    regenerated — one of them found a real trap worth recording: Rust strips a
    `\`-continuation's newline *and* the next line's leading spaces while
    xgettext strips neither, so a wrapped msgid literal is one no translation
    ever matches. Full gate green: `cargo fmt --check`; `cargo clippy
    --all-targets --locked -- -D warnings` (default-members) and the seven-crate
    EDS-gated clippy both clean; `cargo test --locked` (1358 passed) and the
    seven-crate `cargo test` (1199 passed) both green, 0 failed.
    **NEEDS HUMAN VERIFICATION in real Evolution** — nothing headless can drive
    right-click → Delete against a live registry, so D1 is still not tagged
    complete.
- **D2 `[claude]` Calendar colour.** `Calendar.color` is parsed
  (`jmap-proto/src/calendars.rs:29`) then dropped — thread it Resource→Child and
  emit an ESourceSelectable `("Calendar","Color", …)` setting in
  `jmap-collection-sync/src/child_source.rs`; write-back rides on D1's
  `Calendar/set`.
  - **DONE 2026-08-19 (read path)** — `color: Option<String>` now flows
    `Calendar` → `Resource` → `Child` → a `("Calendar", "Color", …)`
    `Setting`, emitted only when the server named one (same
    omitted-vs-empty rule as `Port`/`User`/`Method`), and
    `jmap-backend-collection/src/child_source.rs::write` grew one match arm
    calling `e_source_selectable_set_color` — `ESourceCalendar` derives from
    `ESourceSelectable`, so the "Calendar" extension already fetched for
    `BackendName` answers it too, no second extension or allowlist change
    needed. TDD'd at both layers (`jmap-collection-sync`'s `Setting`-triple
    tests, `jmap-backend-collection/tests/child_source.rs`'s round trip
    through a real `ESource`). **Finding recorded in the test, not treated as
    a bug:** `ESourceSelectable:color` is not NULL-by-default — EDS's own
    GParamSpec defaults it to `#62a0ea` (GNOME's accent blue) — so a calendar
    the server named no color for reads back as that default, not as no
    color at all; leaving the setting unwritten (rather than writing an
    empty string) is what makes that the *only* thing overriding it.
    **Still open:** write-back (a local colour edit reaching the server)
    rides on D1's `Calendar/set`, whose EDS-side `create_resource_sync`
    wiring is not done yet — out of scope here, as the roadmap text already
    said.
  - **RESEARCHED, NOT CLAIMABLE YET (2026-08-19)** — now that
    `create_resource_sync` has landed, a session checked whether write-back
    was newly unblocked by reading `evolution-ews-3.52.4` for precedent
    (`e-ews-folder.c`). Finding: EWS has **no server round-trip for calendar
    colour at all** — it only ever locally assigns a colour from a fixed
    palette when a folder is first discovered, never reads one from the
    server, and never writes a local edit back. So there is no incumbent
    EDS/EWS pattern to mirror here, unlike `child_added`/`create_resource_sync`.
    Making a local colour edit reach the server needs a new design: detecting
    an `ESourceSelectable` `"notify::color"` signal on the child `ESource`
    (fired on whatever thread touches the source) and safely getting that
    across to a `Calendar/set` call made from `ECalMetaBackend`'s sync worker
    thread — genuine signal-lifecycle/concurrency reasoning, not a mechanical
    port. Needs that design before it is CLAIMABLE.

### Track E — Sharing + scheduling — Path A APPROVED (2026-08-19)
Design: `docs/PRINCIPALS-DESIGN.md` (commit 98c0576). Two premises the spike
corrected against datatracker, kept because they shape the work:
- **Free/busy slot-picking is NOT in RFC 9670.** `Principal/getAvailability`
  lives in the **JMAP-for-Calendars draft** (repo pins -27); RFC 9670 supplies the
  shared `Principal` vocabulary + capability URNs (`urn:ietf:params:jmap:principals`,
  `…:principals:owner`) that both availability and sharing build on.
- **Mail has permissions but not sharing.** `Mailbox` has `myRights` (RFC 8621)
  but **no standard `Mailbox.shareWith`** — a mailbox cannot be shared under
  current specs, only its rights read. Calendar/AddressBook `shareWith` are draft.

**MAINTAINER DECISION (2026-08-19): Path A is GREENLIT — "make the basics work."**
Build Phase 0, then Path A. Do NOT start Phase B or Phase C (recorded as future
work below) until Path A lands and the maintainer approves the next phase.

**CLAIMABLE NOW — Phase 0, then Path A (mock-first, TDD, headless).** Full detail
in design §4–§6; the ordered increments:
1. **Phase 0 — shared floor.** `jmap-proto`: new feature-gated `principals.rs`
   (`Principal` type; `capabilities` stays a `Value` bag so one unknown per-principal
   capability can't sink the response) + the two capability constants in
   `session.rs`. `jmap-client`: new `principals.rs` — `principals()` (Principal/get),
   `principal_query()` (Principal/query). `jmap-mock`: Principal/get|query handlers,
   advertise the two URNs, seed a couple of Principals. Pure-additive (design §4.1–4.3).
   - **DONE 2026-08-19** — landed exactly as scoped: `jmap-proto::principals`
     (feature `principals`, on by default alongside `mail`/`contacts`/
     `calendars`) with `Principal`/`PrincipalQueryFilter`, and
     `CAPABILITY_PRINCIPALS`/`CAPABILITY_PRINCIPALS_OWNER` in `session.rs`;
     `jmap-client::principals()`/`principal_query()`; `jmap-mock`'s
     `Principal/get`/`Principal/query` handlers, `AccountState::
     seed_principal`/`seed_current_user_principal`, and a session document
     that advertises both URNs (server-wide, and per-account with real
     content — `currentUserPrincipalId`, and `accountIdForPrincipal`/
     `principalId` once an account has an owning principal — unlike the
     other four account capabilities' uniform empty-object placeholder).
     TDD'd in `jmap-client/tests/principals.rs` (list, query-by-email hit
     and miss, and the session document itself). Full gate green: `cargo
     fmt --check`; `cargo clippy --all-targets --locked -- -D warnings`
     (default-members) and the seven-crate EDS-gated clippy both clean;
     `cargo test --locked` and the seven-crate `cargo test` both green.
     Path A (`getAvailability`, the mock computation, and the escalation-
     worthy free/busy vfunc) is next on this thread.
2. **Path A — availability.** `Principal/getAvailability` request/response +
   `BusyPeriod` in proto; `get_availability()` client method (using-set names BOTH
   principals AND calendars); mock computes `BusyPeriod`s from seeded CalendarEvents
   (deterministic slot tests). Then the one heavy piece: the `ECalBackend`
   `get_free_busy_sync` vfunc in `jmap-backend-cal` + a `BusyPeriod→VFREEBUSY`
   marshaller in `jmap-ical` (invoked from `jmap-cal-sync`), attendee→principal via
   `Principal/query` (design §4.2–4.4). That vfunc is **L / escalation-worthy**
   (unsafe FFI, EDS-only testing); everything above it is ordinary additive work.
   **Build and test against the mock — it is fully headless-testable that way.**
   - **PARTIAL 2026-08-19 (proto/client/mock slice DONE, vfunc still open)** —
     landed exactly the non-FFI half: `jmap-proto::principals` gained
     `GetAvailabilityRequest`/`GetAvailabilityResponse`/`BusyPeriod` (bespoke
     shapes per design §4.1, `principals` feature now pulls in `calendars` for
     `BusyPeriod.event: Option<CalendarEvent>`); `jmap-client::get_availability
     (account_id, principal_id, utc_start, utc_end, show_details)` naming both
     `CAPABILITY_PRINCIPALS` and `CAPABILITY_CALENDARS` in its `using` set;
     `jmap-mock`'s `Principal/getAvailability` handler computes `BusyPeriod`s
     from the account's seeded `CalendarEvent`s — skips `cancelled` events and
     ones explicitly marked `freeBusyStatus: "free"` (RFC 8984 §4.4.2 defaults
     unset to busy), returns `notFound` when the principal's per-principal
     `urn:ietf:params:jmap:calendars` capability says `mayGetAvailability:
     false` (absent the capability, allowed), and includes the full event only
     when `showDetails` is true (`eventProperties` projection not implemented —
     no test needs it yet). `busyStatus` is `tentative` when the source event's
     `status` is, else `confirmed`; the draft's third value, `unavailable`, has
     no source concept in this crate's `CalendarEvent` and is not produced.
     TDD'd: proto round-trip tests in `principals.rs`; three new
     `jmap-client/tests/principals.rs` cases (in-window busy periods sorted by
     start with an out-of-window and an explicitly-free event both excluded,
     `showDetails` including the event, and the denied-principal `notFound`);
     mock-side unit tests for the small duration-to-end helper. **Deliberate
     scope limits, both left open and documented in place, not silently
     dropped:** (a) the draft's other named error, `tooLarge` (window too
     wide) — no clean way to check window width without the calendar-date
     arithmetic `UtcDate`'s own doc says this crate avoids, and no test needs
     it yet; (b) computing a `BusyPeriod`'s end from the event's `duration`
     is a same-day, second-of-day-only helper (`jmap-mock/src/
     principals.rs::busy_end`) — no month/leap-year/midnight-crossing
     arithmetic, consistent with `UtcDate`'s "this crate never does date
     arithmetic" stance; a duration it can't parse or that would cross
     midnight falls back to a zero-length period rather than guessing. Full
     gate green: `cargo fmt --check`; `cargo clippy --all-targets --locked --
     -D warnings` (default-members) and the seven-crate EDS-gated clippy both
     clean; `cargo test --locked` and the seven-crate `cargo test` both green,
     every `test result: ok`, 0 failed. **Still open, and the last piece of
     Path A:** the `ECalBackend get_free_busy_sync` vfunc + `BusyPeriod→
     VFREEBUSY` marshaller — FFI, unsafe, EDS-only testing, escalation-worthy
     as already flagged, left for a session that wants to take that on
     deliberately (or an escalation to a stronger model).

**OPERATOR CONFIRMATION (you-task, like the OAuth Fastmail test).** The runner
cannot reach Stalwart (MAINTAINER DECISIONS #3), so the design's "half-day probe:
fire `Principal/getAvailability` at Stalwart and record the draft field spelling"
is an **operator** step, not a blocker on the mock-side build. Run it when
convenient to confirm the real field names match the proto/mock; if Stalwart does
not implement `getAvailability`, report it and we reorder to Phase B.

**FUTURE WORK — recorded for a future agent (do NOT start until Path A lands + maintainer OK):**
- **Phase B — per-source permissions.** Typed `myRights`/`shareWith` on
  Mailbox/AddressBook/Calendar (design §4.1) + rewire `children.rs`/`layout.rs` so
  per-source read-only derives from `myRights.mayWrite` — **narrows, never widens**;
  absent rights → today's account-wide fallback unchanged. Makes the known-wrong
  heuristic at `children.rs:93-97` correct. Effort S (types) + M (rewire), mostly
  published-RFC surface. Design §4.1, §4.4, §5–6.
- **Phase C — write-side sharing.** `shareWith` writes + `ShareNotification`
  (design §4.1 stub, §5). Deliberately LAST: least spec-stable (calendars draft,
  absent for mail), the only destructive surface, and needs a share-dialog UI that
  does not exist. Effort L. **Needs a fresh maintainer decision before starting.**

### Track F — Portability to newer Evolution/EDS (SPIKE FIRST — gated on "no spaghetti")
The **data/backend** side is already version-portable: `eds-sys/build.rs` probes
the installed headers and `eds-sys/src/compat.rs` selects `#[cfg]`-gated wrappers
(`eds_vcard_version_enum`, `camel_*`, …); M10's `eds-version-matrix` proves it
green on EDS 3.52 **and** 3.60. The **config/GUI** side is the untested axis.
Reassuringly, our setup UI uses the `EMailConfigServiceBackend`/
`EMailConfigServicePage` API (`jmap-config/src/backend.rs`), **not** the
`GtkUIManager` that Evolution ≥3.56 dropped — so that particular churn does not
touch us, and there is zero `GtkUIManager` usage in `jmap-config`/`evo-sys`.
**Spike (`[claude]`, deliverable `docs/NEWER-EVOLUTION-SPIKE.md`):** compile
`jmap-config` + `evo-sys` against the newer-Evolution/EDS container already used
by `ci/eds-matrix.sh`; characterize any `EMailConfig*` (or other shell/UI) API
drift 3.52 → 3.56/3.60; then judge honestly whether extending the existing
`compat.rs`-style build.rs-probed `#[cfg]` gating absorbs it cleanly or whether it
would sprawl. **Maintainer directive: recommend 3.52-only if it would be messy —
staying single-version beats carrying spaghetti.** Do NOT implement multi-version
support in the spike; produce the go/no-go + effort estimate and let the
maintainer decide. (If it turns out nothing drifts, the bonus finding is that we
may already build on newer Evolution unchanged.)

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
