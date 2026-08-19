<!--
SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
SPDX-License-Identifier: GPL-3.0-or-later
-->

# Design spike: JMAP Principals & Sharing (RFC 9670)

Status: **design spike only.** Nothing here is implemented. No crate changes
accompany this document — it exists so the maintainer can greenlight (or
re-shape) the work before any Rust is written. Every claim is anchored to a
`file:line` in this tree or to an RFC/draft section; where the spec is a moving
target or the premise in the brief turned out to be wrong, that is called out
rather than smoothed over.

## 1. Summary

RFC 9670 (*JMAP Sharing*) standardises the vocabulary for "someone other than
me": a **`Principal`** is a person / group / resource / room that a JMAP server
knows about, and RFC 9670 gives us the methods to look principals up
(`Principal/get`, `Principal/query`) and to be told when a share changes
(`ShareNotification`). On top of that shared vocabulary, two things we want fall
out:

- **Scheduling** — reading another party's free/busy so we can pick a slot they
  are free for. **This is _not_ in RFC 9670.** The method that does it,
  **`Principal/getAvailability`**, is defined in the *JMAP for Calendars* draft
  (draft-ietf-jmap-calendars §2.2), layered on RFC 9670's `Principal` object and
  gated by a per-principal `urn:ietf:params:jmap:calendars` capability. See §2.3
  — this correction matters for phasing, because the heart of the scheduling ask
  rides on a draft, not a published RFC.
- **Sharing** — granting another principal rights on a collection, and knowing
  what rights *we* hold on a collection someone shared with us. This is the
  `shareWith` / `myRights` pair. Again with a caveat (§2.4): `myRights` is
  standard on `Mailbox` (RFC 8621) but `shareWith` on `Mailbox` is **not**;
  `AddressBook` has both as published RFC 9610; `Calendar` has both only in the
  calendars draft.

So RFC 9670 is the common floor, and each ask stands on a different, partly
draft-status extension above it. The design below builds the floor once and then
lets the two asks proceed independently.

## 2. RFC 9670 primer (verified against datatracker, 2026-08)

### 2.1 The `Principal` object (RFC 9670 §2)

Properties, verified against the RFC text:

| property | type | notes |
| --- | --- | --- |
| `id` | `Id` | immutable, server-set |
| `type` | `String` | `individual` \| `group` \| `resource` \| `location` \| `other` |
| `name` | `String` | display name |
| `description` | `String\|null` | |
| `email` | `String\|null` | the address you'd resolve a person by |
| `timeZone` | `String\|null` | IANA name |
| `capabilities` | `String[Object]` | server-set; **per-principal** capability bag — this is where the calendars extension hangs `mayGetAvailability` (§2.3) |
| `accounts` | `Id[Account]\|null` | server-set; the accounts this principal exposes |

`Principal/get`, `Principal/changes`, `Principal/set`, `Principal/query`,
`Principal/queryChanges` are the standard RFC 8620 §5 shapes over this object.
For our asks only `Principal/get` and `Principal/query` are needed (resolve an
email/name → a principal id and its capability bag). `Principal/set` is how a
server that lets you edit principals would take edits; we do not need it.

### 2.2 `ShareNotification` (RFC 9670 §3)

A read-mostly inbox of "your access to X changed". Properties: `id`, `created`
(`UTCDate`), `changedBy` (an `Entity`: `name`, `email`, `principalId`),
`objectType`, `objectAccountId`, `objectId`, `oldRights`/`newRights`
(`String[Boolean]|null`), `name`. Methods are the standard `/get`, `/changes`,
`/set` (destroy-only in practice), `/query`, `/queryChanges`. Not on the
critical path for either ask; listed for completeness and revisited in §4.

### 2.3 `Principal/getAvailability` — **calendars draft, not RFC 9670**

The single most important correction in this spike. The brief assumed
`getAvailability` lived in RFC 9670; it does not. Verified two ways:

- A full-text search of RFC 9670 finds **no** occurrence of `availability`,
  `getAvailability`, `BusyPeriod`, `freeBusy`, or `free/busy`. Its section list
  is only: Introduction, Principals, ShareNotifications, Framework for Shared
  Data, Internationalization, Security, IANA, References.
