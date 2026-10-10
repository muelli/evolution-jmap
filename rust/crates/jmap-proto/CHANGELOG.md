<!--
SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
SPDX-License-Identifier: GPL-3.0-or-later
-->

# Changelog

All notable changes to the `evolution-jmap-proto` crate will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.5.0] - 2026-10-10

### Added
- Comprehensive crate `README.md` with protocol specification overview, feature flags, MSRV, scope, and missing doc gap tracking.
- Minimal working example doctest on `jmap_proto` module root and integration test suite `tests/readme.rs`.
- Package metadata: repository URL, readme reference, keywords, categories, and cargo package verification.

### Changed
- Refined Quota wire type names to match RFC 9425 specification.
- Accepted nullable `newState` on `Foo/set` responses for servers that return null when state does not advance (e.g. Fastmail).
- Trimmed scheduling messages on `CalendarEvent/set` when `hideAttendees` is set.
- Stamped `isOrigin` on `CalendarEvent/get` responses.

## [0.4.0] - 2026-09-11

### Added
- Property-based testing via proptest and hostile fuzzing across all protocol serialization and deserialization boundaries.
- Fluent builder methods and ergonomic constructors across request, response, and entity types.
- Payload-driven protocol conformance testing against RFC test matrices.
- Hardened `#![forbid(unsafe_code)]` enforcement across the entire crate.

## [0.3.0] - 2026-08-19

### Added
- RFC 9670 JMAP Sharing types: `Principal`, `ShareNotification`, and availability calculation.
- RFC 9404 JMAP Blob Management: `Blob/get` and `Blob/upload` models.
- RFC 9425 JMAP for Quotas: `Quota` and `Quota/query` filter models.
- RFC 9661 JMAP for Sieve Scripts: `SieveScript` and validation types.
- RFC 8887 JMAP Subprotocol for WebSocket: frames and push enablement.
- RFC 9749 JMAP Web Push VAPID types.
- Draft extension models: `FileNode` (draft-ietf-jmap-filenode), `RefPlus` (draft-ietf-jmap-refplus), `Metadata` (draft-ietf-jmap-metadata), and `MailShare` (draft-ietf-jmap-mail-sharing).

## [0.2.0] - 2026-07-24

### Added
- Feature `mail`: RFC 8621 JMAP for Mail types (`Mailbox`, `Email`, `Identity`, `EmailSubmission`, `SearchSnippet`).
- Feature `contacts`: RFC 9610 JMAP for Contacts types (`AddressBook`, `ContactCard` with JSContact RFC 9553 representation).
- Feature `calendars`: draft-ietf-jmap-calendars types (`Calendar`, `CalendarEvent` with JSCalendar RFC 8984 representation, `ParticipantIdentity`).
- Sharing rights models on `Mailbox`, `AddressBook`, and `Calendar`.

## [0.1.0] - 2026-06-15

### Added
- Initial release of core JMAP wire types per RFC 8620: Session resource, Request and Response envelopes, Invocation arrays, ResultReference back-references, Id and State primitives, MethodError and SetError taxonomy.
