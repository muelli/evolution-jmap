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

**Refreshed 2026-10-13 (item 96), tip `e466767e`.** `master` had gained 18
commits since item 95's `ae19d06c` (real fixes: `a75d29a3` SieveScript/validate's
wire shape, `df8e19f3` the resolver thread pool addressing F31, `1a9b09bc`
confirming item 63 live, `ed7d7973` the `/usr` install-prefix default; the rest
is live-server test coverage and an FFI re-audit report, no further behaviour
change). Merged `origin/master` into `release/0.5.0` with no conflicts (the two
packaging fixes, `d2ba3be5` and `ed7d7973`, touch disjoint files: RPM-generator
gating vs. the install-prefix default, so neither undoes the other). `NEWS.md`
and `docs/packaging/changelog` updated: item 63's live-session caveat dropped
(now confirmed), the resolver-pool fix added, and the changelog brought back in
sync with `NEWS.md` (it had silently fallen behind the real-world-scale bullet
since `b006dab3`, before this session). `ci/checks.sh` green on the merge tip,
foreground, `EXIT=0`; `package-default-generator` (added by `d2ba3be5`) now
exercises the exact `cmake --build build --target package` release.yml runs,
so this is no longer a gap `ci/checks.sh` misses. Reran the `release` job's own
steps by hand on this tip: cacheless `jmap-mockd` release build, source
tarball, SBOM (byte-identical across two runs), checksums, see below. `master`'s
CI is green on `a2e57762` (the merged tip): `checks`, `reproducible`, `build`
all `success`; the GUI-smoke/EDS-matrix/functional jobs show `skipped`, matching
this project's normal job gating, not a failure.

**Refreshed 2026-10-14 (release cut), tip `ec791729` plus this checklist
commit.** `master` merged again (`17f6c666`, no conflicts), bringing in the
item 101 consent-loop completion (`ea7e61c3`), the `resolve_redirect` panic
fix (`621b075c`) and the rustls security bump (`296c2de8`); workspace
`Cargo.lock` entries refreshed separately (`3aaefedd`, master's new
`jmap-tracing-capture` at 0.4.1 broke `--locked`); `NEWS.md` and the
changelog grew the three matching user-visible bullets and the changelog
entry flipped UNRELEASED to unstable (`ec791729`). CI green on `ec791729`
via draft PR #1, run 38668054657: `checks`, `build`, `reproducible` all
`success`, gated jobs skipped as normal. Gates re-run on the evo-test VM
(real git clone of the pushed tip): `cargo test --locked` 190 suites green
with ONE test skipped (`consumer_check_script_fails_loudly_when_tools_
missing`, unportable: it assumes `PATH=/usr/bin` lacks cargo, but apt's
cargo lives exactly there; CI runs it green in its rustup layout);
`cargo deny` green; the 13-leg ctest suite green including
`rust-test-eds` (one timezone-sensitive assertion in `eds-sys`'s
`camel_folder_search_utilities_in_eds` fails under Europe/Berlin and
passes under TZ=UTC, proven A-B; CI and the runners are UTC), lintian,
`package-deb-reproducible`, `release-workflow` and
`debian-copyright-in-sync`; boundary lint clean. Both environment
findings are queued in the harness, neither is a product defect. Release
job dry run on this tip: `.deb` 11050036 bytes, cacheless `jmap-mockd`,
source tarball, SBOM byte-identical across two runs, SHA256SUMS written.
The pinned release container digest answers HTTP 200 to an anonymous
GHCR manifest probe, so the tag run's pull is verified reachable.

## Before tagging

- [ ] Confirm `release/0.5.0` is the branch to release from and `ci/checks.sh`
      is green on its tip (it is, as of this checklist, tip `e466767e`).
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

## Rerun on the refreshed tip (item 96, 2026-10-13, tip `e466767e`)

Same steps, repeated after merging 18 `master` commits in (above). No new
bugs found; both fixes above still hold with nothing to re-fix or duplicate.

```
c4600b55743814747d330ad392b0fd9f8a9c7128fd97206aabfb1a0a63c29bc5  evolution-jmap-v0.5.0.cdx.json
87e7a829cf40956b25a362453aab62998faca87be4353fcc9a1f9c554d06b96b  evolution-jmap-v0.5.0.tar.xz
e9234d4ba95e92a051a5f178969c30cb972cd32f998059f927717fa240e6a16f  evolution-jmap_0.5.0_amd64.deb
dca8b97792f3f2807bc3a3aac2b8fa65177573795e12ef647e5de2536d2dab80  jmap-mockd
```

These are this dry run's own checksums, not the real release's (the real
tag build will differ since it runs on the pinned `ci-image` container, not
this VM). Re-running a tag still needs its own `SHA256SUMS` verification per
`docs/verifying-artifacts.md`; this table is evidence the pipeline runs
clean end to end on the refreshed tip, not a value to check against later.
