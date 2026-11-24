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
- `--mailbox-get-omits-unnamed-share-with` and
  `MockServerBuilder::mailbox_get_omits_unnamed_share_with()`: omit
  `shareWith` from `Mailbox/get` unless a caller names it in
  `properties`, matching a real server's quirk. (`6027de11`)
- RFC 9610 contact query filter properties and sort. (`0230cb2b`)
- RFC 9670 principals filter properties, `Principal/set`, `ShareNotification/set`
  and sort; `currentUserPrincipalId` in the capability; the account owner is
  rejected in a collection's `shareWith`. (`8b778745`)
- draft-ietf-jmap-calendars-33 filter properties, `calendarAddressInUse`,
  `start` sort, notification `changes` and `created` sort. (`20253c6c`)
- RFC 9661 Sieve conformance: server-assigned script names, name validation,
  activation mutual exclusion, case-insensitive name filter, multi-property
  sort; `AddressBook` cascade delete and `addressBookHasContents`. (`3796bee5`)
- RFC 8984 JSCalendar validation on `CalendarEvent/set` create. (`49db6732`)
- `EmailSeed::raw_source` and `rawSourceBase64` in the `--seed` JSON for a
  verbatim message/rfc822 blob. (`1bd73c5c`)
- Faults `email_update_always_not_found` (`f5c92f45`) and
  `calendar_destroy_rejects_remove_contents` (`3f7ecb06`).
- Configurable advertised `maxConcurrentRequests` and `maxConcurrentUpload`
  limits. (`2544d5cc`)
- The default seed has `bob@example.com` and `carol@example.com` as
  principals. (`9d56210b`)
- `EmailSeed::html_body` and `bodyIsHtml` in the `--seed` JSON, so a seeded
  message can carry an HTML body instead of always `text/plain`. (`208b0b74`)
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
- Principal and delegation support: `Principal/get` across shared accounts,
  `--disable-principal-query` (and `MockServerBuilder::disable_principal_query`)
  matching Stalwart directory policy, `shareWith` with `mayReadFreeBusy` visibility
  rules on calendars and address books, and `Principal/getAvailability`.
- Hostile server scenarios via `jmap-mockd --hostile <scenario>` and
  `MockServerBuilder::hostile`: `oversized-response`, `deep-json`,
  `numbers-out-of-range`, `dangerous-ids`, `blob-size-mismatch`,
  `session-urls-file`, `session-urls-other-origin`, `redirect-other-origin`,
  `wrong-method-call-id`, `duplicate-method-call-id`, `out-of-order-responses`,
  `replayed-response`, `state-backwards`, and `eventsource-unknown`. (`b67c05d6`)
- Earlier, narrower hostile flags, each a single fault rather than a named
  scenario: `--well-known-redirect-to` (`8804d6b0`), `--changes-never-terminates`
  (`4e76e8e6`), `--advertise-origin` (`70cedc1d`), `--email-get-includes-
  unrequested` and `--query-never-terminates` (`2581e459`), `--shared-account-
  name-copies-user` (`d845d749`), `--contact-photo-uses-file-uri` (`e205cd14`),
  and `--calendar-event-link-uses-file-uri` (`46dfa405`).
- `--sieve-validate-long-error` to model a server that rejects an oversized
  Sieve script on `SieveScript/validate` for test coverage. (`164d97e3`)
- Opt-in `Task` and `TaskList` methods, and `--sieve-vacation-script` to seed a
  vacation responder script. (`855cc44c`)
- Sieve realism modes: `--no-sieve`, `--sieve-extensions`, `--snooze-extension`,
  `--no-size-upload`, and `--without-primary-accounts`. (`56085ba6`)
- Shared account mode and collection-level share rights checks for
  `Calendar`/`AddressBook` methods. (`bfc68a06`)
- Shared-mail rights checks for `Email` methods. (`54a33b97`)
- `ContactCard` group member support (`contacts/create`, `addressbookIds`
  membership). (`ef282762`)
- Contact photos with a `data:` URI are now served back as a `blobId`, like a
  real server. (`9244847d`)

### Fixed
- `ContactCard/query` now honors a `uid` filter; previously every card
  matched regardless of the filter, so a group with more than one other
  member resolved every member to the same wrong id. (`8d2db27f`)
- `query_hides_limit` now also hides the limit of an `Email/query` page, not
  only contact and calendar event queries. (`f0aef04e`)
- Sieve statement checks no longer reject an unrecognized but
  well-formed statement. (`064dae1f`)
- `SieveScript/set` no longer rejects an unknown blob id outright, matching a
  real server's tolerance. (`98e18eab`)

## [0.5.0] - 2026-10-24

### Added
- Optional `parse` feature to gate `ContactCard/parse` and `CalendarEvent/parse`
  behavior.
- `jmap-mockd` support for machine-readable startup output with `--port 0`.
- `jmap-mockd --seed <path>` JSON fixture loading for accounts, mailboxes, emails,
  contacts, and calendar events.
- Grouped hostile mode controls through a public `Faults` API.
