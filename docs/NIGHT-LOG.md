# Night log

Running record of the autonomous work sessions: what was done, what was
decided and why, what is blocked. Newest entries at the bottom.

## 2026-08-08

Milestones M1 (`eds-sys`) and M2 (`jmap-backend-core`) are done.

**M1 — found already written but uncommitted** in the working tree from an
earlier session that ran out of budget before pushing. Verified rather than
assumed: `cargo test -p eds-sys` (5 tests), `cargo clippy -p eds-sys
--all-targets -- -D warnings`, `cargo build --workspace`, `cargo fmt --check`
all clean, then committed as-is. The load-bearing part is
`tests/layout.rs`, which cross-checks every instance/class struct size against
`g_type_query()` — bindgen output that disagrees with the runtime would
misplace every vfunc slot silently.

**M2 — implemented this session**, test-first. New rlib
`rust/crates/jmap-backend-core` with four modules and 22 tests:

- `subclass` — `ObjectSubclass` trait plus `register_static` /
  `register_dynamic`, the hand-written `G_DEFINE_TYPE` equivalent.
- `trampoline` — `guard` / `guard_bool` / `guard_ptr` `catch_unwind` wrappers.
- `cancel` — `CancelBridge`, `GCancellable` → `CancelFlag`.
- `error` — `jmap_client::Error` → `GError`.

Decisions taken:

- **Error mapping targets `E_CLIENT_ERROR`, not a private domain.** Evolution
  branches on domain and code, so the mapping is behaviour, not logging:
  401 → `AUTHENTICATION_FAILED` (drives the credentials prompt), 403 →
  `PERMISSION_DENIED`, transport failure → `REPOSITORY_OFFLINE` (a meta
  backend then serves its cache instead of showing an empty address book),
  `Error::Cancelled` → `G_IO_ERROR_CANCELLED` (EDS suppresses the alert).
  Everything else is `OTHER_ERROR` carrying the client error's `Display`
  text. This needed `EClientError` and `e_client_error_*` added to the
  eds-sys allowlist — deliberately just the enum and its constructors, not
  the `EClient` class, which backends never talk to. A private
  `evolution-jmap-backend` domain remains, used only for caught panics, so a
  bug in our code is distinguishable in a log from a misbehaving server.
- **Registration is idempotent, guarded by a mutex.** A second
  `g_type_register_static` under the same name is a fatal GLib error, EDS
  module entry points can be reached more than once per process, and
  check-then-register is not atomic on its own. There is a test for the
  double-registration case specifically.
- **Both registration flavours are provided.** M3+ register against the
  `GTypeModule` EDS loads the module as (so types go away on unload); tests
  and the Camel provider's own types register statically.
- **`catch_unwind` uses `AssertUnwindSafe`.** A vfunc body inevitably touches
  `&mut` state through raw pointers, so unwind safety is unprovable and the
  alternative would be not guarding at all. The guarantee actually needed is
  weaker: the C caller must not see a half-finished operation reported as
  success, which returning the failure value provides.
- **Panic text is logged via `g_log` with a `"%s"` format**, not as the format
  itself — a server-supplied string containing `%` would otherwise be read as
  a printf directive.
- **`jmap-backend-core` stays out of `default-members`.** It depends on
  `eds-sys` and hence on the EDS headers. `cmake/Rust.cmake`'s `rust-test-eds`
  target now runs `-p eds-sys -p jmap-backend-core`, which is the only place
  those tests get exercised.

No blockers hit.

Not verified locally: `reuse lint` (the tool is not installed on this VM and
the container image needs docker socket access the session does not have) and
`cargo deny check` (`cargo-deny` not installed). Both run in CI. All new files
carry an SPDX `GPL-3.0-or-later` header, and every license the new
`bindgen`/`system-deps` dependency tree pulls in (MIT, Apache-2.0, ISC,
BSD-3-Clause, Apache-2.0 WITH LLVM-exception) is already on `deny.toml`'s
allowlist, so both are expected to pass.

Next: M3, the `EBookMetaBackend` subclass. Suggested first increment is the
JSContact ↔ `EContact` mapping against fixtures — pure data, no GObject
lifecycle, and it is what the vfuncs will be plumbing.

## 2026-08-08 (second session)

M3, first increment: the JSContact ↔ vCard mapping, as the new crate
`rust/crates/jmap-vcard` (`evolution-jmap-vcard`, lib `jmap_vcard`). 27 tests
in three files — `syntax` (11), `mapping` (15), `server_roundtrip` (1).
Workspace total is now 69.

Decisions taken:

