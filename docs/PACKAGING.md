# Packaging evolution-jmap

This document is for distribution packagers. The upstream project does not
plan to file a Debian ITP or to maintain distribution packages. This document
shows what exists, what it proves and what a packager must still do.

## What the package contains

The package contains five modules. `evolution-data-server` and Evolution load
them with `dlopen`.

| Module | Install component |
| --- | --- |
| `libebookbackendjmap.so` (address book backend) | `book-backend` |
| `libecalbackendjmap.so` (calendar backend) | `cal-backend` |
| `libcameljmap.so` and `libcameljmap.urls` (Camel mail provider) | `camel-provider` |
| `module-jmap-backend.so` (collection backend, source registry) | `collection-backend` |
| `module-jmap-configuration.so` (account setup, Evolution) | `config-module` |

The install directories come from the `pkg-config` variables (`backenddir`,
`camel_providerdir`, `moduledir`) in `cmake/Backends.cmake`. The directories
are therefore correct on multiarch and `lib64` systems.

Build the modules against the exact EDS and Evolution versions that they will
run with. The ABI is not stable across releases (see `docs/eds-versions.md`).
A package split that lets the modules and Evolution use different versions is
a bug.

The modules link the private libraries of Evolution. These libraries are in
`pkg-config --variable=privlibdir evolution-shell-3.0` (`/usr/lib/evolution`
on Debian). The packaging gives this directory to the dependency scanner.
The modules that need it carry it as RUNPATH.

## Debian: the CPack `.deb`

The tested package build is CPack. `cmake/Packaging.cmake` configures it:

```bash
cmake -S . -B build -G Ninja -DCMAKE_INSTALL_PREFIX=/usr
cmake --build build
ctest --test-dir build -R '^package-deb'
cmake --build build --target package
```

These are the CTest tests:

- `package-deb`: builds the `.deb` and checks its file list and control
  fields (`cmake/tests/check-deb-package.cmake`).
- `package-deb-reproducible`: packages the tree three times with different
  `SOURCE_DATE_EPOCH` environments. The test requires identical bytes
  (`cmake/tests/check-deb-reproducible.cmake`).
- `package-deb-lintian`: runs `lintian`. The test fails unless `lintian` is
  clean (`cmake/tests/check-deb-lintian.cmake`). This test exists only if
  `lintian` is installed. The only override is
  `docs/packaging/lintian-overrides`. It accepts the `/usr/lib/evolution`
  RUNPATH on the four modules that need it.

## Debian: the `debian/` skeleton

`debian/` is a `dh` skeleton. A packager can use it as a start. It is not
ready for upload.

- `debian/control`: `Build-Depends` follows `ci/install-deps.sh`. It also
  contains `debhelper-compat (= 13)`, `cargo` and `rustc`. The binary package
  `Depends` on `evolution-data-server (>= 3.52)` and `evolution (>= 3.52)`.
- `debian/rules`: `dh` over the CMake/Ninja build. `override_dh_auto_install`
  installs the five components above. `override_dh_shlibdeps` gives the
  private library directory of Evolution to `dpkg-shlibdeps`.
  `override_dh_auto_test` does nothing, because the tests need crates.io.
- `debian/changelog` and `debian/copyright` are symlinks to
  `docs/packaging/changelog` and `docs/packaging/copyright`.
- `debian/watch` follows the GitHub `vX.Y.Z` tags. Nobody has tested it with
  `uscan`.

Build the package with `dpkg-buildpackage -us -uc -b`. This works only if
`~/.cargo/registry` already contains the crates from `rust/Cargo.lock`. A
Debian buildd has no network. See the next section.

### Rust dependencies: not solved here

Before `debian/rules` can build from a bare `.dsc`, the packager must choose
one of these options:

- **`cargo vendor`**: put the dependency graph into the source package. This
  is the usual approach for a Rust leaf package.
- **`debcargo`**: depend on one `librust-*-dev` package for each crate. Debian
  policy prefers this option. It is a large task that does not end.

### Copyright files

`tools/generate-debian-copyright.py` generates two files under
`docs/packaging/`. The `debian-copyright-in-sync` CTest test fails if the
files differ from the output of the script:

