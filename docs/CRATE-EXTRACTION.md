<!--
SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
SPDX-License-Identifier: GPL-3.0-or-later
-->

# Crate extraction assessment

**Date:** 2026-08-28

Which of this workspace's crates could be published to crates.io (or split
into their own repositories), what that would cost, and what stands in the
way.

## Method

- Enumerated all 23 workspace members from `rust/Cargo.toml` and
  `rust/crates/`.
- For each crate: read `Cargo.toml` (dependencies, crate-type, features,
  `publish` flag), the `lib.rs` module documentation, and skimmed the public
  API.
- Coupling classified by dependency edges: a dependency on `eds-sys` /
  `evo-sys` / `glib-sys` et al. means hard FFI coupling to Evolution Data
  Server and GObject; a crate whose dependency closure is pure Rust is
  mechanically extractable.
- Test counts are cheap grep counts of `#[test]` attributes plus `proptest!`
  blocks across `src/` and `tests/` — an indicator of maturity, not a
  precise number of test cases.
- Prior-art and name-collision checks were run against the live crates.io
  API on 2026-08-28 (`https://crates.io/api/v1/crates/<name>`).

Every crate currently carries `publish = false`; nothing here is published
today. The workspace shares one version (0.2.0), edition 2024, and
`rust-version = "1.97"`.

## Summary table

| Crate | Purpose | Coupling | Potential |
|---|---|---|---|
| jmap-proto (`evolution-jmap-proto`) | JMAP wire types: RFC 8620/8621/9610/9670 + calendars draft, serde only | Pure Rust, no siblings | **High** |
| jmap-client (`evolution-jmap-client`) | Blocking JMAP client: session, batching, blobs, OAuth discovery/PKCE, SRV seam | Pure Rust; jmap-proto | **High** (rename required) |
| jmap-mock (`evolution-jmap-mock`) | Stateful in-memory mock JMAP server + `jmap-mockd` binary | Pure Rust; jmap-proto | **Medium-High** |
| jmap-ical (`evolution-jmap-ical`) | JSCalendar (RFC 8984) ↔ iCalendar (RFC 5545) mapping | Pure Rust; jmap-proto, calcard | **Medium** |
| jmap-vcard (`evolution-jmap-vcard`) | JSContact (RFC 9553) ↔ vCard 3.0 mapping | Pure Rust; jmap-proto, calcard | **Medium** |
| po-compile (`evolution-jmap-po-compile`) | `.po` → `.mo` compiler (msgfmt subset), lib + bin | Pure Rust, no siblings | **Medium-Low** |
| eds-sys | Raw bindgen FFI bindings to the EDS backend libraries | FFI (is the coupling itself) | **Medium-Low** |
| jmap-book-sync | Address book sync logic shaped for `EBookMetaBackend` vfuncs | Pure Rust; client, proto, vcard | Low |
| jmap-cal-sync | Calendar sync logic shaped for `ECalMetaBackend` vfuncs | Pure Rust; client, proto, ical | Low |
| jmap-mail-sync | Mail sync logic shaped for Camel vfuncs | Pure Rust; client, proto | Low |
| jmap-collection-sync | One JMAP account's fanout into mail/book/cal sources | Pure Rust; client, proto | Low |
| jmap-backend-core | Shared GObject/EDS subclassing glue for all backends | eds-sys (hard FFI) | Low |
| evo-sys | Raw FFI bindings to the Evolution shell/setup headers | eds-sys (hard FFI) | Low |
| example-module | Evolution plugin template in Rust (LGPL-2.1, ex-Red Hat) | pkg-config/FFI | Low (template repo, not a crate) |
| jmap-backend-book | The EDS address book backend | eds-sys (hard FFI) | Not-worth |
| jmap-backend-cal | The EDS calendar backend | eds-sys (hard FFI) | Not-worth |
| jmap-backend-collection | The source-registry collection backend | eds-sys (hard FFI) | Not-worth |
| jmap-config | The Evolution account setup module | eds-sys + evo-sys (hard FFI) | Not-worth |
| jmap-mail | The Camel mail provider (`libcameljmap.so`) | eds-sys (hard FFI) | Not-worth |
| jmap-backend-book-module | cdylib entry-point shim (42 LOC) | FFI shim | Not-worth |
| jmap-backend-cal-module | cdylib entry-point shim (42 LOC) | FFI shim | Not-worth |
| jmap-backend-collection-module | cdylib entry-point shim (73 LOC) | FFI shim | Not-worth |
| jmap-config-module | cdylib entry-point shim (80 LOC) | FFI shim | Not-worth |