- **vCard text, not `EContact`, is the boundary.** The obvious reading of the
  milestone is JSContact ↔ `EContact`, but `EContact` *is* a vCard —
  `e_contact_new_from_vcard()` / `e_vcard_to_string()` are the only two calls
  the backend needs. Mapping to text instead of to the GObject keeps the whole
  translation free of GLib and the EDS headers, so the crate goes **into**
  `default-members` and its tests run on any machine, unlike everything else
  M1–M3 has produced. The backend will do the one-line `EContact` conversion.
- **vCard 3.0.** Not a preference: `EVCardFormat` has exactly one member,
  `EVC_FORMAT_VCARD_30`, so 4.0 output would be reparsed as 3.0 anyway.
- **The vCard `UID` is the JMAP `id`, not the JSContact `uid`.** EDS keys its
  cache on the `UID` and hands it straight back to `load_contact_sync()` and
  `remove_contact_sync()`, so it has to be the identifier `ContactCard/get`
  and `/set` take. The JSContact `uid` is a different namespace and rides
  along in `X-JMAP-UID`. Consequence the backend must respect: a vCard coming
  *from* Evolution has a locally invented `UID` (`pas-id-…`) that is not a
  JMAP id, so `vcard_to_card` fills `id` from it unconditionally and the
  caller drops it before a create. There is a test naming that case.
- **Map keys round-trip through `X-JMAP-KEY`.** `emails`/`phones` are keyed
  maps and a JSContact PatchObject addresses entries by key
  (`emails/work/address`). Losing the key on the way through vCard would turn
  every edit into a remove-and-re-add — a new key server-side, and any
  property of that entry we do not model silently dropped. `EVCard` preserves
  unknown parameters, so the key survives the trip through `EContact`. Keys
  are only invented (`e1`, `p1`, …) for vCards that never had one.
- **Unmapped JSContact properties are dropped, deliberately.** The set is UID,
  FN, N, EMAIL, TEL; `organizations`, addresses, notes and the rest do not
  survive. That is only safe because saving goes back as a PatchObject naming
  the mapped properties — a property never mapped is a property never
  overwritten. `unmodeled_jscontact_properties_are_dropped_not_mangled` pins
  the expectation so the invariant is not quietly broken later.
- **`N` is never guessed from `FN`.** A vCard with only `FN` yields
  `name.full` and no components. Splitting on whitespace would be written
  back to the server on the next save, which makes a display heuristic into
  data corruption.
- **The syntax layer keeps values escaped** and exposes `text()` /
  `components()`. Structured values have to be split on their real separators
  *before* unescaping, or `N:Olden\;burg;Vera` gains a component.

