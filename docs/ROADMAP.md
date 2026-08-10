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
- **Signal milestone completion.** When a milestone's acceptance
  criteria are fully met (or a standing directive is fully carried out),
  append one line to `docs/MILESTONES.md` and commit it with the work:
  `<TAG> COMPLETE <short-sha> <ISO-date>` — e.g. `M5 COMPLETE a1b2c3d
  2026-08-10`, or `CALCARD COMPLETE …` for the calcard directive. This
  file is a machine-readable trigger (the re-audit watcher watches it);
  write a tag only when you would defend the milestone as genuinely
  done, and never remove or edit prior lines.