(jmap-functional is omitted from ranking: a test harness that drives real EDS
daemons against the mock, bound to this repository's build layout and CTest.
Not extractable by design.)

## Cross-cutting constraints

### License

The workspace is `GPL-3.0-or-later`, with a single copyright holder (Tobias
Mueller) on every file except `example-module` (which carries Red Hat
copyright and is `LGPL-2.1-or-later`). GPL-3 is an unusual and adoption-
hostile license for a library crate: any dependent becomes GPL-3-compatible
or cannot ship. The Rust ecosystem's norm is `MIT OR Apache-2.0`; for
GNOME-adjacent libraries `LGPL-2.1-or-later` is the conventional choice
(EDS itself is LGPL). Because Tobias is the sole copyright holder of every
extraction candidate, relicensing is legally straightforward — but it is a
maintainer decision, not a technical one, and it should be made per crate
before first publish (relicensing after external contributions arrive is
much harder). `example-module` cannot be unilaterally relicensed (Red Hat
copyright), but it is LGPL already.

### Naming

The pure-Rust crates already carry crates.io-safe package names
(`evolution-jmap-proto`, `evolution-jmap-client`, …, all currently free),
with short internal `lib` names. That prefix is honest for in-tree publishing
but poor branding for general-purpose crates: `evolution-jmap-proto` says
"Evolution-specific" about a crate that is pure RFC wire types. Collisions
found on crates.io (2026-08-28):

- **`jmap-client` is taken** by Stalwart (v0.4.2, ~72k downloads, actively
  maintained — updated 2026-06). A rename is mandatory. `jmap-blocking` is
  free.
- `jmap` (v0.0.5, parser/generator) and `jmap-tools` (Stalwart, JSON
  pointer/patch) exist; `jmap-proto`, `jmap-mock`, `jmap-vcard`,
  `jmap-ical`, `eds-sys`, `po-compile`, `evolution-data-server-sys`,
  `evolution-jmap` are all **free**. Caveat on `jmap-proto`: Stalwart's
  mail-server monorepo contains an *unpublished* internal crate of the same
  name; taking the name on crates.io is legitimate but invites confusion.
- `jscontact` (RFC 9553, v0.2.1) and `jscalendar` (RFC 8984, v0.1.0) exist
  as data-model crates — prior art for the object models, not for the
  vCard/iCalendar mapping.

### Publish order and dev-dependency cycles

The dependency DAG forces the order: proto → mock → client → vcard/ical
(→ sync crates, if ever). Dev-dependency back-edges (client dev-depends on
mock; vcard dev-depends on client and mock; proto ← mock is a real edge) are
not blockers — cargo strips path-only dev-dependencies at publish — but the
published tarballs' tests then don't run standalone unless mock is published
first with a version requirement.

### Versioning and MSRV

All crates share workspace version 0.2.0. Publishing means deciding between
lockstep versioning (simple, churn-heavy) and per-crate versions (the norm).
Edition 2024 + `rust-version = 1.97` is aggressive for a library; adopters
on distro toolchains will notice.

---

## Per-crate assessment

### jmap-proto (`evolution-jmap-proto`) — High

- **Purpose:** Pure serde data types for the JMAP wire format: RFC 8620
  core (session, request/response envelopes, `Id`/`State`, error taxonomy),
  RFC 8621 mail, RFC 9610 contacts, the JMAP Calendars draft, RFC 9670
  principals — each behind a feature flag. No I/O.
