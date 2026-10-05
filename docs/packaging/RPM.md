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