- `docs/packaging/copyright`: DEP-5 for the own source of this project. The
  script reads the REUSE metadata in `REUSE.toml`.
- `docs/packaging/third-party-notices`: a list of the crates.io crates that
  the shipped modules link. The list has one entry for each crate (name,
  version, SPDX licence, repository). It is not DEP-5, because the source
  package does not contain these sources.

```bash
ctest --test-dir build -R debian-copyright-in-sync
python3 tools/generate-debian-copyright.py                         # copyright
python3 tools/generate-debian-copyright.py --third-party-notices   # notices
```

## Fedora: the RPM

If `rpmbuild` is installed, `cmake/PackagingRpm.cmake` adds the CPack RPM
generator. It uses the same install tree and file list as the `.deb`.

The simplest way to build and lint the RPM is this command:

```bash
ci/rpm.sh
```

If `podman` is installed, `ci/rpm.sh` runs in a pinned Fedora container and
installs the build dependencies there. It builds into `build-rpm/`, runs
`cpack -G RPM` and runs `rpmlint` on the result. To build on the host, use
`ci/rpm.sh --host`. This command stops if `rpmbuild` or `rpmlint` is missing.

If `rpmbuild` and `rpmlint` are both installed at configure time, CTest has
two more tests:

```bash
cmake -S . -B build-rpm -G Ninja
ctest --test-dir build-rpm --output-on-failure -R '^package-rpm$|^package-rpm-lint$'
```

`package-rpm` builds the tree and runs `cpack -G RPM`. `package-rpm-lint` runs
after it. It fails if there is no RPM or if `rpmlint` reports a problem.

## Flatpak: an extension for `org.gnome.Evolution`

The `org.gnome.Evolution` application on Flathub has the extension point
`org.gnome.Evolution.Extension`. The extension point is mounted at
`evolution/extensions`. Build the extension against the Flatpak runtime.
Modules that you build on the host link different EDS sonames. They do not
load in the sandbox.

`packaging/flatpak/org.gnome.Evolution.Extension.evolution-jmap.from-source.yml`
builds the extension `org.gnome.Evolution.Extension.evolution-jmap` (branch
`stable`) from this source tree. It uses the `rust-stable` and `llvm21` SDK
extensions. It builds with cargo offline.

1. Install the application, the SDK and the SDK extensions:

   ```bash
   flatpak install --user flathub org.gnome.Evolution org.gnome.Sdk//50 \
     org.freedesktop.Sdk.Extension.rust-stable org.freedesktop.Sdk.Extension.llvm21
   ```

2. Vendor the Rust crates. The manifest requires `packaging/flatpak/vendor`.
   It points cargo at this directory through
   `packaging/flatpak/cargo-vendor-config.toml`:

   ```bash
   cd rust && cargo vendor --locked --versioned-dirs ../packaging/flatpak/vendor && cd ..
   ```

3. Build and install the extension:

   ```bash
   flatpak-builder --user --install --force-clean \
     --state-dir packaging/flatpak/.flatpak-builder \
     packaging/flatpak/.build \
     packaging/flatpak/org.gnome.Evolution.Extension.evolution-jmap.from-source.yml
   ```

4. Check that Evolution shows the extension:

   ```bash
   flatpak info --show-extensions org.gnome.Evolution
   ```

The extension branch must be the same as the branch of the Evolution
application. A Flathub submission also needs these items. None of them exists
yet:

- the vendored sources as manifest sources
- an AppStream MetaInfo file
- installed licence files

## Reproducible builds

`ci/checks.sh`, `ci/build.sh` and `ci/reproducible.sh` define the build.
GitHub Actions and GitLab CI only call them. `ci/reproducible.sh` builds
`jmap-mockd` twice in different directories with `--remap-path-prefix` and
`SOURCE_DATE_EPOCH`. It fails if the binaries differ. `rust/rust-toolchain.toml`
pins the toolchain. `rust/Cargo.lock` pins the dependencies. To check a
release, see `docs/verifying-artifacts.md`.

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
