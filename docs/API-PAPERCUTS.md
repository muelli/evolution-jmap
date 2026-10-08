<!--
SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
SPDX-License-Identifier: GPL-3.0-or-later
-->

# API Paper Cuts: External Consumer Audit (Batch 28 Item 1)

This audit documents usability friction, missing re-exports, ergonomic gaps,
and documentation omissions discovered when consuming `evolution-jmap-client`
and `evolution-jmap-proto` from packaged `.crate` tarballs outside the
repository workspace.

## Audit Methodology

1. Packaged both `evolution-jmap-proto` and `evolution-jmap-client` via `cargo package`.
2. Extracted the resulting `.crate` archives into a scratch directory outside
   the repository workspace.
3. Created an isolated consumer binary crate (`jmap-consumer`) depending strictly
   on the extracted packages by path (with a Cargo `[patch.crates-io]` mapping
   for the unpublished proto crate).
4. Implemented five standard client operations using only public APIs and README
   guidance:
   - Connect and open a session.
   - List mailboxes.
   - Create and destroy a mailbox.
   - Upload an RFC 822 blob, import an email, and read it back via `email_get`.
   - Open an EventSource push stream and receive a state change.
5. Executed the consumer binary against two target backends:
   - Local mock server: `jmap-mockd` (ephemeral port, basic auth `alice:secret`).
   - Live server: Stalwart v1.0.0 (`stalwart-runner`, admin account).

Both test runs completed with 100% success. Every friction point encountered
during implementation is catalogued below with an actionable verdict:
- **fix now**: Non-breaking, low-risk ergonomic improvements to implement in Item 2.
- **fix at next breaking release**: Desirable improvements that would alter public
  method signatures or trait definitions.
- **accept**: Inherent properties of the underlying RFC specifications or Cargo
  packaging semantics.

---

## Catalogued Paper Cuts

### Paper Cut 1: Missing re-export of `evolution-jmap-proto` in `evolution-jmap-client`
- **Category:** Missing re-export / module organization
- **Location:** `rust/crates/jmap-client/src/lib.rs`
- **Observation:** `Client` methods take and return domain types from `evolution-jmap-proto`:
  - `primary_account(&self, capability: &str) -> Result<jmap_proto::Id, Error>`
  - `mailbox_get(&self, account_id: &Id) -> Result<GetResponse<Mailbox>, Error>`
  - `mailbox_create(&self, account_id: &Id, mailbox: &Mailbox) -> Result<Mailbox, Error>`
  - `email_import(&self, account_id: &Id, email: &EmailImport) -> Result<Email, Error>`
  - `email_get(&self, account_id: &Id, ids: &[Id], ...) -> Result<Vec<Email>, Error>`
  However, `evolution-jmap-client` does not re-export `evolution_jmap_proto` or its
  domain types. A consumer adding only `evolution-jmap-client` to `Cargo.toml`
  cannot write signatures or declare variables for these types without also
  discovering and adding a direct dependency on `evolution-jmap-proto`.
- **Verdict:** **fix now**. Add `pub use evolution_jmap_proto as proto;` and
  re-export foundational primitives `Id` and `State` at the root of `jmap-client`.

### Paper Cut 2: `EventSource` push stream requires high-friction manual boilerplate
- **Category:** Ergonomics / missing convenience method
- **Location:** `rust/crates/jmap-client/src/eventsource.rs` and `client.rs`
- **Observation:** Setting up an EventSource subscription requires six separate steps:
  1. Reading `client.session().event_source_url`.
  2. Calling `jmap_client::eventsource::expand_url(&template, types, close_after, ping)`.
  3. Reading `client.authorization_header()`.
  4. Mapping `Option<String>` to `vec![("Authorization".to_string(), auth)]`.
  5. Wrapping the headers in `jmap_client::eventsource::SharedHeaders::new(headers)`.
  6. Instantiating `CancelFlag::new()` and calling `EventSourceSubscription::start(...)`.
  `Client` holds both the session document and the authorization header, yet provides
  no helper method to spawn an EventSource subscription.
- **Verdict:** **fix now**. Add `client.event_source(&self, types: &[&str]) -> Result<EventSourceSubscription, Error>`
  and `client.event_source_with_timeouts(...)` on `Client`.

### Paper Cut 3: Incomplete re-exports for `eventsource` submodule types at crate root
- **Category:** Missing re-export
- **Location:** `rust/crates/jmap-client/src/lib.rs`
- **Observation:** `EventSourceSubscription`, `EventSourceItem`, and `EventSourceTimeouts`
  are re-exported at `jmap_client::*`. However, `SharedHeaders` and `expand_url`
  are only available through the sub-module path `jmap_client::eventsource::*`.
  A caller constructing a subscription manually must mix root imports and
  submodule imports.