- **Coupling:** None. Only serde/serde_json. The cleanest crate in the tree.
- **Maturity:** ~4,000 LOC, 68 test attributes including a
  `malicious_input.rs` suite and proptest; ~680 doc-comment lines; good
  module-level docs with RFC links. Feature-gated surface is
  semver-conscious. The calendars module tracks an IETF *draft*, which is a
  standing 0.x semver hazard until the RFC lands.
- **Uniqueness:** No published general-purpose JMAP type crate covers
  RFC 9610 contacts + calendars draft + RFC 9670 principals. `jmap` (0.0.5)
  is stale; `jmap-tools` is JSON plumbing, not typed objects; Stalwart's
  types live unpublished inside their server monorepo.
- **Extraction cost:** Low. Rename decision (`jmap-proto` free but shadows
  Stalwart's internal name; `evolution-jmap-proto` free and unambiguous but
  misbrands), license decision, and a documented stance on the draft-based
  `calendars` feature. No internal-API leakage inward; everything else in
  the workspace leaks *its* types outward, which is normal for a proto crate.
- **Verdict: High.** Lowest cost, clearest gap in the ecosystem, and the
  anchor every other candidate depends on — it has to go first regardless.

### jmap-client (`evolution-jmap-client`) — High, rename required

- **Purpose:** Blocking JMAP client: session discovery
  (`/.well-known/jmap`), method batching, blob up/download, plus OAuth 2.0
  endpoint discovery (RFC 8414), dynamic client registration (RFC 7591),
  PKCE token exchange (RFC 7636), and an RFC 8620 §2.2 SRV-autodiscovery
  seam (`Resolver`, no DNS dependency imposed).
- **Coupling:** Pure Rust. Depends on jmap-proto; HTTP is behind a
  `Transport` trait (default `ureq`, feature-gated) precisely so an embedder
  can substitute libsoup — the seam is already designed for reuse.
  `CancelFlag` maps cleanly onto `GCancellable` without importing it.
- **Maturity:** ~10,300 LOC, 204 test attributes across 21 integration-test
  files (protocol edges, redirects, size limits, cancellation, OAuth,
  proptest on the untrusted-server boundary); ~870 doc lines. Tested against
  both the in-tree mock and (opt-in, `live-server` feature) real servers.
- **Uniqueness:** Stalwart's `jmap-client` 0.4.2 is the incumbent (async +
  blocking features, websockets, actively maintained). Differentiators here:
  tiny synchronous dependency tree (no tokio), pluggable transport,
  built-in OAuth discovery/registration/PKCE, and — as of Stalwart 0.4.x —
  coverage of the contacts RFC 9610 / calendars-draft / principals surface
  their client does not target. Real, but it competes with 72k downloads.
- **Extraction cost:** Medium. Mandatory rename (`jmap-blocking` free;
  keeping `evolution-jmap-client` also works). Public API exposes jmap-proto
  types throughout (fine once proto is published). `rebase_urls_from_env`
  is a test-rig affordance in the public API worth a second look before
  freezing semver.
- **Verdict: High.** Publishable and genuinely useful, provided the
  positioning ("blocking, embeddable, OAuth-capable, contacts+calendars")
  is stated against the incumbent rather than pretending it isn't there.

### jmap-mock (`evolution-jmap-mock`) — Medium-High

- **Purpose:** Stateful in-memory mock JMAP server (session, Basic/Bearer
  auth, mail read/send/import, contacts CRUD, calendar CRUD, inspectable
  state handle) plus a standalone `jmap-mockd` binary.
