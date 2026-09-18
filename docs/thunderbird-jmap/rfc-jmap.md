Draft RFC for docs/engineering/rfcs/ in thunderbird/thunderbird-android.
Status: DRAFT, not filed. The operator files it; agents never post to GitHub.
Last updated: 2026-09-15. Format follows docs/engineering/README.md's RFC
template as of that date; adjust to match if the template has since changed.

---

# RFC: JMAP as a third mail protocol

- Issue: thunderbird/thunderbird-android#3272
- Milestone: https://github.com/thunderbird/thunderbird-android/milestone/26
- Status: Draft

## Summary

Finish and wire up `backend/jmap` (RFC 8620/8621) as an installable, opt-in
account type, reusing the `developmentBackends` / feature-flag pattern
already used for the demo backend, with no changes to the account storage
schema in the first phase.

## Motivation

JMAP support has been requested since 2018 (#3272) and partially built in
2019/2020 under Prototype Fund funding (#4459 and follow-ups). The module
was left in the tree, unwired, when the standalone `k9mail-jmap` app was
removed (#5872). "JMAP Support Exploration" now appears on the published
2026 Android roadmap. This RFC proposes finishing it incrementally.

## Current state (verified against `main` at <fill in commit>)

`backend/jmap` compiles against the current `Backend` interface; its tests
pass. 9 of 19 `Backend` methods are implemented for real (folder list,
message sync, flags, move, delete, upload); the rest throw
`UnsupportedOperationException` (download, search, send, push) or are
no-ops (`findByMessageId`). One correctness issue: `deleteMessages` performs
a hard `Email/set destroy`, removing the message from every mailbox it
belongs to rather than just the folder being synced.

## Design

### Account model (no schema change)

A JMAP account is a `ServerSettings` with `type = "jmap"`, `host` = the
session resource's host, `port = 443`, `connectionSecurity =
SSL_TLS_REQUIRED`, `authenticationType` PLAIN (HTTP Basic) or XOAUTH2
(Bearer), and `extra["sessionUrl"]` holding the full session URL. The same
`ServerSettings` value is stored as both `incomingServerSettings` and
`outgoingServerSettings`, following the existing `DemoServerSettings`
precedent, so `LegacyAccountDto.outgoingServerSettings`'s non-null
invariant holds unchanged and sending is entirely `Backend.sendMessage`
(`EmailSubmission/set`), never SMTP.

### Gating

Two independent, already-precedented gates:
- Runtime: a `JmapBackendFactory` registered only in the `developmentBackends`
  Koin qualifier for debug and daily build types, same as `DemoBackendFactory`.
- UI: a feature flag (`config/featureflag/`) filtering `IncomingProtocolType`
  in the account-setup protocol picker, so the enum addition is inert in
  release builds until the flag flips.

### Library

`rs.ltt.jmap:jmap-client`, currently pinned at 0.3.1 (dependabot's bump PR
was closed with "ignore this dependency"). This RFC proposes bumping to
0.9.0, which is required for EventSource push and Bearer auth and adds a
`ConnectionConfig` hook for the app's trust manager. Cost: 0.9.0 changes
Guava from a transitive to a `provided` dependency, so it must be declared
directly; Gson also stays. If the resulting APK size or ProGuard reflection
rules (#4569) are unacceptable, the fallback is an in-house OkHttp +
kotlinx-serialization or Moshi client scoped to the ~15 method calls this
module actually uses; the existing MockWebServer JSON fixtures are
protocol-level and would carry over unchanged either way. Asking for a
decision on this trade-off is one purpose of this RFC.

### Phasing

See the linked technical design (or the phased roadmap this RFC's author
can share) for the full breakdown: library bump and hygiene, account setup
and manual server entry, send, on-demand download and search, push,
folder management, autodiscovery (`.well-known/jmap` and DNS SRV
`_jmap._tcp`), OAuth/Bearer, and, as a deferred and separately justified
item, storing an email's membership in more than one mailbox (#4570),
which needs a schema change shared with the Gmail-labels ask (#752).

## Alternatives considered

- **SMTP outgoing alongside JMAP incoming**, to get sending sooner. Rejected
  as the persisted model: it would need two credential sets from the user
  for one account and duplicate the outgoing-settings screen for a protocol
  that does not need one. Kept only as an internal, temporary fallback
  inside the backend factory until `EmailSubmission/set` lands.
- **Skip the `jmap-client` library entirely from the start.** More initial
  work with no immediate benefit while the library's method coverage
  already matches what phase 0-3 need; revisit if 0.9.0 turns out
  unacceptable in practice.

## Open questions for maintainers

- Is the `jmap-client` dependency, at 0.9.0, acceptable, or should this
  start from an in-house client instead?
- Should JMAP land in Thunderbird for Android, K-9 Mail, or both, first?
- Is #4570 (multi-mailbox membership) a hard prerequisite for taking JMAP
  out of the feature flag, as the original 2020 milestone implied, or can
  it ship with the documented single-row-per-membership limitation?
