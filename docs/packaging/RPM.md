<!--
SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
SPDX-License-Identifier: GPL-3.0-or-later
-->

# RPM packaging notes

## 2026-10-05 Batch 3 A1 evidence pass

This document records what an RPM package needs, compared with the existing
Debian packaging in `cmake/Packaging.cmake`, for the same installed component
set and install tree.

### Inputs checked

- `cmake/Packaging.cmake`
- `cmake/Backends.cmake`
- `docs/PACKAGING.md`
- `docs/packaging/lintian-overrides`
- `docs/packaging/changelog`
- `docs/packaging/copyright`

### RPM specific requirements for the same install tree

1. Library directory and RUNPATH handling are Fedora specific.
   - Fedora places EDS and Evolution private libraries under `/usr/lib64` on
     x86_64 systems.
   - Any packaging logic that needs those directories must query them from
     pkg-config variables, as `cmake/Backends.cmake` already does, not by
     hardcoding Debian multiarch paths.
   - In practice this means using `pkg-config --variable=...` for Evolution and
     EDS module directories and private library directories when configuring
     RPM dependency scanning inputs.

2. Runtime dependency metadata differs from Debian.
   - Debian uses `dpkg-shlibdeps` via `CPACK_DEBIAN_PACKAGE_SHLIBDEPS`.
   - RPM packaging uses RPM dependency generation (`Requires`/`Provides`) from
     ELF metadata and RPM macros during `rpmbuild`.
   - If extra manual requirements are needed, they are expressed as RPM
     `Requires`, not Debian control fields.

3. License file handling is explicit in RPM payload policy.
   - RPM package metadata should mark the license text as a license payload
     entry so tools and policy checks can validate it.
   - Debian's `copyright` file under
     `/usr/share/doc/<package>/copyright` is not a drop in substitute for RPM
     license tagging, even if both can coexist in the installed tree.

4. Changelog expectations differ.
   - Debian linting requires changelog content in
     `/usr/share/doc/<package>/changelog.gz`, and this repository already
     installs and checks that.
   - RPM packages do not need the Debian style `changelog.gz` payload entry for
     policy compliance.

5. Lint tooling and overrides are distro specific.
   - Debian uses `lintian` and this repo tracks overrides in
     `docs/packaging/lintian-overrides`.
   - RPM uses `rpmlint` with its own message set and override/config mechanism,
     which should be tracked in a separate RPM specific file.

### Scope notes

This is the evidence-only step for Batch 3 item A1. It documents the required
RPM specific deltas before any build-system changes are made.