- **Coupling:** Pure Rust: jmap-proto, tiny_http, serde. No EDS anywhere.
- **Maturity:** ~5,200 LOC, ~730 doc lines. Only 28 in-crate test
  attributes, but its real test suite is every other crate in the workspace
  exercising it — it is battle-tested indirectly, which a standalone
  publisher cannot see. Honest about its limits in the docs ("not a real
  server").
- **Uniqueness:** Nothing comparable on crates.io: no published mock/test
  JMAP server exists. Anyone building a JMAP client (in any language — the
  `jmap-mockd` binary is language-neutral) currently has to stand up
  Stalwart or Cyrus for tests.
- **Extraction cost:** Low-Medium. `jmap-mock` is free. The API surface
  (`ServerState`, `Store`, `Change`) is test-oriented and can be published
  as such with weak semver promises. Needs a README stating fidelity limits.
  Must publish after jmap-proto.
- **Verdict: Medium-High.** Small cost, unique niche, and the strongest
  "gift to the ecosystem" candidate — mocks get adopted because nobody
  wants to write one.

### jmap-ical (`evolution-jmap-ical`) — Medium

- **Purpose:** Semantic JSCalendar (RFC 8984) ↔ iCalendar (RFC 5545)
  mapping — events, recurrence rules and overrides, alerts, locations,
  time-zone resolution including a Windows→IANA table, `prune_time_zones`,
  and VFREEBUSY rendering of busy periods.
- **Coupling:** Pure Rust: calcard (text layer, `default-features = false`),
  jmap-proto (`CalendarEvent`, `RecurrenceRule`, `BusyPeriod` leak into the
  public API), serde_json.
- **Maturity:** The second-largest crate: ~19,900 LOC, 351 test attributes
  (fixtures, hostile-input suite, proptest fuzz), ~1,800 doc lines, plus
  `docs/ICAL-MAPPING.md` documenting the mapping decisions.
- **Uniqueness:** Complicated. calcard's own default `jmap` feature does
  vCard↔JSContact and iCal↔JSCalendar *conversion* — this workspace
  deliberately disables it and owns the mapping, because the target here is
  what EDS/libical actually round-trips (vCard 3.0 dialect, libical zone
  handling, Evolution semantics), not a generic format conversion. As a
  general-purpose converter it duplicates calcard; as "the mapping that
  survives libical and ECalMetaBackend" it is unique but niche.
- **Extraction cost:** Medium. Names free. Public API leaks jmap-proto
  types (publish-order dependency). The value proposition needs honest
  framing against calcard to avoid looking like NIH.
- **Verdict: Medium.** Excellent engineering and test depth, but the
  ecosystem gap is partly filled by its own dependency. Best published as
  the EDS-dialect companion to calcard, or offered upstream to calcard as
  fixes/test corpus.

### jmap-vcard (`evolution-jmap-vcard`) — Medium

- **Purpose:** JSContact (RFC 9553) ↔ vCard **3.0** mapping for the
  EContact property set: N/FN/ADR/TEL/EMAIL/ORG/PHOTO/CATEGORIES plus the
  `X-` lines EDS keeps IM handles, spouse, manager, assistant on.
- **Coupling:** Pure Rust: calcard (no default features), jmap-proto
  (contacts types in the public API), base64.
- **Maturity:** ~23,500 LOC, 333 test attributes (fixtures, hostile suite,
  proptest fuzz, and a `server_roundtrip.rs` against the mock), ~750 doc
  lines, plus `docs/VCARD-MAPPING.md`.
- **Uniqueness:** Same calcard caveat as jmap-ical — calcard converts
  vCard↔JSContact itself, but targets vCard 4.0 semantics; the vCard *3.0*
  + EDS `X-` dialect mapping here exists nowhere else. `jscontact` (0.2.1)
  is a data model only. Rare, narrow.
- **Extraction cost:** Medium. `jmap-vcard` free (though the name promises
  more generality than "EDS vCard 3.0 dialect" delivers — `jscontact-vcard3`
  would be more honest). Public API is wide (many `states_*` mapping
  functions re-exported) and would want curation before a semver promise.
- **Verdict: Medium.** Same shape as jmap-ical: publish as a pair, framed
  as the vCard-3.0/EDS-dialect layer, or contribute the corpus upstream.

### po-compile (`evolution-jmap-po-compile`) — Medium-Low

- **Purpose:** A `.po` → `.mo` compiler (msgfmt subset) as lib + bin, built
  so CI needs no gettext tools; refuses rather than drops anything outside
  its subset (msgctxt, plurals, unknown escapes, non-UTF-8), with
  line-numbered diagnostics.
- **Coupling:** None. Zero dependencies. 857 LOC, 18 test attributes.
- **Uniqueness:** `polib` (1.7M downloads) reads/manipulates/stores PO but
  does not compile MO; `gettext` (1M downloads) is a runtime reader;
  `msgfmt` and `mo-pack` are free names. A dependency-free build-time
  po→mo compiler is a real, if small, gap — every Rust project shipping
  gettext catalogues has this problem in build.rs.
- **Extraction cost:** Low. Rename to `msgfmt` or keep `po-compile` (both
  free). The deliberate refusal semantics (no plurals, no msgctxt) must be
  front and center or users will file them as bugs; accepting plural support
  later is scope growth the maintainer may not want.
- **Verdict: Medium-Low.** Trivial to publish, useful to a small audience;
  the main cost is becoming the maintainer of other people's catalogues'
  expectations.

### eds-sys — Medium-Low

- **Purpose:** Raw bindgen FFI bindings to the Evolution Data Server backend
  libraries (libebook, libecal, libebackend…), re-exporting gtk-rs `*-sys`
  types so GLib values are the same Rust types everywhere.
- **Coupling:** It *is* the FFI boundary: build.rs probes pkg-config,
  generates bindings against installed headers at build time, `links =
  "evolution-data-server"`.
- **Maturity:** ~10,000 LOC (mostly generated/test), 138 test attributes
  (including layout/ABI assertions). No hand-written docs to speak of.
- **Uniqueness:** No EDS bindings exist on crates.io at all (`eds-sys` and
  `evolution-data-server-sys` both free; `libical-sys` shows appetite for
  this family). Anyone wanting EDS from Rust today starts from zero.
- **Extraction cost:** High relative to size. Bindings are generated at
  build time against whatever EDS is installed — the supported version
  matrix (see `docs/eds-version-matrix.md`) becomes a public contract;
  `-sys` convention expects the crate license to be permissive/matching the
  wrapped library (EDS is LGPL-2.1 — a GPL-3 `-sys` crate is a red flag);
  and the binding surface here is curated for this project's backends, not
  a complete EDS API.
- **Verdict: Medium-Low.** The only candidate that could seed a wider
  "EDS from Rust" ecosystem, but publishing it means committing to a
  version matrix and an LGPL relicense; only worth it if that ecosystem is
  a goal in itself.

### jmap-book-sync / jmap-cal-sync / jmap-mail-sync / jmap-collection-sync — Low

- **Purpose:** The pure-Rust halves of each backend: sync logic shaped
  1:1 around `EBookMetaBackend` / `ECalMetaBackend` / Camel vfuncs, and the
  collection backend's account fanout (session-document capability layout →
  child sources).
