<!--
SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
SPDX-License-Identifier: GPL-3.0-or-later
-->

# RPM packaging notes (Batch 3A item 1)

Date: 2026-10-05 (UTC)

This note records what an RPM package needs, beyond the existing Debian package
shape in `cmake/Packaging.cmake`, for the same install tree and install
components.

## Scope checked

- `cmake/Packaging.cmake`
- `cmake/Backends.cmake`
- `docs/PACKAGING.md`
- `docs/packaging/` (existing Debian packaging notes and lint overrides)

## RPM-specific requirements for the same install tree

1. **RPM generator and metadata, separate from the DEB config**
   - The current packaging config hard-selects only `DEB` as `CPACK_GENERATOR`.
   - RPM support needs a dedicated CPack RPM configuration file, plus one
     include hook from the existing packaging entry point.
   - The package payload should stay component-driven and match the same five
     install components as `.deb` (`book-backend`, `cal-backend`,
     `camel-provider`, `collection-backend`, `config-module`; plus
     `translations` when present).

2. **Fedora libdir and backend directories must come from `pkg-config`**
   - Fedora commonly uses `/usr/lib64` where Debian uses multiarch dirs.
   - Backend and module install directories are already discovered through
     `pkg-config --variable` in `cmake/Backends.cmake` (`backenddir`,
     `camel_providerdir`, `moduledir`) and must remain source-of-truth for RPM.
   - No hardcoded Debian multiarch path assumptions are valid for Fedora RPM
     packaging.

3. **Runtime dependency generation and private library resolution**
   - The Debian package uses generated shared-library dependencies
     (`CPACK_DEBIAN_PACKAGE_SHLIBDEPS`) and points shlibdeps at Evolution's
     private library dir from `evolution-shell-3.0` `privlibdir`.
   - RPM packaging needs the equivalent automatic dependency generation by the
     RPM toolchain, and must ensure private Evolution libs in Fedora paths are
     resolvable during dependency scanning.
   - Any directory path used for this must come from `pkg-config` variables, not
     hardcoded `/usr/lib64/evolution`.

4. **RUNPATH expectations for Fedora Evolution private libs**
   - The shipped modules are dlopened by Evolution and EDS processes, and link
     against Evolution private libs.
   - On Fedora, the private lib location is expected under the system libdir
     variant (for example `/usr/lib64/evolution` on x86_64).
   - RPM validation should confirm the built module set has a runtime search
     path compatible with the private library location reported by
     `pkg-config`.

5. **Explicit RPM `Requires` ownership for non-ELF runtime needs**
   - Auto-generated ELF dependencies do not cover all runtime requirements
     (for example service package relationships or script-level helpers).
   - RPM packaging should define explicit `Requires` entries where needed, while
     leaving pure ELF requirements to automatic generator output.
   - Required package names should be derived from the Fedora build environment,
     then documented alongside the eventual `PackagingRpm.cmake` settings.

6. **License file installation for RPM policy checks**
   - Fedora RPM policy expects an installed license file in the package payload.
   - Debian packaging already tracks copyright and notices under
     `/usr/share/doc/<pkg>/`; RPM packaging needs `%license`-style mapping for
     the SPDX-licensed source artifacts used by this project.

7. **No Debian changelog gzip convention in RPM payload**
   - Debian-specific `changelog.gz` handling exists to satisfy lintian and
     Debian policy.
   - RPM packaging should not mirror Debian's `changelog.gz` requirement. Fedora
     tooling uses different metadata and validation conventions.

8. **RPM lint flow parallels lintian but with RPM-native tooling**
   - Debian gate uses `lintian` plus project-local overrides.
   - RPM path needs `rpmlint` and, when needed, explicit documented overrides or
     justifications in a dedicated RPM lint policy file.

## Evidence pointers from the current tree

- DEB-only generator and component packaging are configured in
  `cmake/Packaging.cmake`.
- Backend destination discovery already relies on `pkg-config --variable` in
  `cmake/Backends.cmake` for every module class.
- Debian-specific changelog/copyright/lintian behavior is documented in
  `docs/PACKAGING.md` and `docs/packaging/lintian-overrides`.

## Out of scope for this increment

