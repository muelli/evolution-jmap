# Packaging evolution-jmap for a distro

This document is for a prospective distro packager (Debian, Fedora, Ubuntu,
whoever picks this up first), not for this project's own contributors.
Nobody upstream is planning an ITP or an upload — see "Why no ITP" below —
but a lot of the groundwork a packager needs is already here. This is a
factual map of what exists, what it proves, and what it deliberately does
not solve.

## Why no ITP

The maintainer is deliberately not filing a Debian ITP or uploading this
package. That is not an oversight to work around; it is a considered
decision (`docs/ROADMAP.md`, Track C4) to leave "who packages this" to
whoever wants to, rather than have upstream become the Debian maintainer.
Everything below exists to make that person's first hour easier, not to
pretend the packaging work is finished.

## What already works: the lintian-clean CPack `.deb`

The canonical, tested package build is CPack, driven from
`cmake/Packaging.cmake`, not `debian/rules` (see below for that). It is
exercised by CTest:

```bash
cmake -S . -B build -G Ninja
ninja -C build
ctest --test-dir build -R package-deb
```

That runs three legs:
- `package-deb` — builds the `.deb` and asserts its contents/control
  fields (`cmake/tests/check-deb-package.cmake`).
- `package-deb-reproducible` — builds it twice from different build-tree
  paths and diffs the result byte-for-byte
  (`cmake/tests/check-deb-reproducible.cmake`) — the same
  `--remap-path-prefix`/`SOURCE_DATE_EPOCH`/pinned-toolchain story the
  mock-server reproducibility job already uses (see "CI, reproducibility,
  transparency" in the top-level README).
- `package-deb-lintian` — runs `lintian` against the built `.deb`
  (`cmake/tests/check-deb-lintian.cmake`) and fails if it is not clean.
  The one deliberate exception is a scoped, commented override in
  `docs/packaging/lintian-overrides` (installed to
  `/usr/share/lintian/overrides/evolution-jmap`) for a single justified
  RUNPATH entry: `module-jmap-configuration.so` needs
  `/usr/lib/evolution` (Evolution's own private libdir) at load time to
  resolve `libevolution-mail.so`/`libevolution-util.so` — read
  `docs/ROADMAP.md`'s CURRENT PRIORITY item 8 for the full story if you
  need to know why that RUNPATH is there rather than removed.

If you are packaging a Rust GNOME module for the first time, this is the
thing to imitate structurally even if your target distro's tooling differs
from CPack — it is the build this project's own CI keeps green on every
push, so it will not silently rot out from under you the way a
hand-maintained-and-never-run packaging recipe would.

The `.deb` installs five dlopened modules (address book backend, calendar
backend, Camel mail provider, the collection backend that fans one account
out into all three, and the account-setup module) plus their supporting
files. All five are built against, and must be installed alongside, one
specific EDS/Evolution version — the shared libraries are dlopened by
`evolution-data-server` and by Evolution itself with no ABI-compatibility
promise across versions, so a package split that lets the module and its
matching Evolution drift apart is a bug, not a packaging convenience.

## The `debian/` skeleton (Track C3): a starting point, not upload-ready

`debian/` in this repository is a working `dh` skeleton — `control`,
`rules` (using `dh` over the existing CMake/Ninja build), `watch`,
`source/format`, `README.source` — verified end to end with
`dpkg-buildpackage -us -uc -b -d` on this project's own CI dependency set
plus `debhelper`. The resulting `.deb` is lintian-clean to the same
standard as the CPack one. Concretely, it gives you:

- `debian/control`: `Build-Depends` mirroring `ci/install-deps.sh`'s
  package list plus `debhelper-compat (= 13)`/`cargo`/`rustc`; runtime
  `Depends` on `evolution-data-server (>= 3.52)` and `evolution (>= 3.52)`.
- `debian/rules`: `dh` with an `override_dh_auto_install` that loops
  `cmake --install --component` over the same five components the CPack
  path names, so the in-tree demo module (`src/`) is excluded the same way.
  `override_dh_shlibdeps` passes Evolution's private libdir
  (`pkg-config --variable=privlibdir evolution-shell-3.0`) to
  `dpkg-shlibdeps`, the same value `cmake/Packaging.cmake` already passes
  to CPack — without it, `dh_shlibdeps` cannot find the private
  `libevolution-*.so`s the modules link against.
- `debian/changelog` and `debian/copyright` are symlinks to
  `docs/packaging/{changelog,copyright}` rather than second copies — see
  below for why those are already the right format and already kept
  current.
- `debian/watch`: the usual GitHub-tags convention (`vX.Y.Z`, matching the
  tags already pushed). **Unverified against the live GitHub tags page** —
  `uscan`/`devscripts` are not installed anywhere this has been built, so
  treat it as a reasonable first draft, not a confirmed-working watch file.
- `dh_auto_test` is a deliberate no-op (see the comment in `debian/rules`):
  the Rust test suite already runs via `ci/checks.sh`/CTest against this
  exact source, and re-running it during the package build would need the
  same crates.io network access the next section explains a real buildd
  does not have.

**What it does not solve, and says so in `debian/README.source`:** the
build above only succeeds because the machine already has the ~140 crates
in `rust/Cargo.lock` cached in `~/.cargo/registry` from ordinary
`cargo build`/`cargo test` use. A real Debian buildd has no network access
during a build, so this is a stress-free local demo of the skeleton, not
proof it survives an official archive build. See the next section.

## The Rust-in-Debian reality: vendoring is not solved here

This is the one gap every path through official Debian packaging has to
cross, and it is *not* attempted in this repository — by decision, not
oversight (Track C2/C3 both flag it and defer it explicitly). `dh-cargo`
and Debian policy expect one of two things for a Rust package's
dependencies, and this project has picked neither yet:

- **(a) `cargo vendor` the full dependency graph into the source
  package.** An `orig.tar` that ships all ~140 crates' source, checked
  against `Cargo.lock` with `dh-cargo`/`cargo-checksum.json`. This is the
  common approach for a Rust *leaf* package (an application, not a
  library other packages depend on) already in Debian, and is probably
  the pragmatic choice here.
- **(b) `debcargo`-generate a `librust-*-dev` binary package per
  dependency**, and depend on those instead of vendoring. This is the
  approach official Debian archive policy prefers in general (each crate
  becomes its own trackable, shareable source package), but it is a large,
  ongoing undertaking against a ~140-crate graph that shifts on every
  `cargo update` — not a one-time cost.

Pick one before trusting `debian/rules` to build from a bare `.dsc` on a
real buildd rather than from a working tree with a warm cargo cache. This
project's own `docs/packaging/copyright` (see next section) is blocked on
the same fork in the road from the opposite direction — it could not write
a DEP-5 `Files:` stanza for sources that are neither vendored nor shipped,
which is why it took the third-party-notices-appendix path instead of
waiting for this decision.

## The generated `debian/copyright` and third-party-notices appendix (Track C2)

Two files under `docs/packaging/`, both generated (not hand-written) by
`tools/generate-debian-copyright.py`, and both kept honest by the
`debian-copyright-in-sync` CTest — it regenerates each and byte-compares
against the committed copy, so a drift between the code's actual licensing
and the shipped copyright file fails CI rather than silently rotting:

```bash
ctest --test-dir build -R debian-copyright-in-sync
```

- **`docs/packaging/copyright`** — a real DEP-5 file for this project's
  *own* source, generated straight from the REUSE metadata this project
  already maintains (`REUSE.toml`'s `[[annotations]]`, cross-checked by
  `reuse lint`). If you already trust this project's REUSE annotations,
  you can trust this file; there is no second, hand-maintained source of
  truth to drift out of sync with them.
- **`docs/packaging/third-party-notices`** — not a DEP-5 file. It is a
  plain per-crate notices appendix (name, version, SPDX license
  expression, upstream repository URL) for the ~117 third-party
  crates.io crates statically linked into the five shipped `.so`s,
  generated by walking `cargo metadata`'s dependency closure of exactly
  those five cdylib crates. This is **the maintainer's explicit decision**
  (Track C2, 2026-08-22): a notices appendix, not DEP-5 `Files:` entries
  and not vendoring — see the previous section for why DEP-5 has no honest
  path pattern for sources that are not in the source package. Revisit
  that decision only if a real upload under option (a) or (b) above is
  actually pursued; until then, treat the appendix as the authoritative
  answer to "what licenses are baked into this binary."

Regenerate either by hand if you need to check them without CTest:

```bash
python3 tools/generate-debian-copyright.py                    # docs/packaging/copyright
python3 tools/generate-debian-copyright.py --third-party-notices  # the appendix
```

## The reproducible-build story

`ci/checks.sh`, `ci/build.sh`, and `ci/reproducible.sh` are the single
definition of "the build" — GitHub Actions and GitLab CI are thin wrappers
that call them, and so is every packaging CTest above; there is no second,
CI-only build recipe hiding somewhere a packager would have to
reverse-engineer. `ci/reproducible.sh` in particular builds the mock server
twice from different filesystem paths and fails on any checksum
difference, using a `--remap-path-prefix`/`SOURCE_DATE_EPOCH`/pinned-Rust-
toolchain/committed-`Cargo.lock` recipe — the same discipline
`package-deb-reproducible` applies to the actual `.deb`. If your packaging
pipeline needs the build to be bit-for-bit reproducible (Debian's own
reproducible-builds effort, or your own distro's equivalent), start from
these scripts rather than writing a new recipe.

## Summary: what's done, what's a stub, what you own

| Area | State |
|---|---|
| CPack `.deb`, lintian-clean, CTest-verified | **Done** |
| Reproducible build of that `.deb` | **Done** |
| `debian/` dh skeleton, builds end to end locally | **Done as a skeleton** — not upload-tested on a real buildd |
| `debian/copyright` (DEP-5, own source) | **Done**, generated + CI-checked |
| Third-party crate license notices | **Done**, generated + CI-checked, as a non-DEP-5 appendix by decision |
| `debian/watch` | Written, **unverified** against the live tags page |
| `dh_auto_test` | Deliberately a **no-op** (network-dependent; tests already run elsewhere) |
| Crate vendoring (`cargo vendor` vs `debcargo`) | **Not attempted** — your first real decision |
| Official Debian ITP/upload | **Explicitly not this project's goal** |

If you're the packager reading this: the fastest path to a working local
`.deb` is `ctest --test-dir build -R package-deb`, and the fastest path to
something you can iterate on for an actual upload is the `debian/`
skeleton plus a vendoring decision from the section above.