- The method is defined in **draft-ietf-jmap-calendars §2.2** (this tree already
  pins that draft at version -27 for its calendar types — see
  `rust/crates/jmap-proto/src/calendars.rs:9`).

Verified shape (draft-27):

- **Request `Principal/getAvailability`** arguments: `accountId` (`Id`), `id`
  (`Id`, the principal), `utcStart` (`UTCDateTime`, inclusive), `utcEnd`
  (`UTCDateTime`, exclusive), `showDetails` (`Boolean`), `eventProperties`
  (`String[]|null`).
- **Response**: `list` — an array of **`BusyPeriod`** objects.
- **`BusyPeriod`**: `utcStart`, `utcEnd`, `busyStatus` (one of `confirmed`,
  `tentative`, `unavailable`), `event` (`CalendarEvent|null`, populated only when
  `showDetails` is true and the caller is allowed to see it).
- The server merges/splits overlapping periods, resolving conflicting
  `busyStatus` in the order `confirmed` > `unavailable` > `tentative`.
- Errors: `notFound` (no such principal, or caller not allowed), `tooLarge`
  (`utcStart`→`utcEnd` window wider than the server will compute).

Gating capability: a principal's `capabilities` map carries an entry keyed
`urn:ietf:params:jmap:calendars` whose object holds (draft-27)
`mayGetAvailability` (`Boolean`) among others (`accountId`, `mayShareWith`,
`calendarAddress`). **These per-principal capability property names are
draft-version-sensitive** — older drafts and some server docs spell adjacent
fields differently (`account`, `sendTo`). Treat the exact set as
verify-against-the-wire, not settled (§7).

### 2.4 The sharing surface — `shareWith` / `myRights` (and where it is *not*)

RFC 9670 §4 ("Framework for Shared Data") is a *framework*, not a per-type
schema: it describes the pattern — a shareable type gains a `shareWith` map
(principal id → a rights object) and a server-set `myRights` — and how
`ShareNotification`s arise from it. The concrete properties live in each data
type's own spec, and they are **not uniform**:

| collection | `myRights` | `shareWith` | rights object (verified field names) |
| --- | --- | --- | --- |
| `Mailbox` | **yes**, RFC 8621 §2 | **no standard property** | `MailboxRights`: `mayReadItems`, `mayAddItems`, `mayRemoveItems`, `maySetSeen`, `maySetKeywords`, `mayCreateChild`, `mayRename`, `mayDelete`, `maySubmit` |
| `AddressBook` | **yes**, RFC 9610 §2 | **yes**, RFC 9610 §2 | `AddressBookRights`: `mayRead`, `mayWrite`, `mayShare`, `mayDelete` |
| `Calendar` | **yes**, calendars draft §4 | **yes**, calendars draft §4 | `CalendarRights` (draft-27): `mayReadFreeBusy`, `mayReadItems`, `mayWriteAll`, `mayWriteOwn`, `mayUpdatePrivate`, `mayRSVP`, `mayShare`, `mayDelete` |

Two corrections to the brief here, both verified:

1. **RFC 8621 defines only `myRights` on `Mailbox`, not `shareWith`.** There is
   no standardised JMAP way to grant another principal rights on a mailbox. Mail
   *sharing* (the write side) is therefore effectively out of scope until/unless
   a future spec adds it; mail *permissions* (the read side, `myRights`) are
   available and stable.
2. The brief's `CalendarRights` guess included `mayAdmin`; the draft-27 field is
   `mayShare`. Use the draft's names.

### 2.5 Capability URNs (RFC 9670 §2)

- `urn:ietf:params:jmap:principals` — advertised in `capabilities` (server-wide)
  and in an account's `accountCapabilities`, where its object carries
  `currentUserPrincipalId` (`Id|null`): "which principal is *me* in this
  account".
- `urn:ietf:params:jmap:principals:owner` — an account-capability whose object
  carries `accountIdForPrincipal` (`Id`) and `principalId` (`Id`): it ties a
  data account back to the principal that owns it.

