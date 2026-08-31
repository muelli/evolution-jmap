# Newer-Evolution/EDS portability spike — the config/GUI axis (Track F)

Track F's premise: M10's `eds-version-matrix` job already
proves the **data/backend** crates (`eds-sys`, `evo-sys`, `jmap-backend-*`)
green on EDS 3.52 and 3.60, but flagged the **config/GUI** crate
(`jmap-config`, `EMailConfigServiceBackend`/`EMailConfigServicePage`) as the
untested axis, since it is the one directly exposed to Evolution's shell API
rather than only to EDS's backend API.

## What this spike found: the premise was already answered

`jmap-config` was not actually untested. `ci/eds-matrix.sh` — the exact
script `eds-version-matrix` runs — already lists `-p jmap-config`, and
`docs/eds-version-matrix.md`'s 2026-08-17 measurement already reports it
green. `rust/crates/jmap-config/src/backend.rs` (the `EMailConfigService*`
code Track F worried about) was added 2026-08-10, a week before that
measurement, so it was included, not added after. This spike is therefore a
**re-confirmation with a narrower focus**, not a first look: rerun the exact
container, isolate `jmap-config`+`evo-sys`+`eds-sys` (drop the mail/book/cal/
collection crates the matrix doc had already separately closed) and read the
result specifically for `EMailConfig*`/shell-API drift.

**Method.** Reproduced `docs/eds-version-matrix.md`'s pinned container locally
(`docker.io/library/fedora@sha256:6c75d5bf57cb0fa5aa4b92c6a83c86c791644496d9ac230de7711f5b8ec3b898`,
the exact digest `ci.yml`'s `eds-version-matrix` job uses), installed the same
package set `ci.yml` installs (`evolution-data-server-devel evolution-devel
pkgconf-pkg-config gcc clang-devel`), copied a clean checkout of `master`
(`3912476`) in (not the read-only mount, so `target/` writes don't touch the
host), and ran `cargo build --locked -p eds-sys -p evo-sys -p jmap-config`
followed by `cargo test --locked` on the same three crates.

**The digest still resolves to the same version the matrix doc measured**:
`evolution-3.60.2-1.fc44` / `evolution-data-server-3.60.2-1.fc44` (Fedora 44,
`pkg-config --modversion` agrees across all four `.pc` files). No version
drift between this run and the 2026-08-17 measurement to account for.

**Result: clean build, clean test, on both counts.**
- `cargo build --locked -p eds-sys -p evo-sys -p jmap-config`: exit 0.
  `jmap-config` itself emits **zero** warnings. The only warnings anywhere in
  the build are `eds-sys`'s 5 known `unnecessary_transmute` lints in
  bindgen's own generated code for glibc's `_IO_FILE` bitfield accessors —
  already recorded in `docs/eds-version-matrix.md` (C) as a bindgen/glibc
  artefact unrelated to this repository, not a new finding.
- `cargo test --locked -p eds-sys -p evo-sys -p jmap-config`: **290 tests, 0
  failed**, across every test binary in the three crates, including the ones
  that answer the Track F question directly:
  - `evo-sys/tests/layout.rs`'s `service_backend_layout_matches_the_gtype_system`
    and `the_page_changed_entry_point_resolves_and_its_handle_carries_no_layout`
    — the ABI cross-checks for exactly `EMailConfigServiceBackend`/
    `EMailConfigServicePage`, the two shell types Track F named — pass on
    3.60.2 unchanged.
  - `eds-sys/tests/layout.rs`'s `pkg_config_describes_the_headers_it_pointed_at`
    and `the_running_eds_can_serve_what_these_bindings_were_compiled_against`
    (the pkg-config/headers/runtime three-way cross-check from
    `docs/eds-versions.md`) both pass.
  - All of `jmap-config`'s own suites (`module.rs`, `oauth2*.rs`,
    `textdomain.rs`, plus its unit tests) pass unmodified — no `#[cfg]`, no
    conditional compilation, nothing gated on EDS/Evolution version anywhere
    in the crate today, and nothing here needed one.
- Confirmed the `rpm -q` versions directly rather than trusting the doc's
  prior measurement: `evolution-3.60.2-1.fc44.x86_64`,
  `evolution-data-server-3.60.2-1.fc44.x86_64`.

No `EMailConfig*` symbol was renamed, no signature changed, no struct moved.
`GtkUIManager` — the one shell-side removal `docs/eds-versions.md` and Track F
both flag for Evolution ≥3.56 — has zero usage in `jmap-config`/`evo-sys` to
begin with (confirmed by grep, and implicitly by the clean build/test above),
so that removal cannot bite here regardless.

## Go/no-go

**No-op — nothing to gate.** There is no drift to absorb into
`compat.rs`-style `#[cfg]`s because nothing on the config/GUI axis has
drifted between 3.52 and 3.60.2. Per the roadmap's own instruction
("recommend 3.52-only if it would be messy — staying single-version beats
carrying spaghetti"): the inverse case applies instead — multi-version
support costs nothing extra here, because the single code path already
covers both, unmodified. There is no effort estimate to give because there
is no work item this spike surfaces.

**What this does and does not prove.** This is a `cargo build`/`cargo test`
result, not a live-Evolution one: the actual `EMailConfigServicePage` GTK
widget still cannot be constructed on this (or any CI) runner without a
display connection — the same limitation `docs/eds-version-matrix.md` and
every M7 night-log entry already note, orthogonal to EDS version. Whether the
setup UI *behaves* identically inside a real Evolution 3.60 session (as
opposed to compiling and passing its headless unit/layout tests) is still
something only a human in a VM with that Evolution installed can confirm —
same as it already is for 3.52. Recorded as a fact, not a new blocker: no
such VM exists in this environment, so it is out of scope for this spike as
it is for every other headless session.

## Bottom line for the maintainer

Track F's stated concern (config/GUI drift on newer Evolution/EDS) does not
exist as of Evolution/EDS 3.60.2 — confirmed directly, not inferred from the
data/backend crates' prior green run. No follow-up work is queued from this
spike; it closes Track F rather than opening new tasks under it.
