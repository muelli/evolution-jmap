<!--
SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
SPDX-License-Identifier: GPL-3.0-or-later
-->

# Changelog

All notable changes to the `evolution-jmap-client` crate will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.5.0] - 2026-10-10

### Added
- Comprehensive crate `README.md` with positioning against async incumbents, feature flags, MSRV, license, scope, and missing doc gap tracking.
- Minimal working example doctest on crate root and integration test suite `tests/readme.rs`.
- Package metadata: repository URL, readme reference, keywords, categories, and standalone cargo package verification.
- Added `Client::is_anonymous()` predicate to detect unauthenticated 200 OK session responses.
- Added `Client::current_user_principal()` and `Client::current_user_principal_id()` with fallback resolution past server account ID mismatches.
- Added tolerant handling for server-omitted properties on `Mailbox/set` create, `CalendarEventNotification/set` destroy, and `ParticipantIdentity/get`.
- Added request batch chunking and enforcement across `maxObjectsInGet` and `maxCallsInRequest`.

### Changed
- Normalized array-wrapped parsed contact cards in `Client::contact_card_parse`.
- Cleaned private intra-doc links in `oauth` module documentation.

## [0.4.0] - 2026-09-11

### Added
- Free/busy and availability queries via `Principal/getAvailability`.
- Live-server test coverage for ACL sharing and ShareNotification delivery across Mailbox, Calendar, and AddressBook.
- Push notification streaming over EventSource (`EventSourceSubscription`).
- RFC 9404 Blob management (`Blob/copy`, `Blob/get`, `Blob/upload`).
- RFC 9425 Quota querying and verification.
- Property-based testing of the untrusted-server boundary via proptest.

## [0.3.0] - 2026-08-19

### Added
- RFC 8414 OAuth 2.0 authorization server metadata discovery.
- RFC 7591 dynamic client registration.
- RFC 7636 PKCE authorization code exchange and token refresh.
- RFC 8620 §2.2 DNS SRV autodiscovery seam (`Resolver`).
- Opt-in transparent URL rebasing (`rebase_urls_to_origin`) for reverse proxy and NAT traversal.
- First-class cancellation via `CancelFlag`.

## [0.2.0] - 2026-07-24

### Added
- Full mail operations: `Mailbox/set` create, update, and destroy, `Email/import`, `EmailSubmission/set`.
- RFC 9610 contacts CRUD: `AddressBook` and `ContactCard`.
- Calendars draft CRUD: `Calendar` and `CalendarEvent`.
- Incremental synchronization via `/changes` with multi-page resumption and aggregation (`Client::all_changes`).

## [0.1.0] - 2026-06-15

### Added
- Initial release of blocking JMAP client: session discovery (`/.well-known/jmap`), Basic and Bearer authentication, method batching against API endpoints, and pluggable `Transport` trait.