Plus, for scheduling, the per-principal `urn:ietf:params:jmap:calendars`
capability from §2.3 (distinct from the *account* capability of the same URN we
already advertise for calendar data).

## 3. Current state in this repo

We consume **none** of the above today. Concretely:

- **Capabilities.** `rust/crates/jmap-proto/src/session.rs:14-18` declares
  exactly five URN constants (`CORE`, `MAIL`, `SUBMISSION`, `CONTACTS`,
  `CALENDARS`). There is no `principals`, no `principals:owner`. The mock
  advertises the same closed set — `ACCOUNT_CAPABILITIES` at
  `rust/crates/jmap-mock/src/server.rs:872-877`, woven into the session document
  at `server.rs:950-958`.
- **`myRights` / `shareWith` are dropped on the floor.** Every collection struct
  has a `#[serde(flatten)] extra: BTreeMap<String, Value>` bag, so a server that
  sends `myRights`/`shareWith` round-trips it but nothing reads it:
  - `Mailbox` — `rust/crates/jmap-proto/src/mail.rs:41-42`
  - `AddressBook` — `rust/crates/jmap-proto/src/contacts.rs:33-34`
  - `Calendar` — `rust/crates/jmap-proto/src/calendars.rs:36-37`
  The bytes survive; the meaning is invisible to every layer above.
