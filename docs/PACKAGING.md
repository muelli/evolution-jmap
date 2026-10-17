# Packaging evolution-jmap

This document is for distribution packagers. Upstream does not plan to file a
Debian ITP or to maintain distribution packages. This is a map of what
exists, what it proves and what is left for a packager.

## What the package contains

Five modules that `evolution-data-server` and Evolution `dlopen`:

| Module | Install component |
| --- | --- |
| `libebookbackendjmap.so` (address book backend) | `book-backend` |
| `libecalbackendjmap.so` (calendar backend) | `cal-backend` |
| `libcameljmap.so` and `libcameljmap.urls` (Camel mail provider) | `camel-provider` |
| `module-jmap-backend.so` (collection backend, source registry) | `collection-backend` |
| `module-jmap-configuration.so` (account setup, Evolution) | `config-module` |

Install directories come from `pkg-config` variables (`backenddir`,
`camel_providerdir`, `moduledir`) in `cmake/Backends.cmake`, so they are
correct on multiarch and `lib64` systems.

Build the modules against the exact EDS and Evolution that they will run
with. Their ABI is not stable across releases (see `docs/eds-versions.md`),
so a package split that lets the modules and Evolution drift apart is a bug.

The modules link Evolution's private libraries in
`pkg-config --variable=privlibdir evolution-shell-3.0` (`/usr/lib/evolution`
on Debian). The packaging passes that directory to the dependency scanner,
and the modules that need it carry it as RUNPATH.

## Debian: the CPack `.deb`

The tested package build is CPack, configured in `cmake/Packaging.cmake`:

```bash
cmake -S . -B build -G Ninja -DCMAKE_INSTALL_PREFIX=/usr
cmake --build build
ctest --test-dir build -R '^package-deb'
cmake --build build --target package
```

The CTest tests:

- `package-deb`: builds the `.deb` and checks its file list and control
  fields (`cmake/tests/check-deb-package.cmake`).
- `package-deb-reproducible`: packages the tree three times with different
  `SOURCE_DATE_EPOCH` environments and requires identical bytes
  (`cmake/tests/check-deb-reproducible.cmake`).
- `package-deb-lintian`: runs `lintian` and fails unless it is clean
  (`cmake/tests/check-deb-lintian.cmake`). This test exists only when
  `lintian` is installed. The only override,
  `docs/packaging/lintian-overrides`, accepts the
  `/usr/lib/evolution` RUNPATH on the four modules that need it.

## Debian: the `debian/` skeleton

`debian/` is a `dh` skeleton for a packager to start from. It is not
upload-ready.

- `debian/control`: `Build-Depends` follow `ci/install-deps.sh`, plus
  `debhelper-compat (= 13)`, `cargo` and `rustc`. The binary package
  `Depends` on `evolution-data-server (>= 3.52)` and `evolution (>= 3.52)`.
- `debian/rules`: `dh` over the CMake/Ninja build. `override_dh_auto_install`
  installs the five components above. `override_dh_shlibdeps` gives
  `dpkg-shlibdeps` Evolution's private library directory.
  `override_dh_auto_test` does nothing, because the tests need crates.io.
- `debian/changelog` and `debian/copyright` are symlinks to
  `docs/packaging/changelog` and `docs/packaging/copyright`.
- `debian/watch` follows the GitHub `vX.Y.Z` tags. Nobody has tested it with
  `uscan`.

Build it with `dpkg-buildpackage -us -uc -b`. This works only when
`~/.cargo/registry` already has the crates from `rust/Cargo.lock`. A Debian
buildd has no network, so see the next section.

### Rust dependencies: not solved here

Before `debian/rules` can build from a bare `.dsc`, the packager must choose
one of these:

- **`cargo vendor`** the dependency graph into the source package. This is
  the usual approach for a Rust leaf package.
- **`debcargo`**: depend on one `librust-*-dev` package per crate, as Debian
  policy prefers. This is a large and continuing task.

### Copyright files

`tools/generate-debian-copyright.py` generates two files under
`docs/packaging/`, and the `debian-copyright-in-sync` CTest test fails if
they differ from what the script generates:

- `docs/packaging/copyright`: DEP-5 for this project's own source, from the
  REUSE metadata in `REUSE.toml`.