- **Verdict:** **fix now**. Re-export `SharedHeaders` and `expand_url` in `jmap_client::*`.

### Paper Cut 4: `Client::upload_blob` requires owned `Vec<u8>` instead of borrowed slice
- **Category:** Needless conversion / allocation
- **Location:** `rust/crates/jmap-client/src/mail.rs:1025`
- **Observation:** `Client::upload_blob(&self, account_id: &Id, content_type: &str, data: Vec<u8>)`
  requires `data` by value. Callers with borrowed slices (`&[u8]`) or static payloads
  (such as `b"..."` RFC 822 messages) cannot pass them directly and must allocate
  an unnecessary `Vec<u8>` via `.to_vec()`.
- **Verdict:** **fix at next breaking release**. Changing `Vec<u8>` to `&[u8]`
  or `impl Into<Vec<u8>>` changes the public method signature on `Client`.
  In the meantime, a non-breaking helper `upload_blob_slice(&self, account_id: &Id, content_type: &str, data: &[u8])`
  can be added.

### Paper Cut 5: `ClientBuilder::connect` does not default to `rebase_urls_from_env()`
- **Category:** Ergonomics / configuration divergence
- **Location:** `rust/crates/jmap-client/src/client.rs:89,263`
- **Observation:** `Client::connect(origin, creds)` automatically honors
  `JMAP_LIVE_SERVER_REBASE_URLS` via `rebase_urls_from_env()`. However, `ClientBuilder::default()`
  initializes `rebase_urls_to_origin` to `false`. When a developer uses the builder
  pattern as recommended in the README (`Client::builder().timeout(...).connect(...)`),
  the environment variable is silently ignored unless `.rebase_urls_to_origin(rebase_urls_from_env())`
  is explicitly chained. On test deployments advertising internal DNS names (such
  as Stalwart's `https://mail.example.internal`), calls fail with connection refused.
- **Verdict:** **fix now**. Initialize `ClientBuilder::default().rebase_urls_to_origin`
  with `rebase_urls_from_env()` so builder usage matches `Client::connect`.

### Paper Cut 6: README lacks working examples for mailbox and email workflows
- **Category:** Documentation gap
- **Location:** `rust/crates/jmap-client/README.md`
- **Observation:** The README minimal working example only illustrates connecting
  with a mock transport and inspecting session username and primary accounts.
  It provides no guidance on method names or patterns for listing mailboxes,
  importing messages, or handling push streams. An outsider is forced to inspect
  the crate source code or rustdoc to find methods like `mailbox_get` or `email_import`.
- **Verdict:** **fix now**. Add concise code snippets in `README.md` showing
  mailbox listing and message retrieval.

### Paper Cut 7: `cargo package` checks registry for local sibling dependency
- **Category:** Packaging / Cargo semantics
- **Location:** `rust/crates/jmap-client/Cargo.toml`
- **Observation:** `jmap-client` specifies `evolution-jmap-proto = { path = "../jmap-proto", version = "0.4.1" }`.
  When packaging `jmap-client`, Cargo strips the `path` and checks whether
  `evolution-jmap-proto 0.4.1` exists in the crates.io index. Because it is not
  yet published, packaging fails unless `--config 'patch.crates-io.evolution-jmap-proto.path="rust/crates/jmap-proto"'`
  is supplied.
- **Verdict:** **accept**. This is standard Cargo behavior when packaging dependent
  crates before publishing the underlying library. Documented in `docs/PUBLISHING.md`.

### Paper Cut 8: `Email/import` requires a pre-existing `blobId`
- **Category:** Protocol design
- **Location:** `rust/crates/jmap-proto/src/mail.rs:655`
- **Observation:** `EmailImport::new(blob_id, mailbox_id)` requires an uploaded
  blob ID. A consumer cannot directly import raw message bytes in one step without
  first making an `upload_blob` round trip.
- **Verdict:** **accept**. This matches RFC 8621 Section 4.8 specification rules
  where message content is staged as a blob before import.

### Paper Cut 9: `Email.id` is optional on fetched and imported objects
- **Category:** Data model typing
- **Location:** `rust/crates/jmap-proto/src/mail.rs`
- **Observation:** `Email.id` is typed as `Option<Id>`. Even immediately after
  an import or get call where the ID is guaranteed to exist by the server,
  callers must handle or unwrap the option (`imported.id.as_ref().unwrap()`).
- **Verdict:** **accept**. In JMAP RFC 8621, `Email` represents both server objects
  and draft creation payloads where `id` may not yet be assigned. Unifying on one
  type is standard idiomatic design.
