<!--
SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
SPDX-License-Identifier: GPL-3.0-or-later
-->

# Operator Publishing Checklist

This document is a practical guide for the human operator when publishing
crates from this workspace to crates.io.

Publishing to crates.io is solely an operator decision. Autonomous agent lanes
never run `cargo publish`, never flip `publish = false` to `true`, and never
rename crates.

## Order of Publication

The dependency DAG across the workspace strictly determines publication order:

1. **`evolution-jmap-proto`** (or `jmap-proto`)
   - Dependencies: Pure Rust, external crates only (`serde`, `serde_json`).
   - Role: The anchor crate defining wire types for all other members.
   - Status: Must be published and indexed on crates.io before any dependent crate.

2. **`evolution-jmap-mock`** (if published)
   - Dependencies: `evolution-jmap-proto`, `tiny_http`, `serde`.
   - Role: In-memory test server and `jmap-mockd` test runner.

3. **`evolution-jmap-client`** (or `jmap-blocking`)
   - Dependencies: `evolution-jmap-proto` (plus optional `ureq`, `rustls`).
   - Role: Blocking JMAP client with pluggable transport and OAuth 2.0.
   - Requirement: `evolution-jmap-proto` must already exist on crates.io at the
     matching version requirement.

4. **`evolution-jmap-ical` and `evolution-jmap-vcard`** (if published)
   - Dependencies: `evolution-jmap-proto`, `calcard`.
   - Role: Format translation between RFC 5545 / RFC 6350 and JSCalendar / JSContact.

## Pre-Publication Verification Checklist

For each crate to be published, complete the following verification steps:

- [ ] **Code gates**:
  - Run `cargo fmt --all -- --check` (must be clean).
  - Run `cargo clippy --all-targets -- -D warnings` (must be clean with zero warnings).
  - Run `cargo test -p <crate>` (all tests must pass).
  - Run `reuse lint` (must be 100% compliant with REUSE Specification).
  - Run `./ci/checks.sh` (complete CI gate must be green).

- [ ] **Documentation**:
  - Verify `RUSTDOCFLAGS="-D warnings" cargo doc -p <crate> --no-deps` passes without errors.
  - Confirm `README.md` is present in the crate directory, contains working examples,
    and describes feature flags, MSRV, and scope boundaries.
  - Confirm `CHANGELOG.md` reflects all notable changes for the release version.

- [ ] **Packaging and standalone verification**:
  - Run `cargo package -p <crate> --list` to inspect every file included in the tarball.
  - Package the archive with `cargo package -p <crate>`.
  - Extract the `.crate` tarball from `rust/target/package/` into a temporary directory
    outside the workspace and verify `cargo build` and `cargo test` pass standalone.

- [ ] **Manifest preparation**:
  - Ensure version numbers in `Cargo.toml` and `CHANGELOG.md` match.
  - Ensure package metadata (`description`, `repository`, `readme`, `keywords`,
    `categories`, `license`, `rust-version`) is complete and accurate.
  - Remove `publish = false` or set `publish = true`.

- [ ] **Execute release**:
  - Run `cargo publish -p <crate>`.
  - Tag the release commit in git: `git tag -a v<version> -m "Release v<version>"`.
  - Push the git tag: `git push origin v<version>`.

## What is Irreversible

Keep these permanent consequences in mind before running `cargo publish`:

- **Registry immutability**: Once a crate version is uploaded to crates.io, its
  contents cannot be edited, overwritten, or replaced. A bug or packaging error
  requires releasing a new version number (e.g. `0.4.2`).
- **Cannot unpublish**: crates.io does not allow deleting or unpublishing releases.
  The `cargo yank` command only prevents new projects without a lockfile entry from
  resolving that version; existing projects and lockfiles continue using it, and the
  yanked version number can never be reused.
- **Public API contract**: Every public struct, enum, function, and trait becomes an
  immediate semantic versioning contract for external consumers.
- **License fixation**: The license string stamped in the package metadata cannot be
  retracted for already-distributed versions of the crate tarball.