- **Coupling:** Pure Rust (client, proto, vcard/ical) — mechanically
  extractable, deliberately GObject-free.
- **Maturity:** Substantial (3,700–11,400 LOC each, 85–269 test attributes,
  good docs).
- **Uniqueness/cost:** The APIs are named after and contractually bound to
  EDS vfunc semantics (`list_existing_sync`, Camel folder-info trees,
  `dup_resource_id`). Outside an EDS backend they answer questions nobody
  asks. Publishing them would freeze this project's internal seam as public
  semver for near-zero adoption.
- **Verdict: Low.** Extractable but not worth it; their value is exactly
  that they are this project's testable core. Revisit only if another
  EDS-backend-in-Rust project materializes (in which case jmap-backend-core
  and eds-sys move up too).

### jmap-backend-core — Low

- **Purpose:** Shared GObject/EDS subclassing glue (instance layout,
  vfunc marshalling, cancel bridging, journald logging setup) for the five
  FFI crates.
- **Coupling:** Hard: eds-sys, gobject-sys, plus — awkwardly for reuse —
  jmap-client and jmap-proto.
- **Verdict: Low.** A general "write EDS backends in Rust" glue crate is a
  genuinely interesting idea, but this one would first need its JMAP
  dependencies factored out and eds-sys published. Only meaningful as part
  of a deliberate eds-rs ecosystem play.

### evo-sys, example-module — Low