- **Read-only is an account-wide heuristic.** The only permission signal we act
  on is RFC 8620 §1.6.2's account-level `isReadOnly`. It is read in
  `rust/crates/jmap-collection-sync/src/layout.rs:141-149` (`fn service` →
  `ServiceAccount.read_only`, doc at `layout.rs:64-65`: "the whole data set is
  read-only, not one collection in it"), then stamped onto every child in
  `rust/crates/jmap-collection-sync/src/children.rs:173`
  (`read_only: account.read_only`). The code already knows this is a stopgap —
  `children.rs:93-97` says per-collection `myRights` "is a second, finer question
  that is not read yet, so a writable-account child is not thereby known to be
  writable." That comment is the exact seam this work fills.
- **No principal/availability plumbing anywhere.** The `using`-set builder and
  method dispatch have no arm for any of it:
  - client `using` sets are per-domain constants, e.g.
    `rust/crates/jmap-client/src/calendars.rs:18`
    (`const USING: &[&str] = &[CAPABILITY_CORE, CAPABILITY_CALENDARS]`), and the
    per-type capability switch in
    `rust/crates/jmap-client/src/changes.rs:27-36` knows only Mail/Contacts/
    Calendars.
  - the mock method registry `handle_method` at
    `rust/crates/jmap-mock/src/dispatch.rs:229-277` has no `Principal/*`.
  - the calendar backend's `ECalMetaBackend` subclass
    (`rust/crates/jmap-backend-cal/src/backend.rs`, "seven vfunc slots",
    doc lines 4-48) has no free/busy vfunc and nothing that could answer
    Evolution's meeting-scheduler free/busy query.

## 4. Proposed design

The two asks share a floor (a `Principal` type + `Principal/get`/`query` +
capability plumbing). Build the floor once, then each ask is a thin, mostly
independent slice. File names below are the ones that change.

### 4.1 `jmap-proto` — new types and fields

- **New module `principals.rs`** (add `#[cfg(feature = "principals")] pub mod
  principals;` to `rust/crates/jmap-proto/src/lib.rs:25-37`, mirroring how
  `calendars`/`contacts` are feature-gated). Contents:
  - `struct Principal { id, type, name, description, email, time_zone,
    capabilities: BTreeMap<String, Value>, accounts, #[serde(flatten)] extra }`
    — `capabilities` stays a `Value` bag on purpose: it is where the calendars
    per-principal capability lives, and one server's unknown per-principal
    capability must not fail the whole `Principal/get` (same "one bad row must
    not sink the response" rule the calendar/contact types already follow, e.g.
    `calendars.rs:118-123`).
  - `struct ShareNotification { … }` + the `Entity` sub-struct (defer until §5
    Phase C; listed for shape).
- **New capability constants** in `session.rs` next to the existing five
  (`session.rs:14-18`): `CAPABILITY_PRINCIPALS =
  "urn:ietf:params:jmap:principals"`, `CAPABILITY_PRINCIPALS_OWNER =
  "urn:ietf:params:jmap:principals:owner"`. The per-principal calendars
  capability reuses the existing `CAPABILITY_CALENDARS` string but is read out of
  the *principal's* `capabilities` map, not the account's.
- **`getAvailability` request/response types** (in `principals.rs`, since the
  method is `Principal/getAvailability` even though it is spec'd in the calendars
  draft): `GetAvailabilityRequest { account_id, id, utc_start, utc_end,
  show_details, event_properties }` and a response `{ list: Vec<BusyPeriod> }`
  with `struct BusyPeriod { utc_start, utc_end, busy_status, event:
  Option<CalendarEvent> }`. These are bespoke shapes, not the generic
  `GetRequest`/`GetResponse` in `methods.rs:20-61`, because the argument set is
  its own (mirrors how `EmailImportRequest` is bespoke in `mail.rs:187-233`).
- **Typed rights on the three collections.** Add optional, server-set fields so
  the meaning stops living in `extra`:
  - `Mailbox.my_rights: Option<MailboxRights>` (`mail.rs`), `MailboxRights` per
    §2.4.
  - `AddressBook { my_rights: Option<AddressBookRights>, share_with:
    Option<BTreeMap<Id, AddressBookRights>> }` (`contacts.rs`).
  - `Calendar { my_rights: Option<CalendarRights>, share_with:
    Option<BTreeMap<Id, CalendarRights>> }` (`calendars.rs`).
  Keep `#[serde(flatten)] extra` as the catch-all so unmodeled/renamed rights
  fields still survive.

### 4.2 `jmap-client` — new methods

- **`principals.rs`** (new): `principals(account_id)` → `Principal/get` with
  `ids: null`; `principal_query(account_id, filter)` → `Principal/query` (filter
  by name/email/text to resolve a person). `using` set `&[CAPABILITY_CORE,
  CAPABILITY_PRINCIPALS]`, built exactly like `calendars.rs:18-27` via
  `single_call` (`client.rs:255-269`).
- **`get_availability(account_id, principal_id, start, end, show_details)`**
  (put in the new `principals.rs`, or extend `calendars.rs`). `using` set must
  name **both** `CAPABILITY_PRINCIPALS` and `CAPABILITY_CALENDARS` — the object
  is a principal but the method is a calendars-draft extension. Returns
  `Vec<BusyPeriod>`.
- Optional: teach the per-type capability switch in `changes.rs:27-36` about
  `Principal` if we ever sync principals; not needed for the two asks.

### 4.3 `jmap-mock` — server support (so tests don't need Stalwart)

- Advertise the two new URNs: extend `ACCOUNT_CAPABILITIES`
  (`server.rs:872-877`) and the server `capabilities` map (`server.rs:950-958`),
  plus a `currentUserPrincipalId` in the `principals` account-capability object.
  Follow the existing `without_capability` builder switch
  (`server.rs:123-126`) so a test can model a server that lacks principals.
- New `principals.rs` handler module + registry arms in `handle_method`
  (`dispatch.rs:229-277`): `Principal/get`, `Principal/query`, and
  `Principal/getAvailability`. `getAvailability` can compute `BusyPeriod`s from
  the account's seeded `CalendarEvent`s (reuse the query-time-window logic in
  `calendars.rs:159-195`), which gives deterministic slot-picking tests.
- Seed a couple of `Principal`s in `state.rs` (a person with `mayGetAvailability`
  true, one with it false → `notFound`) and let collection `/get` echo a
  `myRights`/`shareWith` so the backend mapping (§4.4) has something to read.

### 4.4 Backend wiring

- **Per-source permissions replace the account-wide heuristic.** The seam is
  `children.rs:173`. Today `Child.read_only` is `account.read_only`; with typed
  `myRights` available on each collection, `jmap-collection-sync` can compute a
  per-child value: e.g. an address book with `myRights.mayWrite == false` is
  read-only even inside a writable account. This narrows, never widens: absent
  `myRights`, fall back to the account bit exactly as now, so servers that don't
  send rights behave identically. `ServiceAccount`/`Child` and the `fn service`
  derivation (`layout.rs:141-149`, `children.rs:161-175`) are the files that
  change; the doc comment at `children.rs:93-97` gets to come true.
