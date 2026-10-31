<!--
SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
SPDX-License-Identifier: GPL-3.0-or-later
-->

# Changelog

All notable changes to the `evolution-jmap-mock` crate are documented in this file.

The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project follows [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added
- Publication metadata in `Cargo.toml`: repository, readme, documentation URL,
  keywords, and categories.
- Crate `README.md` with mode overview and `jmap-mockd` usage from shell and Python.
- `examples/client_smoke.rs` with a minimal `evolution-jmap-client` smoke test against
  a `MockServer` instance.
- `ci/package-check.sh` to verify `cargo package`, unpacked tarball build and tests,
  and rustdoc with warnings denied.
- `MockServer::builder().contact_create_omits_uid()` and `jmap-mockd
  --contact-create-omits-uid`: leave `ContactCard/set`'s `uid` unset on a create
  that omits it, modelling a real server found not to assign one.

## [0.5.0] - 2026-10-24

### Added
- Optional `parse` feature to gate `ContactCard/parse` and `CalendarEvent/parse`
  behavior.
- `jmap-mockd` support for machine-readable startup output with `--port 0`.
- `jmap-mockd --seed <path>` JSON fixture loading for accounts, mailboxes, emails,
  contacts, and calendar events.
- Grouped hostile mode controls through a public `Faults` API.