- `evo-sys`: bindings to the Evolution shell headers the setup module
  extends; tiny audience (Evolution *application* plugins in Rust), no docs,
  depends on eds-sys. Low.
- `example-module`: an LGPL-2.1 Evolution plugin template (ex-Red Hat
  scaffold). Its natural form is a template *repository*, not a crates.io
  library; cannot be relicensed unilaterally. Low (but cheap to split as a
  repo if desired).

### The products: jmap-backend-book/cal/collection, jmap-config, jmap-mail, and the four `*-module` shims — Not-worth

These are the deliverable: rlib backend bodies plus 42–80-LOC cdylib shims
that EDS/Evolution `dlopen`s (`libebookbackendjmap.so`, `libcameljmap.so`,
`module-jmap-configuration.so`, …). They are built by CMake against
installed Evolution headers, are meaningless as library dependencies, and
`publish = false` is simply correct for them. jmap-functional likewise: a
CTest-driven harness for real EDS daemons, bound to this build tree.

---

## Ranked shortlist

1. **jmap-proto** — publish first; anchor for everything else; only real
   ecosystem gap with near-zero extraction cost. Decide name
   (`jmap-proto` vs keeping `evolution-jmap-proto`) and the calendars-draft
   semver stance.
2. **jmap-mock** — unique on crates.io, small, language-neutral via
   `jmap-mockd`; publish right after proto.
3. **jmap-client** — real differentiators (blocking, pluggable transport,
   OAuth discovery/PKCE, contacts+calendars coverage) but a strong,
   actively-maintained incumbent owns the name; rename (e.g.
   `jmap-blocking`) and position honestly.
4. **jmap-ical + jmap-vcard** (as a pair) — deep, well-tested, rare
   mappings, but partially shadowed by calcard's own `jmap` feature;
   publish framed as the vCard-3.0/EDS-dialect layer, or upstream the
   corpus to calcard instead.
5. **po-compile** — trivial cost, small audience, dependency-free; publish
   if the maintenance appetite exists.
6. **eds-sys** (stretch) — only if "EDS from Rust" is a goal in itself;
   requires an LGPL relicense and a public version-matrix commitment.

Everything else: keep in-tree, `publish = false`.

## jmap-tools adoption spike (roadmap item 58)

Item 58 asked whether Stalwart's `jmap-tools` (crates.io 0.1.8, Apache-2.0
OR MIT) should replace the hand-rolled JSON-pointer/PatchObject/
resultReference code this workspace carries in several places:
`jmap-mock/src/dispatch.rs` (`eval_pointer`, RFC 8620 section 3.7 pointer
evaluation with the `*` wildcard), `jmap-mock/src/patch.rs` (`apply_patch`,
RFC 8620 section 5.3 PatchObject application), and three independent
RFC 6901 escape helpers (`jmap-mail-sync/src/pointer.rs::member`,
`jmap-cal-sync/src/patch.rs::escaped`/`member_of`,
`jmap-book-sync/src/patch.rs::escape`). No production code was changed;
this section is the recorded verdict.

**Verdict: do not adopt.** Verified against the real crate (cloned
`github.com/stalwartlabs/jmap-tools`, built a scratch crate pulling
`jmap-tools = "0.1.8"` and exercising its actual `JsonPointerHandler` impl,
not just its rustdoc, which is only 12% populated):

- `jmap_tools::Value<P, E>` is its own generic type, parameterised over
  `Property`/`Element` traits, not `serde_json::Value`. This workspace has
  no `preserve_order` feature enabled and passes plain `serde_json::Value`
  pervasively (`jmap-proto`'s wire types, `jmap-mock`'s `Invocation`, every
  `PatchObject` as `serde_json::Map`). Wiring `jmap-tools` in anywhere means
  a conversion layer at every boundary, not a drop-in swap.
