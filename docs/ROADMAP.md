# Roadmap

Goal: a **secure, easy to use, natively integrated** way to use JMAP from
GNOME Evolution — mail, contacts, and calendars — structured like
evolution-ews, written in Rust, developed test-first against the in-repo
mock server (`jmap-mockd`), and shipped as installable artifacts.

Round 1 (done): protocol crate, blocking client, stateful mock server,
42-test TDD suite, dual CI with reproducible builds and provenance. See
README.

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

## Integration testing (parallel track)
Once M3 exists: gated CI job + local recipe against a real
[Stalwart](https://stalw.art/) server (full JMAP mail/contacts/calendars)
— `infra/gcp/create-stalwart.sh` provisions one. The mock stays the
default test target.

## Rules for autonomous work sessions

- Work only inside this repository; never force-push; never rewrite
  history; do not modify `infra/` or `.github/workflows/ci-image.yml`
  unless a milestone requires it.
- TDD: red test first (against fixtures or `jmap-mockd`), then green.
  `cargo test` and `cargo clippy --all-targets -- -D warnings` must pass
  before every push. Crates needing EDS headers stay out of
  `default-members`.
- Every source file: SPDX header, `GPL-3.0-or-later` (`reuse lint` must
  stay green). Commits: small, imperative subject, author
  `Tobias Mueller <muelli@cryptobitch.de>`, **no Co-Authored-By
  trailers**.
- Push after each green increment (deploy key is configured).
- Keep a running log in `docs/NIGHT-LOG.md`: what was done, decisions
  taken, blockers hit. If blocked on a milestone, log it and take the
  next tractable item instead of spinning.
