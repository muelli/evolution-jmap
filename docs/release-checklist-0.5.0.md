<!--
SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
SPDX-License-Identifier: GPL-3.0-or-later
-->

# Release checklist: v0.5.0

Item 95's part (4): the exact operator steps to tag and publish, plus what
to verify afterwards. `NEWS.md` and the version bump (part 1) are already on
this branch; part (3), a local dry run of everything
`.github/workflows/release.yml` does short of publishing, found and fixed two
real release-blocking bugs (below) before they could hit a real tag push.

## Before tagging

- [ ] Confirm `release/0.5.0` is the branch to release from and `ci/checks.sh`
      is green on its tip (it is, as of this checklist).
- [ ] Re-read `NEWS.md`'s new `[0.5.0]` section; confirm nothing merged after
      it needs adding.
- [ ] Decide whether `release/0.5.0` merges to `master` before or after
      tagging (this checklist does not do either; merging is the operator's
      call, same as the release itself).

## Tag and publish

1. Merge/fast-forward `master` to the reviewed tip of `release/0.5.0` (or tag
   directly on the branch, if that is the intended history).
2. `git tag v0.5.0 <commit>` and `git push origin v0.5.0`. This is the only
   step that triggers anything: the Release workflow runs only on a pushed
   `v*` tag.
3. Watch the `Release` workflow's two jobs (`package`, then `release`) to
   completion in GitHub Actions.
4. Once published, verify per `docs/verifying-artifacts.md`:
   ```bash
   sha256sum --check --ignore-missing SHA256SUMS
   gh attestation verify jmap-mockd --repo muelli/evolution-jmap
   gh attestation verify evolution-jmap_0.5.0_amd64.deb --repo muelli/evolution-jmap
   ```
5. Spot-check the release notes GitHub auto-generated
   (`--generate-notes`) against `NEWS.md`'s `[0.5.0]` section; they are not
   the same text and both get published, so a mismatch is worth a second
   look, not a blocker.

## What this session's local dry run covered

Ran, outside of GitHub Actions, in order:

- `cmake -S . -B build -G Ninja -DCMAKE_INSTALL_PREFIX=/usr`, full build,
  then `ctest -R '^package-deb'` (the `package`, `package-deb-reproducible`,
  `package-deb-lintian` legs `ci/checks.sh` already runs every push).
- `cmake --build build --target package`, the exact command
  `release.yml`'s `package` job runs (not `-G DEB` directly, which is what
  the `package-deb*` tests above call instead).
- The `release` job's own steps: cacheless `cargo build --release --locked
  -p evolution-jmap-mock` under `SOURCE_DATE_EPOCH` and the remapped
  `RUSTFLAGS`, the `git archive | xz` source tarball, `cargo install
  --locked --version 0.5.9 cargo-cyclonedx`, `ci/sbom.sh`, and `sha256sum`
  over the resulting `dist/`.
- Ran `ci/sbom.sh` twice over the same `.deb` under the same
  `SOURCE_DATE_EPOCH`; the two CycloneDX documents came out byte-identical,
  matching the determinism the comments in `ci/sbom.sh` and
  `cmake/Packaging.cmake` claim.

**Not reproducible locally, by design:** the `attest-build-provenance` step
needs GitHub's OIDC token (`id-token: write`), and `gh release create` needs
a real tag push. Neither has a local equivalent; both only run for real on
the actual tag push in step 2 above.

### Bugs found and fixed by this dry run (already on this branch)

1. **`cmake --build build --target package` failed outright** on a release
   image with no `rpmbuild` (which is exactly what the pinned CI image is):
   `cmake/PackagingRpm.cmake` appended `RPM` to `CPACK_GENERATOR`
   unconditionally, so the *default* `cpack` invocation the `package` target
   runs tried to build an RPM too and aborted the whole run ("RPM package
   requires rpmbuild executable"), taking the `.deb` down with it. The
   `package-deb*` tests never caught this because they all call
   `cpack -G DEB` directly, bypassing `CPACK_GENERATOR`'s full list. Fixed by
   gating the `list(APPEND CPACK_GENERATOR RPM)` on `find_program(rpmbuild)`,
   and added `package-default-generator`
   (`cmake/tests/check-package-default-generator.cmake`), a new ctest that
   runs the exact `cmake --build <dir> --target package` command instead of
   `cpack -G DEB`, so a future generator added to the default list without a
   present toolchain fails this test rather than only the real release.
2. **The release workflow's own packaging step never passed
   `-DCMAKE_INSTALL_PREFIX=/usr`**, unlike `ci/checks.sh` (fixed there in
   `863d7f7d` once `po/LINGUAS` shipped a first language). Without it
   `CMAKE_INSTALL_PREFIX` defaults to `/usr/local`, and the translation
   catalogue installs under `/usr/local/share/locale`, which
   `package-deb-lintian` rejects as `dir-in-usr-local`/`file-in-usr-local`.
   Since `ci/checks.sh`'s own build tree already carries the flag, this gap
   was invisible to every push gate; it would only have surfaced on an
   actual tag push. Fixed by adding the same flag to `release.yml`'s cmake
   configure line.

Both were confirmed red (reproduced the exact release-workflow failure
locally) before the fix, and green after, using the same pinned-image
package set this runner has (Ubuntu 24.04, EDS 3.52.3), not a guess from
reading the YAML.