- `patch_jptr` (the crate's patch primitive) does not implement RFC 8620
  section 5.3 semantics: a spike test set `/subject` to `Value::Null` and
  the key stayed in the object holding a null, where a `PatchObject` must
  remove it. A second test confirmed it also refuses to create a missing
  intermediate object (`/keywords/$seen` against `{"subject": "x"}` returns
  `false`) where `apply_patch` auto-vivifies on purpose. Both are exactly
  the behaviour our 34-line `apply_patch` exists to get right, so adopting
  `patch_jptr` would mean wrapping it with the same null-check and
  auto-vivify logic we already have, for no net simplification.
- Pointer *evaluation* (`eval_jptr`) is semantically equivalent for the
  wildcard case, RFC 8620 section 3.7 flattening included: a fixture
  matching `jmap-mock`'s own `pointer_object_and_array` test round-tripped
  correctly through the real crate. This part could be adopted, but our
  `eval_pointer` is 15 lines with its own passing test, and adopting it
  still needs the `serde_json::Value <-> jmap_tools::Value` conversion
  above for a 15-line function; not worth the new dependency.
- The `extra` catch-all-map question (item 58 point 2): `ObjectAsVec` is
  `Vec<(Key, Value)>` (`json/object_vec.rs:25-26`), so it is
  order-preserving by construction the same way serde_json's `preserve_order`
  (backed by `indexmap`) is, but with linear-scan lookup rather than a hash
  index. For the ~256 `extra: serde_json::Value` edges in `jmap-proto`, at
  typical JMAP object sizes that is a fine tradeoff either way; it is not a
  reason on its own to add the dependency, and simply turning on
  serde_json's own `preserve_order` feature gets the same ordering property
  without a new type system.
- One genuine reuse opportunity found along the way:
  `jmap_tools::JsonPointer::encode` (`pointer/mod.rs:63-83`) is a pure
  `&str`-escaping utility with the identical RFC 6901 semantics as the three
  duplicated `.replace('~', "~0").replace('/', "~1")` helpers above. It
  would deduplicate them without touching the `Value`/pointer-eval
  machinery. Not taken here either: pulling in a 2,941-line crate (plus its
  optional `rkyv` machinery) for one string function is worse on the
  "clarity" axis item 58 itself asks to weigh, against three
  well-documented, well-tested, two-line functions that already explain in
  their own doc comments why the escaping is centralised per-crate. If a
  fourth copy of this helper is ever needed, extracting a shared
  zero-dependency helper inside this workspace is the better fix.

No cargo-deny entry, no new dependency, and no code changed as a result of
this spike; the existing hand-rolled implementations and their fixpoint
tests stand.

## Maintainer decisions needed

1. **License** (blocking for all candidates): stay GPL-3.0-or-later and
   accept near-zero library adoption, or relicense the shortlist crates —
   `MIT OR Apache-2.0` for maximum reach, `LGPL-2.1-or-later` to match the
   GNOME neighbourhood (and effectively required for eds-sys). Sole
   copyright holder Tobias Mueller makes this possible today; it gets
   harder after the first outside contribution.
2. **Naming**: adopt-friendly short names (`jmap-proto`, `jmap-mock`,
   `jmap-blocking`, `jmap-ical`, `jmap-vcard` — all free as of 2026-08-28)
   vs keeping the unambiguous but Evolution-branded `evolution-jmap-*`
   package names. `jmap-client` is not available either way.
3. **Repo split vs in-tree publish**: cargo publishes fine from this
   workspace (in-tree publish keeps the mock-driven test topology and one
   CI), at the cost of issue-tracker mixing and GPL headers sitting next to
   relicensed crates. A split repo (`jmap-rs`?) is cleaner branding for
   proto/client/mock but forks the test infrastructure. Recommendation
   implicit in the tree's design: publish in-tree first, split only if
   external contributors actually arrive.
4. **Versioning policy**: lockstep workspace 0.2.0 vs per-crate semver once
   published; plus whether edition 2024 / MSRV 1.97 is worth relaxing for
   adopters on distro toolchains.
5. **calcard relationship**: compete (publish jmap-ical/jmap-vcard) or
   contribute (upstream the hostile-input corpus and mapping fixes to
   Stalwart) — doing both needs a stated story, since this workspace
   depends on calcard while disabling the feature these crates replace.