- `docs/packaging/third-party-notices`: a per-crate list (name, version,
  SPDX licence, repository) of the crates.io crates linked into the shipped
  modules. It is not DEP-5, because those sources are not in the source
  package.

```bash
ctest --test-dir build -R debian-copyright-in-sync
python3 tools/generate-debian-copyright.py                         # copyright
python3 tools/generate-debian-copyright.py --third-party-notices   # notices
```

## Fedora: the RPM

`cmake/PackagingRpm.cmake` adds the CPack RPM generator when `rpmbuild` is
installed. It uses the same install tree and file list as the `.deb`.

The simplest way to build and lint it:

```bash
ci/rpm.sh
```

When `podman` is installed, `ci/rpm.sh` runs in a pinned Fedora container
and installs the build dependencies there. It then builds into `build-rpm/`,
runs `cpack -G RPM` and runs `rpmlint` on the result. Use `ci/rpm.sh --host`
to build on the host instead; it stops if `rpmbuild` or `rpmlint` is
missing.

When both `rpmbuild` and `rpmlint` are installed at configure time, CTest
also has two tests:

```bash
cmake -S . -B build-rpm -G Ninja
ctest --test-dir build-rpm --output-on-failure -R '^package-rpm$|^package-rpm-lint$'
```

`package-rpm` builds the tree and runs `cpack -G RPM`. `package-rpm-lint`
runs after it and fails when there is no RPM or when `rpmlint` complains.

## Flatpak: an extension for `org.gnome.Evolution`

Flathub's `org.gnome.Evolution` has the extension point
`org.gnome.Evolution.Extension`, mounted at `evolution/extensions`. The
extension must be built against the Flatpak runtime: modules built on the
host link different EDS sonames and do not load in the sandbox.

`packaging/flatpak/org.gnome.Evolution.Extension.evolution-jmap.from-source.yml`
builds the extension `org.gnome.Evolution.Extension.evolution-jmap` (branch
`stable`) from this source tree. It uses the `rust-stable` and `llvm21` SDK
extensions and builds with cargo offline.

1. Install the app, the SDK and the SDK extensions:

   ```bash
   flatpak install --user flathub org.gnome.Evolution org.gnome.Sdk//50 \
     org.freedesktop.Sdk.Extension.rust-stable org.freedesktop.Sdk.Extension.llvm21
   ```

2. Vendor the Rust crates. The manifest requires `packaging/flatpak/vendor`
   and points cargo at it through `packaging/flatpak/cargo-vendor-config.toml`:

   ```bash
   cd rust && cargo vendor --locked --versioned-dirs ../packaging/flatpak/vendor && cd ..
   ```

3. Build and install:

   ```bash
   flatpak-builder --user --install --force-clean \
     --state-dir packaging/flatpak/.flatpak-builder \
     packaging/flatpak/.build \
     packaging/flatpak/org.gnome.Evolution.Extension.evolution-jmap.from-source.yml
   ```

4. Check that Evolution sees it:

   ```bash
   flatpak info --show-extensions org.gnome.Evolution
   ```

The extension branch must match the Evolution app branch. Flathub
submission also needs the vendored sources as manifest sources, an AppStream
MetaInfo file and installed licence files. None of these exist yet.

## Reproducible builds

`ci/checks.sh`, `ci/build.sh` and `ci/reproducible.sh` define the build.
GitHub Actions and GitLab CI only call them. `ci/reproducible.sh` builds
`jmap-mockd` twice in different directories with `--remap-path-prefix` and
`SOURCE_DATE_EPOCH`, and fails if the binaries differ. `rust/rust-toolchain.toml`
pins the toolchain and `rust/Cargo.lock` pins the dependencies. See
`docs/verifying-artifacts.md` for how to check a release.

## Summary

| Area | State |
| --- | --- |
| CPack `.deb`, lintian-clean, tested by CTest | Done |
| Reproducible `.deb` | Done |
| `debian/` dh skeleton | Builds locally with a warm cargo cache; not tested on a buildd |
| `debian/copyright` and third-party notices | Generated and checked by CTest |
| `debian/watch` | Not tested |
| Crate vendoring for Debian | Not started |
| CPack RPM, `rpmlint` | Done (`ci/rpm.sh`, `package-rpm` tests) |
| Flatpak extension | Builds from source locally; not submitted to Flathub |