- No CMake or CI behavior changed in this increment.
- No RPM build target added yet.
- No containerized Fedora validation run yet.

## 2026-10-05 implementation increment (item 2 start)

- Added `cmake/PackagingRpm.cmake` and included it from `cmake/Packaging.cmake` with one include line, keeping RPM logic split from DEB logic.
- Added `ci/rpm.sh` as a loud gate wrapper that errors clearly when `rpmbuild` or `rpmlint` are unavailable.
- Initial RPM config is intentionally small for a safe first step and will be extended with Fedora container reproducibility and lint policy documentation in the next increments.

## Batch 3A item 2 implementation note (2026-10-06 UTC)

`ci/rpm.sh` now runs rootless in Podman by default when `podman` is available,
using the pinned Fedora image digest
`docker.io/library/fedora@sha256:6c75d5bf57cb0fa5aa4b92c6a83c86c791644496d9ac230de7711f5b8ec3b898`
for reproducibility before invoking the host build step with `--host`.

The dependency bootstrap intentionally omits `evolution-mapi-devel`, because
this repository does not build against MAPI and rawhide package churn had made
that package unavailable in prior runs.

This satisfies the reproducible-in-container requirement for the RPM target.
If Podman is unavailable, the script still runs on host as a fallback and keeps
the same `rpmbuild` and `rpmlint` checks.

## Batch 3A item 3 implementation note (2026-10-09 UTC)

`cmake/PackagingRpm.cmake` now registers two RPM CTest gates only when both
`rpmbuild` and `rpmlint` are present on the machine where CMake configures the
tree:

- `package-rpm` first runs `cmake --build` in `${CMAKE_BINARY_DIR}`, removes
  any previous `*.rpm` there, and then runs `cpack -G RPM` into that same
  build directory.
- `package-rpm-lint` depends on `package-rpm`, fails loudly when no RPM payload
  exists, and then runs `rpmlint` on the produced RPM files.

`ci/rpm.sh` remains the loud missing-tool gate for environments where CTest is
not used directly.

Operator CI wiring for this lane, without editing `.github/workflows` in-repo:

```bash
cmake -S . -B build-rpm-ci -G Ninja
ctest --test-dir build-rpm-ci --output-on-failure -R '^package-rpm$|^package-rpm-lint$'
```

## Batch 3A item 4 verification note (2026-10-09 UTC)

Installed the locally built RPM in a clean Fedora container and verified the
installed module locations against the same container's `pkg-config` variables.

Container command used:

```bash
podman run --rm -v "$PWD:$PWD:Z" -w "$PWD" \
  docker.io/library/fedora@sha256:6c75d5bf57cb0fa5aa4b92c6a83c86c791644496d9ac230de7711f5b8ec3b898 \
  bash -lc '
    dnf -y install pkgconf-pkg-config evolution-data-server-devel evolution-devel
    dnf -y install ./build-rpm/*.rpm
    pkg-config --variable=backenddir libedata-book-1.2
    pkg-config --variable=backenddir libedata-cal-2.0
    pkg-config --variable=camel_providerdir camel-1.2
    pkg-config --variable=moduledir libebackend-1.2
    pkg-config --variable=moduledir evolution-shell-3.0
  '
```

Observed paths from `pkg-config` in that install-test container:

- `book_dir=/usr/lib64/evolution-data-server/addressbook-backends`
- `cal_dir=/usr/lib64/evolution-data-server/calendar-backends`
- `camel_dir=/usr/lib64/evolution-data-server/camel-providers`
- `registry_dir=/usr/lib64/evolution-data-server/registry-modules`
- `config_dir=/usr/lib64/evolution/modules`

Installed payload checks passed:

- `${book_dir}/libebookbackendjmap.so` exists.
- `${cal_dir}/libecalbackendjmap.so` exists.
- `${camel_dir}/libcameljmap.so` and `${camel_dir}/libcameljmap.urls` exist.
- `${registry_dir}/module-jmap-backend.so` exists.
- `${config_dir}/module-jmap-configuration.so` exists.

This confirms the three backend modules and the Camel provider install where
EDS and Evolution discover them according to `pkg-config` in Fedora. Runtime
loading under a real Evolution session remains unverified in a running
Evolution.