- **A free/busy hook for slot-picking.** Evolution's meeting scheduler asks a
  calendar backend for free/busy via the `ECalBackend` `get_free_busy` vfunc
  (an `ECalBackendSync::get_free_busy_sync`), which is *not* one of the seven
  `ECalMetaBackend` slots the backend currently installs
  (`backend.rs:4-48`). The wiring: add that vfunc slot in
  `jmap-backend-cal/src/backend.rs`, a body in
  `jmap-backend-cal/src/ops.rs` that calls the new client
  `get_availability`, and a `BusyPeriod → VFREEBUSY` marshaller (natural home:
  `jmap-ical`, beside the existing iCalendar mapping, invoked from
  `jmap-cal-sync`). Resolving the attendee address → principal id uses
  `Principal/query`. This is the concrete path from "user typed an attendee in
  the meeting editor" to "server told us their busy blocks".

## 5. Phasing + recommendation

**Phase 0 — the shared floor (do first regardless).** `Principal` proto type +
the two capability constants + `Principal/get`/`query` in client + mock support
+ advertise the URNs. Small, pure-additive, unblocks both asks. Nothing
user-visible yet.

Then the brief's two candidate orders:

- **Path A — availability / free-busy first.** Adds `Principal/getAvailability`,
  `BusyPeriod`, the client method, the mock computation, and the calendar
  free/busy vfunc. Delivers the **scheduling** ask end-to-end. Read-only (we only
  *read* others' busy state), so blast radius is small and there is no destructive
  edge. Risk concentrated in one external unknown: does our Stalwart actually
  implement `Principal/getAvailability`, and at which draft field spelling (§7)?
- **Path B — ACLs / `myRights` first.** Adds typed rights on the three
  collections and rewires `children.rs` to per-source read-only. Delivers
  **correct per-source permissions** — the gap the code already flags
  (`children.rs:93-97`). Mostly published-RFC surface (AddressBook RFC 9610,
  Mailbox RFC 8621); Calendar rights are draft. Does not by itself deliver either
  headline ask — `shareWith` (the write side of *sharing*) is a further,
  higher-risk step.

**Recommendation: Phase 0, then Path A, then Path B, then (only if wanted)
`shareWith`/`ShareNotification` as Phase C.**

Reasoning: the maintainer's ask #1 is scheduling, and Path A is the most direct,
lowest-blast-radius answer to it — read-only, self-contained, and it makes the
meeting editor genuinely more useful. Its one real risk (Stalwart + draft
spelling) is cheap to retire up front, so **the first task inside Path A is a
half-day spike that fires `Principal/getAvailability` at the throwaway Stalwart
and records what comes back**; if Stalwart doesn't implement it, we learn that
before building the backend vfunc and can reorder to B without waste. Path B is
the right *second* step because it turns a known-wrong heuristic into a correct
one on mostly-stable spec surface, and it reuses Phase 0's principal plumbing.
Actual `shareWith` writing is deliberately last: it is the least
spec-stable (draft for calendars, absent for mail) and the only destructive
surface, so it should wait until the read sides have proven the types against a
real server.

## 6. Effort estimate

Rough size per component (S ≈ ≤1 day, M ≈ 2–4 days, L ≈ ≥1 week), assuming the
existing test-first rhythm and the mock carrying its weight.

| Component | Size | Notes |
| --- | --- | --- |
| Phase 0: `Principal` proto + capability consts | **S** | pure additive types, mirrors existing modules |
| Phase 0: `Principal/get`/`query` client + mock | **M** | new client module + mock handlers + seeds |
| Path A: `getAvailability` proto + client | **S–M** | bespoke request/response; field spelling is the risk, not the code |
| Path A: mock `getAvailability` computation | **M** | reuses calendar time-window logic |
| Path A: free/busy vfunc + `VFREEBUSY` marshal + backend wiring | **L** ⚠ | new `ECalBackend` vfunc slot in unsafe FFI; only testable against a live/mock EDS (`rust-test-eds`), which is where the escalation risk sits |
| Path B: typed rights on 3 collections | **S** | 3 structs + fields, `extra` stays |
| Path B: per-source read-only rewire (`children.rs`/`layout.rs`) | **M** | logic + tests for narrow-not-widen fallback |
| Phase C: `shareWith` write + `ShareNotification` | **L** | destructive surface, draft-status, UI questions |

**Escalation-worthy:** the free/busy backend vfunc (unsafe FFI + EDS-only
testing, like every `jmap-backend-*` slot), and anything under Phase C (write
path, draft volatility, and Evolution UI for a share dialog that does not exist
yet). Everything in Phase 0 + Path A above the vfunc is ordinary additive work.

## 7. Risks / unknowns

- **Stalwart reality check.** Our real-server testing is a throwaway Stalwart
  reached over a plaintext localhost forward (see the memory note / the
  `rebase_urls_to_origin` escape hatch in `jmap-client/src/client.rs:110-113` and
  the redirect/plaintext gotchas already documented). We do **not** yet know
  whether that Stalwart advertises `urn:ietf:params:jmap:principals`, populates
  `currentUserPrincipalId`, or answers `Principal/getAvailability` at all — and
  if it does, at which draft field spelling. **These need real-server
  verification before Path A's backend work is committed to.** The mock lets us
  build and test the whole path deterministically, but the mock cannot tell us
  what Stalwart actually does.
- **Draft volatility.** `Principal/getAvailability`, the per-principal
  `urn:ietf:params:jmap:calendars` capability fields, and `CalendarRights` all
  live in draft-ietf-jmap-calendars (this tree pins -27,
  `calendars.rs:9`). Field names have moved between drafts (§2.3's
  `mayGetAvailability` vs. older `account`/`sendTo`), so the proto types for
  these should be treated as version-tracking and kept tolerant (`extra` bag,
  `Option` everywhere) rather than asserted. `AddressBook` sharing (RFC 9610) and
  `Mailbox` `myRights` (RFC 8621) are published and stable.
- **The brief's two premises were slightly off** and are corrected here so the
  estimate is honest: (a) `getAvailability` is a calendars-draft method, not RFC
  9670 (§2.3); (b) `Mailbox` has `myRights` but no standard `shareWith` (§2.4),
  so mail *sharing* is not a thing we can implement against the current specs.
- **Interaction with the deferred OAuth work — does NOT block this.** The
  in-flight OAuth2 discovery/registration (M7) is orthogonal: principals and
  availability are ordinary JMAP methods that ride on whatever credentials the
  client already holds (Basic against Stalwart today, Bearer later). No part of
  this design needs OAuth to land first, and nothing here changes the auth
  surface.

## 8. Open questions for the maintainer

1. **Scheduling depth.** Is the ask just "show an attendee's busy blocks in the
   meeting editor" (free/busy view, Path A as scoped), or full iTIP-style
   invitation scheduling (send invites, collect RSVPs)? The latter is a much
   larger, separate body of work; this spike scopes only the former.
2. **Draft appetite.** Are we comfortable shipping the scheduling path on a
   draft method (`getAvailability`), given we already ship draft-based calendar
   types? Or should Path B (mostly published RFCs) go first to keep the
   draft-status surface minimal until the calendars draft becomes an RFC?
3. **Sharing UI.** Phase C (`shareWith`) needs an Evolution-side "share this
   calendar/address book with …" dialog that does not exist today. Is that in
   scope at all, or is the sharing ask satisfied by *consuming* shares others
   grant us (read `myRights`, show shared collections) without us granting any?
4. **Stalwart spike priority.** Shall the first concrete step be the half-day
   Stalwart probe (does it answer `Principal/getAvailability`, and how) before
   any code, so phasing can react to what the real server actually supports?
5. **Mailbox permissions.** Do we want `Mailbox.myRights` wired into per-source
   read-only too (Path B), even though mail has no `shareWith` to complement it,
   or leave mail on the account-wide heuristic and apply per-source rights only
   to calendars/address books?