On the TDD: tests were written first, but they only ever failed to *compile*,
which is a weak red. Verified they discriminate by mutating the implementation
— wrong `UID` source, dropped `X-JMAP-KEY`, missing unfold, disabled folding
all fail the suite. One mutation survived: an off-by-one in the fold limit
(the continuation's leading space counts against the 75 octets). The folding
test used only 2-octet characters, where that boundary happens to land in the
same place; it now runs an ASCII value too, and catches it.

Not verified locally, same as last session: `reuse lint` and `cargo deny`
(neither tool on this VM). All new files carry the SPDX header, the fixture is
covered by the existing `rust/crates/*/tests/fixtures/**` annotation, and the
crate adds no new external dependencies. `cargo clippy --all-targets -D
warnings` is clean on the default members, which is what CI runs;
`--workspace` also lints `example-module`, whose 26 findings pre-date this
work (verified against a clean tree) and belong to the Red Hat LGPL port.

Next: the `EBookMetaBackend` subclass itself — `connect_sync` /
`list_existing_sync` / `load_contact_sync` over the client, with
`jmap-backend-core`'s trampolines, in `jmap-backend-*` out of
`default-members`.

## 2026-08-08 (third session)

M3, second increment: `rust/crates/jmap-book-sync`
(`evolution-jmap-book-sync`, lib `jmap_book_sync`) — everything
`EBookMetaBackend` needs a JMAP address book to do, with none of the GObject
lifecycle. One entry point per vfunc: `list_existing`, `load_contact`,
`save_contact`, `remove_contact`, `get_changes`. 18 tests in two files
(`sync` 8, `save` 10) against a mock server seeded with *two* address books,
so "only this book" is observable rather than assumed. Workspace total is now
87.

Like `jmap-vcard`, it depends only on the client and the mapping, so it goes
**into** `default-members` and its tests run anywhere. What is left for the
subclass on top is lifecycle, credentials and marshalling — the parts that
genuinely need EDS.

Decisions taken:

- **The revision is an FNV-1a digest of the rendered vCard**, not JSContact's
  `updated`. RFC 9553 leaves `updated` optional, so a server that omits it
  would make every card look permanently unchanged. The digest is also the
  *better* token: it changes exactly when something EDS can see changes, so a
  server-side edit to a property this mapping drops does not churn every
  client's cache. FNV rather than `DefaultHasher` because revisions are
  persisted in the EDS cache and compared across restarts, and
  `DefaultHasher`'s output is explicitly unstable between Rust releases.
- **`get_changes` distinguishes created from updated**, which is what makes
  `ContactCard/changes` — an account-wide feed — usable for one book without
  consulting the local cache. A card that shows up as *updated* and is no
  longer in this book may have just been moved out, so it is reported
  removed; leaving it out would strand a contact in Evolution's view forever.
  A card that shows up as *created* and is not ours was never in this book,
  so it is ignored rather than reported as a removal EDS has no record of.
- **Saving is a read-modify-write PatchObject, and the merging is the point.**
  A vCard is a lossy view, so a save that sent the parsed card back whole
  would delete what it could not represent. That extends *inside* the mapped
  properties, which is the part that is easy to get wrong: `contexts` and
  `features` are merged so a context like `school` (no vCard `TYPE`) survives;
  `pref` keeps a rank the server already had, because vCard 3.0's flag can
  only introduce or remove a preference, never renumber one; unmapped
  `name.components` kinds are carried across the replacement. Entries are
  addressed by the `X-JMAP-KEY` the previous session preserved, so an edit
  stays an edit. A save that changes nothing sends no request at all, rather
  than bumping the server state and waking every other client.
- **A property absent server-side is written whole, not reached into.** RFC
  8620 §5.3 requires every path segment before the last to already exist. The
  mock creates intermediates on demand and would not have caught this.
- **`SyncError` keeps `jmap_client::Error` intact** rather than flattening it
  to a string, so `jmap-backend-core`'s `E_CLIENT_ERROR` mapping still has
  something to branch on. `is_cannot_calculate_changes()` is a predicate
  rather than a string match at the call site: it is not really an error, it
  is the signal to fall back to a full listing. Added
  `error::method::CANNOT_CALCULATE_CHANGES` to `jmap-proto` for it.
- **The mock now rejects a create that supplies `id`.** It previously
  overwrote it silently, which meant nothing could detect the exact mistake
  this code has to avoid — sending a vCard `UID` Evolution invented locally
  (`pas-id-…`) as a JMAP id. RFC 8620 §5.3 makes `id` server-set.
- **`jmap-vcard` grew three predicates** (`maps_name_component`,
  `maps_context`, `maps_phone_feature`). The patch builder needs to know
  exactly which JSContact fields a vCard can carry, and that knowledge belongs
  next to the tables that answer for it, not duplicated in a second crate.

On the TDD: `patch::diff` was a `todo!()` stub while the tests were written,
so the seven save tests failed at runtime for the right reason. The read-path
tests were a weaker red — two of the eight failed, but both for reasons worth
having found: one asserted a destruction that a card created *and* destroyed
inside the same window correctly appears in neither list, and the other
exposed the created/updated question above, which is a design decision the
test made me take rather than assume.

Eight mutations were then run against the implementation to check the suite
discriminates. Six failed immediately; two survived and both were real gaps,
now closed: dropping the unmapped-name-component filter left the card with two
surnames and no test noticed (the assertion checked membership, not the whole
list), and removing `card.id = None` before a create changed nothing because
the mock overwrote it — hence the mock fix above. Both are caught now.

Not verified locally, same as the previous two sessions: `reuse lint` and
`cargo deny` (neither tool is installed on this VM; both run in CI). All new
files carry an SPDX `GPL-3.0-or-later` header and the crate adds no new
external dependencies. `cargo test` (87), `cargo clippy --all-targets -D
warnings` and `cargo fmt --check` are clean on the default members, and
`cargo test`/`clippy` are clean for `-p eds-sys -p jmap-backend-core` too,
since this touched `jmap-proto` and `jmap-vcard`.

No blockers hit.

Next: the `EBookMetaBackend` subclass, now a thin shell — register the type
against the `GTypeModule`, build a `BookSync` in `connect_sync` from
`ESourceAuthentication` credentials (libsecret, never a config file), and
marshal each vfunc onto the method of the same name through
`jmap-backend-core`'s trampolines.

## 2026-08-08 (fourth session)

M3, third increment: `jmap_backend_core::source` — turning the `ESource` a
backend is handed into the two things a JMAP client needs (an origin and a
user) plus the address book id. 15 new tests (12 in `tests/source.rs` against
a real `ESource`, 3 unit); `-p eds-sys -p jmap-backend-core` is now 42, the
default members are unchanged at 87, workspace total 129.

Decisions taken:

- **The account lives in the standard `ESource` extensions, not a private
  one.** `Authentication` carries host/port/user, `Security` the scheme,
  `Resource:identity` the JMAP address book id. A JMAP-specific extension
  needs an `ESourceExtension` subclass with keyfile-bound properties, which is
  M6's business along with the collection backend; until then a hand-written
  account looks exactly like a CalDAV or IMAP one, and the `.source` recipe M3
  asks for is in the module docs.
- **The password is not in the config at all.** It arrives at `connect_sync`
  as an `ENamedParameters` EDS filled from libsecret. Reading a credential
  from a keyfile would be the security failure the milestone specifically
  rules out, so there is deliberately no field it could be put in.
- **`ESourceSecurity:secure` defaults to FALSE, and `e_source_get_extension`
  creates the extension it cannot find.** Together those mean the obvious
  implementation — get the extension, read the flag — cannot distinguish "the
  keyfile has no `[Security]` group" from "the user turned TLS off", and
  answers the first with plain HTTP. `e_source_has_extension` is asked first,
  before anything creates the extension; absent means TLS, present means what
  it says. This was found by the test, which was written expecting the
  opposite default.
- **The host is validated, because the origin is built by concatenation.** A
  `.source` file is a plain file in the user's home. A host field carrying
  `http://evil.example.com` or `good.example.com/../evil` would aim the client
  elsewhere, or slip a plaintext endpoint past the TLS check. Only a bare host
  name or an IP literal is accepted; an IPv6 literal is bracketed so its
  colons stay out of the port slot.
- **Plaintext is refused unless the host is loopback.** 127/8, `::1` and
  `localhost` — which is what keeps `jmap-mockd` and a local Stalwart usable
  without weakening the rule for anything else. The near-misses
  (`localhost.example.com`, `127.0.0.1.example.com`, `0.0.0.0`) have their own
  test.
- **The refusal is `E_CLIENT_ERROR_TLS_NOT_AVAILABLE`, a missing or malformed
  host is `E_CLIENT_ERROR_INVALID_ARG`.** Same reasoning as M2's error
  mapping: Evolution renders the TLS code as a message about a secure
  connection, which is actionable, where `OTHER_ERROR` is not. These are
  configuration faults, so they get their own `SourceError` rather than being
  folded into the `jmap_client::Error` mapping — retrying will not help and
  serving the offline cache is not the right answer either.
- **`eds-sys` now allowlists vars.** The extension names are `#define`d
  strings, not symbols, so retyping them in Rust makes a typo an address book
  that silently reports no host instead of a link error. `generate_cstr` turns
  them into `&CStr`, so passing one to a `*const gchar` parameter is
  `.as_ptr()`.

Also fixed, unrelated to the increment but found by running the suite
repeatedly: `tests/subclass.rs` asserted the `class_init` counter without ever
referencing a class, and GObject runs `class_init` lazily. It passed only when
the instantiation test happened to run first, which cargo's concurrency made
usual but not certain — roughly one run in five was red. It now refs the class
itself.

On the TDD: `from_source` and `SourceError`'s methods were `unimplemented!()`
stubs while the tests were written, so all 11 failed at runtime for the right
reason. Two then failed for substantive reasons once implemented — the
`Security` default above, and a trailing-space host that `ESource` turns out
to strip in the setter, so that case was replaced with an interior space,
which it does not.

Eight mutations were run against the implementation. Seven were killed. The
survivor was treating an empty string as present: `ESource` normalises a
cleared key to NULL, so the integration test could not reach that branch at
all. There is now a unit test on the reader itself, the comment no longer
claims EDS hands out empty strings, and the mutation dies.

Not verified locally, as in the previous three sessions: `reuse lint` and
`cargo deny` (neither tool is installed on this VM; both run in CI). Both new
files carry an SPDX `GPL-3.0-or-later` header and no dependency was added.
`cargo fmt --check`, `cargo test` and `cargo clippy --all-targets -D warnings`
are clean on the default members and on `-p eds-sys -p jmap-backend-core`.

No blockers hit.

Next: the `EBookMetaBackend` subclass itself. Everything under it now exists —
`SourceConfig` for the account, `BookSync` for the protocol, `register_dynamic`
for the type, the trampolines and the error mapping — so what is left is the
class struct, the vfunc slot overrides, and holding a `BookSync` across
`connect_sync`/`disconnect_sync`.
