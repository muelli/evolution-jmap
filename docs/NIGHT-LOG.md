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

## 2026-08-08 (fifth session)

M3, fourth increment: `rust/crates/jmap-backend-book` (`jmap-backend-book`,
lib `jmap_backend_book`) — the two ends of the pipe the `EBookMetaBackend`
subclass will sit in the middle of. 21 tests in two files (`connect` 10,
`marshal` 11); `-p eds-sys -p jmap-backend-core -p jmap-backend-book` is now
63, the default members are unchanged at 87, workspace total 150.

The subclass itself was *not* written this session, deliberately. What was
missing under it was not lifecycle but the two places where a mistake is a
crash in `evolution-addressbook-factory` rather than a red assertion: the C
ownership rules at the vfunc boundary, and the "should Evolution ask for the
password again?" decision. Both are testable without a live `EBookBackend`,
which needs an `ESourceRegistry` and hence the D-Bus source registry service;
having them tested means the subclass on top can be a marshalling shell over
calls that are already covered.

- `connect` — `open_book(&SourceConfig, password, CancelFlag) -> BookSync`,
  plus `ConnectError`.
- `marshal` — `GSList`s of `EBookMetaBackendInfo` and of strings, vCard ↔
  `EContact`, `ENamedParameters` → password, `gchar **` out-parameters.

Decisions taken:

- **The configured address book is checked against the server, never
  trusted.** A typo in a hand-written `.source` would otherwise present as an
  address book that is merely empty — indistinguishable, from the user's side,
  from a server that lost their contacts. `ConnectError::NoSuchAddressBook`
  names the id that was not found.
- **"No address book configured" resolves to the one flagged `isDefault`, and
  to nothing else.** Falling back to the first book in the list would be a
  guess about where contacts get *written*. An account with no default is
  `NoDefaultAddressBook`. The test fixture seeds the non-default book first
  precisely so that the first-one-wins implementation fails visibly.
- **A source that names a user is never tried anonymously.** With no password
  yet, `open_book` fails with `CredentialsRequired` before opening a
  connection, so the prompt happens before anything is sent. A source with no
  user *is* anonymous on purpose — that is `jmap-mockd` and a development
  Stalwart — and a real server answers it with the 401 that becomes a prompt.
- **`REJECTED` is reserved for a 401.** It is the only `out_auth_result` that
  makes Evolution discard the stored password and ask again, so it has to mean
  "the server said these credentials are wrong" and nothing else: a 403 is
  authenticated-but-not-permitted and a server that is down is neither. Those
  are `ERROR`, which stops the loop instead of re-prompting for a password
  that was never the problem.
- **A stored-but-empty password is reported as present.** Reporting it absent
  would ask EDS to prompt; a user who then enters nothing would be prompted
  forever. Sending it and being told it is wrong terminates. Only a NULL
  `ENamedParameters` — what EDS passes before it has asked libsecret anything
  — is absent.
- **An empty `UID` is not a uid.** `save_contact_sync` tells a create from an
  edit by whether the `EContact` has one, and `EVCard` distinguishes two
  spellings the backend must not: no `UID` line reads back as NULL, but `UID:`
  with an empty value reads back as `""`, which would go to the server as the
  identifier of a card to patch.
- **Everything crossing the boundary is copied.** EDS frees an
  `out_existing_objects` list with `e_book_meta_backend_info_free`, a
  removed-uid list with `g_free` and `out_new_sync_tag` with `g_free`, so a
  node pointing into a Rust `String` is not a leak, it is a double free in
  another process. The `extra` field stays NULL: it is opaque per-object cache
  state and this backend has none, since the JMAP id *is* the uid and the
  revision already carries the change token.
- **`contact_from_vcard` refuses text that is not a vCard.** `EVCard` parses
  lazily and answers garbage with an empty card rather than an error, which
  would surface in Evolution as a contact that exists and has no properties.
  The guard is only the RFC 6350 §6.1.1 envelope — it is a check that the
  input claims to be a vCard, not a second parser.
- **`eds-sys` gained `e_named_parameters_*` and `E_SOURCE_CREDENTIAL_*`**, the
  latter as vars for the same reason the extension names are: retyping
  `"password"` in Rust makes a typo a credential that reads back as absent.
  `cstring_lossy` in `jmap-backend-core::error` became public, since building
  a `GError` is now done in two crates.

On the TDD: both modules were `todo!()` stubs while the tests were written, so
19 of the 21 failed at runtime for the right reason (the two that passed
assert on constants). Eight mutations were then run. Six were killed
immediately; the two survivors were real gaps and are now closed:
`contact_uid` treating an empty string as a uid was unreachable from the test
that only omitted the `UID` line — the empty-value spelling above is the case
that reaches it — and `to_gerror` was only asserted non-NULL, so any
`EClientError` code passed, including one that would have suppressed the
password prompt. That test now pins the domain and the code per variant. A
ninth mutation, dropping the NULL check in `set_out_string`, aborts the test
binary rather than failing an assertion; counted as killed.

Not verified locally, as in the previous four sessions: `reuse lint` and
`cargo deny` (neither tool is installed on this VM; both run in CI). All new
files carry an SPDX `GPL-3.0-or-later` header and the crate adds no new
external dependencies. `cargo fmt --check`, `cargo test` and `cargo clippy
--all-targets -D warnings` are clean on the default members and on
`-p eds-sys -p jmap-backend-core -p jmap-backend-book`, and
`cargo build --workspace --locked` succeeds. `cmake/Rust.cmake`'s
`rust-test-eds` target now runs the new crate too.

No blockers hit.

Next: the `EBookMetaBackend` subclass, which is now genuinely thin — the class
struct, `register_dynamic` against the `GTypeModule`, a `Mutex<Option<BookSync>>`
in the instance struct held across `connect_sync`/`disconnect_sync`, and seven
vfunc bodies that are a `guard` plus a `marshal` call each. After that the
`EBookBackendFactory`, the `e_module_load` entry point and the
`add_cargo_cdylib` install rule.

## 2026-08-08 (sixth session)

M3, fifth increment: `jmap-backend-core::instance` — owning a Rust value with
a destructor inside a GObject instance struct, plus the `finalize` half of the
subclassing scaffold that makes it possible. 7 new tests
(`tests/instance.rs`); `-p eds-sys -p jmap-backend-core -p jmap-backend-book`
is now 70, the default members are unchanged at 87, workspace total 157.

This is the piece the `EBookMetaBackend` subclass was still missing. It has to
hold a live `BookSync` between `connect_sync` and `disconnect_sync`, and
GObject's instance memory does not allow that on its own: the struct arrives
at `instance_init` zeroed and goes back to the allocator as soon as `finalize`
returns, with no Rust destructor anywhere in between. Getting it wrong is a
leak or a use-after-free in `evolution-addressbook-factory`, which is the same
class of failure as last session's marshalling and wanted the same treatment.

- `instance::Slot<T>` — an owning pointer whose **all-zero bytes are its empty
  state**, which is exactly the state GObject leaves the field in.
- `subclass::ObjectSubclass::finalize` — a defaulted hook, with registration
  installing the `GObjectClass.finalize` override and doing the chain-up.

Decisions taken:

- **Every state a real instance can be in is defined, including the broken
  ones.** Reading a slot before `instance_init` yields `None` rather than a
  dangling reference, so a vfunc reached on a half-built instance can report a
  clean error; clearing an empty slot is a no-op, because an instance whose
  `instance_init` stored nothing is finalized all the same. That is the whole
  reason for an owning pointer rather than a `MaybeUninit`: the zeroed state is
  a *value*, not a hole.
- **A second `init` is refused, not honoured.** Overwriting would free
  something another thread may be holding a `get` borrow of. The newcomer is
  dropped rather than leaked and the refusal is a GLib critical, since
  reaching it at all is a bug in the caller.
- **`clear` is `unsafe`, `init` and `get` are not.** With `&self` methods
  throughout, `let v = slot.get().unwrap(); slot.clear(); use(v)` would
  otherwise be a use-after-free with no `unsafe` in sight. `finalize`
  discharges the obligation by construction — it runs once, after the last
  reference is gone.
- **`PhantomData<T>` alongside the `AtomicPtr<T>`.** `AtomicPtr` is
  unconditionally `Send + Sync` regardless of `T`, and a `&Slot<T>` hands out a
  `&T`; without the marker a `Slot<Rc<_>>` would be shareable across threads.
- **The chain-up is outside the panic guard.** A panic in a Rust `finalize` is
  already a bug; skipping the parent's `finalize` would turn it into a leak of
  every instance from then on — for an `EBookMetaBackend` that is the
  `ESource`, the offline cache and the connection state.
- **The parent class is reached by `g_type_class_peek(T::parent_type())`, not
  by `g_type_class_peek_parent` of the instance's class.** The latter is the
  usual C shorthand and is wrong here: a further subclass would make it point
  back at this same trampoline and recurse until the stack ran out. The test
  hierarchy is two levels deep precisely so that this is observable.
- **Registration is installed on every type, not opted into.** A type with
  nothing to destroy pays one empty call; a type that forgot to opt in would
  leak silently. The trait's safety contract now says the parent must derive
  from `GObject`, which registration relies on for the class-struct cast.
- **`register` resolves `T::parent_type()` before taking its lock.** A
  hierarchy declared entirely in Rust bootstraps itself by registering the
  parent from inside `parent_type`, which deadlocked on the very much
  non-reentrant registration mutex. Found by writing the test that does it.

On the TDD: the tests were written against a `jmap_backend_core::instance`
that did not exist, so the first run failed to compile, and all 7 passed once
`Slot` and the `finalize` hook landed. Six mutations were then run and all six
died, though only two by assertion:

- dropping the NULL check in `clear` → SIGABRT (freeing NULL);
- `init` overwriting instead of refusing → SIGSEGV (double free);
- chaining via `g_type_class_peek_parent` of the instance's class → stack
  overflow, as reasoned above;
- skipping the chain-up entirely → assertion, `["quiet"]` vs `["quiet",
  "base"]`;
- never installing the finalize override → two assertions;
- resolving `parent_type()` inside the registration lock → hang (killed by a
  90 s timeout).

Not verified locally, as in the previous five sessions: `reuse lint` and
`cargo deny` (neither tool is installed on this VM; both run in CI). The new
files carry an SPDX `GPL-3.0-or-later` header and no dependency was added.
`cargo fmt --check`, `cargo test` and `cargo clippy --all-targets -D warnings`
are clean on the default members and on `-p eds-sys -p jmap-backend-core
-p jmap-backend-book`, and `cargo build --workspace --locked` succeeds.

No blockers hit.

Next: the `EBookMetaBackend` subclass itself, which now has nothing left under
it — `SourceConfig` for the account, `open_book` for the connection, `marshal`
for the C boundary, `Slot` for the session, `register_dynamic` for the type.
What remains is the instance and class structs, the seven vfunc slot
overrides, and a body per vfunc that is a `guard_bool` around a `marshal`
call. After that the `EBookBackendFactory`, the `e_module_load` entry point
and the `add_cargo_cdylib` install rule.

## 2026-08-08 (seventh session)

M3, sixth increment: `jmap-backend-book::ops` — the bodies of the
`EBookMetaBackend` sync vfuncs, plus the `SyncError` half of the error mapping
and the `EBookClientError` domain in `eds-sys` that it needs. 15 new tests
(`tests/ops.rs`); `-p eds-sys -p jmap-backend-core -p jmap-backend-book` is now
85, the default members are unchanged at 87, workspace total 172.

The subclass was the obvious next item and turned out to have a layer left
under it. Constructing a real `EBookMetaBackend` needs an `ESourceRegistry`,
which needs `evolution-source-registry` on the session bus — so anything
written *inside* a vfunc body is untestable on this VM and in CI. Splitting the
bodies out into functions that take a `&BookSync` and the same out-parameters
EDS passes keeps every one of them under test, and leaves the subclass as the
thing it was always supposed to be: a panic guard and a slot lookup.

- `ops::{list_existing, get_changes, load_contact, save_contact,
  remove_contact}` — the vfunc signatures minus the `EBookMetaBackend *`.
- `ops::to_gerror` — `SyncError` → `GError`, over
  `jmap_backend_core::error::to_gerror` for the client half.
- `eds-sys`: `EBookClientError` and `e_book_client_error_*` allowlisted.

Decisions taken:

- **A missing card is reported in the `E_BOOK_CLIENT_ERROR` domain, not
  `E_CLIENT_ERROR`.** `EBookMetaBackend` matches on exactly
  `E_BOOK_CLIENT_ERROR_CONTACT_NOT_FOUND` to decide that a card is gone rather
  than that the sync failed; any other code and the cache entry never goes
  away. That is what the `eds-sys` allowlist change is for — the enum was not
  bound, and `EClientError` has no equivalent.
- **`get_changes` returns a three-valued `Outcome`, not a `gboolean`.** Two
  situations are neither success nor failure: no sync tag (the first sync) and
  a tag the server will not diff from (RFC 8620 §5.2,
  `cannotCalculateChanges`). Both mean "chain up to `EBookMetaBackend`'s own
  `get_changes_sync`", which lists the book and diffs it against the cache.
  Reporting either as an error would leave the address book empty until
  someone deleted the cache by hand. The chain-up itself needs the parent class
  pointer, so it stays in the subclass; the *decision* is here, where it can be
  tested.
- **An absent sync tag is answered without asking the server.** Sending `""`
  on as a `sinceState` happens to produce the same fallback against the mock,
  because the mock rejects a state it did not issue — which is exactly why the
  test stops the server before calling. A real server that accepted an empty
  state would have answered the first sync with an empty delta.
- **An edit whose contact carries no identifier is refused.** Falling through
  to a create would silently duplicate the user's contact on the server, which
  is worse than a visible failure. `overwrite_existing` is EDS's word for "this
  is a modify", and the uid it implies has to be there.
- **Everything changed is reported as *modified*, and `out_created_objects`
  is left empty.** JMAP does distinguish created from updated, but
  `BookSync::get_changes` has already spent that distinction on a question only
  it can answer — a card that shows up as *updated* and is no longer filed in
  this book has been moved out and must be reported gone, whereas a *created*
  one that is not ours never was our business. EDS runs both lists through the
  same loader, so the split is presentational; inventing one would be a guess
  dressed up as information.
- **A NULL out-parameter does not just skip the write, it skips the work.**
  Building a `GSList` for an out-parameter nobody reads would need freeing with
  the right per-node function, and not building it is simpler than getting that
  right in two places.
- **Nothing is written on failure.** EDS only frees the outputs of a call that
  returned TRUE, so an out-parameter filled in before an error is a leak.

On the TDD: the tests were written against a `jmap_backend_book::ops` that did
not exist, so the first run failed to compile; 14 passed once the module
landed, and two more tests were added for the NULL and empty-string branches.
Ten mutations were then run and eight died:

- `NotFound` mapped to `E_CLIENT_ERROR_INVALID_ARG` → two assertions;
- an edit without a uid falling through to a create → assertion;
- `cannotCalculateChanges` treated as a failure → assertion;
- an absent sync tag sent on as an empty state → assertion (the
  stopped-server test above; it survived the first version of that test, which
  is what prompted rewriting it);
- `read_string` accepting `""` as present → assertion;
- removals never reported → assertion;
- `save_contact` not writing `out_new_uid` → two assertions;
- `out_repeat` never written → assertion (which needed the test fixture to
  start it at TRUE rather than at the FALSE EDS passes; otherwise a body that
  never answers is indistinguishable from one that answers correctly).

Two survivors, both judged equivalent rather than gaps: dropping the explicit
NULL-contact check in `save_contact` still ends in
`E_CLIENT_ERROR_INVALID_ARG` via `vcard_to_card("")`, differing only in the
message text, and pinning message strings is brittle for what it buys.

Not verified locally, as in the previous six sessions: `reuse lint` and
`cargo deny` (neither tool is installed on this VM; both run in CI). The new
files carry an SPDX `GPL-3.0-or-later` header and no external dependency was
added — the one new entry is `evolution-jmap-vcard` as a dev-dependency, to
name a `VCardError` variant in the error-mapping test. `cargo fmt --check`,
`cargo test` and `cargo clippy --all-targets -D warnings` are clean on the
default members and on `-p eds-sys -p jmap-backend-core -p jmap-backend-book`,
and `cargo build --workspace --locked` succeeds. `cmake/Rust.cmake` needed no
change; `rust-test-eds` already runs this crate.

No blockers hit.

Next: the `EBookMetaBackend` subclass, which now really is the last thin
piece — the instance struct (parent plus a `Slot<Mutex<Option<BookSync>>>`),
the class struct, `register_dynamic`, and seven vfunc bodies that are a
`guard_bool` around an `ops` call, with `get_changes_sync` matching on
`Outcome` and chaining up on `ListInstead`. After that the
`EBookBackendFactory`, the `e_module_load` entry point and the
`add_cargo_cdylib` install rule.

## 2026-08-08 (eighth session)

M3, seventh increment: `jmap-backend-book::backend` — the `EBookMetaBackend`
subclass, and the `connect::connect` layer under `connect_sync` that it
delegates to. 13 new tests (`tests/backend.rs`, plus four appended to
`tests/connect.rs`); `-p eds-sys -p jmap-backend-core -p jmap-backend-book` is
now 98, the default members are unchanged at 87.

The subclass is what the last six increments were clearing the way for, and it
came out as small as it was supposed to be: an instance struct, a class struct,
seven vfunc slots, and a body per slot that is `guard_bool` around a look in
the session slot and a call into `ops`.

- `backend::JmapBookBackend` — `EBookMetaBackend` plus a
  `Slot<RwLock<Option<BookSync>>>`, registered as `EBookBackendJmap`.
- `backend::parent_class` — the parent's class struct, for the one chain-up.
- `connect::connect` — `ESource` + `ENamedParameters` + `GCancellable` →
  `BookSync`, with `out_auth_result` and `error` written the way the vfunc has
  to write them.

Decisions taken:

- **An operation with no connection reports `E_CLIENT_ERROR_REPOSITORY_OFFLINE`,
  not `NOT_OPENED`.** EDS calls `connect_sync` before anything else, so the
  realistic way to reach an operation without a connection is a
  `disconnect_sync` racing it — which is what going offline looks like from
  inside. Reported as offline, `EBookMetaBackend` serves its cache and the user
  sees their contacts; reported as anything else they see an error for a state
  they asked for. The cost is that a genuine bug in the dispatch would be
  masked as an offline account, which is why every vfunc names itself in the
  panic guard's log context.
- **The connection lives behind an `RwLock`, not a `Mutex`.** EDS calls the
  read-only vfuncs from several threads at once. One lock over all of them
  would make a long `list_existing_sync` block every `load_contact_sync` behind
  it, for no gain: only connect and disconnect replace the value.
- **`connect_sync` on an already-connected backend answers ACCEPTED without
  reconnecting.** EDS calls it whenever it suspects the connection is gone,
  including when it is not; re-opening would drop a socket other threads are
  mid-request on.
- **`disconnect_sync` on a backend that never connected is a success.** It is
  what EDS asks for on shutdown after a failed connect. There is nothing left
  to do and nothing went wrong.
- **`out_auth_result` is written on every path through `connect::connect`,
  success included.** EDS reads it whenever the vfunc returns, and a stale
  value left from a previous attempt is how an account ends up either never
  prompting or prompting forever.
- **`out_certificate_pem`/`out_certificate_errors` are left untouched.** They
  describe a TLS certificate the user might be asked to accept, and the client
  offers no way to get at one — a bad certificate reaches us as a transport
  failure and nothing more. A made-up value would put a dialog in front of the
  user that cannot be answered truthfully.
- **`e_book_meta_backend_set_connected_writable(TRUE)` on a successful
  connect.** Without it the address book is read-only in the UI. JMAP has no
  per-book "may I write" flag, so the answer is the account's.
- **A NULL `ESource` is refused rather than passed on.** EDS constructs a
  backend *from* a source so it cannot happen, but a NULL dereference in
  `evolution-addressbook-factory` takes every other account in that process
  down with it.

On the testing: everything goes *through the class struct* —
`g_type_class_ref` and then the slot — because a vfunc that is correct but not
installed is a backend that silently uses `EBookMetaBackend`'s defaults, and
that is indistinguishable from an empty address book. The instance is
`JmapBookBackend::detached()`: zeroed parent bytes, an initialised session
slot, and a documented rule that nothing but that slot may be touched. It is
not a shortcut — a real instance needs an `ESourceRegistry` and so
`evolution-source-registry` on the session bus, which neither this VM nor CI
has. That is also why `connect_sync` itself is tested one layer down, at
`connect::connect`, where the input is an `ESource` a test can build with
`e_source_new_with_uid`.

Nine mutations were run and seven died:

- `fail_offline` returning TRUE → assertion;
- `class_init` skipping `get_changes_sync` → assertion;
- `disconnect_sync` keeping the connection → assertion;
- `finalize` not clearing the slot → assertion;
- `store_connection` doing nothing → assertion;
- `detached` leaving the slot empty → assertion;
- `connect` not writing ACCEPTED on success → assertion;
- a configuration failure reported as REJECTED rather than ERROR → assertion.

Two survivors, both understood:

- dropping the explicit NULL-`ESource` check still ends in
  `E_CLIENT_ERROR_INVALID_ARG`, because EDS's own `g_return_if_fail` guards
  turn every accessor into a NULL return and the config comes out with no
  host. The check stays: it is four fewer GLib criticals and it does not
  depend on those guards being there.
- `connect_sync` re-opening a live connection survives because `connect_sync`
  is the one vfunc with no test at all — it reads the parent's `ESource`, which
  a detached instance does not have. Everything below it is covered; the vfunc
  itself is six lines and will first run under a real registry when the module
  loads.

Known gap, deliberately not closed here: **cancellation reaches the connect but
not the operations after it.** `Client` takes its `CancelFlag` when it is built
and offers no way to re-point it, so a `GCancellable` handed to
`list_existing_sync` is observed by nobody. Closing it means a resettable flag
shared between the client and a per-operation `CancelBridge` — a change to
`jmap-client`, not to this crate, and a separate increment.

Not verified locally, as in the previous seven sessions: `reuse lint` and
`cargo deny` (neither tool is installed on this VM; both run in CI). The two
new files carry an SPDX `GPL-3.0-or-later` header and no external dependency
was added — the one new entry is `gio-sys`, already a workspace dependency used
by `jmap-backend-core`, for the `GCancellable` in the vfunc signatures.
`cargo fmt --check`, `cargo test` and `cargo clippy --all-targets -D warnings`
are clean on the default members and on `-p eds-sys -p jmap-backend-core
-p jmap-backend-book`, `cargo doc` is warning-free, and
`cargo build --workspace --locked` succeeds. `cmake/Rust.cmake` needed no
change; `rust-test-eds` already runs this crate.

No blockers hit.

Next: the `EBookBackendFactory` subclass (`E_BOOK_BACKEND_FACTORY` with
`factory_name = "jmap"` and `backend_type = JmapBookBackend`), the
`e_module_load`/`e_module_unload` entry points that register both against the
`GTypeModule`, and the `add_cargo_cdylib` install rule into the
libedata-book backend directory. After that the manual test recipe with a
hand-written `.source` keyfile, which is the first time any of this runs
against a real `evolution-source-registry`.

## 2026-08-08 (ninth session)

M3, eighth increment: the two pieces between `evolution-addressbook-factory`
and the backend — `jmap-backend-book::factory`, the `EBookBackendFactory`
subclass, and `jmap-backend-book::module`, the `e_module_load`/`e_module_unload`
symbols EDS resolves out of the shared object. The crate now builds a `cdylib`
as well as an rlib. 7 new tests (`tests/factory.rs`, plus one appended to
`jmap-backend-core/tests/subclass.rs`); `-p eds-sys -p jmap-backend-core
-p jmap-backend-book` is now 105, the default members are unchanged at 87.

This is the layer EDS actually reaches first. It scans its backend directory,
wraps each `.so` in an `EModule` and `g_type_module_use`s it — which dlopens
the file and calls `e_module_load` — then looks for children of
`EBookBackendFactory` among the types that appeared, and hands an `ESource` to
whichever answers to the `BackendName` in its `[Address Book]` group. All a
subclass has to say is its name and what to build, so `factory.rs` is two
assignments in a `class_init` and `module.rs` is two registrations.

- `factory::JmapBookFactory` — `EBookBackendFactory` with `factory_name`
  `"jmap"` and `backend_type` `EBookBackendJmap`, registered as
  `EBookBackendJmapFactory`.
- `factory::remember_backend_type` — where that `GType` comes from.
- `module::e_module_load` / `module::e_module_unload` — the exported entry
  points, both under the panic guard.

**The one real find: `register_dynamic` must register on every load, not only
the first.** `register_static` has to short-circuit on an
already-registered name because a second `g_type_register_static` is a fatal
GLib error — and the same short-circuit was being applied to the module path,
where it is exactly wrong. `g_type_module_unuse`, which EDS does as soon as the
last backend a module provided goes away, marks every type that module
registered as *unloaded*; the next use calls `e_module_load` again, and if that
call does not re-register, GLib does not degrade gracefully. It aborts the
process: `GLib-GObject-ERROR **: Fatal error - Could not reload previously
loaded plugin`. That is the red this increment started from — a SIGTRAP in the
test runner, not an assertion — and the fix is four lines in
`jmap-backend-core::subclass::register`, guarding the early return with
`module.is_null()`.

Decisions taken:

- **The backend type reaches the factory through a `static AtomicUsize`, not
  through a `register_static` in `class_init`.** This is the Rust spelling of
  what `G_DEFINE_DYNAMIC_TYPE` hands a C backend for free: `e_module_load`
  registers the backend first and records the result, and `class_init` — which
  runs much later, at the first `g_type_class_ref` — reads it. Registering from
  inside `class_init` instead would register *statically*, and a statically
  registered type keeps its class, and so pointers into this shared object,
  alive after EDS has unloaded the module underneath it.
- **…with `register_static` as the fallback when the atomic is still zero.**
  That cannot happen under EDS and does happen in a test that references the
  factory class without loading a module. The alternative is a factory with a
  zero `backend_type`, which is a `g_object_new(0)` per address book: a GLib
  critical, a NULL backend, and no hint as to why.
- **`share_subprocess` is left at its default.** Setting it would put every
  JMAP address book in the session into one
  `evolution-addressbook-factory-subprocess`, and those books belong to
  different accounts holding different credentials. The default gives each
  source its own process: a process more, a blast radius less.
- **`EBackendFactoryClass.e_module` is left alone.** It is what
  `e_backend_factory_get_module_filename` reports and it is not a field the
  EDS backends set for themselves; nothing in the headers or the GIR says a
  subclass should, and inventing a value for it is not something a wrong guess
  fails loudly at.
- **`crate-type = ["cdylib", "rlib"]`, both.** The cdylib is what EDS dlopens;
  the rlib is what the integration tests link. Building both is what keeps the
  tested thing and the shipped thing from drifting apart. Verified with
  `nm -D`: the built `.so` exports `e_module_load` and `e_module_unload` and
  nothing else of ours.

On the testing: `tests/factory.rs` drives the real path rather than the
functions under it. A `GTypeModule` subclass — declared with this project's own
`ObjectSubclass`, which is a pleasing amount of dogfooding — stands in for the
`EModule` that would dlopen the built `.so`, and its `load` vfunc calls our
entry point exactly as `EModule`'s does. The fixture uses that module, unuses
it and uses it again, so the reload path is covered by construction; everything
else is asserted through the class struct, as in `tests/backend.rs`. There is
one module and one `OnceLock` because two `GTypeModule`s cannot register the
same type name, and no test instantiates a factory: `EBookBackendFactory`
derives from `EExtension`, so a real one needs the `EDataFactory` it extends.

Five mutations were run and five died:

- the early return in `register` not guarded by `module.is_null()` → the
  process aborts in `tests/factory.rs`, and the new
  `jmap-backend-core` test fails cleanly on the second `g_type_module_use`;
- `factory_name` set to something other than `"jmap"` → assertion;
- `backend_type` left unset → assertion;
- the entry point registering the backend statically → assertion, via
  `g_type_get_plugin`;
- (from the same run) `e_module_load` doing nothing → four assertions.

Swapping the order of the two registrations in `e_module_load` is *not*
caught, and correctly so: `class_init` is lazy, so the atomic is set either
way. The order stays as it is because it is the order the dependency runs in
and a reader should not have to work that out.

Drive-by: `jmap-backend-core`'s crate docs linked `jmap_client::error::Error`,
which is private; `cargo doc` had been warning about it since M2. Now
`jmap_client::Error`, and `cargo doc --no-deps` is warning-free again.

Not verified locally, as in the previous eight sessions: `reuse lint` and
`cargo deny` (neither tool is installed on this VM; both run in CI). The three
new files carry an SPDX `GPL-3.0-or-later` header and no dependency was added.
`cargo fmt --check`, `cargo test` and `cargo clippy --all-targets -D warnings`
are clean on the default members (87 tests) and on `-p eds-sys
-p jmap-backend-core -p jmap-backend-book` (105), and
`cargo build --workspace --locked` succeeds.

No blockers hit.

Next, and deliberately not started here so this increment could be pushed
green: the CMake side. `add_cargo_cdylib()` in `cmake/Rust.cmake` — the helper a comment
in that file has been promising since M1 — installing
`libjmap_backend_book.so` as `libebookbackendjmap.so` into
`pkg_check_variable(backenddir libedata-book-1.2)`, which on this VM is
`/usr/lib/evolution-data-server/addressbook-backends`. `cargo build
--workspace` already builds the crate in the `rust-build` target, so the work
is the install rule and the rename. After that the manual test recipe with a
hand-written `.source` keyfile, which is the first time any of this runs
against a real `evolution-source-registry`.

## 2026-08-08 (tenth session)

M3, ninth increment: the CMake side. `add_cargo_cdylib()` in
`cmake/Rust.cmake` — the helper a comment in that file has been promising
since M1 — plus `cmake/Backends.cmake`, which uses it to install the cdylib
cargo builds as `libjmap_backend_book.so` under the name and in the directory
`evolution-addressbook-factory` looks for:
`/usr/lib/evolution-data-server/addressbook-backends/libebookbackendjmap.so`
on this VM. One new CTest, `install-book-backend`; `ctest` is now 3 tests, the
Rust suites are unchanged at 87 and 105.

Nothing about this is testable from Rust, so the test is a `cmake -P` script,
`cmake/tests/check-installed-module.cmake`. It runs `cmake --install
--component book-backend` with `DESTDIR` pointing at a scratch directory and
then asks the three questions the build system cannot ask itself: did a file
land at the expected absolute path, is it big enough to be a shared object,
and does it export the entry points EDS resolves. The symbol check reads the
NUL-separated `.dynstr` with `file(STRINGS ... REGEX)`, so it needs no `nm`
and no extra build dependency.

Decisions taken:

- **`rust-build` is now part of `ALL`.** The installed cdylibs are *files* to
  CMake, not targets, so nothing else would build them and `cmake --install`
  on a fresh tree would copy nothing at all — the failure mode being that an
  install rule which quietly installs nothing looks exactly like one that
  works. The cost is that `cmake --build` now runs a release workspace build
  in `${CMAKE_BINARY_DIR}/cargo-target`, which CI's `Swatinem/rust-cache` does
  not cache; correctness first, and the alternative was an install target that
  is a trap.
- **`install(PROGRAMS)`, not `install(FILES)`.** A shared module wants mode
  0755; `FILES` would install it 0644. Verified on the staged tree.
- **The EDS wiring lives in `cmake/Backends.cmake`, not in
  `CMakeLists.txt`.** The top-level file is derived from the GNOME Evolution
  wiki's `example-module.zip` and `REUSE.toml` annotates it Red Hat /
  LGPL-2.1-or-later; keeping our GPL-3 work in a file of our own leaves that
  one line closer to upstream and the licence annotation honest. `CMakeLists.txt`
  gains a single `include()`.
- **The destination is checked against pkg-config, not against itself.** This
  was the one thing the first green version got wrong, and mutation testing is
  what found it: `EXPECTED` was computed from the same `DESTINATION` the
  install rule used, so renaming the module to `libebookbackendxmpp.so` — or
  installing it into the calendar backend directory — passed. The check script
  now re-runs `pkg-config --variable=backenddir libedata-book-1.2` itself and
  compares, via a `VERIFY_DESTINATION_FROM <module> <variable>` argument the
  helper forwards. That argument is suppressed under `FORCE_INSTALL_PREFIX`,
  where moving the destination is the whole point.
- **The installed *name* is still not independently checked, and that is
  honest rather than a gap.** EDS loads every `.so` in the backend directory
  and then looks for `EBookBackendFactory` subclasses among the types that
  appeared; the filename is convention, the directory and the entry points are
  what load-bear. Both of the latter are now checked against their source.
- **`add_cargo_cdylib()` refuses an empty or relative `DESTINATION` at
  configure time.** `pkg_check_variable()` reports a missing variable as the
  empty string, which would otherwise install into the prefix root.

Five mutations were run; three died immediately, one exposed the tautology
above and died after the fix, one survives by design:

- the install rule deleted → the staged path does not exist;
- `SYMBOLS` naming something the module does not export
  (`camel_provider_module_init`) → "does not export";
- `DESTINATION` set from an undefined variable → configure-time
  `FATAL_ERROR`;
- `DESTINATION` pointed at `calendar-backends` → survived the first version,
  dies now on the pkg-config comparison;
- `OUTPUT_NAME` changed to `libebookbackendxmpp.so` → survives, per the naming
  decision above.

Also verified: a clean `cmake -S . -B <fresh> && cmake --build && ctest` — the
exact sequence CI's `build-full` job runs — goes green from an empty tree, and
`-DFORCE_INSTALL_PREFIX=ON -DCMAKE_INSTALL_PREFIX=/opt/…` relocates the
destination as it does for the example module. Drive-by:
`libjmap_backend_book.so` is now in `ci.yml`'s uploaded artifacts, next to
`jmap-mockd` and the example module.

Not verified locally, as in the previous nine sessions: `reuse lint` and
`cargo deny` (neither tool is installed on this VM; both run in CI). The two
new files carry an SPDX `GPL-3.0-or-later` header and no dependency was added.
No Rust source changed; `cargo fmt --check`, `cargo test` and `cargo clippy
--all-targets -D warnings` are clean on the default members (87 tests) and on
`-p eds-sys -p jmap-backend-core -p jmap-backend-book` (105).

No blockers hit.

Next: the manual test recipe — a hand-written `.source` keyfile under
`~/.config/evolution/sources/`, the installed module, and a real
`evolution-addressbook-factory` talking to `jmap-mockd`. That is the first
time any of this runs outside a test harness, and the acceptance criterion M3
still has open.

## 2026-08-08 (eleventh session)

M3's last acceptance criterion: "a documented manual test recipe with a
hand-written `.source` keyfile". `docs/manual-test-book-backend.md` is the
recipe, `docs/examples/jmap-mock.source` is the keyfile it says to copy, and
`rust/crates/jmap-backend-book/tests/recipe.rs` is what keeps the two honest —
four tests, so `-p …-book` is 16 and the EDS crates together are 109; the
default members are unchanged at 87.

A recipe is prose, and prose is the one thing in this repository nothing
fails over. The failure it invites is also the quietest one there is: a
`BackendName` no factory claims is not an error, it is an address book
Evolution never tries to open. So the keyfile is a *file*, and the tests read
it the way the registry does — `e_server_side_source_new` on a `GFile` is what
`evolution-source-registry` calls for every file in its sources directory, and
it turns out to need neither a bus nor a daemon, only an
`ESourceRegistryServer` that is never run. What the reader copies is therefore
parsed by EDS's own keyfile code and handed to the same
`SourceConfig::from_source` the backend calls.

**That test found a real bug on its first run, in a place that had been
reviewed twice: `[Security] Secure=true` does nothing.** `ESourceSecurity:secure`
is a boolean *over* a string property, and the keyfile only ever stores the
string: EDS writes `Method=tls` / `Method=none`, and a group saying
`Secure=true` sets no property EDS knows. It is not rejected — it is ignored,
and what is left reads back as "no method", which is `none`. The recipe in
`jmap-backend-core::source`'s module docs had said `Secure=true` since the
sixth session, and following it against a real server would have produced an
account that refuses to connect while complaining about TLS. Confirmed
in plain C against the installed libraries before believing it, then fixed in
the doc comment and pinned by
`the_keyfile_spelling_that_turns_tls_on_is_method_not_secure`, which asserts
both directions: `Method=tls` on a remote host yields an `https://` origin,
and `Secure=true` on the same host is still refused as insecure transport.

Decisions taken:

- **The recipe's account is anonymous.** `jmap-mockd` with no `--basic` wants
  no credentials, and a source that names a `User` makes the backend ask EDS
  for a password *before* it sends anything — so a recipe with one would open
  with a password prompt instead of a connection, which is not the thing being
  tested. The credential path is documented as the variant: add `User=`,
  `Method=plain/password`, and start the mock with `--basic`.
- **The document quotes the keyfile verbatim, and a test says so.** The reader
  copies the file but reads the document; if they drift, whichever one was
  trusted is the wrong one. `the_recipe_quotes_the_keyfile_verbatim` extracts
  the single ```ini block and compares it byte for byte.
- **`e_server_side_source_.*` is now in the eds-sys allowlist.** No backend
  calls it — a backend is handed a finished `ESource` — but it is the only way
  to turn a keyfile into one without a running registry. M6's collection
  backend meets server-side sources for real.
- **The installed name is *not* what the recipe leans on.** As in the tenth
  session: the directory and the entry points load-bear, and `BackendName`
  selects the factory. Those three are checked; the `.so` name is convention.
- **What is not verified, and honestly labelled as such in the document:
  everything past the keyfile.** This VM has EDS 3.52 dev headers but not the
  `evolution-data-server` runtime package, so `evolution-source-registry` and
  `evolution-addressbook-factory` are not here and the recipe has never been
  executed end to end. It is written from the EDS 3.52 sources (fetched to
  confirm, in particular, that `EDS_ADDRESS_BOOK_MODULES` *replaces* the
  backend directory rather than adding to it — the no-sudo path).

Six mutations were run, all of them fatal to at least one test: the fixture
switched to `Method=tls`, to `Secure=false`, to `BackendName=jmapx`, its
`[Address Book]` group misspelled, the document's port drifted from the
file's, and `FACTORY_NAME` changed to `jmap2`. The first mutation round of the
session was contaminated — `git checkout` cannot restore an untracked file, so
the mutations accumulated — which is worth remembering: for new files, keep a
copy outside the tree.

Not verified locally, as in the previous ten sessions: `reuse lint` and
`cargo deny` (neither tool is installed on this VM; both run in CI). The new
Rust file carries an SPDX `GPL-3.0-or-later` header; the two new `docs/` files
are covered by `REUSE.toml`'s existing `docs/**` annotation, which is why the
keyfile can be copied as-is. No dependency was added. `cargo fmt --check`,
`cargo test` and `cargo clippy --all-targets -D warnings` are clean on the
default members and on `-p eds-sys -p jmap-backend-core -p jmap-backend-book`,
`cargo doc -p jmap-backend-core --no-deps` is warning-free, and a clean
`cmake -S . -B <fresh> && cmake --build && ctest` is 3/3.

No blockers hit.

Next: M3 has no open acceptance criteria left. Either M4 — the calendar
backend, which is M3's shape again on `ECalMetaBackend` with JSCalendar ↔
iCalendar in place of JSContact ↔ vCard — or, first and much smaller, run
this recipe for real on a machine with the EDS daemons installed. Everything
it claims past step 3 is reasoned from the EDS sources, not observed.

## 2026-08-08 (twelfth session)

M3 is closed, so this session opens M4 at the bottom: `jmap-ical`, the
calendar-side counterpart of `jmap-vcard`, starting with the layer everything
above it stands on — the iCalendar lexer/emitter. New default member (it needs
no EDS headers, so `cargo test` picks it up everywhere), 12 tests, taking the
default set from 87 to 99; the EDS crates are untouched at 109.

The shape is `jmap-vcard::syntax`'s, because the two grammars are cousins —
folding at 75 octets, `;`-separated parameters with quoted values, the same
four TEXT escapes. Two things are genuinely different, and both are where the
bugs would live:

- **Components nest.** vCard is a flat property list between `BEGIN` and
  `END`; iCalendar is a tree — `VALARM` inside `VEVENT` inside `VCALENDAR`,
  and `VTIMEZONE` with its `STANDARD`/`DAYLIGHT` children waiting in M4. So
  `parse` keeps a stack of open components and `Component` carries
  `children`. An `END` that names something other than the innermost open
  component is `Mismatched` rather than a quiet pop, and content after the
  calendar closes is `Trailing` rather than a silent truncation: a stream
  with two `VCALENDAR`s would otherwise lose the second one without a word.
- **Only TEXT is escaped.** `DTSTART`, `DURATION`, `RRULE` and `TRIGGER`
  carry their own punctuation — `FREQ=WEEKLY;BYDAY=MO,TU` is structure, not
  a semicolon and a comma that need backslashes. Hence two constructors,
  `Property::new` (escapes) and `Property::raw` (verbatim), and two readers,
  `text()`/`texts()` and `raw_value()`. Getting this backwards would corrupt
  every recurring event M4 touches, which is exactly the RRULE the milestone
  names.

Decisions taken:

- **No vCard-style group prefix, and no bare parameter values.** RFC 5545 has
  neither. `item1.TEL:` has no iCalendar analogue, and where vCard 2.1's
  `EMAIL;INTERNET:` forced `jmap-vcard` to read a bare token as `TYPE`, a
  parameter without `=` here is a malformed line and is rejected. Being
  forgiving in the vCard case bought compatibility with real exporters;
  being forgiving here would only invent a parameter nobody wrote.
- **`Component::text()` returns `Option<String>`, and absence is not an
  error.** Same principle as the vCard mapping: an event missing a property
  is better than a calendar that refuses to open. Only the syntax layer
  fails, and `ICalError` enumerates exactly the five ways it can.
- **The crate is `evolution-jmap-ical`/`jmap_ical`,** matching the
  `evolution-jmap-vcard`/`jmap_vcard` pair, and it has no dependencies at
  all yet — JSCalendar types live in `jmap-proto::calendars` and only the
  semantic layer will need them.

Eight mutations were run against the suite, all against a copy kept outside
the tree (the lesson from the eleventh session): `escape` leaving `;` alone,
`END` closing any component, trailing content ignored, `FOLD_AT` at 76,
`texts()` collapsed to one value, `Component::text` handing out the raw
value, and `Component::new`/`Property::new` not upper-casing their names.
Six died immediately. **The seventh and eighth survived, and that was a real
gap:** every name in the tests reached the accessors through `parse`, which
upper-cases while lexing, so nothing pinned the *constructors*' upper-casing
— a caller writing `Component::new("vevent")` would have emitted
`BEGIN:vevent` and then failed to find it with `child("VEVENT")`.
`names_are_upper_cased_on_the_way_in_too` closes that, and both mutations now
die.

Not verified locally, as in the previous eleven sessions: `reuse lint` and
`cargo deny` (neither tool is installed on this VM; both run in CI). The four
new files carry an SPDX `GPL-3.0-or-later` header and no dependency was
added. `cargo fmt --check`, `cargo test --locked` (99) and `cargo clippy
--all-targets -D warnings` are clean on the default members and on
`-p eds-sys -p jmap-backend-core -p jmap-backend-book` (109), and a clean
`cmake -S . -B <fresh> && cmake --build && ctest` is 3/3 — `rust-test` runs
plain `cargo test`, so the new crate needed no build-system wiring.

No blockers hit.

Next, in M4: the semantic layer on top of this one — `CalendarEvent` ↔
`VEVENT` for the minimal set the roadmap names (UID, SUMMARY, DESCRIPTION,
DTSTART with `timeZone`, DURATION, STATUS, RRULE with FREQ/INTERVAL/COUNT/
UNTIL). JSCalendar's LocalDateTime (`2026-01-15T13:00:00`) and iCalendar's
`20260115T130000` are the same instant spelled differently, and the `TZID`
parameter is where `timeZone` lands; the RRULE mapping is the one that will
want its own fixtures.

## 2026-08-08 (thirteenth session)

M4 continued: the semantic layer on top of the iCalendar lexer —
`jmap_ical::event`, `CalendarEvent` ↔ `VEVENT` for the property set the
roadmap names (UID, SUMMARY, DESCRIPTION, DTSTART with its zone, DURATION,
STATUS, RRULE). 19 tests, taking the default set from 99 to 118; the EDS
crates are untouched at 109. The crate now depends on `jmap-proto`, which it
did not before — JSCalendar types live there.

The shape is `jmap-vcard::contact`'s, deliberately: `event_to_ical` /
`ical_to_event` mirror `card_to_vcard` / `vcard_to_card`, `UID` carries the
JMAP id with the JSCalendar `uid` alongside in `X-JMAP-UID`, and unmapped
properties are dropped rather than refused. Three things are genuinely
calendar-shaped, and each got its own decision:

- **The time zone is spelled three different ways in iCalendar, and picking
  the wrong one moves the appointment.** `timeZone: null` is RFC 5545 form 1
  (floating, no `TZID`, no `Z`), `Etc/UTC` is form 2 (`…T130000Z`, *not*
  `TZID=Etc/UTC`, which would oblige us to ship a `VTIMEZONE` for it), and
  anything else is form 3 (`DTSTART;TZID=Europe/Berlin:…`), leaning on
  libical's built-in Olson table rather than an emitted `VTIMEZONE`. Reading
  back inverts exactly that: a trailing `Z` wins over any `TZID`.
- **`UNTIL` does not get converted to UTC, and that is a knowing deviation.**
  RFC 5545 §3.3.10 wants a UTC instant when `DTSTART` carries a `TZID`;
  JSCalendar's `until` is a local time in the event's own zone. Converting
  needs a zone database, which this crate deliberately does not depend on, so
  `UNTIL` is emitted the way `DTSTART` is — `Z`-suffixed for a UTC event,
  local otherwise. It round-trips, and libical reads it in the event's zone.
  Written down in the code, not just here.
- **A `DTSTART` neither side can read is left out rather than guessed at.**
  Both directions validate the shape (eight digits, `T`, six digits) before
  converting; `VALUE=DATE:20260115` is the one exception and becomes midnight,
  because an all-day event that lost its start entirely is worse than one
  pinned to the top of the day. `showWithoutTime` is not modeled yet, which is
  the honest cost of that choice.

Decisions taken:

- **Unmodeled RRULE parts are dropped, and the drop is made visible.**
  `byDay` and the rest of RFC 8984 §4.3.3 cannot survive the trip (JSCalendar
  spells `byDay` as an array of objects; copying `BYDAY=MO` across would be
  rejected by the server), so `maps_recurrence_rule()` reports whether a rule
  round-trips and `MAPPED_PROPERTIES` names the six properties a save may
  patch. Same principle as `jmap-vcard`'s `maps_name_component()` & co: a
  property we never mapped is a property we never overwrite — but
  `recurrenceRules` is one property, so the save path has to ask first.
- **`STATUS` is table-driven in both directions and unknown values are
  dropped.** Both vocabularies are closed; passing `dithering` through
  uppercased would put a value libical rejects into the component.
- **All `RRULE` lines are mapped, not just the first.** RFC 5545 says `RRULE`
  SHOULD NOT repeat, but honouring a repeat costs one loop and avoids losing
  a rule the server sent.
- **A calendar with no `VEVENT` is the mapping's one error** (`ICalError::
  NoEvent`); everything else it cannot read is treated as absent.

Thirteen mutations were run against a copy kept outside the tree: UTC emitted
as a `TZID`, `INTERVAL=1` written out, `RRULE` escaped as TEXT, the JSCalendar
uid preferred over the JMAP id, `TZID` ignored on the way back, a missing
`VEVENT` tolerated, an unknown `STATUS` passed through, a date-only `DTSTART`
dropped, `@type` left off a parsed rule, `SUMMARY`/`DESCRIPTION` emitted
unescaped, `maps_recurrence_rule` always true, the separators left in the
emitted date-time, and the digit/length checks removed from both converters.
Eleven died at once. **The two that survived were the same gap:** nothing
exercised a malformed start, so both converters could have accepted garbage —
`a_start_that_is_not_a_date_time_is_left_out_rather_than_mangled` covers three
bad values in each direction and both mutations now die.

Not verified locally, as in the previous twelve sessions: `reuse lint` and
`cargo deny` (neither tool is installed on this VM; both run in CI). The two
new Rust files carry an SPDX `GPL-3.0-or-later` header and the new fixture is
covered by `REUSE.toml`'s existing `rust/crates/*/tests/fixtures/**`
annotation. No third-party dependency was added — the two new manifest entries
are in-workspace. `cargo fmt --check`, `cargo test --locked` (118) and `cargo
clippy --all-targets -D warnings` are clean on the default members and on
`-p eds-sys -p jmap-backend-core -p jmap-backend-book` (109), `cargo doc -p
evolution-jmap-ical --no-deps` is warning-free, and a clean `cmake -S . -B
<fresh> && cmake --build && ctest` is 3/3.

No blockers hit.

Next, in M4: `jmap-cal-sync`, the calendar-side counterpart of
`jmap-book-sync` — the pure sync logic (`list_existing`, `get_changes`,
create/update/destroy against `CalendarEvent/*`) that the `ECalMetaBackend`
subclass will call, tested against `jmap-mockd`. The update path is where
`MAPPED_PROPERTIES` and `maps_recurrence_rule` earn their keep: the patch it
builds must name only what the iCalendar round trip preserved.

## 2026-08-08 (fourteenth session)

M4 continued: `jmap-cal-sync`, the calendar-side counterpart of
`jmap-book-sync` — the pure sync logic an `ECalMetaBackend` subclass will
call, tested against `jmap-mockd` rather than a fixture. 20 tests, taking the
default set from 118 to 138; the EDS crates are untouched at 109. No new
third-party dependency: the crate leans on `jmap-client`, `jmap-ical` and
`jmap-proto`, all in-workspace.

`CalSync` is `BookSync` with the nouns changed, deliberately and almost line
for line — `list_existing`/`load_component`/`save_component`/
`remove_component`/`get_changes` onto the five vfuncs, `ComponentInfo` for
`ECalMetaBackendInfo`, the same `MAX_CHANGES_PAGES` guard, the same
created/updated classification that turns an account-wide `/changes` into
"changed here" and "gone from here", and the same FNV-1a digest of the
rendered object as the revision. Two crates reading as one design is worth
more here than the eight lines the shared digest would have saved; the
comment on `revision_of` says so.

The write path is where the calendar genuinely differs, and it came out
*simpler* than the address book's rather than harder:

- **The patch diffs against the round trip, not against the server.**
  `jmap-book-sync::patch` compares the edited card to the stored card and then
  handles each lossy property specially — merged `contexts`, preserved `pref`
  ranks, carried-over name components. The calendar mapping is field for
  field, so one move covers all of it: compute
  `ical_to_event(event_to_ical(current))` and diff against *that*. The
  baseline is by construction exactly what Evolution was shown, so a
  difference from it is an edit and nothing else is. It falls out for free
  that a `timeZone` of `UTC` is not rewritten to `Etc/UTC` because the `Z`
  suffix reads back as the latter, that an `RRULE` which had to drop
  `INTERVAL=1` does not come back with `interval` deleted, and that a
  `status` outside the closed vocabulary is not cleared by a save that never
  touched it.
- **`recurrenceRules` is left alone entirely when the server holds a rule the
  `RRULE` could not carry.** This is what `maps_recurrence_rule` was added
  for last session. `recurrenceRules` is one property, so there is no way to
  patch the part the user edited without restating the part `byDay` lives in;
  ignoring the edit is the lesser harm, and the test says so in its name.
- **`start` is never nulled.** RFC 8984 requires it, and a component whose
  `DTSTART` the mapping cannot read yields no start — which is not the same
  claim as "this event has no start". The server's start stands and the rest
  of the edit still goes through.

Decisions taken:

- **A new event's iCalendar `UID` becomes the JSCalendar `uid`.** The book
  side simply drops Evolution's locally invented `pas-id-…` and lets the
  server name the card. A calendar `UID` is not the same kind of string: it
  is the identity any iTIP correspondence already quotes, so it moves to
  `uid` — the JSCalendar property that means what it means — while the JMAP
  id stays the server's to assign.
- **`list_existing` applies no time range.** `CalendarEventQueryFilter` has
  `after`/`before` and it is tempting to bound the sync, but `ECalMetaBackend`
  keeps a full local cache and answers ranged queries out of it; narrowing
  here would hide events rather than save work.

Nineteen mutations were run against a copy kept outside the tree: the
baseline replaced by the raw server event, the `maps_recurrence_rule` guard
dropped, that guard consulting the edited rules instead of the stored ones,
`start` allowed to be nulled, each of `timeZone`/`duration`/`status` dropped
from the diff, a cleared property written as `""` instead of `null`, the
create keeping the local id, the create discarding the local uid, an empty
patch sent anyway, `holds()` always true, the listing unfiltered, a created
event elsewhere counted as removed, destroyed ids not reported, the revision
constant, the revision ignoring the component text, and changed events
rendered as empty. **Eighteen died.** Two gaps were found *before* the run,
by asking which arms of the diff no test reached: nothing exercised
`timeZone`, `duration` or `status` through the patch path, so all three could
have been deleted silently —
`moving_an_event_to_another_zone_lengthening_it_and_unconfirming_it_all_arrive`
closes that, and the three mutations now die.

**The one survivor is left alive knowingly.** Dropping the
`delta.updated.contains(…)` filter on `/get`'s `not_found` changes behaviour
only when an event is destroyed *between* the `/changes` call and the `/get`
that follows it — a race the mock cannot be made to produce, because both
calls happen inside `get_changes` with no hook between them. The branch is
identical to `jmap-book-sync`'s and correct for the same reason; contorting
the mock to reach it would test the mock. Noted here rather than papered
over.

Not verified locally, as in the previous thirteen sessions: `reuse lint` and
`cargo deny` (neither tool is installed on this VM; both run in CI). The six
new files carry an SPDX `GPL-3.0-or-later` header and `rust/Cargo.lock` is
already covered by `REUSE.toml`. `cargo fmt --check`, `cargo test --locked`
(138) and `cargo clippy --all-targets -D warnings` are clean on the default
members and on `-p eds-sys -p jmap-backend-core -p jmap-backend-book` (109),
`cargo doc -p evolution-jmap-cal-sync --no-deps` is warning-free, and a clean
`cmake -S . -B <fresh> && cmake --build && ctest` is 3/3 — `rust-test` runs
plain `cargo test`, so the new crate needed no build-system wiring.

No blockers hit.

Next, in M4: `jmap-backend-cal`, the `ECalMetaBackend` subclass — the mirror
of `jmap-backend-book`, and the first calendar code that needs the EDS
headers, so it goes into `rust/crates` but stays out of `default-members`.
`e_cal_meta_backend_*` vfuncs take an `ICalComponent` where the book side
took a `vCard` string, so the C boundary is `i_cal_component_new_from_string`
/ `i_cal_component_as_ical_string` around what `CalSync` already returns;
`get_changes_sync` has a wider signature than the book's (it hands back three
`GSList`s rather than two) and is the piece to read the headers for first.

## 2026-08-08 (fifteenth session)

M4 continues: the calendar backend's **C boundary**, which is where the
calendar stops being a translation of the address book. Two commits.

**`eds-sys` reaches libical-glib and `ECalComponent`.** The `ECalMetaBackend`
vfuncs do not traffic in strings the way the book's do: `load_component_sync`
hands back an `ICalComponent *` and `save_component_sync` is given a `GSList`
of `ECalComponent *`. Both types were already in the bindings — pulled in
transitively as fields and arguments — but none of their *functions* were, so
nothing could be done with either. Allowlisted `i_cal_component_.*` and
`e_cal_component_.*` (not all of `i_cal_.*`: `jmap-ical` does the property- and
value-level work in Rust on the text, so the component is the only libical type
that has to cross at all), plus the `ICal.*` / `ECalComponent.*` type families,
which is what brings the class structs `tests/layout.rs` checks against
`g_type_query`. No pkg-config change was needed: `libedata-cal-2.0` already
carries libical-glib's include path in its Cflags and `-lical-glib` in its
Libs.

The new `tests/ical.rs` pins the ownership rules the marshalling rests on, and
one of them was learned the hard way: `i_cal_component_take_component` takes
ownership, so a component reached through its parent — which is every component
the vfuncs hand us, since an `ECalComponent` only *lends* its own out — has to
be cloned before it can be given to another one. The first version of that test
aborted the process on a double free.

**`jmap-backend-cal`, so far just `marshal`** (13 tests). Out of
`default-members`, in `rust-test-eds`, rlib only for now — the cdylib arrives
with the module entry point, and installing a shared object that loads and
resolves nothing would be worse than not installing one.

Decisions taken:

- **An envelope with no `VEVENT` in it is refused.** libical reports junk as
  NULL, which `EVCard` did not, but it parses an empty `VCALENDAR` happily; and
  `load_component_sync` handing that back would reach Evolution as an
  appointment that exists and has no properties.
- **The master instance is found by having no `RECURRENCE-ID`, not by
  position.** EDS passes every instance of one uid; taking the first node would
  map a single moved occurrence as if it were the whole series. The overrides
  are then dropped, which is the mapping's existing story rather than a new
  loss — JSCalendar keeps them in `recurrenceOverrides`, which `jmap-ical` does
  not cover, so a save never names that property and never overwrites what the
  server holds. A set of instances with **no** master is refused rather than
  guessed at: a visible failure beats rewriting a series to look like one moved
  day.
- **Removals are `ECalMetaBackendInfo`s carrying only a uid.** This is a real
  divergence from `EBookMetaBackend`, whose `out_removed_objects` is a list of
  bare strings; the same list here would be read as structs, dereferencing the
  first bytes of a uid as pointers. `e_cal_meta_backend_info_new` documents
  `revision`, `object` and `extra` as nullable and the uid as not, which is
  exactly what a component that is gone can say.

Eight mutations were run against a copy kept outside the tree: the master taken
by position, the master handed to `take_component` instead of a clone, the
eventless-envelope guard dropped, an empty uid let through, removals rendered
as `g_strdup`'d strings, the info list built in reverse, and the uid read
without descending into the envelope. **Seven died.** The eighth was not a
missing test but a redundant branch: `component_uid` descended into the
`VCALENDAR` by hand, and libical already does that — `icalcomponent_get_uid`
reads the first *real* component of whatever it is given — so the branch was
deleted and the two tests that cover both shapes now pin libical's behaviour
instead of ours. The empty-uid guard also needed a test that reaches it: a
parsed `UID:` line folds to absent, so only a uid *set* to `""` stays empty,
and the test says so.

Noticed in passing, not fixed: `cargo clippy --all-targets --workspace` reports
five `manual_c_str_literals` errors in `example-module`, which predate this
session and are invisible to CI because it clippies the default members only.
Left alone deliberately — it is not this milestone's crate and the rule is one
increment per session.

Not verified locally, as in the previous fourteen sessions: `reuse lint` and
`cargo deny` (neither tool is installed on this VM; both run in CI). The five
new files carry an SPDX `GPL-3.0-or-later` header. `cargo fmt --check`,
`cargo test --locked` (138 on the default members, 129 across the four EDS
crates) and `cargo clippy --all-targets --locked -- -D warnings` are clean both
ways, and a fresh `cmake -S . -B <tmp> && cmake --build && ctest` is 3/3 with
the new crate wired into `rust-test-eds`.

No blockers hit.

Next, in M4: `ops`, the vfunc bodies over a `CalSync`. Three of the four
helpers the book's `ops` uses are not calendar-specific — `set_out_string`,
`read_string` and `password` — so that is the moment to move them into
`jmap-backend-core::marshal` rather than copy them; this session did not need
any of them and so left the book alone. The signatures differ from the book's
in more than the object type: `save_component_sync` is not told which uid it is
saving (it has to come out of the instances, which is why `marshal` reports it),
`get_changes_sync` takes an `is_repeat` flag, and both it and
`remove_component_sync` carry an `EConflictResolution` this backend has no
answer for yet.

## 2026-08-08 (sixteenth session)

M4 continues: `ops`, the calendar's vfunc bodies. Three commits.

**`jmap-backend-core::marshal`, the boundary that is nobody's in particular.**
`read_string`, `set_out_string`, `set_out_list`, `dup_string` and `password` are
about out-parameters and libsecret, not about contacts, and the calendar needed
all five. Moving them rather than copying them turned out to remove a copy that
was already there: `core::source` had grown its own `read_string` with the same
"" -reads-as-absent rule, so this is the third copy avoided rather than the
second. What stayed in each backend's own `marshal` is the part that is not
type-agnostic — an `EBookMetaBackendInfo` and an `ECalMetaBackendInfo` are
neither the same struct nor freed by the same function.

**`eds-sys` reaches the calendar's error domain.** `ECalClientError` and
`e_cal_client_error_.*`, for `E_CAL_CLIENT_ERROR_OBJECT_NOT_FOUND`. The new
`tests/errors.rs` pins the property the whole mapping rests on: the three client
error domains are three *different* quarks, so the address book's
`CONTACT_NOT_FOUND` cannot stand in for the calendar's not-found however equal
the numbers look — `ECalMetaBackend` matches on the pair, and a near miss is a
cache entry that never goes away.

**`jmap-backend-cal::ops`** (17 tests), mirroring the book's five bodies over a
`CalSync`.

Decisions taken:

- **Three vfunc arguments do not appear in these signatures.** `extra` is
  per-object opaque cache state this backend has none of, as on the book side.
  `EConflictResolution` is a promise `CalSync` cannot keep yet — JMAP can express
  it as an `ifInState` on `CalendarEvent/set` and we do not send one, so taking
  the argument and ignoring it would read as support. `ECalOperationFlags`
  carries iTIP scheduling requests, which M4 does not implement.
- **`is_repeat` is taken and ignored**, unlike those three, because the vfunc has
  it where the book's does not and the omission would be the more surprising
  reading. It cannot be true of anything this backend asked for: the paging
  happens inside `CalSync::get_changes` and `out_repeat` is always FALSE. A test
  calls with both values and asserts the same delta, which is what "the flag has
  nothing to change" means concretely.
- **Removals go out through `removed_info_list`**, so all three change lists are
  `ECalMetaBackendInfo`s and the test frees all three with
  `e_cal_meta_backend_info_free`. The assertion is on the whole node, not just
  the uid: a removal must not claim a revision or an object either.

Learned the hard way, and now the subject of a test comment:
**`e_cal_component_new_from_string` invents a `UID`** when the text has none
(`e_util_generate_uid`, a 40-hex-digit checksum). The first version of the
"an edit without an identifier is refused" test built a `VEVENT` with no `UID`
and got back a component EDS had named, so the save reached the server and failed
as a not-found — the right shape of failure in the wrong domain. The test now
empties the uid on the instance it hands over, which is the only way to reach the
guard. That is also the argument for keeping the guard: what it defends against
is a uid that reads back as nothing, and EDS's own generosity with identifiers is
exactly what would otherwise hide it in testing.

Six mutations were run against a copy kept outside the tree: the not-found
reported in the generic client domain, `overwrite_existing` inverted,
`out_repeat` never written, a master-less save returning TRUE, the empty sync tag
passed on to the server as a state, and a load without an identifier defaulting
to `""`. **All six died.** (The empty-tag mutation also took 30 s rather than the
usual 0.07 s, because the fallback test stops the server first and the mutant
actually went out on the network to find that out — the timing is a second signal
that the test reaches what it claims to.)

Not verified locally, as in the previous fifteen sessions: `reuse lint` and
`cargo deny` (neither tool is installed on this VM; both run in CI). The four new
files carry an SPDX `GPL-3.0-or-later` header. `cargo fmt --check`,
`cargo test --locked` (138 on the default members, 150 across the four EDS
crates) and `cargo clippy --all-targets --locked -- -D warnings` are clean both
ways, and a fresh `cmake -S . -B <tmp> -G Ninja && cmake --build && ctest` is
3/3.

No blockers hit.

Next, in M4: the `ECalMetaBackend` subclass and the module entry point — the
mirror of `jmap-backend-book`'s `backend.rs`, `connect.rs`, `factory.rs` and
`module.rs`, at which point the crate grows its cdylib and the CMake install
rule. `connect.rs` is the piece to read first: `SourceConfig` reads an
`address_book_id` out of `ESourceResource:identity`, and the calendar wants the
same field to mean a calendar id — one field, two names, and renaming it touches
the book.

## 2026-08-08 (seventeenth session)

M4 continues: `connect_sync` for the calendar. Three commits.

**The increment was supposed to be one new module and turned out to be one
moved one.** `jmap-backend-book::connect` is 258 lines, of which about eight
are about contacts: the capability URN it resolves the account under, and the
list it looks `[Resource] Identity` up in. Everything else — the NULL-source
guard, the `SourceConfig` read, the credentials-or-prompt choice, the
cancellable bridge, both out-parameters and the `ConnectError` classification
behind them — is the same code the calendar needs, and copying it would have
duplicated the one decision here that must not be made twice: which failures
make Evolution discard the stored password and ask again. A rule written twice
is a rule corrected once. So `jmap-backend-core::connect` now holds all of it,
parameterised by a `Collection` that decides nothing except how the failure is
worded, and both backends are down to `open_book` / `open_calendar`.

Decisions taken:

- **`ConnectError::NoSuchAddressBook` becomes `NoSuchCollection(Collection,
  String)`**, rather than the enum growing a calendar-shaped twin of each
  variant. `Collection` survives at all only because it reaches the user: "the
  account names calendar \"Cal-1\", which the server does not have" is a
  sentence someone can act on and "names collection \"Cal-1\"" is not. A core
  unit test asserts the noun in the message for both, because a backend that
  reported the other one's would send someone editing the wrong account.
- **`SourceConfig::address_book_id` is now `resource_id`.** It is one keyfile
  field, `[Resource] Identity`, that means an address book id under one backend
  and a calendar id under the other. Naming it after the field rather than
  after either meaning is the only name that is not wrong half the time. The
  night log of the previous session flagged this as the piece to read first,
  and it was right to: the rename touches the book, its recipe test and
  `core::source`'s docs.
- **`resolve` takes `(Option<&Id>, Option<bool>)` pairs**, not a slice of
  `AddressBook` or `Calendar`. The two protocol structs have nothing else in
  common that this code needs, and a trait to unify them would be three
  impls to say "id and is_default". It also made room for a rule neither
  backend had stated: a collection the server flagged default *without* giving
  it an id is not a candidate, and must not shadow one that has an id.
- **The capability constant stays in each backend**, which is the one line
  `connect_with` could not absorb and the one line that had no test.

**The mock could not tell the two capabilities apart, so it can now.** Halfway
through, a mutation — the calendar backend asking the session for
`urn:ietf:params:jmap:contacts` — survived the whole suite. It had to: every
account `jmap-mockd` serves offers all four capabilities, so looking an
account up under the wrong URN returns the right account. `MockServerBuilder::
without_capability` drops a URN from all three places the session document
mentions it (server capabilities, account capabilities, `primaryAccounts`),
which is what a server offering only mail actually looks like. Both backends
now have a test that points them at a server missing *their* capability; the
mutation dies on the calendar's.

Five mutations were run against a copy kept outside the tree: the unnamed
default resolving to the first collection listed, the calendar ignoring
`resource_id` entirely, a named user with no password connecting anonymously,
the calendar asking for the contacts capability (twice — once before
`without_capability` existed and once after). **All died, the capability one
only after the mock could express the difference**, which is the honest version
of what a green suite meant an hour earlier.

Not verified locally, as in the previous sixteen sessions: `reuse lint` and
`cargo deny` (neither tool is installed on this VM; both run in CI). The three
new files carry an SPDX `GPL-3.0-or-later` header. `cargo fmt --check`,
`cargo test --locked` (36 test binaries green across the default members and
the four EDS crates) and `cargo clippy --all-targets -- -D warnings` are clean
on both member sets, and a fresh `cmake -S . -B <tmp> -G Ninja && cmake --build
&& ctest` is 3/3.

No blockers hit.

Next, in M4: the `ECalMetaBackend` subclass, the factory and the module entry
point — `jmap-backend-book`'s `backend.rs`, `factory.rs` and `module.rs`, now
that the layer under them exists on both sides. That is the point at which
`jmap-backend-cal` grows its cdylib and the CMake install rule, and where the
calendar's manual test recipe (the mirror of
`docs/manual-test-book-backend.md`) becomes writable — its `.source` keyfile
differs from the book's only in `[Calendar] BackendName=jmap` and what
`Identity` names.

## 2026-08-08 (eighteenth session)

M4 continues: the `ECalMetaBackend` subclass. One commit.

`jmap-backend-cal::backend` is the calendar's `JmapCalBackend` /
`JmapCalBackendClass` pair, its seven vfunc slots, and the connection slot
those slots read — the layer that had nothing under it three sessions ago and
now has all of it. Ten tests, every one of them dispatching *through the class
struct* rather than at the Rust functions, because a vfunc that is correct and
not installed is the failure this file exists to make impossible.

Decisions taken:

- **The book's `backend.rs` was copied, not shared**, and this is the one place
  in the crate where that is the right answer. What the two files have in common
  is a shape; what they do not have in common is a single line that could be
  called from both — every signature names `ECalMetaBackend`, `ICalComponent`
  and the calendar's own class struct, and there is no slot both backends could
  be installed into. The decisions worth writing once — how a failure is
  classified, when the stored password is discarded, what an unnamed default
  resolves to — already live in `jmap-backend-core`, which is why the previous
  session moved them there. Factoring the residue would mean a trait with two
  implementors and one caller each.
- **`search_sync` and `search_components_sync` stay the parent's**, and there is
  now a test that asserts the slots are still literally the pointers
  `ECalMetaBackendClass` shipped. `ECalMetaBackend` answers a query by running
  the S-expression over the offline cache, which for a just-synced calendar is a
  complete answer; JMAP's `CalendarEvent/query` cannot express an S-expression
  at all, so anything installed there would be a narrower filter replacing a
  working one. The book has no equivalent test because `EBookMetaBackend` has no
  equivalent slot — this is the one structural difference between the two
  backends, rather than a naming one.
- **`fail_offline` reports in the *client* error domain**, not the calendar's,
  which is the opposite of `ops::to_gerror`'s choice for a missing component.
  The rule behind both: `E_CAL_CLIENT_ERROR` is for statements about a
  component, and "there is no connection" is not one — it also has no offline
  code to make it in. `E_CLIENT_ERROR_REPOSITORY_OFFLINE` is what makes
  `ECalMetaBackend` serve its cache instead of showing the user a broken
  calendar.
- **`is_repeat` is passed to `ops::get_changes` even though it ignores it.**
  The alternative — dropping it at the boundary — would put the reasoning for
  why the flag has nothing to change in a file that no longer receives it. It
  is also what the chain-up needs, unchanged, if the answer is `ListInstead`.

Five mutations were run against a copy kept outside the tree: the
`load_component_sync` slot left uninstalled, `fail_offline` returning TRUE, a
`finalize` that leaves the slot alone, `parent_type` answering with
`e_book_meta_backend_get_type`, and `search_sync` nulled out in `class_init`.
**All five died** — the last one only because of the slot-identity test written
in this session, which is the point of writing it.

Not verified locally, as in the previous seventeen sessions: `reuse lint` and
`cargo deny` (neither tool is installed on this VM; both run in CI). The two new
files carry an SPDX `GPL-3.0-or-later` header. `cargo fmt --check`,
`cargo test --locked` (36 test binaries green on the default members, and the
four EDS crates green on top, `jmap-backend-cal` now at 10 + 10 + 15 + 17) and
`cargo clippy --all-targets --locked -- -D warnings` are clean on both member
sets, and a fresh `cmake -S . -B <tmp> -G Ninja && cmake --build && ctest` is
3/3.

No blockers hit.

Next, in M4: the factory and the module entry point — `jmap-backend-book`'s
`factory.rs` and `module.rs`, which is where `jmap-backend-cal` grows its
cdylib (`crate-type` is still `["rlib"]`, deliberately, because a shared object
that loads and resolves to nothing is worse than none) and its CMake install
rule into the libedata-cal backend directory. `ctest` gains an
`install-cal-backend` next to the book's. After that the calendar's manual test
recipe, the mirror of `docs/manual-test-book-backend.md`, differing only in
`[Calendar] BackendName=jmap` and what `Identity` names.

## 2026-08-08 (nineteenth session)

M4 continues: the factory and the module entry point. One commit, and with it
`jmap-backend-cal` becomes a shared object EDS can actually load.

`factory.rs` is the `ECalBackendFactory` subclass and `module.rs` the
`e_module_load` / `e_module_unload` pair; `cmake/Backends.cmake` installs the
cdylib as `libecalbackendjmap.so` into the directory `libedata-cal-2.0` reports
as its `backenddir`. Seven tests in `tests/factory.rs`, all of them asserted
through the class struct and reached through a stand-in `GTypeModule` whose
`load` calls our entry point — the same shape as the book's, because the thing
being tested is the path EDS takes rather than the functions along it.

Decisions taken:

- **The calendar factory declares `component_kind`, and declares only
  `I_CAL_VEVENT_COMPONENT`.** This is the one field `EBookBackendFactoryClass`
  has no counterpart for, and it is the reason this file is not the book's with
  the names changed. `ECalBackendFactory` keys itself by name *and* kind — the
  hash key is `"jmap:VEVENT"` — so declaring events alone means a task list or a
  memo list naming `BackendName=jmap` finds no factory at all. That is the
  honest answer while `jmap-cal-sync` maps `CalendarEvent`s and JMAP has no
  standardised task or note type: registering `VTODO` and `VJOURNAL` factories
  would produce backends that connect, sync nothing, and look broken rather than
  absent. The same field is what EDS passes to `g_object_new` as `kind`, so
  `e_cal_backend_get_kind` reports it to every client — a test pins the value
  rather than the hash key, because building a key needs an instantiated
  `EBackendFactory`, which is an `EExtension` and needs something extensible to
  attach to.
- **Both backends are called `jmap` and both export `e_module_load`**, and
  neither is a clash. They are dlopened by different factory processes out of
  different backend directories (`addressbook-backends`, `calendar-backends`),
  and collected into different hash tables. It also means one account can name
  one backend for both of its collections, which is what M6 will write.
- **`FORCE_INSTALL_PREFIX` is honoured for the calendar directory too**, by the
  same `pkg_check_variable` + `string(REGEX REPLACE)` pair the book uses. Not
  factored: the two differ in the pkg-config module, the variable and the
  temporary, which is all three lines of it.

Mutation testing, and the two that did *not* die:

Three died as intended — `component_kind` left out of `class_init`, the factory
answering to `jmap-calendar`, and the module registering its types statically
instead of against the `GTypeModule`.

- **Dropping `remember_backend_type` survived, and is an equivalent mutant
  rather than a test gap.** With `BACKEND_TYPE` still zero, the factory's
  fallback calls `register_static::<JmapCalBackend>()`, and
  `jmap-backend-core::subclass::register` hands an already-registered name
  straight back on the static path — so the fallback resolves to the very type
  `register_dynamic` produced a moment earlier, and every observable is
  unchanged. The memoisation still earns its place: it is what keeps the factory
  from registering the backend *statically* if it is ever the first of the two to
  be reached, which is the case the fallback's doc comment is about. The book's
  factory has the same property.
- **Reverting `crate-type` to `["rlib"]` survived in an incremental build tree
  and died in a fresh one.** Cargo does not remove the `.so` it no longer
  builds, so the install rule copied a stale artifact and `install-cal-backend`
  passed. Deleting the file first and rebuilding failed the test as it should.
  CI configures from scratch, so it is caught there; noted because the same hole
  is in `install-book-backend`, and a `ctest` on a warm tree is weaker evidence
  than it looks.

Not verified locally, as in the previous eighteen sessions: `reuse lint` and
`cargo deny` (neither tool is installed on this VM; both run in CI). The three
new files carry an SPDX `GPL-3.0-or-later` header. `cargo fmt --check`,
`cargo test --locked` (36 test binaries green on the default members, and the
four EDS crates green on top, `jmap-backend-cal` now at 10 + 14 + 13 + 17 + 7)
and `cargo clippy --all-targets --locked -- -D warnings` are clean on both
member sets, and a fresh `cmake -S . -B <tmp> -G Ninja && cmake --build &&
ctest` is 4/4 — `install-cal-backend` reports a 3.4 MB
`libecalbackendjmap.so` in `/usr/lib/evolution-data-server/calendar-backends`
exporting both entry points.

No blockers hit.

Next, in M4: the calendar's manual test recipe, the mirror of
`docs/manual-test-book-backend.md`, and the `tests/recipe.rs` that checks it —
the book's recipe test builds an `ESource` from the documented keyfile through
`e_server_side_source_*` and asserts `core::source` reads back what the recipe
promises, and the calendar's differs only in `[Calendar] BackendName=jmap` and
what `Identity` names. That closes M4's acceptance criteria, and M5 (the Camel
provider) is the next milestone.

## 2026-08-08 (twentieth session)

M4's last acceptance criterion: the calendar's manual test recipe, and the
tests that keep it from rotting. One commit, and with it M4 is complete.

`docs/manual-test-cal-backend.md` and the keyfile it tells the reader to copy,
`docs/examples/jmap-mock-calendar.source`, with four tests in
`tests/recipe.rs` reading that keyfile through `e_server_side_source_new` — the
call `evolution-source-registry` makes for every file in its sources directory,
and one that needs neither a bus nor a running daemon. The shape is the book's
`tests/recipe.rs`; what is new is the group the `BackendName` sits in.

Decisions taken:

- **The calendar-specific assertion is about the extension group, not the
  backend name.** `ECalBackendFactory` keys itself by name *and* component
  kind, and the keyfile spells the kind by choosing a group: `[Calendar]` is
  `VEVENT`, `[Task List]` is `VTODO`, `[Memo List]` is `VJOURNAL`. Since the
  module registers `jmap:VEVENT` alone, a recipe that said `[Task List]` would
  document a source that parses, appears in the registry, and is claimed by no
  factory — the silent failure the whole file exists to prevent. So the test
  carries EDS's own kind → extension table and asserts the pair
  (`factory::COMPONENT_KIND`, `[Calendar]`) agrees, rather than pinning the
  group name on its own: the day a second factory is registered, the assertion
  that fires is the one that says the document needs a second keyfile.
- **`register_calendar_extensions` before every `has_extension` call.** The
  negative half — no `[Task List]`, no `[Memo List]` — is worthless without it:
  `e_source_has_extension` answers out of the table the source built while
  parsing, and a group whose extension type was never registered leaves no
  entry, so "the keyfile has no task list group" and "this binary never
  mentioned task lists" would be the same answer.
- **The `Method=none` versus `Secure=true` pair is not repeated here.** It is a
  property of `SourceConfig`, which both backends share verbatim, and it is
  pinned once in the book's recipe test. The document still explains it, with a
  pointer to the long version.
- **A second keyfile rather than a second group in the first.** One `.source`
  file could carry both `[Address Book]` and `[Calendar]`, but its file name is
  its UID, and the two recipes are meant to be runnable independently and in
  either order.

Two things in the recipe were wrong as first written and were fixed by checking
them rather than by reasoning:

- **The documented `curl` used `/jmap/api`.** The mock's `apiUrl` is `/jmap`;
  `/jmap/api` answers `404 no route`. Run against `jmap-mockd` on a spare port,
  corrected, and the real `Calendar/get` response (`"id":"CAL1"`,
  `"isDefault":true`) is now quoted in the document. It also carried a
  pointless `-u ignored:`, since the mock in this recipe is started without
  credentials.
- **`EDS_CALENDAR_MODULES` was a guess** and happens to be right: confirmed out
  of `strings libedata-cal-2.0.so.2`, next to `EDS_ADDRESS_BOOK_MODULES` in the
  book's library. Worth recording that the fenced `console` blocks are prose to
  the test suite — only the `ini` block is checked against the keyfile — so
  every command in a recipe is a claim that has to be run once by hand.

Mutation testing: five mutants, all dead. `[Calendar]` → `[Task List]` in the
keyfile (3 of 4 tests fail), `BackendName=jmap` → `jmap-calendar` (2),
`Method=none` deleted (1 — the origin becomes `https://`), `extension_for`
mapping `VEVENT` onto the task list (1), and one line of the recipe's quoted
`ini` block edited away from the keyfile (1).

Not verified locally, as in the previous nineteen sessions: `reuse lint` and
`cargo deny` (neither tool is installed on this VM; both run in CI). The new
`.rs` file carries an SPDX `GPL-3.0-or-later` header; the two `docs/` files are
covered by the `docs/**` annotation in `REUSE.toml`, like the book's recipe and
its keyfile. `cargo fmt --check`, `cargo test --locked` (36 test binaries green
on the default members, and the four EDS crates green on top, `jmap-backend-cal`
now at 10 + 14 + 13 + 17 + 7 + 4) and `cargo clippy --all-targets --locked --
-D warnings` are clean on both member sets, and a fresh `cmake -S . -B <tmp> -G
Ninja && cmake --build && ctest` is 4/4.

One process note, since it cost a test run: the mutation script `cd`-ed to the
repository root and ran `cargo` there, where there is no `Cargo.toml` — the
workspace is `rust/`. Every mutant "passed" by printing nothing, which looked
like five dead mutants until the greps came back empty. Re-run from `rust/`,
where they died for real.

No blockers hit.

M4 is done: the backend, the factory, the module entry point, the install rule
and now the recipe, against every acceptance criterion the roadmap names.
Next is M5, the Camel mail provider — the largest milestone in the file. The
first tractable increment is not `CamelJmapStore` but the crate and its entry
point: Camel resolves `camel_provider_module_init`, not `e_module_load`, out of
a `libcameljmap.so` in Camel's own provider directory (`camel-1.2`'s
`camel_providerdir`) alongside a `.urls` file, and none of that machinery
exists yet. `eds-sys` currently allowlists no `Camel*` type, so the increment
before even that one is teaching its build script about `camel-1.2` and pinning
`CamelProvider`'s layout in `tests/layout.rs` the way every other type crossed
so far has been.

## 2026-08-08 (twenty-first session)

M5's first increment, the one the previous entry named: `eds-sys` now probes
`camel-1.2` and allowlists the mail provider's object graph, and the two types
of check that pin it are in place. One commit.

`build.rs` gains `camel-1.2` in the pkg-config loop (same 3.52 floor — Camel
ships in the same tarball and carries the same version), `wrapper.h` gains
`#include <camel/camel.h>`, and the allowlist gains `CamelProvider`,
`CamelService`, `CamelStore`, `CamelOfflineStore`, `CamelTransport`,
`CamelSession`, `CamelSettings`, `CamelNetworkSettings`, `CamelURL` and the
matching function prefixes, plus `EDS_CAMEL_PROVIDER_DIR`. 207 `camel_*`
functions and ~3.5k lines of bindings, which is the whole cost of the increment.
New `tests/camel.rs` (4 tests) and a seventh test in `tests/layout.rs`.

Decisions taken:

- **`CamelProvider` cannot be layout-checked the way every other type has
  been.** `camel_provider_get_type()` registers a **boxed** type, so
  `g_type_query()` reports `instance_size == 0` and `class_size == 0`: the
  `assert_layout!` macro would compare `size_of` against zero and pass whatever
  the struct looked like. That is not a small hole — a provider is the one
  struct the mail module hands to C *by value*, so it is the one place a wrong
  offset cannot be caught by GObject at all. What stands in for it is a round
  trip: build a provider in Rust with `'static` C literals, `camel_provider_
  register` it under a protocol nobody else uses, and read it back out with
  `camel_provider_get`. Verified this is really sensitive to offsets rather than
  merely plausible: a scratch test registering a struct with one field removed
  **segfaults inside `camel_provider_register`**. So a layout drift here is a
  crash, not a wrong answer, which is the argument for the test existing.
- **A first test whose whole content is the boxed-type finding.** It asserts
  `G_TYPE_FUNDAMENTAL(camel_provider_get_type()) == G_TYPE_BOXED` and both
  query sizes zero. It would be tempting to leave this out as a curiosity, but
  it is the reason the file exists, and the day Camel makes the provider a
  classed type it should fail and send the reader to `layout.rs`.
- **Store, transport, service, session, settings — and deliberately no
  `CamelFolder`, `CamelMimeMessage` or `CamelFolderSummary`.** The store's
  folder work is the next increment and each prefix here is another class struct
  the layout test has to vouch for. `CamelOfflineStore` rather than plain
  `CamelStore` as the store parent, since the summary cache has to work
  disconnected.
- **`camel_provider_module_init` is allowlisted as a function even though Camel
  only declares it.** Same reasoning as `e_module_load`: with the declaration
  in scope, the module's `extern "C"` definition is a signature the compiler
  checks rather than a guess.

One comment was written wrong and corrected by running it rather than by
re-reading it: it claimed registering into a table `camel_provider_init()` had
not created was a no-op. Removing the `init` call leaves all four tests
passing — the table is created lazily. The call stays, because it is the state a
loaded `libcameljmap.so` finds itself in, but the comment now says what was
observed. Repeated `camel_provider_init()` calls were checked separately (three
in a row, from C) rather than assumed idempotent.

Mutation testing: five mutants, all dead. Swapping the store and transport slots
in `object_types` (1 test fails), a typo in `EDS_CAMEL_PROVIDER_DIR` (1),
dropping `CamelOfflineStore.*` from the type allowlist (`layout.rs` stops
compiling), dropping `EDS_CAMEL_PROVIDER_DIR` from the var allowlist (`camel.rs`
stops compiling), and dropping `camel-1.2` from the pkg-config loop — which is
worth recording: **bindgen still succeeds**, because Camel's headers sit under
the same `-I/usr/include/evolution-data-server` the data-server packages
already add, so the entry earns its place by emitting `-lcamel-1.2`, and its
absence is a link error rather than a missing-type error.

Not verified locally, as in the previous twenty sessions: `reuse lint` and
`cargo deny` (neither tool is installed on this VM; both run in CI). The new
`.rs` file carries an SPDX `GPL-3.0-or-later` header. `cargo fmt --check`,
`cargo test --locked` (36 test binaries green on the default members, the four
EDS crates green on top, `eds-sys` now at 2 + 6 + 7 + 4) and `cargo clippy
--all-targets --locked -- -D warnings` are clean on the default members and on
each EDS crate, and a fresh `cmake -S . -B <tmp> -G Ninja && cmake --build &&
ctest` is 4/4. `example-module` still fails clippy on `manual_c_str_literals`,
as it did before this change; it is hand-written FFI, outside `default-members`
and outside the set CI lints.

No blockers hit.

Next in M5: the `jmap-mail` crate — a `libcameljmap.so` exporting
`camel_provider_module_init`, whose provider names a `CamelJmapStore` GType and
nothing else yet, plus the CMake install rule into `camel-1.2`'s
`camel_providerdir` (`/usr/lib/evolution-data-server/camel-providers` here,
readable via `pkg_check_variable`) and the `ctest` that checks the artifact
exports the entry point — the mirror of `install-cal-backend`. The `.urls` file
belongs with it: Camel reads it to know which protocols a module provides
without dlopening it.

## 2026-08-08 (twenty-second session)

M5's second increment, the one the previous entry named: `jmap-mail`, a
`libcameljmap.so` that exports `camel_provider_module_init` and registers a JMAP
provider naming a `CamelJmapStore`, plus the `.urls` file beside it, the CMake
install rule into Camel's provider directory and the CTest that checks both
arrived. One commit.

The crate is four small files. `store.rs` is the `CamelOfflineStore` subclass,
registered statically; `provider.rs` builds the `CamelProvider` and hands it to
`camel_provider_register`; `module.rs` is the exported symbol, guarded like every
other C entry point here; `libcameljmap.urls` is the one line Camel reads to
decide whether to dlopen the object at all. `tests/provider.rs` is 10 tests.
`cmake/Backends.cmake` gains the `camel-1.2` probe and the install rule,
`cmake/Rust.cmake` a `DATA` argument to `add_cargo_cdylib` and `-p jmap-mail` in
`rust-test-eds`, and `REUSE.toml` an annotation for the `.urls` file.

Decisions taken:

- **The provider struct is leaked on purpose.** `camel_provider_register` takes
  the pointer and keeps it in a table it never clears, without copying, so the
  struct has to outlive anything that can still reach it — which is the process.
  A `Box::into_raw` behind a `OnceLock` is the honest spelling of that: the
  `OnceLock` is not an optimisation, it is what stops a second
  `camel_provider_module_init` from leaving Camel's table pointing at one struct
  while an earlier caller holds another.
- **The transport slot stays `G_TYPE_INVALID`.** Sending is
  `EmailSubmission/set` and a `CamelJmapTransport` that does not exist. A type
  named there before it works is a crash the first time a user hits Send, which
  is worse than an account that visibly cannot send yet.
- **`translation_domain` is `evolution-jmap`, not NULL.** NULL means, in this
  struct, "a provider inside the EDS source tree, translated from EDS's
  catalogue". These strings are not. There is no catalogue installed under the
  domain yet and gettext falls back to the untranslated string, which is the
  right outcome and not a silent misattribution.
- **`SUPPORTS_SSL` is not a security claim.** It says the user *may choose* an
  encrypted connection, which is what puts the security options in the account
  dialog. Refusing plaintext to anywhere but localhost stays in the client,
  where it can be enforced rather than advertised.
- **`camel_provider_module_init` is a safe `extern "C"`, unlike
  `e_module_load`.** It takes no arguments, so there is no pointer whose
  validity a caller has to promise, and an `unsafe fn` would be claiming a
  contract that does not exist. The declaration is still in scope through
  `eds-sys`, and a test coerces both it and the definition to the same function
  pointer type, so the signature is checked rather than assumed.

Mutation testing found a real bug in the *existing* test infrastructure, which
is the part of this session worth reading. Ten mutants, the last two of which
did not die on the first attempt:

- Six against the Rust: `.urls` claiming `imap` (1 test fails), the store
  parenting on plain `CamelStore` (1), the transport slot filled with the store
  type (1), `domain` spelled `Mail` (1), `NEED_HOST` downgraded to `ALLOW_HOST`
  (1), `IS_STORAGE` dropped (1). All dead.
- A typo in the `DATA` path dies at configure time, because
  `add_cargo_cdylib` checks the file exists.
- **Dropping the `DATA` argument altogether survived.** The install check only
  ever inspected what the caller declared, so a build that simply forgot the
  `.urls` file passed. Fixed by stating Camel's own rule in
  `check-installed-module.cmake`: a module installed as `libcamel<x>.so` needs a
  `libcamel<x>.urls` beside it, full stop. Re-run, the mutant dies.
- **Renaming the exported entry point survived, and this one was not new.**
  `check-installed-module.cmake` looked for the symbol with `file(STRINGS ...
  REGEX)`, on the stated assumption that a match means the dynamic symbol table
  contains the name. It does not: `.dynstr` holds *undefined* symbols too, so
  every module that merely includes the header declaring an entry point passes
  the check for exporting it. `libjmap_mail.so` with the definition renamed to
  `camel_provider_module_lnit` still carried the string once, from `eds-sys`'s
  declaration, and the check was satisfied. The book and calendar backends had
  been passing the same non-check since the rule was written. Replaced with
  `nm --dynamic --defined-only --format=posix`; both the camel mutant and an
  `e_module_unload` rename in `jmap-backend-book` now die on it. `nm` is
  binutils, already in the CI image via `build-essential`.

Not verified locally, as in the previous twenty-one sessions: `reuse lint` and
`cargo deny` (neither tool is installed on this VM; both run in CI). The five new
`.rs` files carry SPDX `GPL-3.0-or-later` headers; `libcameljmap.urls` cannot —
Camel reads every line of it as a protocol name, so a comment header would
register a provider called `# SPDX-FileCopyrightText: ...` — and is annotated in
`REUSE.toml` instead, which the test for its contents also says out loud.
`cargo fmt --check`, `cargo test --locked` (36 test binaries green on the default
members, the five EDS crates green on top, `jmap-mail` at 10) and `cargo clippy
--all-targets --locked -- -D warnings` are clean on both member sets, and a fresh
`cmake -S . -B <tmp> -G Ninja && cmake --build && ctest` is 5/5 — the fifth being
the new `install-camel-provider`.

No blockers hit.

Next in M5: `CamelJmapStore` doing something. The store's own increment is
`connect_sync` plus `get_folder_info_sync` over `Mailbox/get` — the settings
object a service is configured through (`CamelStoreSettings` and
`CamelNetworkSettings` are already allowlisted in `eds-sys`), resolving the JMAP
account out of them the way `jmap-backend-core::source` does for an `ESource`,
and mapping the mailbox tree onto the `CamelFolderInfo` chain Camel expects. That
needs `CamelFolder` and `CamelFolderInfo` in the allowlist, which the previous
session deliberately deferred, so teaching `eds-sys` about them — and the layout
test about the two new class structs — comes first or with it.

## 2026-08-08 (twenty-third session)

M5's third increment, and a deliberate split of the one the previous entry
named. That entry asked for `eds-sys` learning `CamelFolder`/`CamelFolderInfo`
*and* the store's `connect_sync` *and* `get_folder_info_sync` over
`Mailbox/get`. Doing all three at once would have meant writing the mailbox tree
mapping — the part with the real decisions in it — inside a crate that cannot be
tested without EDS headers. So this session did the mapping first, as
`jmap-mail-sync`: the third pure-Rust sync crate, alongside `jmap-book-sync` and
`jmap-cal-sync`, in `default-members` and testable anywhere. One commit.

`MailSync::folder_tree` is `get_folder_info_sync` minus the C: one
`Mailbox/get`, mapped onto a `FolderTree` of `FolderInfo` — id, Camel path,
display name, role, counts, subscription, children. `path.rs` is the
name-to-path encoding, `folder.rs` the tree building, `error.rs` the usual
`SyncError` that keeps `jmap_client::Error` intact for the layer that maps it
onto Camel's error codes. `tests/tree.rs` is 22 tests on hand-written mailbox
lists, `tests/folders.rs` 3 against a live mock. `jmap-mock` gained
`seed_child_mailbox`, so nesting is something a test can set up.

Decisions taken:

- **A mailbox name is not a folder path, and the mapping has to be injective.**
  Camel identifies a folder by a `/`-separated path: it is the key
  `camel_store_get_folder` takes, it is in the `folder://` URIs Evolution saves
  in filters, and it names the folder's directory in the summary cache. A JMAP
  mailbox name is a display string that may hold any character but NUL, unique
  only among siblings. Two mailboxes that map onto one path is a store that
  hands back the wrong folder's mail, so `/` and `%` are percent-encoded per
  component — `%` because it is the escape itself, which is what makes the
  encoding reversible and distinct names stay distinct.
- **`.`, `..` and NUL are encoded because of where the path ends up.** A
  mailbox called `..` is legal JMAP and a directory traversal in the summary
  cache; a NUL truncates the path on the way into C, silently naming a
  different folder. Both are the same class of bug as the `/`, so they get the
  same treatment. `...` is not a filesystem special case and survives verbatim
  — encoding it would make paths noisy for no gain.
- **The illegal duplicate sibling name is settled with the id, not left to
  collide.** `<encoded name>%23<id>`, which no encoded name can produce because
  a `%` in a name is escaped. Which of the two keeps the plain path is decided
  by sibling order and not by reply order, because the path is persisted in the
  cache and in saved filters and may not move between sessions — that is what
  the id tie-break in the sort is for, and a test feeds the same two mailboxes
  in both orders.
- **A broken tree may not lose a mailbox.** A `parentId` naming a mailbox the
  account cannot see, and a `parentId` cycle, both end up as extra top-level
  folders rather than as folders missing from the tree, because a missing
  folder is mail the user cannot reach. Only a violation that leaves nothing to
  show — no id, no name, an id used twice — is an error.
- **Cycles are cut, not detected-and-rejected, and the cut is what makes the
  walk terminate.** After walking the real roots, any unvisited mailbox is in a
  loop or hangs off one; cutting the first such mailbox's parent link makes it
  a root and walks it and everything below it, including the rest of its cycle.
  Each pass cuts one mailbox, so it terminates, and what it leaves is a forest.
  This subsumes the self-parent case — a cycle of one — which is why there is
  no separate check for it; the test for a self-parented mailbox stayed, and now
  also asserts it is not its own subfolder.
- **The walk is iterative.** A pre-order over a server-supplied parent chain is
  a stack overflow waiting for a server with 100k nested mailboxes. Same
  reasoning as `MAX_CHANGES_PAGES` in the other two sync crates: the input is
  not ours.
- **An absent `isSubscribed` means subscribed, and an absent count means zero.**
  A server that does not model subscriptions must not end up with every folder
  hidden. Counts saturate into Camel's 32 bits rather than wrapping, because a
  wrap reads as a nearly-empty folder.
- **Only the six roles this crate can act on are mapped.** The rest of the RFC
  8457 registry (`\Important`, `\Flagged`) describes a view, not a folder type
  Camel has. Roles are matched case-insensitively although RFC 8621 §2 requires
  lower-case: a server that shouts `Inbox` is broken, but the user's inbox
  should still be the inbox. A role claimed twice goes to the first claimant in
  sibling order, because `camel_store_get_inbox_folder` has to answer with one
  folder and the answer should be the same on every run.

Mutation testing, twelve mutants, one survivor worth recording:

- Ten died on the first attempt: dropping the `%` escape, the `..` encoding or
  the NUL encoding (2 test failures each), `isSubscribed` defaulting to false,
  every role claimant keeping its role, subfolder order not restored after the
  reverse assembly, the id tie-break dropped, duplicate paths left colliding,
  `sortOrder` ignored, root order not restored (13 failures), and an orphan
  marked visited instead of promoted to a root.
- **Wrapping the counts instead of saturating survived**, because the test used
  `u64::MAX` — which casts to exactly `u32::MAX`, so a wrapping cast and a
  saturating one agree on it. The test now uses `u32::MAX + 5`, where a wrap
  reports five messages: a plausible-looking lie rather than an obvious one.
  Re-run, the mutant dies. A reminder that a pathological test value can be
  pathological in the wrong direction.
- Two tests were added because a mutant showed nothing covered them:
  `subfolders_are_ordered_among_themselves` (no earlier test had two children
  under one parent) and the reversed-input half of the duplicate-name test.

Not verified locally, as in the previous twenty-two sessions: `reuse lint` and
`cargo deny` (neither tool is installed on this VM; both run in CI). All six new
files carry SPDX `GPL-3.0-or-later` headers. `cargo fmt --check`, `cargo test
--locked` (40 test binaries green on the default members, up from 36 — the new
crate is 22 + 3 — and the five EDS crates green on top) and `cargo clippy
--all-targets --locked -- -D warnings` are clean on both member sets, and a
fresh `cmake -S . -B <tmp> -G Ninja && cmake --build && ctest` is 5/5.
`example-module` still fails to link its lib test and still fails clippy on
`manual_c_str_literals`, as it did before this change; it is hand-written FFI,
outside `default-members` and outside the set CI lints.

No blockers hit.

Next in M5: the other half of what the previous entry asked for, now with the
mapping already written and tested. `eds-sys` learns `CamelFolder` and
`CamelFolderInfo` (and `tests/layout.rs` learns their class structs), and
`CamelJmapStore` gets `connect_sync` plus `get_folder_info_sync`: resolving the
JMAP account out of `CamelStoreSettings`/`CamelNetworkSettings` the way
`jmap-backend-core::source` does for an `ESource`, then turning a `FolderTree`
into the `CamelFolderInfo` chain — a `g_malloc`ed linked forest whose ownership
rules (`camel_folder_info_free` walks it) are the part to get right. Two things
this crate deliberately does not carry yet and will need: the `Mailbox/get`
state string, for `Mailbox/changes` when folder refresh arrives, and the
role-to-`CamelFolderInfoFlags` translation, which belongs on the C side. The
README's architecture block still lists only the round-1 crates; it has been
stale for three crates now and is worth a paragraph of its own some session.

## 2026-08-08 (twenty-fourth session)

M5's fourth increment, and the second half of what the twenty-second entry
asked for — again split, and again along the line between "needs a `CamelStore`
instance" and "does not". `eds-sys` learned `CamelFolder`/`CamelFolderInfo`, and
`jmap-mail` gained `folder_info.rs`: the translation from `jmap-mail-sync`'s
`FolderTree` into the `CamelFolderInfo` forest `get_folder_info_sync` returns.
The vfunc override itself, and the `connect_sync` that resolves an account out
of `CamelStoreSettings`, are still ahead — but they are now a few lines of
marshalling over a mapping that is tested, instead of the whole thing at once.
Two commits.

`eds-sys` gained the exact names `CamelFolder`, `CamelFolderClass`,
`CamelFolderInfo` and `CamelFolderInfoFlags`, the `camel_folder_info_.*` pair
the forest is allocated and freed with, `camel_folder_get_type`, and the
`CAMEL_FOLDER_TYPE_BIT`/`_MASK` `#define`s. `tests/layout.rs` vouches for
`CamelFolder`'s class struct; `tests/camel.rs` gained three tests about the
struct that is *not* an object. `jmap-mail/src/folder_info.rs` is
`FolderInfoChain` — an owning wrapper with `from_tree`, `as_ptr`, `into_raw` and
a `Drop` — and `tests/folder_info.rs` is 13 tests that build a chain and walk it
back pointer by pointer.

Decisions taken:

- **Exact type names, not a `CamelFolder.*` prefix.** The prefix also matches
  `CamelFolderSummary`, `CamelFolderSearch` and `CamelFolderThread` — three more
  class structs `tests/layout.rs` would then be claiming to have checked against
  `g_type_query` while checking nothing. Same for the functions:
  `camel_folder_info_.*` and `camel_folder_get_type`, not `camel_folder_.*`. The
  folder object's own API arrives with the increment that subclasses it.
- **What stands in for a layout test on `CamelFolderInfo` is the allocator's
  contract.** It is a plain struct behind a boxed `GType`, so `g_type_query`
  reports zero sizes and `assert_layout!` would pass on anything — the same hole
  `tests/camel.rs` was created for when `CamelProvider` hit it. The three things
  the builder actually rests on are pinned instead: that
  `camel_folder_info_new` hands back a *zeroed* struct (which is what lets the
  builder write only the fields it has an answer for and trust `next`/`child` to
  be NULL rather than garbage), that the two name fields survive a `g_strdup`
  and a `camel_folder_info_free` (they are `g_free`d, so a `CString::into_raw`
  there is heap corruption that surfaces elsewhere), and that a folder's *type*
  is a small integer packed into a field of the flags word rather than a bit of
  its own — which is what makes OR-ing one type in correct.
- **Ownership is all-or-nothing and lives at the head.**
  `camel_folder_info_free` walks `next` and `child` from the pointer it is
  given, so there is exactly one owner for a whole forest.
  `FolderInfoChain` is that owner while the chain is ours and `into_raw` is the
  single point where it stops being — `std::mem::forget`, so the `Drop` cannot
  also run. A NULL head is a legitimate value (an account with no folders is
  how Camel reads a NULL return with no error set), so there is no `Option`
  around it.
- **A half-built forest has no owner, so building may not fail part-way.**
  Nothing in `from_tree` can: `g_malloc` aborts rather than returning NULL, and
  the one fallible step — a name with a NUL in it — is resolved by rewriting the
  name rather than by returning an error. That is the reason `c_string` cannot
  be `?`-shaped, and it is worth stating because the obvious refactor to
  `Result` would introduce exactly the leak the current shape rules out.
- **A NUL in a mailbox name becomes U+FFFD, not a truncation.** A JMAP string is
  a JSON string and can carry a NUL even though RFC 8621 §2 forbids it. Passing
  the bytes through would show `Work\0Secret` as `Work`, sitting in the tree
  next to the real `Work` and indistinguishable from it. The replacement
  character keeps the name distinct and visibly broken. The *path* needs nothing
  here — `jmap-mail-sync` already encodes the NUL as `%00`, which is what that
  encoding was for.
- **Counts saturate a second time, now into a signed field.** Camel's `unread`
  and `total` are `gint32` and it uses negative values for "not known yet", so a
  count whose top bit survived a cast would read as *unknown* rather than as
  implausibly large. `jmap-mail-sync` saturates the server's 64 bits into 32;
  this is the signed half of the same argument.
- **The build is iterative, with a stack of sibling groups.** Tree depth comes
  from a `parentId` chain the server chose, so recursing over it is a stack
  overflow a server can ask for — the same reasoning as the walk in
  `jmap-mail-sync`. `camel_folder_info_free` recursing over the result is
  Camel's own bound and not one this side can lift, which is why the deep-tree
  test stops at 2000 levels: deeper would be testing Camel's stack, not ours.
- **A role folder is a *system* folder; a leaf is `NOCHILDREN` and never
  `NOINFERIORS`.** `SYSTEM` is what stops Evolution offering to rename or delete
  the six role folders — the server would refuse, and on JMAP the refusal
  arrives well after the user believed the folder was gone; it is also how
  evolution-ews marks the same folders. `NOINFERIORS` is the stronger claim that
  a folder can never *have* children, which is false for every JMAP mailbox, and
  making it would remove "New Subfolder" from every leaf for the life of the
  account.

Mutation testing, nine mutants, no survivors: swapped `full_name` and
`display_name`, the inbox losing `SYSTEM`, a truncating instead of saturating
count, a NUL truncating the display name, the `parent` back-pointer left NULL,
`CHILDREN`/`NOCHILDREN` swapped, `head` reassigned for every sibling group
instead of only the roots (10 failures), the subscription flag never set, and
`into_raw` leaving the `Drop` in place — which is a double free, and died
because the test frees the chain itself, which is what that test is for.

Not verified locally, as in the previous twenty-three sessions: `reuse lint` and
`cargo deny`. Both new files carry SPDX `GPL-3.0-or-later` headers. Also not
verified: that the forest is *leak*-free. There is no valgrind on this VM and no
sanitizer on a stable toolchain, so the `Drop` in every test is an assertion only
a leak checker can read; the suite was at least re-run under
`G_SLICE=always-malloc MALLOC_CHECK_=3 MALLOC_PERTURB_=42`, which is what would
catch the double free and the use-after-free. `cargo fmt --check`, `cargo test
--locked` (40 test binaries green on the default members, the five EDS crates
green on top, `jmap-mail` now 23 tests) and `cargo clippy --all-targets --locked
-- -D warnings` are clean on both member sets, and a fresh `cmake -S . -B <tmp>
-G Ninja && cmake --build && ctest` is 5/5. `example-module` still fails to link
its lib test and still fails clippy on `manual_c_str_literals`, unchanged and
outside both `default-members` and the set CI lints.

No blockers hit.

Next in M5: the store vfuncs, which now have somewhere to send their results.
`connect_sync` first — resolving the JMAP account out of
`CamelStoreSettings`/`CamelNetworkSettings` and the `CamelService`'s credentials
the way `jmap-backend-core::source` does for an `ESource`, which is the piece
with the security decisions in it (TLS for non-localhost, the token from
libsecret and never from a URL) — and then `get_folder_info_sync` over
`FolderInfoChain::into_raw`, mapping `SyncError` onto Camel's error codes.
Deliberately still absent, and needed soon after: the `Mailbox/get` state string
for `Mailbox/changes`, so a folder refresh is not a full re-list. The README's
architecture block still lists only the round-1 crates; it has been stale for
four crates now.

## 2026-08-08 (twenty-fifth session)

M5's fifth increment, and a detour the previous entry did not see coming.
`connect_sync` was next, "resolving the JMAP account out of
`CamelStoreSettings`/`CamelNetworkSettings`" — except that a `CamelJmapStore`
has no `CamelNetworkSettings` to resolve anything out of. A `CamelService` is
configured through a settings object whose class its own class names, and
inherited from `CamelOfflineStore` that class is `CamelOfflineSettings`, which
knows about offline synchronisation and nothing about a network. Host, port,
user and security method live on the `CamelNetworkSettings` *interface*, and no
stock Camel settings class implements it: IMAPx, POP and SMTP each declare a
settings subclass that does. So this session built the one thing `connect_sync`
was going to read from. Three commits.

`eds-sys` gained `CamelOfflineSettings` and its class struct, the accessors, and
`CamelNetworkSecurityMethod`. `jmap-backend-core::subclass` gained
`ObjectSubclass::interfaces` — the `G_IMPLEMENT_INTERFACE` half of
`G_DEFINE_TYPE_WITH_CODE`. `jmap-mail/src/settings.rs` is `CamelJmapSettings`,
a `CamelOfflineSettings` that implements the interface and overrides its five
properties, and `store.rs` names it in `CamelServiceClass.settings_type`. Seven
new tests in `jmap-mail`, three in `jmap-backend-core`, two in `eds-sys`.

Decisions taken:

- **Interfaces are declared on the trait, not added by the caller.** The timing
  is the whole point. `g_object_class_override_property` — how an implementer
  satisfies an interface's properties — runs in `class_init` and only finds
  properties of interfaces the type already implements. A caller that added the
  interface after `register_static` returned would be holding a `GType` that,
  for one window, implements nothing, and anything that referenced the class in
  that window makes the omission permanent. Resolved before the registration
  mutex is taken, like `parent_type`, so an interface accessor that registers
  something cannot deadlock.
- **A NULL `interface_init` is a complete implementation here.**
  `CamelNetworkSettings` declares no vfuncs at all — its interface struct is
  twenty pointers of padding — so there is no slot to fill. A type that did need
  one would fill it in `class_init` through `g_type_interface_peek`.
- **The tests drive the properties, not the interface.** Claiming the interface
  and overriding its properties are two halves and only the first shows up in
  the type system. Skip the second and the type still passes
  `CAMEL_IS_NETWORK_SETTINGS`, the accessors still work — the interface keeps
  its values in per-object data, not a struct field — and the only complaint is
  five criticals at class-init time that nothing fails on. What breaks is
  everything going *through* the property system, which on this path is
  everything: `e_source_camel_configure_service` binds an `ESource`'s extension
  properties to these by name, and `camel_settings_clone`/`_equal` walk the
  property list, so two accounts on different servers would compare equal.
  `cloning_carries_the_server_along` is the test that says so.
- **The overrides are a security property, and the previous guess was
  backwards.** The interface's properties are `G_PARAM_CONSTRUCT`, so
  `g_object_new` pushes each declared default through the class's
  `set_property`: a class that overrides them starts at the interface's own
  default of TLS, and a class that does not is never told and starts at the
  enum's zero value, which is plaintext. The `eds-sys` test written earlier in
  this same session claimed the opposite — that an unconfigured settings object
  reads back `NONE` — and its comment was corrected in the third commit rather
  than left to mislead. The enum values themselves are still worth pinning:
  `NONE` being zero is exactly what makes the un-overridden case insecure.
- **"Not configured" is the empty string, not NULL.** The construct default for
  `host` is `""`, where an unset `ESource` field reads back as NULL. The origin
  mapping still ahead has to treat the two alike, or an account nobody
  configured becomes a request to `https://`. Pinned in
  `an_unconfigured_settings_object_already_asks_for_tls`.
- **`STARTTLS_ON_STANDARD_PORT` is a name about a protocol JMAP does not have.**
  JMAP is HTTP, so both non-`NONE` values mean the same thing — TLS — and the
  only bit really in that field is `NONE` or not.
- **The overrides forward to the interface's accessors rather than to storage of
  their own**, which is what Camel's providers do and what keeps the property
  door and the accessor door looking at one value instead of two. The string
  reads use the `dup_` accessors with `g_value_take_string`, so the `GValue`
  takes a copy rather than pointing into storage another thread may replace.
- **`log_critical` is public now.** It was `pub(crate)` in
  `jmap-backend-core::trampoline`; the unknown-property-id arm of a
  `set_property` is exactly the case it exists for — GObject is the caller and
  there is no `GError` to hand anything to.
- **Property IDs are local and dense from 1.** They cannot collide with the
  parent's: `g_object_set_property` dispatches to the class that *owns* the
  pspec, so `filter-inbox` goes to `CamelStoreSettings`'s own `set_property` and
  never reaches ours. That is also why nothing chains up.

Mutation testing, five mutants, no survivors: the interface not declared (dies
with a SIGSEGV, since Camel's accessors assert on a type that is not one), the
override loop removed, `settings_type` left inherited on the store, `host` and
`user` swapped in `set_property`, and `get_property` handing back NULL for the
host.

Not verified locally, as in the previous twenty-four sessions: `reuse lint` and
`cargo deny`. Both new files carry SPDX `GPL-3.0-or-later` headers. `cargo fmt
--check`, `cargo test --locked` (green on the default members, the five EDS
crates green on top, `jmap-mail` now 30 tests) and `cargo clippy --all-targets
--locked -- -D warnings` are clean on both member sets; the settings suite was
also run under `G_DEBUG=fatal-criticals`, which is the check that matters for a
class whose failure mode is a critical nothing fails on. A fresh `cmake -S . -B
<tmp> -G Ninja && cmake --build && ctest` is 5/5. `example-module` is unchanged
and still outside both `default-members` and the set CI lints.

No blockers hit.

Next in M5: `connect_sync` on the store, which now has settings to read. The
mapping from host/port/user/security-method to an origin is `jmap-backend-core`'s
`SourceConfig` argument again — the same host validation and the same refusal to
speak plaintext to anything but loopback — but over a different pair of empty
values, so it is a sibling of `source.rs` rather than a caller of it. Then
`get_folder_info_sync` over `FolderInfoChain::into_raw`. Still absent and needed
soon after: the `Mailbox/get` state string for `Mailbox/changes`. The README's
architecture block still lists only the round-1 crates; five crates stale.

## 2026-08-08 (twenty-sixth session)

M5's sixth increment: the mapping the previous session left as "next" — host,
port, user and security method off a `CamelJmapSettings`, into the origin a
JMAP client is built from. One commit.

`jmap-mail/src/server.rs` is `ServerConfig::from_settings`, the Camel-side
sibling of `jmap-backend-core`'s `SourceConfig::from_source`. The two sides
carry the same account in different shapes — `ESource` extensions there, the
`CamelNetworkSettings` interface here — so they read different fields, and the
rules they must agree on were lifted out of `from_source` into
`jmap_backend_core::source::origin(host, port, secure)`: the host validation
and the refusal to speak plaintext to anything but loopback, in one place, with
one caller on each side. Eleven new tests in `jmap-mail`, one in
`jmap-backend-core` on the extracted function itself.

Decisions taken:

- **The shared half is the security half.** What is duplicated between the two
  sides is cheap to write and expensive to get subtly different: a second copy
  of `is_bare_host_name` or of the loopback exception is a second thing to
  forget when one of them is fixed. What is *not* shared is the reading, which
  is genuinely different — and that split is why `server.rs` is short.
- **"Not configured" really is two different values.** An unset `ESource` key
  reads back NULL; a `CamelNetworkSettings` property, being
  `G_PARAM_CONSTRUCT`, reads back `""`. `read_string` already folds both to
  `None`, which is the whole reason `origin` takes an `Option<&str>` rather
  than a `&str` — an unconfigured account must be "no server", not a request
  to `https://`.
- **The security method is one bit, read as one bit.** Its three values name a
  protocol JMAP does not have: JMAP is HTTP, so there is no STARTTLS handshake
  and no alternate port. `NONE` or not `NONE` is all that is in that field, and
  `every_security_method_but_none_is_just_tls` is what says so.
- **The host is punycoded before it is validated, not after.** Camel offers
  `dup_host_ensure_ascii` because an account editor accepts an
  internationalised name while the wire does not, and the validator accepts
  ASCII only — so reading the plain `dup_host` would reject a working account.
  Converting first also keeps the string that is checked and the string that is
  used the same one, which after-the-fact conversion would not.
- **That accessor never fails, it falls back**, which was checked rather than
  assumed: `dup_host_ensure_ascii` hands back the configured spelling unchanged
  when it cannot convert one — not NULL. The first draft had an arm for the
  NULL case, mapping it to `InvalidHost` so an unconvertible host would not be
  reported as an absent one; probing glib showed the arm was unreachable (it
  punycodes over-long labels happily, and returns the original for input that
  is not even UTF-8), so it went, and
  `a_host_camel_cannot_convert_is_rejected_rather_than_lost` pins the fallback
  instead. Same outcome, no dead branch.
- **Settings of the wrong class answer "no server" rather than asserting.**
  Camel only ever hands a service settings of the class its `settings_type`
  names, so this is defence in depth — but the alternative to the type check is
  not a wrong answer, it is four `g_return_if_fail`s and four NULLs, which
  produce the same result with criticals attached. The check makes the quiet
  answer the deliberate one, and `settings_that_carry_no_network_name_no_server`
  is a test that would otherwise be a crash.
- **`take_string` is local, not in `marshal`.** It is `read_string` plus the
  `g_free` a `dup_` accessor's ownership demands; the `dup_` accessors are a
  Camel idiom and this is so far the only crate calling them, so it stays here
  until there is a second caller to share it with.

Mutation testing, seven mutants, no survivors: the plain `dup_host` (dies on
the punycode test), the security comparison inverted, the user dropped, the
port ignored, the type check removed (no test *fails* — it comes back as ten
GLib criticals, which is exactly the difference the check exists to remove, and
which `G_DEBUG=fatal-criticals` turns into a failure), and, on the extracted
shared function, the TLS rule and the host validation each deleted.

Not verified locally, as in the previous twenty-five sessions: `reuse lint` and
`cargo deny`. Both new files carry SPDX `GPL-3.0-or-later` headers.
`cargo fmt --check`, `cargo test --locked` (green on the default members, the
five EDS crates green on top, `jmap-mail` now 41 tests) and
`cargo clippy --all-targets --locked -- -D warnings` are clean on both member
sets; the `jmap-mail` suite was also run under `G_DEBUG=fatal-criticals`, which
is what says the wrong-class path logs nothing. A fresh
`cmake -S . -B <tmp> -G Ninja && cmake --build && ctest` is 5/5.
`example-module` is unchanged and still outside both `default-members` and the
set CI lints.

No blockers hit.

Next in M5: `connect_sync` itself, which now has both halves it was waiting for
— a settings object with a server on it and a mapping from that to an origin —
plus the password, which comes from the `CamelSession` rather than from the
settings, and `CamelServiceClass.connect_sync`/`disconnect_sync` to hang it on.
Then `get_folder_info_sync` over `FolderInfoChain::into_raw`. Still absent and
needed soon after: the `Mailbox/get` state string for `Mailbox/changes`. The
README's architecture block still lists only the round-1 crates; five crates
stale.

## 2026-08-08 (twenty-seventh session)

M5's seventh increment: the half of `connect_sync` that has no GObject in it —
opening a JMAP mail account from the origin the previous session learned to
read, and the slot on the store that holds the result between connect and
disconnect. One commit.

`jmap-mail/src/connect.rs` is `open_mail` plus `StoreError`, the mail-side
counterpart of `jmap-backend-core`'s `connect` module; `store.rs` grows a
`Slot<RwLock<Option<MailSync>>>` and the three accessors around it, mirroring
`JmapBookBackend`. Fifteen new tests in `jmap-mail`, against `jmap-mockd`.

Decisions taken:

- **Camel's authentication enum has no `REQUIRED`, so the store does not refuse
  in advance.** `ESourceAuthenticationResult` has four values and the EDS
  backends use the fourth to say "prompt before anything is sent";
  `CamelAuthenticationResult` has three, and the only thing that produces a
  prompt is `REJECTED`. So an account that names a user and has no password yet
  must *reach* the server, take the 401, and report that — the exact opposite of
  what `SourceConfig`'s side does, and right on both sides for the machinery
  each answers to.
- **No password means no credentials, not an empty one.** The tempting reading
  of "user configured, password absent" is `Basic user:`, which is not a weaker
  credential but a wrong one, and a server that counts failed attempts counts
  it. `no_password_means_no_credentials_rather_than_an_empty_one` runs against a
  mock that *would* accept `vera` with an empty password, so the refusal is
  evidence rather than a coincidence.
- **The error domain is Camel's, not EDS's.** Camel does not read
  `E_CLIENT_ERROR`. What it branches on is `CAMEL_SERVICE_ERROR`, and
  `UNAVAILABLE` is the mail-side equivalent of `E_CLIENT_ERROR_REPOSITORY_OFFLINE`
  — the difference between a store that serves its summary cache and one that
  reports the account as broken. `URL_INVALID` for a misconfigured account is
  the other half of the same decision: it says "edit the account" where
  `UNAVAILABLE` says "try later", and reporting a missing host as unavailable
  would be a store Evolution reconnects to forever.
- **`G_IO_ERROR_CANCELLED` stays GLib's on both sides.** It is not Camel's
  domain and not EDS's; every caller above tests for it before deciding
  anything went wrong at all, so it is the one code that is not translated.
- **The shared half is, again, the security half.** What must not differ
  between the two stacks is *which failure means the password was wrong*, so it
  was lifted out of `ConnectError::auth_result` into
  `jmap_backend_core::connect::is_wrong_password` and is asked from both. The
  two enums have nothing in common; the question in front of them is one
  question, and a 403 treated as a bad password is a prompt loop no password
  ends.
- **`StoreError` is not `ConnectError`.** Reuse was considered and rejected:
  two of `ConnectError`'s four variants name a *collection*, which a store —
  being the whole account — does not stand for, and its `to_gerror` speaks a
  domain Camel ignores. What is genuinely common is one predicate, and that is
  what is shared.
- **The connection lives in the instance struct, before the vfuncs that use
  it.** Adding a field later is cheap in Rust and not cheap here: the layout is
  what `g_type_register_static` was told the instance size is, and every vfunc
  reads the connection through it. `instance_init`/`finalize` are wired now so
  that the socket cannot outlive the account even once, rather than after the
  first increment that forgets.

Mutation testing, eight mutants, no survivors: `is_wrong_password` matching 403
instead of 401 (kills tests on both sides), the mail capability swapped for
contacts, the empty-basic credential reintroduced, the transport arm of
`service_error_code` deleted, a misconfigured account reported as unavailable,
the cancellation special case removed, `store_connection` made a no-op, and
`drop_connection` reporting the opposite.

Not verified locally, as in the previous twenty-six sessions: `reuse lint` and
`cargo deny`. Both new files carry SPDX `GPL-3.0-or-later` headers.
`cargo fmt --check`, `cargo test --locked` (green on the default members, the
five EDS crates green on top, `jmap-mail` now 56 tests) and
`cargo clippy --all-targets --locked -- -D warnings` are clean on both member
sets. A fresh `cmake -S . -B <tmp> -G Ninja && cmake --build && ctest` is 5/5.
`example-module` is unchanged and still outside both `default-members` and the
set CI lints.

No blockers hit.

Next in M5: the vfuncs themselves — `CamelServiceClass.connect_sync`,
`authenticate_sync` and `disconnect_sync`, which is now only the GObject half:
read the settings, hand `ServerConfig` and the password to `open_mail`, put the
result in the slot. The open design question there is which of the two drives
the other: Camel's own providers have `connect_sync` call
`camel_session_authenticate_sync`, which prompts and then calls back into
`authenticate_sync` with the password on `camel_service_get_password` — that is
the idiom, and it is what makes a JMAP account prompt like an IMAP one, but it
is also the first thing in this crate that cannot be tested without a
`CamelSession`. Then `get_folder_info_sync` over `FolderInfoChain::into_raw`.
Still absent and needed soon after: the `Mailbox/get` state string for
`Mailbox/changes`. The README's architecture block still lists only the round-1
crates; five crates stale.

## 2026-08-08 (twenty-eighth session)

M5's eighth increment: the state a folder listing is current as of, and
noticing that it moved. Two commits, one of them the paging primitive the
whole repo had three unexercised copies of.

**`jmap-client`: `all_changes`.** RFC 8620 §5.2 lets a server truncate a
`/changes` answer whenever it likes; `maxChanges` is the client's cap, not the
only reason for one. `Client::all_changes` follows `hasMoreChanges` to the end
and returns a `ChangeSet` — three `BTreeSet<Id>`s and the state to resume from.
`jmap-book-sync` and `jmap-cal-sync` had a loop each doing this; both now call
the one implementation. `MockServerBuilder::changes_page_size` makes the mock
page, which is what turned that loop from untested into tested.

**`jmap-mail-sync`: `folder_tree` keeps its state, and `folder_tree_since`.**
`Mailbox/get` was throwing the response's `state` away, which made the folder
tree something that could only ever be re-fetched in full. It now comes back
with the tree, and `folder_tree_since` spends one `Mailbox/changes` to find out
whether anything happened. Six tests in a new `tests/refresh.rs`, plus three
mock helpers (`create_mailbox`, `rename_mailbox`, `destroy_mailbox`) that
mutate as state transitions rather than seeding.

Decisions taken:

- **Paging must not change the answer.** `all_changes` folds the pages back
  into what one response would have carried, by the rule RFC 8620 states for
  one: an object created and destroyed inside the window is reported neither
  way, because the caller never learned it existed, and one created and then
  modified is created. Without this, whether a client hears about an object
  that came and went depends on where the server happened to split — a
  difference no caller can do anything sensible with.
  `following_every_page_answers_what_one_page_would_have` asserts it as an
  equality between the capped and uncapped servers, which is the property
  stated directly rather than a proxy for it.
- **The mock truncates at a state boundary, and always serves the first
  transition whole.** `newState` has to be a state the client can ask again
  from, and half a `/set` is not one. The first transition goes out however
  large it is, because a cap that could withhold all of it is a client asking
  again from the same state forever.
- **A mailbox delta is not applied folder by folder.** A Camel path is built
  from a mailbox's ancestors, so `Mailbox/changes` reporting a renamed parent
  says nothing about the descendants whose paths just moved with it — and the
  delta names only the parent.
  `renaming_a_parent_moves_children_the_delta_never_names` is that case. The
  account's mailbox list is one `Mailbox/get`, so the honest answer to any
  change at all is the tree again; what the delta is genuinely worth is the
  answer *no*, which is what it gives nearly every time it is asked. Hence
  `FolderUpdate::Unchanged`/`Rebuilt` rather than lists of ids the caller
  could not apply.
- **A rebuilt tree is labelled with the listing's state, not the delta's.**
  The tree is what was walked, and the account may have moved again between the
  two calls; taking the delta's state would record the account as current at a
  point the tree does not reflect, and lose whatever happened in between.
- **`cannotCalculateChanges` is answered, not reported — on the mail side.**
  The EDS meta backends pass it up because EDS knows how to diff a collection
  against its cache. Camel has nothing of the kind, so a store that reported it
  would be a folder tree that never recovers: `folder_tree_since` lists the
  account instead. The predicate itself moved to
  `jmap_client::Error::is_cannot_calculate_changes`, with book and cal
  delegating — three callers now ask the same question, and a state that is too
  old must not mean different things to mail and to contacts.
- **The mock grows mailbox mutators rather than a `Mailbox/set`.** Nothing in
  the client sends one, and a `/set` implementation nobody calls is a second
  server to keep correct. What the tests need is for a folder to appear, be
  renamed or vanish *as a state transition* — the thing `seed_mailbox`
  deliberately does not do, since a seeded mailbox predates every state a
  client has seen.

Mutation testing, eight mutants, no survivors: the mock ignoring the page cap,
`all_changes` stopping after the first page, created-then-destroyed reported as
created, `cannotCalculateChanges` back to being an error, the mock creating a
mailbox without a transition, a non-empty delta not rebuilding, an empty delta
rebuilding anyway, the listing inventing its state, and the mock's mailbox
destroy and rename each staging nothing.

One self-inflicted blocker, worth recording: the mutation harness reverted each
mutant with `git checkout --`, which on *uncommitted* work reverts the work
too. Two files were lost that way and rewritten from the session's own context;
the harness now restores from a copy. Mutation testing against a dirty tree
needs a revert that knows nothing about git.

Not verified locally, as in the previous twenty-seven sessions: `reuse lint`
and `cargo deny`. The one new file carries an SPDX `GPL-3.0-or-later` header.
`cargo fmt --check`, `cargo test --locked` (green on the default members, the
five EDS crates green on top) and `cargo clippy --all-targets --locked -D
warnings` are clean on both member sets. A fresh `cmake -S . -B <tmp> -G Ninja
&& cmake --build && ctest` is 5/5. `example-module` is unchanged and still
outside both `default-members` and the set CI lints.

Next in M5: still the vfuncs — `CamelServiceClass.connect_sync`,
`authenticate_sync` and `disconnect_sync`, whose open question (which of
`connect_sync` and `camel_session_authenticate_sync` drives the other) is
unchanged from last session, and `get_folder_info_sync` over
`FolderInfoChain::into_raw`, which now has both halves it needs: a tree, and a
cheap way to find out it is still current. The store will want a field for the
folder state next to its connection slot. The README's architecture block still
lists only the round-1 crates; five crates stale.

## 2026-08-08 (twenty-ninth session)

M5's ninth increment: the folder listing a store keeps between calls, and what
Camel's `CAMEL_STORE_FOLDER_INFO_REFRESH` bit asks of it. One commit.

`jmap-mail-sync` already answered both halves — `folder_tree` lists,
`folder_tree_since` says whether the listing still holds. What
`get_folder_info_sync` needs and neither half provided is somewhere to keep the
answer: Camel asks a store for its folder tree constantly, on every folder the
user opens and every counter update, and sets `REFRESH` on the few of those
calls that mean "go and look". `JmapStore` grows a second slot for it —
`Slot<RwLock<Option<Listing>>>`, a tree and the state it is current as of — and
`JmapStore::folders(flags)`, which takes Camel's flags word verbatim and returns
an `Arc<FolderTree>`. Twelve tests in a new `tests/folders.rs`.

Decisions taken:

- **The first listing ignores the flags.** A store with nothing in hand has
  nothing else to answer with, so it lists whether or not `REFRESH` was asked
  for; the alternative is an account that opens empty and stays that way until
  something happens to set the bit.
- **A refresh that finds nothing keeps the same tree, not an equal one.**
  `folders` hands out `Arc<FolderTree>` and an unchanged refresh reinstalls the
  `Arc` it already had, so `Arc::ptr_eq` holds across it. Camel diffs the
  `CamelFolderInfo` forests it is handed to decide which folders to announce as
  created or deleted; a tree that is a new allocation every refresh is churn no
  folder actually did. The `Arc` is also what lets the tree outlive the lock —
  translating it into a forest must not hold the store's listing locked, and
  copying it per call would be a walk of every mailbox for an answer that did
  not change.
- **A rebuilt listing carries the state to measure the *next* refresh against.**
  Storing the listing's own state rather than the delta's is `jmap-mail-sync`'s
  rule; the consequence here is that a store which kept asking from the state of
  its first listing would rebuild the tree on every refresh forever after.
- **The listing is a slot of its own, tied to the connection by an ordering
  rule rather than by nesting.** Putting it inside the connection would make a
  folder refresh and a reconnect queue behind each other, and would need an
  identity to compare after re-taking the lock. Instead: a listing is written
  while the connection it was read over is still read-locked, and
  `store_connection` — which needs that lock exclusively — clears the listing
  under it. So a reconnect racing a refresh cannot have its clearing undone by a
  tree the previous connection produced.
- **A reconnect discards the tree.** Camel reconnects because something about
  the account changed, and the server behind the new connection may not be the
  one the old tree — paths, counts, and the JMAP ids every later request is
  built from — describes.
- **`StoreError::Disconnected`, reported as `CAMEL_SERVICE_ERROR_NOT_CONNECTED`.**
  Camel drives a store it *believes* is connected and the belief goes stale;
  that code is what makes it connect and ask again rather than show the account
  as broken. `SyncError` now converts into `StoreError`, which is where the
  `jmap_client::Error` kept intact across two crate boundaries finally becomes a
  `CAMEL_SERVICE_ERROR`.
- **The other flags are documented as unread rather than silently ignored.**
  `SUBSCRIBED`/`SUBSCRIPTION_LIST` are a filter on the tree, not a different
  request, and `FAST` asks for it without counts JMAP includes in the mailbox
  anyway.

Mutation testing, six mutants, one deliberate survivor: the cache fast path
never taken, an unchanged refresh cloning the tree, the refreshed listing not
stored, `store_connection` keeping the old listing, and a store with no
connection answering an empty tree all die. The survivor is `drop_connection`
not clearing the listing — with no connection nothing can reach the tree and the
reconnect clears it anyway, so freeing it there is memory and not behaviour. It
stays (a disconnected account should not hold its mailbox tree until Evolution
quits) and both the method's doc and the test say so rather than implying a test
covers it.

One thing worth recording for future tests: a request sent over a pooled
connection whose mock server has just been dropped does not fail fast, it waits
out the client's 30-second global timeout. `a_listing_that_fails_is_reported_
rather_than_answered_empty` builds its client with a 500 ms one, which took the
file from 30 s to 0.5 s.

Not verified locally, as in the previous twenty-eight sessions: `reuse lint` and
`cargo deny`. The one new file carries an SPDX `GPL-3.0-or-later` header.
`cargo fmt --check`, `cargo test --locked` (green on the default members, the
five EDS crates green on top, `jmap-mail` now 68 tests) and `cargo clippy
--all-targets --locked -- -D warnings` are clean on both member sets. A fresh
`cmake -S . -B <tmp> -G Ninja && cmake --build && ctest` is 5/5.
`example-module` is unchanged and still outside both `default-members` and the
set CI lints.

Next in M5: the vfuncs, now with everything they need behind them —
`CamelServiceClass.connect_sync`/`authenticate_sync`/`disconnect_sync`, whose
open question (which of `connect_sync` and `camel_session_authenticate_sync`
drives the other) is unchanged from the last two sessions, and
`CamelStoreClass.get_folder_info_sync`, which is now `folders(flags)` followed
by `FolderInfoChain::from_tree(&tree).into_raw()` and a `catch_unwind`. After
that, the subscription flags `folders` deliberately does not read yet. The
README's architecture block still lists only the round-1 crates; five crates
stale.

## 2026-08-08 (thirtieth session)

M5's tenth increment: the `CamelService` vfuncs — `connect_sync`,
`authenticate_sync` and `disconnect_sync` — and with them the answer to the
question the last three sessions logged as open. One commit, a new
`src/service.rs` and ten tests in `tests/service.rs`.

The open question was which of `connect_sync` and `camel_session_authenticate_
sync` drives the other, and Camel's answer is the counter-intuitive one: a
service does **not** open its connection in `connect_sync`. It asks its
`CamelSession` to authenticate it, and the session — the only object allowed to
touch a stored password or put a prompt in front of the user — calls
`authenticate_sync` back, once if the password it had works and once more for
every password the user then types. IMAPX and POP3 are both built that way, and
the reason is the re-prompt: a service that opened its own connection would
have nowhere to send the user when the password turned out to be wrong. So
`connect_sync` is a short-circuit and a delegation, and everything that happens
happens in `authenticate_sync`.

Decisions taken:

- **Only an `ERROR` verdict carries a `GError`.** `authenticate_sync` answers
  twice and the two answers are not independent: `camel_session_authenticate_
  sync` reads `REJECTED` as "ask for another password and call me again" and
  keeps looping, and only gives up — and only propagates an error — on `ERROR`.
  An error set alongside a `REJECTED` is one reported for an attempt that has
  not failed yet, leaked at best and shown to a user who is being asked for a
  password at worst. `report_authentication` is the single place either answer
  is produced, which is what makes the rule testable without a session.
- **A failed attempt leaves a working connection alone.** Camel
  re-authenticates a service it already has a connection for — a changed
  password, a session that lost track — and a store that dropped its connection
  on the way to being told the new password would stop serving folders it was
  serving a moment ago, for a password nobody has typed yet. A *successful* one
  does replace it, listing and all, which is `store_connection`'s existing rule.
- **`connect_sync` short-circuits on an already-connected store**, the same
  check the address book backend's makes and for the same reason: Camel
  reconnects whenever it suspects the connection is gone, including when it is
  not, and re-opening a live one would drop a socket other threads are
  mid-request on.
- **`disconnect_sync` drops ours first, then chains up.** The parent's
  implementation is what marks the service disconnected; a connection still in
  the slot after that is one a racing operation could pick up and use against a
  service Camel believes is closed. Chaining up at all follows POP3 and IMAPX,
  which both do.
- **The password comes from `camel_service_get_password`**, i.e. from what the
  session just put on the service, and from nowhere else. Nothing reads a
  credential off the settings object, which Evolution serialises into a config
  file.
- **A NULL mechanism, and `query_auth_types_sync` left alone.** JMAP
  authenticates over HTTP and offers no SASL mechanisms to choose between, so
  there is nothing to advertise and nothing to branch on.
- **A panicked `authenticate_sync` is `CAMEL_SERVICE_ERROR_INVALID`, not
  `UNAVAILABLE`.** The guard cannot produce a verdict, so the vfunc reports one
  itself; telling Camel the server is unreachable would have it retry the
  account forever over a bug that is deterministic.

Mutation testing, five mutants, none surviving: the error set for every verdict,
every failure flattened to `ERROR`, the opened connection not installed, a
failed attempt clearing the store, and a success reported as anything but
`ACCEPTED` all die. The harness restores from a copy rather than with `git
checkout --`, per the twenty-eighth session's lesson.

Not verified locally, as in the previous twenty-nine sessions: `reuse lint` and
`cargo deny`. The two new files carry SPDX `GPL-3.0-or-later` headers.
`cargo fmt --check`, `cargo test --locked` (green on the default members, the
five EDS crates green on top, `jmap-mail` now 78 tests) and `cargo clippy
--all-targets --locked -- -D warnings` are clean on both member sets. A fresh
`cmake -S . -B <tmp> -G Ninja && cmake --build && ctest` is 5/5.
`example-module` is unchanged and still outside both `default-members` and the
set CI lints.

Next in M5: `CamelStoreClass.get_folder_info_sync`, which is now `folders(flags)`
followed by `FolderInfoChain::from_tree(&tree).into_raw()` and a guard that
returns NULL — every piece exists, including the connection that finally gets
installed. After that the subscription flags `folders` deliberately does not
read, and then `CamelFolder` itself. Nothing about the service vfuncs has been
exercised against a real `CamelSession` yet, and cannot be until M6 gives the
account a collection backend and M7 a way to create one; the manual test recipe
with a hand-written `.source` keyfile is where that will first be checked. The
README's architecture block still lists only the round-1 crates; five crates
stale.

## 2026-08-08 (thirty-first session)

M5's eleventh increment: `CamelStoreClass.get_folder_info_sync`, the vfunc the
last session named as next. One commit, a new `src/folders.rs`, and the wiring
that turns the two halves already in the tree into an account with folders.

This session opened on an unusual state: a previous session had left
`src/folders.rs`, the depth argument on `FolderInfoChain`, and 13 tests
uncommitted and **unwired** — `folders` was not in `lib.rs` and its
`install_vfuncs` was not called from `class_init`. So the suite compiled without
the module and passed without ever reaching the vfunc. Confirming that was the
red step: `cargo test --test folders` failed on `unresolved import
jmap_mail::folders`. Wiring it was the green one.

Decisions taken:

- **`RECURSIVE` is obeyed, although IMAPX has a `/* FIXME: obey other flags */`
  where it would be.** Every real caller — Evolution's folder cache and
  subscription editor, `camel_store_delete_folder_sync` — passes it, and the two
  that do not are `camel_store_get_folder_info_sync`'s virtual-folder paths,
  which strip it deliberately and want the top level back. Obeying the
  documented contract costs nothing a caller depends on and saves a deep account
  from marshalling its whole tree into C to answer a question about one level.
- **The depth is applied while the forest is built, not to a finished one.** A
  cut afterwards would have allocated — and freed — every folder of a deep
  account to keep its first level. `FolderInfoChain::from_forest` therefore
  carries the remaining depth on its explicit stack alongside each sibling
  group.
- **A cut folder still says `CHILDREN`.** The cut is on what is emitted, not on
  what the folder *is*: that flag is what makes the folder tree draw the
  expander, and the expander is how the user asks for the level that was cut. A
  cut folder reporting itself a leaf would be a subtree nothing could fetch.
- **The depth differs by one between the two `top` cases**, which is easy to
  lose: "the immediate subfolders of `top`" is one level below a folder that is
  itself in the answer, but the account's top-level folders *are* the root's
  immediate subfolders — the root is not a folder and is not returned — so no
  level is left below them. Hence `Some(1)` for a `top` and `Some(0)` without.
- **A `top` that names no folder is an empty answer, not an error.** Camel
  documents the wrapper as able to "return NULL without setting a GError if no
  folders match the search criteria", and the case is ordinary: a folder another
  client deleted is asked for once more before Camel notices. Reporting that as
  a failure would turn someone else's tidying into a broken account. This is why
  NULL-with-no-error and NULL-with-`NOT_CONNECTED` both have a test.
- **A NULL and an empty `top` are the same question.** The wrapper itself tests
  `top == NULL || *top == '\0'`, so a store reading the two spellings
  differently would disagree with the function calling it.
- **The refresh flag is the listing's business, not the request's.** A call for
  one subtree still refreshes the whole tree: JMAP cannot ask for part of a
  `Mailbox/changes`, and a partial answer would leave the store's state
  describing folders it did not fetch.

Three flags are still unread. `SUBSCRIBED` and `SUBSCRIPTION_LIST` want the tree
filtered to what the user subscribed to — a filter on folders rather than a
different request. `FAST` is documented as deprecated and as making no
difference to most backends, which is true of this one because JMAP puts the
counts in the mailbox anyway. `NO_VIRTUAL` is not this vfunc's business: the
wrapper adds and removes vTrash and vJunk around the call.

Added this session: a third part to `tests/folders.rs` that calls the vfunc
**through the pointer in the class**, the way Camel does, rather than by name.
That is the only test that can prove the two halves are joined to each other and
to the slot — and it is exactly what the uncommitted state above would have
passed without. Six tests: the whole account, an empty `top`, a `top` rooting
the answer (with the head's `next` asserted NULL), the non-recursive cut, the
empty-answer case, and the disconnected-store error; plus a NULL instance to
exercise the guard. `Answered` owns both answers and frees them with
`camel_folder_info_free` and `g_error_free`, as Camel's caller does.

Mutation testing, six mutants, none surviving: the vfunc not installed on the
class (8 tests), an empty `top` read as a folder named `""`, the top depth off by
one, an unknown `top` falling back to the whole account, the depth ignored while
building, and the vfunc ignoring `top` and depth altogether (3 tests).

Lesson, at the cost of one false "survivor": **run the mutation harness after
`cargo fmt`, and assert the target string was found.** Mutant 6 first reported
as surviving because `cargo fmt` had split the line the patch matched on, so the
edit silently did nothing. A mutation harness that cannot fail loudly on a
missed target reports clean code and broken tests identically. The retry asserts
the substring is present before writing.

Not verified locally, as in the previous thirty sessions: `reuse lint` and
`cargo deny` (neither binary is installed on this VM). The one new file,
`src/folders.rs`, carries an SPDX `GPL-3.0-or-later` header. `cargo fmt --check`,
`cargo test --locked` (green on the default members; the five EDS crates green on
top, `jmap-mail` now 99 tests) and `cargo clippy --all-targets --locked -- -D
warnings` are clean on both member sets. A fresh `cmake -S . -B <tmp> -G Ninja &&
cmake --build && ctest` is 5/5. `example-module` is unchanged and still outside
both `default-members` and the set CI lints.

Next in M5: `CamelFolder` itself — `CamelJmapFolder` as a `CamelOfflineFolder`
subclass, whose summary comes from `Email/query` + `Email/get`. The store's
`get_folder_sync` is the slot that hands one out, and it is the first vfunc that
needs a per-folder object rather than a view of the tree. The subscription flags
this vfunc deliberately does not read want `CamelSubscribable`, which is an
interface on the store and a separate increment. Still unexercised against a
real `CamelSession`: everything in `service.rs` and now `folders.rs` — that
waits on M6 for a collection backend and M7 for a way to create an account, and
the manual test recipe with a hand-written `.source` keyfile is where it will
first be checked. The README's architecture block still lists only the round-1
crates; five crates stale.

## 2026-08-08 (thirty-second session)

M5's twelfth increment: `CamelJmapFolder`, the object a store hands out for one
mailbox. One commit, a new `src/folder.rs` and `tests/folder.rs`, and the
`eds-sys` allowlist entries the type needs.

Everything the store has produced so far describes folders from the outside — a
`CamelFolderInfo` forest, plain structs Camel reads once and frees. This is the
folder itself: a `CamelOfflineFolder` subclass, registered statically like the
store, constructed from one `FolderInfo`. The red step was `cargo test -p
jmap-mail --test folder` failing on `unresolved import jmap_mail::folder` and on
four `camel_folder_*` accessors bindgen had never been asked for.

Decisions taken:

- **The folder carries the JMAP mailbox id, and that is the reason it is an
  object at all.** Camel has no field for it and nothing can recover it later:
  the path Camel keys the folder by is an identifier this crate invented out of
  the mailbox's *name*, and `path.rs`'s encoding is not reversible by anything
  holding only the result — while `Email/query`, which is where the folder's
  contents will come from, filters on `inMailbox`. A folder that knew only its
  path could describe itself and fetch nothing. It lives in a `Slot`, like the
  store's connection, because the instance struct arrives zeroed and is freed
  without a destructor running over it.
- **The id is written after `g_object_new`, not through a property of our own.**
  A GObject property would have to be construct-only to be equally safe, and
  declaring one would put the id in the public API of a type whose only reader
  is this crate. Nothing can observe the folder in between: the reference
  `g_object_new` returned is still the only one.
- **The three Camel properties are all set at construction**, because
  `parent-store` is construct-only and a folder whose name arrived afterwards
  would exist, briefly, as a nameless folder in a store keyed by name.
- **`CamelFolderFlags` is not `CamelFolderInfoFlags`.** The info's word says what
  kind of folder this is (its type field, subscribed, has children); the
  object's says how Camel *treats* it. Two enums one word apart, with
  overlapping bit positions and no type-level distinction once they are `u32`s
  — which is why `CamelFolderFlags` is named in the allowlist rather than left
  to arrive transitively.
- **Only the inbox gets flags, and only two.** `FILTER_RECENT` and `FILTER_JUNK`
  are what run the user's incoming filters and the junk test over new mail;
  IMAPX sets exactly this pair on the folder it identifies by comparing its name
  against `"INBOX"`, and this provider takes the same decision from the JMAP
  role instead — from the account's data rather than from a convention about a
  name.
- **`HAS_SUMMARY_CAPABILITY` is deliberately absent.** It is a claim that the
  folder keeps a `CamelFolderSummary`, which is the next increment; Camel reads
  it to decide whether it may ask for a message count at all, so claiming it
  early is a folder that says it can be counted and then cannot. `IS_TRASH` and
  `IS_JUNK` are absent for a different reason: they are what
  `camel_store_get_trash_folder_sync` and its junk counterpart mark the folder
  they *return* with, not properties of a mailbox with that role.
- **No `instance_init`.** A zeroed `Slot` is already an empty one, and there is
  nothing else to fill until `new_folder` has the `FolderInfo`. `finalize` is
  not optional the same way — by then the slot may hold an id.
- **The NUL rewrite is shared with the folder-info forest**, not written a second
  time: `folder_info::c_string` became `pub(crate)`. A mailbox name is a JSON
  string and can carry a NUL that RFC 8621 forbids; handing the bytes to
  `g_object_new` would truncate the name there, leaving a folder called `Work`
  beside the real `Work`.

The tests construct a real account rather than a detached instance, which is new
for this crate: a `CamelSession` from `g_object_new`, and a `CamelJmapStore` on
it with the provider struct `provider::register()` leaks. That is forced —
Camel's `folder_set_parent_store` asserts `CAMEL_IS_STORE`, so there is no
folder to test without one. The session and store are kept together in an
`Account` with a `Drop`, because `CamelService` holds only a *weak* reference to
its session and a test that unreffed the session first would leave the store
pointing at nothing.

Mutation testing, seven mutants, none surviving: the flags word always empty,
the flags never reaching the folder, the mailbox id never stored, the parent
type a plain `CamelFolder`, the path and display name swapped, the folder built
with no `parent-store`, and a name with a NUL dropped rather than rewritten.
The harness asserted each target string was present before writing, per last
session's lesson. What no test covers is `finalize` clearing the slot: the only
observable effect is a leak, and nothing in the suite can see one.

Not verified locally, as in the previous thirty-one sessions: `reuse lint` and
`cargo deny` (neither binary is installed on this VM). Both new files carry SPDX
`GPL-3.0-or-later` headers. `cargo fmt --check`, `cargo test --locked` (green on
the default members; the five EDS crates green on top, `jmap-mail` now 104
tests) and `cargo clippy --all-targets --locked -- -D warnings` are clean on
both member sets. A fresh `cmake -S . -B <tmp> -G Ninja && cmake --build &&
ctest` is 5/5. `example-module` is unchanged and still outside both
`default-members` and the set CI lints.

Next in M5: `CamelStoreClass.get_folder_sync`, which is now the store's side of
the type this session added — and with it the folder cache, because Camel
expects the same path to give back the same object (Evolution holds a folder
open while the user reads it, and a second one for the same mailbox would be a
second summary over the same mail). After that the summary itself:
`CamelFolderSummary` filled from `Email/query` + `Email/get`, which is the
increment `HAS_SUMMARY_CAPABILITY` and the folder's message-count vfuncs wait
on. Still unexercised against a real `CamelSession`: `service.rs` and
`folders.rs` — that waits on M6 for a collection backend and M7 for a way to
create an account, though this session's `Account` helper shows how much of a
session a test can stand up by hand. The README's architecture block still lists
only the round-1 crates; five crates stale.

## 2026-08-08 (thirty-third session)

M5's thirteenth increment: `CamelStoreClass.get_folder_sync`, the store's side
of the folder type the last session added. One commit; `src/folders.rs` grew a
second vfunc, `StoreError` grew a variant, and the tests grew a
`tests/common/mod.rs` for the real store this one needs. The red step was
`cargo test -p jmap-mail --test folders` failing on `JmapStore::borrow`, which
did not exist.

Decisions taken:

- **No folder cache of our own — Camel already has one, and the plan to write
  a second was wrong.** `CamelStore` owns a `CamelObjectBag` of the folders it
  has open; it is public as `camel_store_get_folders_bag`, and the two
  `CamelStoreClass` fields nothing else explains — `hash_folder_name` and
  `equal_folder_name` — are the hash and equality it is keyed with.
  `camel_store_get_folder_sync` reserves the name in that bag *before* it
  dispatches, so the vfunc is only ever reached on a miss and its contract is
  to build a folder every time. A cache here would be a second answer to a
  question already answered, and the way two `CamelFolder`s over one mailbox —
  two summaries, two flag words — get handed out. `camel_hands_back_the_folder_
  it_already_opened` calls the wrapper twice and asserts one pointer, which is
  the only way to test a decision whose whole content is *not* writing code.
  Incidental confirmation from the mutation run: with the vfunc uninstalled
  that test does not fail, it *deadlocks* — the wrapper's
  `g_return_val_if_fail` leaves the reservation standing and the second
  `camel_object_bag_reserve` waits on it forever.
- **A path the held listing does not know is a reason to look again, not to
  report a missing folder.** Evolution reopens the folder the user last had
  selected when it starts, from a URI in its own settings, before anything has
  asked the store to refresh; another client creating a mailbox mid-session is
  ordinary. So a miss retries once with `CAMEL_STORE_FOLDER_INFO_REFRESH` —
  one `Mailbox/changes` on the path that was about to fail anyway — and a hit,
  which is every folder the user clicks, is answered out of the tree with no
  request at all. Without it, a folder that plainly exists stays unopenable
  until Evolution restarts.
- **`CAMEL_STORE_ERROR_NO_FOLDER`, which is the one place `StoreError` leaves
  the service domain.** Nothing is wrong with the connection or the account
  when one folder is gone, and a `CAMEL_SERVICE_ERROR` is what Camel reads to
  decide the account is broken or offline. That made `to_gerror` pick a
  (domain, code) pair rather than a code, which is also where the cancelled
  case already lived.
- **Unlike the listing vfunc, NULL is not a legitimate answer here.** There is
  no such thing as half a folder, so NULL always means failure and always
  carries an error — the opposite of `get_folder_info_sync`, where a `top`
  naming nothing is an empty answer with no error at all.
- **The flags word is not read, and each bit has its own reason.** `CREATE`
  asks for a mailbox to be made, which is a `Mailbox/set` and belongs to
  `create_folder_sync`; `BODY_INDEX` asks for an index this provider does not
  build; `PRIVATE` is about vFolder membership, which is the wrapper's
  business; `EXCL` is documented as not honoured.
- **Both store vfuncs stayed in one module.** They are one question asked in
  two directions — the listing hands out paths, this turns a path back into the
  mailbox it came from — and both read the same `JmapStore::folders`. Splitting
  them would have duplicated the instance borrow, the failure path and the
  `install_vfuncs` call for no separation that exists.
- **`tests/common/mod.rs`.** This is the second test file that needs a *real*
  store: `JmapStore::detached` is not a GObject, and Camel type-checks the
  store it is asked to build a folder on. The session-and-store helper moved
  there out of `tests/folder.rs` rather than being written twice, and
  `JmapStore::borrow` — the same accessor the vfuncs use, now public — is how a
  test installs a connection on one.

Mutation testing, six mutants, none surviving: the vfunc never installed, no
second look on a miss, a missing folder reported as `NOT_CONNECTED`, the first
root returned instead of the folder named, the path never read, and the
no-folder error raised in the service domain. Each target string was asserted
present before writing.

Not verified locally, as in the previous thirty-two sessions: `reuse lint` and
`cargo deny` (neither binary is installed on this VM). The one new file,
`tests/common/mod.rs`, carries an SPDX `GPL-3.0-or-later` header. `cargo fmt
--check`, `cargo test --locked` (green on the default members; the five EDS
crates green on top, `jmap-mail` now 110 tests) and `cargo clippy --all-targets
--locked -- -D warnings` are clean on both member sets. A fresh `cmake -S . -B
<tmp> -G Ninja && cmake --build && ctest` is 5/5. `example-module` is unchanged
and still outside both `default-members` and the set CI lints.

Next in M5: the summary. `CamelFolderSummary` filled from `Email/query` +
`Email/get`, which is what `CAMEL_FOLDER_HAS_SUMMARY_CAPABILITY` and the
folder's message-count vfuncs wait on, and the first thing the mailbox id this
folder carries is actually spent on. Two smaller pieces are now also unblocked
and are worth taking before it if the summary stalls: `get_inbox_folder_sync`,
whose inherited implementation opens the folder literally named `INBOX` while
this provider knows the inbox from its JMAP role, and the `CamelSubscribable`
interface, which is what the subscription flags in the folder-info forest are
for. Still unexercised against a real `CamelSession`: `service.rs` — that waits
on M6 for a collection backend and M7 for a way to create an account. The
README's architecture block still lists only the round-1 crates; five crates
stale.

## 2026-08-08 (thirty-fourth session)

M5's fourteenth increment, and the first one on the *contents* of a mailbox:
`MessageSummary` — one `Email` as the row `CamelFolderSummary` keeps — and
`MailSync::messages`, the `Email/query` + `Email/get` pair that produces them.
All of it in `jmap-mail-sync`, so none of it needs the Camel headers to be
tested; the GObject half, the summary object itself, is the next increment and
is what the deferred `camel_folder_summary_*` binding in `eds-sys`'s allowlist
is waiting for. The red step was `cargo test -p evolution-jmap-mail-sync`
failing to compile on `MessageSummary`, which did not exist, followed by the
two limit knobs the mock needed to make the paging tests fail for the right
reason.

Decisions taken:

- **Not the one-round-trip `Email/query`+`Email/get` back-reference the client
  already has.** Chaining them through `#ids` sends every matching id straight
  into the `/get`, and `maxObjectsInGet` — 256 on the mock, and a number every
  RFC 8620 server has to publish — is a hard cap: asking for more is a
  `requestTooLarge` that fails the whole call, not a truncated answer. A
  mailbox is exactly the JMAP type with no bound on how many objects match, so
  the two calls stay separate and the fetch is chunked. `jmap-mock` now
  advertises *and enforces* that limit (`MockServerBuilder::objects_in_get`),
  which is what makes `more_messages_than_one_get_may_ask_about_are_fetched_in_
  several` fail with the server's own error rather than with a bad assertion.
- **A `/query` answer is read to the end, not to the first page.** RFC 8620
  §5.5 lets a server cap a result set whether or not the client sent a `limit`,
  and requires it to report the cap it applied in `limit`. So the loop asks
  again from `position = ids.len()` for as long as the answer *says* it was
  capped, which costs one call for the common case (no cap, one page, done) and
  no extra call for a server that pages. The alternative — paging until an
  empty answer — costs every folder open an extra round-trip forever.
  `MockServerBuilder::query_page_size` is the knob that exercises it, mirroring
  the `changes_page_size` one already there for the same class of bug.
- **The query's order is restored after the fetch, because `/get` has none.**
  RFC 8620 §5.1 does not promise the answer comes back in the order the ids
  were named, and chunking makes that visible in a second way. So the rows are
  collected by uid and then emitted in the order `Email/query` returned —
  which also settles two races for free: an id the `/get` did not answer for is
  a message deleted between the two calls and is dropped, and a message that
  shifted position and arrived on two pages is listed once. The test seeds its
  messages *newest first* so that creation order, id order and the required
  order are all different; seeded the other way round, every one of those
  mutants would have passed.
- **`receivedAt` is the sort key, not the `Date` header.** `sentAt` is the
  sender's clock at the sender's offset, and one message with a wrong one would
  sort into the wrong place in the folder forever. Both are kept — Camel has a
  field for each — but only the server's own timestamp orders the list.
- **A date is a number here, not in the Camel layer.** Camel stores both dates
  as `gint64` seconds since the epoch and JMAP sends both as text, and doing
  the arithmetic in `jmap-mail-sync/src/date.rs` is what makes it testable
  without the Evolution headers. Hand-rolled, against this project's standing
  directive to outsource iCalendar/vCard parsing to `calcard`: the difference
  is that the whole grammar is RFC 3339 plus the proleptic Gregorian calendar —
  fixed, small, and pinned by 30-odd cases including the leap-year rule at 1900
  and 2000 — where the directive is about text formats with decades of
  accumulated deviation. `jmap-proto` keeps `UtcDate` a string on purpose (a
  wire crate that reinterpreted values could lose them), so this is the layer
  that gets to interpret one.
- **An unreadable date leaves the message dateless rather than failing the
  listing.** `epoch_seconds` returns `Option`, and one malformed `Date` header
  cannot hide a mailbox. `None` rather than 0 so a caller can still tell "the
  server said nothing" from "the server said the epoch".
- **`CAMEL_MESSAGE_DELETED` has no field, and that is the finding.** JMAP has
  no deleted keyword: deleting mail is `Email/set` taking the message out of
  the mailbox. So the bit stays what Camel makes it — a local mark this
  provider will have to turn into a mailbox change at expunge time — and
  `MessageFlags` has a field only for the bits a keyword or property actually
  says something about. `hasAttachment` is the one that is a property rather
  than a keyword; `$notjunk` is kept distinct from the absence of `$junk`,
  because it is what stops a filter reconsidering.
- **Keywords are matched case-insensitively and `false` means unset.** RFC 8621
  §4.1.1 restricts keywords to lower case and defines the value as always
  `true`; a server that shouts `$Seen` should still not leave every message
  unread and every mailbox labelled, and one that sends `false` is saying
  nothing rather than something.
- **The keywords with no flag become Camel's user flags, verbatim including the
  `$`.** A flag change sends the keyword back to the server, and a normalised
  one would not be the same keyword.
- **Addresses stay structured; the 64-bit threading digests stay undecided.**
  Camel's summary holds `from`/`to`/`cc` as one formatted string each, but
  formatting an address list is where RFC 5322's quoting and encoded-word rules
  live and `CamelInternetAddress` already has them — so this crate hands over
  the parts. `message_id` and `references` stay text for the mirror-image
  reason: Camel stores a truncated MD5 (`CamelSummaryMessageID`) with no public
  function to compute one, and since those digests are only ever compared
  against digests this provider wrote itself, the choice of hash belongs to the
  layer that fills the summary. `In-Reply-To` is folded onto the end of
  `References` when the chain does not already name it — a mailer that sends
  the first and not the second is common, and its replies would otherwise
  thread as new conversations.
- **`SUMMARY_PROPERTIES` is named explicitly.** RFC 8621 §4.2 makes the default
  property set *everything*, including `bodyStructure`, `textBody` and
  `bodyValues`; listing a mailbox with the default would multiply the answer by
  the size of the mail in it.
- **`Email/query` grew a `position` argument rather than a second method**, and
  `Session::max_objects_in_get` went on the session document in `jmap-proto`
  next to `primary_account`, returning `Option` — what to fall back to when a
  server breaks the rule and publishes no limit is the caller's decision, not
  the protocol type's. `MailSync` falls back to 50 and caps the advertised
  number at 500 either way: one `/get` for fifty thousand messages is a
  response Evolution waits on with the folder half open.

Mutation testing, twelve mutants, none surviving: the fetch unchunked, the
query stopped after one page, the `/get` order handed back instead of the
query's, the sort comparator dropped, the mailbox filter dropped,
`hasAttachment` ignored, a `false` keyword counted as set, keywords matched
case-sensitively, the size cast wrapping instead of saturating, `In-Reply-To`
never folded in, the offset's sign flipped, and the two dates read from each
other's field.

Not verified locally, as in the previous thirty-three sessions: `reuse lint`
and `cargo deny` (neither binary is installed on this VM). The four new files —
`src/date.rs`, `src/message.rs`, `tests/summary.rs`, `tests/messages.rs` — all
carry SPDX `GPL-3.0-or-later` headers. `cargo fmt --check`, `cargo test
--locked` (green on the default members; `jmap-mail-sync` now 40 tests) and
`cargo clippy --all-targets --locked -- -D warnings` are clean on both member
sets, the five EDS crates included — `jmap-client`'s changed `email_query`
signature has no other caller, and the mock's new `Email/get` limit is far
above what any other test seeds.

Next in M5: the Camel half of this — `CamelFolderSummary` on `CamelJmapFolder`,
which means the `camel_folder_summary_*` and `camel_message_info_*` allowlist
entries `eds-sys` has been deferring, a `CamelMessageInfo` per `MessageSummary`
(where the 64-bit message-id digest gets decided), and the folder flag
`CAMEL_FOLDER_HAS_SUMMARY_CAPABILITY` that `folder.rs` deliberately does not
set yet. `get_inbox_folder_sync` and the `CamelSubscribable` interface are still
the two smaller unblocked pieces if that stalls. Still unexercised against a
real `CamelSession`: `service.rs`, which waits on M6 and M7. The README's
architecture block still lists only the round-1 crates; five crates stale.

## 2026-08-08 (thirty-fifth session)

M5's fifteenth increment: `get_inbox_folder_sync`, the third and last of
`CamelStoreClass`'s folder vfuncs this provider can answer without a folder
summary. It is the one folder Camel asks for by *purpose* rather than by name,
and answering it needed one new thing underneath — `FolderTree::role`, the
lookup from a `FolderRole` back to the folder holding it — plus the vfunc that
turns the answer into an open `CamelFolder`. The red step was three
`FolderTree::role` tests failing to compile in `jmap-mail-sync`, then seven
`jmap-mail` tests against a store whose inbox is nested, foreign-named, and
shadowed by a decoy.

Decisions taken:

- **The inherited implementation is wrong for this provider, and the test says
  so by name.** `CamelStoreClass` is not one of the classes that leaves this
  slot NULL: its default asks the store's own `get_folder_sync` for a folder
  literally named `inbox`, and IMAPX overrides it to do the same thing one
  spelling up against `"INBOX"`. Both are IMAP conventions rather than facts
  about mail stores. RFC 8621 §2 puts a `role` on the mailbox and says nothing
  about its name or its place in the hierarchy, so the fixture is the account
  that tells the two apart — the real inbox is `Accounts/Posteingang`, and a
  top-level decoy is named `inbox` in exactly the case the default looks for.
  That the default *does* open the decoy was measured, not assumed: a throwaway
  test called the parent class's slot directly and printed the folder it came
  back with. Without the override this account runs the user's incoming filters
  over the decoy — silently, since both folders exist and both open.
- **The folder is not built here; it is asked for by path through
  `camel_store_get_folder_sync`.** Evolution opens the inbox both ways — by
  purpose at startup, where the filters are wired up, and by path when the user
  clicks it — and building one here would hand out a second `CamelFolder` over
  the same mailbox, with a second summary and a second set of flags, bypassing
  the `CamelObjectBag` the store keeps precisely to stop that. Delegating also
  means the inbox inherits every later fix to the opening path for free. The
  cost is one extra hash lookup; `the_inbox_is_the_folder_camel_already_has_
  open_for_that_path` is what pins it, by asserting pointer equality.
- **An account with no `role: "inbox"` has no inbox, and that is an error
  rather than a guess.** `role` is nullable on every mailbox, so this is a legal
  account, but Camel asked a question with no half-answer. Falling back to a
  mailbox *named* Inbox would be the provider guessing where the user's mail
  arrives, and guessing wrong means new mail filtered into a folder nobody
  reads — the same failure the inherited default has, reintroduced. New variant
  `StoreError::NoInbox`, reported in `CAMEL_STORE_ERROR_NO_FOLDER` beside
  `NoFolder`, whose case it is: a folder Camel asked for that the account does
  not have.
- **`tree_naming` became `tree_holding`, taking a question about the tree
  rather than a path.** The "look again if the held listing does not have it"
  rule is the same for both callers and the reason is the same — Evolution
  reopens the last-selected folder from its own settings before anything asks
  the store to refresh, and another client creating a mailbox mid-session is
  ordinary — but the question differs: a path for one, a role for the other. A
  closure keeps one implementation of the rule instead of two that drift.
- **`FolderTree::role` reads the role this crate assigned, not the mailbox's own
  property.** `claim_roles` already settles the two-inboxes case by giving the
  role to the first mailbox in sibling order; a lookup that re-derived it from
  `Mailbox::role` could pick the other one, and then the folder Camel opens as
  the inbox would not be the folder the listing marked `CAMEL_FOLDER_TYPE_INBOX`
  and `CAMEL_FOLDER_FILTER_RECENT`. It walks the whole tree rather than the
  roots, because nothing in RFC 8621 puts the inbox at the top level.
- **`cancellable` is passed on, for the first time in this module.** The two
  older vfuncs document why they cannot observe it — `Client` takes its
  `CancelFlag` at construction — and the listing this one does itself has the
  same gap. But the call it delegates to is Camel's own and has no such gap, so
  dropping the argument on the floor would be a regression against a caller
  that is already doing the right thing.

Mutation testing, six mutants, five killed and one equivalent: the vfunc left
unset (three behavioural failures now, not just the null-store crash), the
folder built here instead of through the bag, the role searched among the roots
only, the second look at the tree removed, and a mailbox named `Inbox` standing
in for the role. The equivalent one — taking the last mailbox claiming the role
rather than the first — changes nothing, because `claim_roles` has already left
exactly one folder carrying it; that it is equivalent is the invariant, not a
gap.

Not verified locally, as in the previous thirty-four sessions: `reuse lint` and
`cargo deny` (neither binary is installed on this VM). No new files this time,
so every file touched already carries its SPDX `GPL-3.0-or-later` header.
`cargo fmt --check`, `cargo test --locked` and `cargo clippy --all-targets
--locked -- -D warnings` are clean on both member sets — the default eight and
the five EDS crates the `rust-test-eds` target names.

Next in M5 is still the Camel half of the message work: `CamelFolderSummary` on
`CamelJmapFolder`, which needs the `camel_folder_summary_*` and
`camel_message_info_*` allowlist entries `eds-sys` has been deferring, a
`CamelMessageInfo` per `MessageSummary` (where the 64-bit message-id digest gets
decided), and the folder flag `CAMEL_FOLDER_HAS_SUMMARY_CAPABILITY` that
`folder.rs` deliberately does not set yet. `get_trash_folder_sync` and
`get_junk_folder_sync` are *not* the obvious follow-on to this increment even
though they share its signature: Camel's defaults build virtual folders
(vTrash/vJunk) rather than failing, and overriding them to return the real JMAP
mailbox is a behaviour change that IMAPX gates behind a `use-real-trash-path`
setting — so it is a settings decision first and a vfunc second. The
`CamelSubscribable` interface remains the smaller unblocked piece. Still
unexercised against a real `CamelSession`: `service.rs`, which waits on M6 and
M7. The README's architecture block still lists only the round-1 crates.

## 2026-08-08 (thirty-sixth session)

M5's sixteenth increment: `CamelMessageInfo`, the object one summary row is
kept in — the first half of the Camel side of the message work the previous two
sessions prepared in `jmap-mail-sync`. New module `jmap-mail`'s
`message_info`, and with it the `camel_message_info_*`, `camel_address_*`,
`camel_internet_address_*` and `camel_name_value_array_*` entries `eds-sys` has
been deferring. The red step was eleven tests that could not find the module.

Most of a row is a copy. Three columns are not, and they are what the increment
is about: the flags word, the formatted address headers, and the two 64-bit
digests Camel threads on.

Decisions taken:

- **The digests are checked against Camel, not against a constant.** Camel
  stores a message id as eight bytes of an MD5 over the `Message-ID` value with
  the brackets off — `CamelSummaryMessageID` — and there is no public function
  to compute one, which is why `jmap-mail-sync` left the choice to this layer.
  The layer's answer is that there is no choice: `camel_message_info_new_from_
  headers` is the path a message parsed locally takes, it lands in the same
  summary as a row built here, and two digests for one `Message-ID` thread one
  conversation as two. So the two tests that cover this hand Camel the headers
  the JMAP properties came from and assert that its digest is ours. That the
  function is declared in camel-folder-summary.h and rides in on the
  `camel_message_info_.*` prefix is noted in the allowlist as wanted rather than
  tolerated: it is the oracle.
- **The ancestors go in reversed, and that was measured.** `MessageSummary`
  holds the chain oldest first with the `In-Reply-To` parent appended, which is
  header order. Camel's own builder stores it the other way round — nearest
  ancestor at the front — and its threader walks from the front taking the first
  ancestor the folder actually holds. Filled in header order every reply in a
  long thread hangs off the root of its conversation instead of off its parent,
  which is a thread that looks flat rather than one that looks broken. The
  oracle test compares the whole array, order included.
- **No ancestors is a NULL array, not an empty one.** Camel leaves the column
  unset for a message with neither header, and an empty `GArray` is one the
  threader allocates, walks and finds nothing in on every rebuild. The same rule
  one column over: a header the message did not carry leaves the column unset
  rather than empty, because the summary goes to a database and reads back, and
  an empty `Cc` there is a `Cc:` that was present and blank.
- **The addresses are formatted by `CamelInternetAddress`, not by joining
  strings.** JMAP sends name/address pairs; Camel stores one display string per
  header. A display name may hold a comma, a quote or a backslash, and the rules
  for which have to be quoted are RFC 5322's, already implemented once in Camel
  — so the mapping builds an address object and asks it. The test that pins this
  has a comma in a display name, which naive joining turns into a second
  recipient.
- **`set_flags` is given a mask, not just a word.** The mask is the eight bits
  JMAP can speak to. `DELETED` and `FOLDER_FLAGGED` are local marks the user
  made — JMAP has no deleted keyword — and a refresh that cleared them because
  the server said nothing about them would undo a deletion the user is waiting
  to have expunged. On a fresh row the mask is invisible; it is there for the
  caller this function does not have yet.
- **The row is built with no summary behind it, and that is not a placeholder.**
  `camel_message_info_new` consults the summary only to learn which message-info
  type to instantiate. A summary that declares none — which is what this
  provider's will be — gets `CamelMessageInfoBase`, the same class NULL
  produces, so the row is the object it will be when there is a summary to add
  it to. `camel_folder_summary_.*` therefore stays off the allowlist for one
  more increment.
- **Notifications are frozen while the row is filled.** Every setter emits a
  property notification and marks the row dirty; a row filled column by column
  under a watching summary is a dozen changes to a message that has not been
  listed yet. Camel's own builders do the same.

Mutation testing, eight mutants, all eight killed: the ancestors not reversed,
the digest read big-endian, an empty array where NULL belongs, the two dates
read from each other's field, `$notjunk` mapped onto the junk bit, the flags
mask missing `ATTACHMENTS`, an absent address header stored as an empty column,
and the user flags never set.

`eds-sys` grew four layout assertions — `CamelMessageInfo`,
`CamelMessageInfoBase`, `CamelAddress`, `CamelInternetAddress` — and one test
that pins `CamelSummaryMessageID` at eight bytes, which is the contract
`message_id_digest` takes off the front of a sixteen-byte MD5.

Not verified locally, as in the previous thirty-five sessions: `reuse lint` and
`cargo deny` (neither binary is installed on this VM). The two new files —
`jmap-mail/src/message_info.rs` and `jmap-mail/tests/message_info.rs` — carry
SPDX `GPL-3.0-or-later` headers. `cargo fmt --check`, `cargo test --locked` and
`cargo clippy --all-targets --locked -- -D warnings` are clean on both member
sets, the five EDS crates included.

Next in M5 is the other half of the same work: `CamelFolderSummary` on
`CamelJmapFolder` — the summary subclass, the folder's `summary` property, the
rows above added to it from `MailSync::messages`, and the
`CAMEL_FOLDER_HAS_SUMMARY_CAPABILITY` flag `folder.rs` still deliberately does
not set. That is the increment `camel_folder_summary_.*` arrives with. The
`CamelSubscribable` interface remains the smaller unblocked piece;
`get_trash_folder_sync` and `get_junk_folder_sync` are still a settings decision
before they are a vfunc. Still unexercised against a real `CamelSession`:
`service.rs`, which waits on M6 and M7. The README's architecture block still
lists only the round-1 crates.

## 2026-08-08 (thirty-seventh session)

M5's seventeenth increment: `CamelFolderSummary`, the collection the rows of
the previous increment go into — and with it the flag `folder.rs` had been
deliberately withholding. New module `jmap-mail`'s `summary`, and the
`camel_folder_summary_.*`, `camel_folder_(get|take)_folder_summary`,
`camel_folder_has_summary_capability` and `camel_named_flags_.*` entries
`eds-sys` had been deferring. The red step was ten tests that could not find
the module.

Decisions taken:

- **No subclass, and that is the finding rather than the shortcut.** Camel's
  own providers subclass `CamelFolderSummary`, so the plan named a subclass;
  what the subclass exists for turns out to be building rows out of *messages*
  — the three `message_info_new_from_*` vfuncs and `next_uid_string`, which
  invents a uid for a message that arrived without one. A JMAP folder is
  listed rather than parsed: the rows come from `Email/get` already
  structured, each carrying the server's own immutable id, so all four would
  be overrides of paths this provider does not take. The one thing the base
  class decides for us is which message-info class to instantiate, and its
  answer is `CamelMessageInfoBase` — exactly what the previous increment
  built and pinned against, which is why that increment could pass NULL for
  the summary and still be building the object it will really be. A subclass
  becomes real when something local has to be numbered, which is `append` and
  `EmailSubmission`.
- **A refresh rewrites two columns and no others.** RFC 8621 §4.1 makes every
  property of an `Email` immutable except `keywords` and `mailboxIds`, and the
  mailbox is not a column of the row — it is which folder the row is in. So a
  listing that meets a row already there sets the flags word and the user
  flags and leaves the other dozen columns alone: it is what the server can
  honestly be said to have changed, it saves re-deriving two MD5s and three
  formatted address lists per message per refresh, and it is what keeps
  `CAMEL_MESSAGE_DELETED` — a local mark JMAP has no keyword for — from being
  undone by every refresh. The mutable half was factored out of
  `new_message_info` as `update_message_info`, which the fresh-row path now
  calls too, so the two can not drift.
- **User flags are replaced wholesale, empty set included.** Labels are
  keywords Camel has no bit for, and the keyword set is the whole truth about
  them: a keyword the server stopped sending is one taken off in some other
  client, and its absence is the only notice there is. Handing over an empty
  `CamelNamedFlags` is therefore right rather than merely harmless — unlike
  the text columns, where the summary database distinguishes an empty value
  from an absent one, user flags are stored as one joined string with no way
  to spell "absent". The test walks two labels down to one and then to none,
  because the last one is the case a `return if empty` gets wrong.
- **The reconciliation is the listing, not a delta.** A message the listing no
  longer names has left the mailbox, and from inside one folder that is
  indistinguishable from being deleted — JMAP moves mail by changing
  `mailboxIds`. `Email/query` answering without it is the only notice, so the
  row goes. The whole pass runs under the summary's (recursive) lock: half a
  mailbox is a worse answer than the previous one.
- **The test harness was building a store Camel never would.** `Account::open`
  used `g_object_new`; a `CamelStore` is a `GInitable`, and what its `init`
  does is open the summary database every folder writes its rows to. The
  store looked complete and had none — `camel_store_get_db` returned NULL —
  and the first row removed took the process down inside Camel with a
  SIGSEGV. Now `g_initable_new`, with a temporary directory per account
  (removed on drop) so that two tests running at once are not two tests
  sharing one folder's rows. That was a harness defect all along; it only
  became visible once a folder had a summary to remove something from.

Mutation testing, seven mutants, six killed: rows never removed, no summary
attached, the flag not set, an existing row replaced rather than updated, user
flags left alone when the listing carries none (which *survived* the first
round — the label test only ever went from two labels to one — and is what the
third listing in that test was added for), and the flags mask widened to the
whole word.

The seventh survives, and the honest answer is that it cannot be killed here:
`camel_folder_summary_add` with `force_keep_uid` FALSE renumbers only a uid
that is empty or already loaded, and neither reaches that call — the second
because `apply_message` checks first and takes the update path. TRUE is still
what is passed, because it is the statement that a server-assigned id is not
ours to change, and it is the value that stays right if the check above it
ever stops holding. The code comment says that rather than claiming the flag
is what prevents the renumbering.

`eds-sys` grew one layout assertion, `CamelFolderSummary`.

Not verified locally, as in the previous thirty-six sessions: `reuse lint` and
`cargo deny` (neither binary is installed on this VM). The two new files —
`jmap-mail/src/summary.rs` and `jmap-mail/tests/summary.rs` — carry SPDX
`GPL-3.0-or-later` headers. `cargo fmt --check`, `cargo test --locked` and
`cargo clippy --all-targets --locked -- -D warnings` are clean on both member
sets, the five EDS crates included.

Next in M5 is what now has somewhere to go: `refresh_info_sync` and
`get_message_count`/`get_uids` on `CamelJmapFolder` — the vfuncs that call
`MailSync::messages` and hand the result to `apply_listing`, which is the
first time the two halves of this and the previous three increments meet a
server. That increment is also where `CamelFolderChangeInfo` arrives: nothing
yet tells Camel *which* rows a refresh added or dropped, so a folder open in
Evolution would not redraw. `CamelSubscribable` remains the smaller unblocked
piece; `get_trash_folder_sync` and `get_junk_folder_sync` are still a settings
decision before they are a vfunc. Still unexercised against a real
`CamelSession`: `service.rs`, which waits on M6 and M7. The README's
architecture block still lists only the round-1 crates.

## 2026-08-08 (thirty-eighth session)

M5's eighteenth increment: `refresh_info_sync`, the vfunc where the last four
increments meet a server. New modules `jmap-mail`'s `refresh` and `changes`, a
`JmapStore::messages` for the folder to ask its store through, a `class_init` on
`CamelJmapFolder` — which had none until now — and the `CamelFolderChangeInfo`
type plus `camel_folder_change_info_.*`, `camel_folder_changed`,
`camel_folder_refresh_info_sync`, `camel_folder_get_message_count` and
`camel_folder_(get|free)_uids` in `eds-sys`. The red step was eleven tests: six
against `apply_listing`'s diff, four against the vfunc, one in `eds-sys` that
could not find `camel_folder_change_info_free`.

Decisions taken:

- **The diff is returned, not emitted.** `apply_listing` is handed a summary and
  now hands back a [`Changes`]; emitting is a fact about a `CamelFolder`, which
  is one level up. That keeps every reconciliation rule testable without a
  signal to listen for, and it is what let the six diff rules be written against
  the same detached folder the previous increment used.
- **A row is reported as changed only when it moved.** Both of Camel's setters
  return whether they changed anything, so the verdict is Camel's rather than a
  comparison of our own; `update_message_info` now passes it up. A refresh is a
  poll, so nearly every row a listing meets is the row it left there — a folder
  that announced all of them would redraw the message list the user is reading
  every time the timer went off. The two setter calls are two statements joined
  by `||` rather than one `||` expression, because `||` short-circuits and a row
  whose flags moved must still have its labels written. That distinction was the
  one mutant the first round did not kill, and
  `a_row_that_moved_in_both_columns_has_both_of_them_written` is what was added
  for it: read *and* relabelled in one listing is what Evolution's own "mark as
  read and file it" rule does.
- **Nothing is ever recent, deliberately.** Camel's fourth uid list is what runs
  the user's incoming filters, and a JMAP listing cannot tell a message that has
  just arrived from one that was always there — the first refresh of an account
  finds the whole mailbox, so "added" and "recent" would be the same list and the
  user's rules would file, forward or delete every message they already had.
  `Email/changes` against a state kept across restarts is what could answer the
  question honestly, and it needs somewhere on disk to keep that state.
- **Two things Camel does that the tests had to be rewritten around, both found
  by a red test that stayed red.** First, `camel_folder_refresh_info_sync`
  connects the folder's parent store before it dispatches to the class — so the
  disconnected case never reaches this vfunc through the wrapper, and its test
  calls the class pointer directly the way `tests/folders.rs` calls
  `get_folder_sync`. What made this visible was the error code: `URL_INVALID`
  with "the account does not name a JMAP server", which is our own
  `authenticate_sync` answering, not our refresh. Second,
  `camel_folder_changed` does not emit where it is called — it queues the diff
  and delivers it from the folder's main context, coalescing whatever else is
  pending into one emission. A Rust test thread never iterates a main loop, so
  the first version of the signal test observed silence and would have passed
  no matter what the vfunc did; `emissions()` now pumps the default context
  before it reads.
- **The parent store is type-checked, unlike a vfunc's first argument.** The
  store vfuncs get an instance GObject dispatched on the class, so
  `JmapStore::borrow` on that argument is sound by construction; `parent-store`
  is an ordinary construct property, so the folder asks
  `g_type_check_instance_is_a` first. Reading a `JmapStore` out of someone
  else's store would be undefined behaviour rather than a wrong answer.
- **A whole listing per refresh, and that is the known cost.** `Email/changes`
  would ask a much smaller question and is the same later increment as the
  recent list. Listing is correct meanwhile.

Mutation testing, seven mutants, six killed on the first round and the seventh
after the test above was added: the signal emitted unconditionally (which killed
*both* signal tests, so the coalescing had been understood correctly), every met
row reported as changed, removals never reported, the vfunc not installed on the
class, the recent list filled from the added one, and the `||` short-circuit.

Not verified locally, as in the previous thirty-seven sessions: `reuse lint` and
`cargo deny` (neither binary is installed on this VM). The three new files —
`jmap-mail/src/changes.rs`, `jmap-mail/src/refresh.rs` and
`jmap-mail/tests/refresh.rs` — carry SPDX `GPL-3.0-or-later` headers.
`cargo fmt --check`, `cargo test --locked` and
`cargo clippy --all-targets --locked -- -D warnings` are clean on both member
sets, the five EDS crates included.

Next in M5 is what a filled summary makes possible and what a refreshed folder
still cannot do: `get_message_sync`, the body behind a row — `Email/get` with
the body properties, or the blob download, turned into a `CamelMimeMessage` —
which is the last thing between this provider and reading mail in Evolution.
`synchronize_sync` is the other half of the same conversation, and the first
thing in the crate that writes: a row Camel marked read or deleted is a
`keywords` patch through `Email/set`. `CamelSubscribable` remains the smaller
unblocked piece; `get_trash_folder_sync` and `get_junk_folder_sync` are still a
settings decision before they are a vfunc. Still unexercised against a real
`CamelSession`: `service.rs`, which waits on M6 and M7. The README's
architecture block still lists only the round-1 crates.

## 2026-08-08 (thirty-ninth session)

M5's nineteenth increment: `MailSync::message_source`, the RFC 5322 bytes
behind one summary row — the fetch half of `get_message_sync`, without the
`CamelMimeMessage` construction that is the other half. With it, the store
accessor the folder vfunc will reach it through, the error a uid that outlived
its message becomes on the way up, and a mock that serves a real message
instead of an empty blob. Also, unplanned: the fix for an intermittent failure
in `tests/refresh.rs` that turned out to be a misunderstanding of GLib rather
than of Camel.

Decisions taken:

- **The blob id is fetched, not remembered.** `MessageSummary` carries one and
  the folder throws it away, because a `CamelFolderSummary` row has nowhere to
  keep it — the same problem the folder's own mailbox id has, without the
  folder's solution: a row is Camel's struct, not ours, and there is one per
  message in the account rather than one per folder. So a uid is all the call
  can be given and the blob id is one `Email/get` away. That is a round trip
  per message opened and it is also the only version that stays correct: RFC
  8621 §4.1 makes an `Email` immutable but nothing stops a server reissuing
  blob ids, and RFC 8620 §6.2 lets it forget one whenever it likes. A cached
  blob id turns such a server into a mailbox that reads fine until it suddenly
  does not. `SOURCE_PROPERTIES` is two properties and not the summary's
  sixteen, for the same reason the summary's list is not the server's default
  set.
- **A missing message is not a client error.** `SyncError::NoSuchMessage` is a
  variant of its own, and it exists so that `StoreError::NoMessage` can map it
  to `CAMEL_FOLDER_ERROR_INVALID_UID` — a third Camel error domain beside the
  service's and the store's, added to `eds-sys` for it. A uid is a claim about
  the last listing; another client deleting the message since is ordinary, and
  reported as a service error it would be a working account shown as broken
  because one message went away between a listing and a click. Two tests hold
  that line: the domain and code of the `GError`, and the `From` that must not
  flatten the variant back into a client error on the way through.
- **Three ways to have no answer, told apart.** A uid the account does not hold
  is gone. A message returned *without* a `blobId` is the protocol violation it
  is, with no fallback — reassembling the message from its body parts would
  produce different bytes than the ones it was signed as. A blob the server
  will not serve is the download's own failure, which is neither of the first
  two: the row is fine and retrying is not hopeless. `tests/source.rs` asserts
  each, and asserts that the third is *not* reported as the first.
- **The mock now serves a message.** A seeded email's `message/rfc822` blob was
  `Vec::new()`, so a download that worked and one that silently returned
  nothing looked identical. It is now rendered from the seed: headers, a blank
  line, the body, CRLF throughout. Single-part deliberately — attachments are
  their own blobs with their own ids, and building the multipart to contain
  them would be writing a MIME composer inside a test server. The `Date` header
  carries no day of the week, which RFC 5322 §3.3 permits: deriving it needs a
  calendar the mock has no other use for, and a wrong one would be worse than
  none.

The unplanned half. `tests/refresh.rs` had been failing roughly one run in
four since it landed last session, on both of the tests that assert about the
`changed` signal — and on master, so it was not this increment's doing. The
previous session's diagnosis was right about Camel (the signal is queued and
delivered from a main context) and wrong about GLib: `g_main_context_iteration`
*acquires* the context first, and returns immediately having dispatched nothing
when another thread already owns it. A Rust test binary runs its tests on
threads of one process, all pumping the one global default context, so a test's
pump could be a no-op — its emission then arriving one pump too late, read as
silence by the test waiting for it and as an unexplained emission by whichever
test pumped next. Camel queues onto whatever context was thread-default when
`camel_folder_changed` was called, so a `Context` pushed at the top of each such
test is a queue per test that no other thread can take a turn on. Eight
consecutive runs green afterwards; the record of emissions stays a thread local,
because with the contexts separated the pumping thread is the only one that can
deliver.

Not verified locally, as in the previous thirty-eight sessions: `reuse lint` and
`cargo deny` (neither binary is installed on this VM). The one new file —
`jmap-mail-sync/tests/source.rs` — carries an SPDX `GPL-3.0-or-later` header.
`cargo fmt --check`, `cargo test --locked` and
`cargo clippy --all-targets --locked -- -D warnings` are clean on both member
sets, the five EDS crates included. (`example-module` fails clippy on
pre-existing `manual_c_str_literals` findings; it is in neither CI clippy
invocation and was not touched.)

Next in M5 is the half this increment stopped short of: `get_message_sync`
itself — `CamelMimeMessage`, a `CamelStream` over the bytes, and
`camel_data_wrapper_construct_from_stream_sync`, none of which are in `eds-sys`
yet — and the offline cache question that comes with it, since a message fetched
once should not be fetched again. `synchronize_sync` is the first thing in the
crate that writes: a row Camel marked read is a `keywords` patch through
`Email/set`. `CamelSubscribable` remains the smaller unblocked piece;
`get_trash_folder_sync` and `get_junk_folder_sync` are still a settings decision
before they are a vfunc. `Email/changes` against a state kept on disk is still
what the empty `recent` list and the whole-mailbox refresh are both waiting for.
Still unexercised against a real `CamelSession`: `service.rs`, which waits on M6
and M7. The README's architecture block still lists only the round-1 crates.

## 2026-08-09 (fortieth session)

M5's twentieth increment, and the one the previous nineteen were building
towards: `get_message_sync`, the folder vfunc that turns a message list row
into mail a person can read. The fetch half landed last session
(`MailSync::message_source` — an `Email/get` for the blob id, then the blob);
this is the half that makes an object out of the bytes, plus the `eds-sys`
bindings the object needs and a home for the store accessor two vfuncs now
share.

The session opened on a working tree that already held the increment,
uncommitted — the previous run wrote the code and the tests and stopped before
the gates. So the first thing done was the check that TDD would otherwise have
given for free: the vfunc installation was removed and `tests/message.rs` run
against the rest, which failed all three, the success case with "the message
would not open: no error". That last part is worth recording — Camel's
`camel_folder_get_message_sync` does not guard a class with an empty
`get_message_sync` slot, so an uninstalled vfunc is not a critical or a
`G_IO_ERROR_NOT_SUPPORTED` but a silent NULL with no `GError`, which a caller
that only tests the error pointer would read as success. Then restored, green
again, and the gates run on both member sets.

Decisions taken:

- **Camel parses it, not us.** `camel_data_wrapper_construct_from_data_sync`
  on the message's `CamelDataWrapper` face. The object has to agree with the
  rest of Camel — the filters, the reply composer, the save-as dialogue —
  about what the message says, and a provider that read the headers itself
  would be a second MIME implementation inside the same process, disagreeing
  with the first at exactly the edge cases MIME has. The corollary is that a
  parse failure is passed through rather than reclassified: a message Camel's
  parser rejects is one this crate has no better account of, and dressing it
  as a service error would report a malformed message as a broken account.
- **One buffer, not a stream.** `construct_from_data_sync` rather than a
  `CamelStreamMem` and `construct_from_stream_sync`, which is what the
  previous session's note assumed would be needed. A blob download already
  produced the whole message in memory, so the stream would wrap a buffer this
  code is holding anyway and would put `CamelStream`, `CamelStreamMem` and
  their class structs across the ABI to do it. The cost is the message being
  in memory twice for the length of the parse, which is the trade every caller
  of `get_message_sync` makes regardless, since what they get back *is* the
  whole message. The length is `gssize::try_from(...).unwrap_or(MAX)`: a
  saturating cast leaves a truncated parse, which fails loudly, where a
  wrapping one would leave a negative length that Camel reads as "to the end
  of the buffer" and walks off it.
- **Two error domains, and the distinction is the feature.** A uid the account
  no longer holds is `CAMEL_FOLDER_ERROR_INVALID_UID`; a store with no
  connection is `CAMEL_SERVICE_ERROR_NOT_CONNECTED`. The second is what makes
  Camel reconnect and ask again, the first is what makes it drop one row.
  Swapped, a message another client deleted between a listing and a click
  would take a working account offline — which is why both are tests and not
  just code. The third case, a parse that fails without setting an error, gets
  a synthesised `CAMEL_FOLDER_ERROR_INVALID`: the uid was fine and the account
  is fine, and answering NULL with no error set is the one thing Camel logs a
  critical for.
- **`parent_store` moved to `folder.rs`.** It was a private helper in
  `refresh.rs` because `refresh_info_sync` was the only vfunc that needed the
  store behind the folder. It is a fact about the folder object, not about
  refreshing, and with `get_message_sync` it is the first line of two vfuncs;
  the type check it does — `parent-store` is an ordinary construct property,
  not a GObject dispatch, so a `JmapStore` read out of one unchecked would be
  undefined behaviour rather than a wrong answer — is the sort of thing that
  should exist once.
- **The tests ask Camel, not the mock.** Comparing the downloaded bytes
  against the mock's own rendering would assert that the mock agrees with
  itself. What has to be true is that Camel can read the result, so the parsed
  message is interrogated through Camel's accessors: the subject, the `From`
  run through `camel_address_format`, and the body decoded via
  `camel_medium_get_content` and the wrapper's own decoder — the last being
  the assertion that needs the whole message to have arrived rather than its
  first few hundred bytes. `body_of` guards the NULL buffer a resizable
  `GMemoryOutputStream` has when nothing was written to it, because
  `from_raw_parts` on it is undefined behaviour and would turn a test that
  should fail with a readable message into an abort with no assertion in it.
- **Four `eds-sys` types for one call.** `CamelDataWrapper`, `CamelMedium`,
  `CamelMimePart` and `CamelMimeMessage` are one inheritance chain, and all
  four are allowlisted because the parse entry point is declared on the *last*
  ancestor: the provider crosses the ABI at every level of it. Layouts
  spot-checked against `g_type_query` like the rest.

Not verified locally, as in the previous thirty-nine sessions: `reuse lint`
and `cargo deny` (neither binary is installed on this VM). The two new files —
`jmap-mail/src/message.rs` and `jmap-mail/tests/message.rs` — carry SPDX
`GPL-3.0-or-later` headers. `cargo fmt --check`, `cargo test --locked` and
`cargo clippy --all-targets --locked -- -D warnings` are clean on both member
sets, the five EDS crates included.

Next in M5, in the order they look tractable. **The offline cache**, which is
the gap this increment leaves: every open is two round trips, and RFC 8621
§4.1 makes an `Email` immutable, so a message fetched once never needs
fetching again — `CamelDataCache` is where IMAPX keeps one, a file per message
under the account's cache directory, with a `purge_message_cache_sync` to
bound it. **`synchronize_sync`**, still the first thing in the crate that
writes: a row Camel marked read or deleted is a `keywords` patch through
`Email/set`. `CamelSubscribable` remains the smaller unblocked piece;
`get_trash_folder_sync` and `get_junk_folder_sync` are still a settings
decision before they are a vfunc. `Email/changes` against a state kept on disk
is what the empty `recent` list and the whole-mailbox refresh are both still
waiting for. Unexercised against a real `CamelSession`: `service.rs`, which
waits on M6 and M7. The README's architecture block still lists only the
round-1 crates.

## 2026-08-09 (forty-first session)

M5's twenty-first increment, and the gap the twentieth left open: a message
opened twice was downloaded twice. `get_message_sync` now consults a
`CamelDataCache` under the account's own cache directory before it looks for a
connection, so the second click on a row costs nothing and a message already
read opens with the network gone — which is what a provider whose store is a
`CamelOfflineStore` is claiming to be able to do.

Red first, in two steps. `tests/cache.rs` against a `jmap_mail::cache` that did
not exist yet (seven tests, the wrapper's whole contract); then, once those were
green, two tests in `tests/message.rs` that failed for the right reasons — the
offline reopen with "transport error: timeout" after the mock was dropped, and
the account-directory test with "the opened message was not cached under the
account".

Decisions taken:

- **Keyed by uid, under the account, not the folder.** A JMAP mailbox is closer
  to a label than to a directory: the same `Email` is filed in several of them
  and carries one id in all of them. So the cache directory is the *service's*
  — `camel_service_get_user_cache_dir`, the one Evolution's "empty cache"
  clears and Camel removes with the account — and the key is the uid alone,
  which makes a message filed in five mailboxes one file. IMAPX keys per folder
  because an IMAP uid only means something inside one; ours means something
  inside the account, and copying IMAPX's shape would have stored the same mail
  five times. The object holding the cache is still the folder, because
  `new_folder` is the one place in this crate that has a fully constructed
  store in hand at a well-defined moment — `instance_init` runs before the
  construct properties the cache directory is derived from are set.
- **A uid is about to become a file name.** `camel_data_cache_add` joins the key
  onto a path, so an `Email/query` answering `../../../.config/autostart` would
  otherwise be a server choosing where this provider writes. Keys are checked
  against RFC 8620 §1.2's own grammar — one to 255 of `A-Za-z0-9_-`, no leading
  dash — and a key that fails it is not cached rather than sanitised: a
  rewritten key would still be a file, and the id it came from is not one this
  provider should be talking to a server about. `.` and `..`, the two that
  matter most, fall to the character set. The test walks eight of them and then
  checks the parent directory is still empty.
- **Best-effort, and that is the contract rather than a shortcut.** `open`
  answers `None`, `load` answers `None`, `store` answers a `bool` the caller
  ignores. Every failure a cache can have — a full disk, a read-only directory,
  an entry another process removed mid-read — is a condition under which mail
  must still open, just slower, and the only error out-parameter in reach
  belongs to the vfunc, where it means "this message cannot be produced". A
  cache that turned a working account into a broken one would be worse than no
  cache. The failures are logged as criticals instead, because a cache
  directory that cannot be made is a broken installation and the symptom
  without a log line is mail that is merely slow.
- **An empty entry is not a message, at both ends.** Camel's parser makes an
  empty `CamelMimeMessage` out of zero bytes rather than refusing them, so an
  entry a process died before writing would be served as an empty message in
  preference to the download that would have replaced it — forever, since
  nothing invalidates. `store` refuses an empty source and `load` refuses an
  empty entry. The related hole that is *not* closed is a short write: MIME has
  no length, so a truncated file parses as a complete message with a truncated
  body. A failed write removes its entry, which covers the case this process
  can see; the case it cannot is a crash mid-write, and the check that would
  close it is the one number a summary row already carries — the `Email`'s
  `size`, which RFC 8621 §4.1 defines as the octets of exactly these bytes.
- **A cached entry that will not parse falls through to the fetch.** The cached
  path parses with a NULL error out-parameter, which `set_raw_gerror` already
  defines as "free it": an entry Camel's parser rejects is not a message to
  report, it is one to replace. The fetched path keeps its bytes *before* it
  parses them, so a parse failure does not turn every later open of that
  message into two more round trips.
- **Our lock, not Camel's.** Camel documents no thread-safety guarantee for
  `CamelDataCache`, and Camel drives a folder from several threads at once, so
  the pointer lives in a `Mutex` — held for one entry's IO and never across a
  network fetch, which is the part of an open worth overlapping. That plus the
  pointer never being handed out is what the `unsafe impl Send`/`Sync` rests on.
- **The close gets a GError of its own.** `g_io_stream_close` is what flushes,
  so a write that reported success and a close that failed is still an
  incomplete entry — but passing the same out-parameter to both makes GLib log
  a critical of its own for the second `g_set_error` over an already-set one.
  The write's reason is the one worth reporting, so the close's is only adopted
  if the write left none.
- **`CamelDataCache` in `eds-sys`, layout-checked like the rest.** Nothing
  subclasses it — the provider only ever holds one — but it crosses the ABI, so
  it is in `tests/layout.rs` against `g_type_query` with the other twenty-odd
  types. The streams come from gio-sys, not from a second binding: the entry is
  a `GIOStream`, and `g_output_stream_write_all`'s buffer is typed `*mut` there
  although the C declaration says `const void *`, which is one cast with a
  comment on it.

Not verified locally, as in the previous forty sessions: `reuse lint` and
`cargo deny` (neither binary is installed on this VM). The two new files —
`jmap-mail/src/cache.rs` and `jmap-mail/tests/cache.rs` — carry SPDX
`GPL-3.0-or-later` headers. `cargo fmt --check`, `cargo test --locked` and
`cargo clippy --all-targets --locked -- -D warnings` are clean on both member
sets, the five EDS crates included. One measurement worth keeping: the message
suite ran in 30s while the offline test was red, waiting on a transport timeout
against a server that had been dropped, and 0.04s once the cache answered it.

Next in M5. **`synchronize_sync`** is still the first thing in the crate that
writes: a row Camel marked read or deleted is a `keywords` patch through
`Email/set`, and it is now the largest unbuilt piece of the folder. **Bounding
the cache** is the other half of what landed today — nothing removes an entry,
so it grows with every message ever opened; `CamelDataCache` has
`set_expire_age`/`set_expire_enabled` and Evolution's "empty cache" is
`camel_data_cache_clear`, and which of the two this provider should offer is a
settings question before it is a mechanism one. `CamelSubscribable` remains the
smaller unblocked piece; `get_trash_folder_sync` and `get_junk_folder_sync` are
still a settings decision before they are a vfunc. `Email/changes` against a
state kept on disk is what the empty `recent` list and the whole-mailbox
refresh are both still waiting for. Unexercised against a real `CamelSession`:
`service.rs`, which waits on M6 and M7. The README's architecture block still
lists only the round-1 crates.

## 2026-08-09 (forty-second session)

M5's twenty-second increment, and the first thing `jmap-mail-sync` does that is
not a read: the `Email/set` a changed flag becomes. `MailSync::set_keywords`
takes one message id and a `KeywordChange` — the difference between the keywords
the last listing found and the keywords the row claims now — and patches exactly
those members of the server's `keywords` object. Everything the Camel side needs
to turn a row Camel marked read or important into a request now exists; what is
still missing is `synchronize_sync`, the vfunc that decides *which* rows and
holds the two ends of the difference.

Red first: `tests/keywords.rs`, twelve tests against a `jmap_mail_sync::keywords`
that did not exist — seven on the mapping itself, five against a live mock.

Decisions taken:

- **A difference, not a state.** The obvious shape — send the row's whole
  keyword set and let the server replace what it has — says something about
  every keyword on the message, and what it says is "gone" for any that arrived
  after the listing the row came from: a label from the user's phone, a
  `$phishing` verdict from the server's own filter, a keyword this provider has
  no name for. A patch of named members leaves everything neither side mentions
  as it was, which is the only thing a client that holds no lock can honestly
  claim about them. It is also the cheap shape: Camel hands the folder a row
  that changed, and the keywords it *had* are what its summary was filled from.
  Two of the twelve tests are about this and nothing else — a keyword seeded on
  the server that neither side of the diff names survives the write, and a
  keyword in both halves appears in neither half of the patch.
- **No `ifInState`.** The state a folder holds is its listing's, so a
  conditional write would fail for any change to any *other* message in the
  account — a mailbox with traffic would refuse every flag change the user
  makes. Keyword changes commute, being a patch of named members rather than a
  replacement, so the concurrency that matters is per keyword, and sending only
  what changed is what handles it.
- **Keys are JSON pointers, and a keyword may contain the two characters that
  makes special.** RFC 8620 §5.3 makes each `PatchObject` key a pointer (RFC
  6901) into the object, and an RFC 5788 keyword is an IMAP atom — which permits
  `/` and `~`. Unescaped, a user's `home/todo` label would address a `todo`
  member of a `home` object inside `keywords`, inventing structure instead of
  setting a keyword. Escaped in RFC 6901's order, `~` before `/`, or the `~1`
  produced for a slash would be read again. Tested both as a patch shape and as
  a round trip through the mock, which comes back holding one keyword named
  `home/todo`.
- **Keywords are compared folded, removed as spelled.** RFC 8621 §4.1.1 takes
  its vocabulary from RFC 5788, whose keywords are case-insensitive, so a server
  that stores `Work` and a row that spells it `work` hold the same keyword and a
  diff that missed that would rewrite it on every synchronisation. `Keywords` is
  therefore keyed by the folded name and remembers the spelling it arrived with:
  an addition quotes the row's spelling, a removal quotes the *server's*, because
  the key a patch takes off an object has to be the key the object has.
- **`hasAttachment` is not a keyword.** It is the one bit of `MessageFlags` that
  comes from a property RFC 8621 §4.1.1 has the server compute, not from a label,
  and sending it back as one would put a label on the message that every other
  client would then show. One test asserts the empty set.
- **An empty change is not a request.** Camel marks a row as needing a write for
  reasons that are not keywords, and a provider that asked the server about each
  of them would spend a round trip per row on every synchronisation. The test
  that proves it is the one that succeeds for a uid the account has never
  held — the neighbouring test shows the same call *with* a change reports that
  uid as gone.
- **`notFound` is `SyncError::NoSuchMessage`, everything else stays the
  server's.** The same judgement `message_source` already makes about the same
  situation: a uid in a folder summary is a claim about the last listing, and
  another client destroying the message makes the flag change moot rather than
  the account broken. A keyword the server will not accept or a mailbox gone
  read-only are things the user has to be told about, so they stay
  `SyncError::Client` with the server's own reason inside.
- **`Client::email_update` takes a `PatchObject`, not an `Email`.** Mirrors
  `contact_update`. Most of what an `Email` holds is immutable (RFC 8621 §4.1)
  and the two members that are not are sets other clients write to, so there is
  no correct whole-object update to offer.

Not verified locally, as in the previous forty-one sessions: `reuse lint` and
`cargo deny` (neither binary is installed on this VM). The two new files —
`jmap-mail-sync/src/keywords.rs` and `jmap-mail-sync/tests/keywords.rs` — carry
SPDX `GPL-3.0-or-later` headers. `cargo fmt --check`, `cargo test --locked` and
`cargo clippy --all-targets --locked -- -D warnings` are clean on the default
member set and on the five EDS crates. `example-module` does not link on this
VM — `e_mail_shell_view_get_type` is in Evolution's mail shell library, which is
not installed here — and it is outside both sets this session touched; nothing in
this increment reaches it.

Next in M5. **`synchronize_sync`** is now the missing half of what landed today:
the vfunc that walks the folder's summary for rows Camel marked dirty, builds
the two keyword sets a `KeywordChange` is made from, and clears the dirty bit on
the ones the server accepted. The hard part is the *before* set — a summary row
is mutated in place by the user, so the keywords the listing found are gone by
the time the write happens unless the folder keeps them; IMAPX solves this with
a server-flags field in its own message-info subclass, which is a decision for
that increment. **Bounding the cache** is still open from the forty-first
session. `CamelSubscribable` remains the smaller unblocked piece;
`get_trash_folder_sync` and `get_junk_folder_sync` are still a settings decision
before they are a vfunc. `Email/changes` against a state kept on disk is what the
empty `recent` list and the whole-mailbox refresh are both still waiting for.
Unexercised against a real `CamelSession`: `service.rs`, which waits on M6 and
M7. The README's architecture block still lists only the round-1 crates.

## 2026-08-09 (forty-third session)

M5's twenty-third increment, and the missing half of the last one: the row now
remembers what the server was last seen holding. `CamelJmapMessageInfo` is a
`CamelMessageInfoBase` with the listing's keyword set beside it, and
`CamelJmapSummary` is the one-field subclass that makes Camel build rows of that
type when it reads a folder back off disk. `synchronize_sync` now has both ends
of the difference it has to send; what is still missing is the vfunc that walks
the summary for the rows Camel marked dirty.

Red first: two tests in `jmap-mail-sync`'s `tests/keywords.rs` for the set built
back out of the names it was stored as, six in `jmap-mail`'s
`tests/message_info.rs` and three in `tests/summary.rs` — the last of those
against a folder closed and opened again over the store's real summary database,
which is the only test that would have caught a column that saves and does not
load.

Decisions taken:

- **The before is a column, not a recomputation.** A summary row holds Camel's
  flags word and its user flags, and both are the *after*: the user marking a
  message read mutates the row in place, so by the time a write happens the
  keywords the listing found are gone unless the row kept them. IMAPX solves the
  same problem the same way — a `server_flags` word next to Camel's in its own
  message-info subclass — and there is no other place to put it: Camel's row has
  no spare field, and inventing one out of user flags would put this provider's
  bookkeeping in the namespace Evolution draws labels from.
- **`bdata`, because it is the field Camel reserves for exactly this.** A
  `CamelMIRecord` has one string per row for what a provider knows and Camel does
  not, appended to by the class chain on the way out and read back through the
  same cursor on the way in. The names go through `camel_util_bdata_put_string`
  rather than joined here: its encoding is length-prefixed, and a label like
  `Read later` or `9-lives` is what says why that matters — a separator-joined
  format would bring one keyword back as two. The count is written first because
  the cursor is a stream shared with every other class in the chain, so a reader
  has to know when to stop.
- **A row that lost the column remembers nothing rather than failing to load.**
  Reporting failure would drop the whole row over the one column nothing else
  needs, and the empty set is the *conservative* answer rather than merely a
  tolerable one: a difference from nothing only ever adds keywords, so a summary
  written before this column existed removes none. Same rule for a count that
  runs past the end of the string — the reads stop at the first name that is not
  there and keep the ones that were.
- **The summary subclass overrides no vfunc.** All four a `CamelFolderSummary`
  subclass usually exists for build rows out of *messages* — a parser, a MIME
  message, a header list, a locally invented uid — and a JMAP folder is listed
  rather than parsed. What it declares is `message_info_type`, which is not a
  vfunc at all: it is the field Camel reads when it instantiates a row itself,
  which is every row of every folder after a restart. `tests/summary.rs` pins
  that field against the same function `new_message_info` constructs through, so
  the two paths cannot answer differently.
- **A clone carries the column only when the copy is one of ours.** The parent's
  clone builds its result out of the summary it is told to assign the copy to, so
  a row cloned into no summary comes back a plain `CamelMessageInfoBase` — and
  that is left exactly as the parent made it. Forcing the type would mean
  rebuilding every column of the row here, which is the parent's job and would
  silently stop copying whatever column Camel adds next. A copy that is not of
  this type is a copy in a folder with no JMAP keywords to be asked about.
- **The set is renewed by a refresh and is not part of what a refresh reports.**
  `update_message_info` rewrites it beside the flags and the labels, and
  deliberately does not fold it into the answer it gives the folder: what the
  server holds is not a column the message list draws, so a listing that only
  re-spelled a keyword is not a change to announce. A keyword the server really
  added arrives as a flag or a label as well, and is reported as one of those.
- **A mutex, unlike the folder's mailbox id.** The id is written once before
  anything can reach the object; this is written by every refresh and read by
  every synchronisation, from the several threads Camel drives a folder from. The
  lock is never held across anything but a clone of the set, and the accessor
  hands out a copy rather than a borrow.

Not verified locally, as in the previous forty-two sessions: `reuse lint` and
`cargo deny` (neither binary is installed on this VM). No new files this session,
so every file touched already carries its SPDX `GPL-3.0-or-later` header.
`cargo fmt --check`, `cargo test --locked` and
`cargo clippy --all-targets --locked -- -D warnings` are clean on the default
member set and on the five EDS crates; `jmap-mail` is at 173 tests. Two
allowlist entries in `eds-sys`: `camel_util_bdata_.*` and the `CamelMIRecord`
typedef.

Next in M5. **`synchronize_sync`** is now unblocked in full: the vfunc walks the
summary for rows carrying `CAMEL_MESSAGE_FOLDER_FLAGGED`, builds a
`KeywordChange` from the column that landed today against the row as it now is,
sends it, and — on success — renews the column and clears the bit. The one design
question left is what to do with a row whose write failed: leaving the bit set
retries it on the next synchronisation, which is right for a network failure and
a loop for a keyword the server will never accept. **Bounding the cache** is
still open from the forty-first session. `CamelSubscribable` remains the smaller
unblocked piece; `get_trash_folder_sync` and `get_junk_folder_sync` are still a
settings decision before they are a vfunc. `Email/changes` against a state kept
on disk is what the empty `recent` list and the whole-mailbox refresh are both
still waiting for. Unexercised against a real `CamelSession`: `service.rs`, which
waits on M6 and M7. The README's architecture block still lists only the round-1
crates.

## 2026-08-09 (forty-fourth session)

M5's twenty-fourth increment, and the first thing in the mail provider that
writes: `synchronize_sync`. The folder walks the rows Camel has not written back,
diffs the keywords each row claims now against the ones it remembers the server
holding, sends the difference as an `Email/set` update, and — on success —
renews the remembered set and takes the row off the work list.

Red first: nine tests in `jmap-mail`'s `tests/synchronize.rs`, all of them
against `jmap-mockd` through a real `CamelStore`, and four more in
`tests/message_info.rs` for the two column-level rules the walk rests on.

Decisions taken:

- **`get_changed` is where the walk starts, not what it trusts.** IMAPX drives
  its own synchronisation from `camel_folder_summary_get_changed`, and the name
  promises something narrower than the function delivers: it gathers the rows
  Camel has not yet written to the *summary database*, not the rows carrying
  `CAMEL_MESSAGE_FOLDER_FLAGGED`. A freshly listed row is on it. So every row on
  the list is diffed, and the diff is what decides whether anything is sent —
  which is what IMAPX effectively does too, by comparing against its own server
  flags rather than believing the list.
- **A listing must not queue what it lists.** This was the session's finding, and
  it was a real bug rather than a tidiness point. Every one of Camel's column
  setters marks the row as having to reach the server, and so does
  `camel_folder_summary_add` — both right for the caller they were written for
  (the user changing a message, a message the user composed) and backwards for a
  provider filling a summary from a listing. Before this increment nothing read
  the bit, so it was invisible; with `synchronize_sync` in place, a refresh would
  have queued every message of every mailbox to be written straight back to the
  server it had just been read from. `new_message_info` and `update_message_info`
  now write inside a `without_queueing` that puts the bit back the way it found
  it, and `apply_message` clears it on the row `summary_add` just marked.
- **Restoring the bit, not clearing it.** The two are the same for a row that
  came out of a listing and different for the row whose flags the user changed a
  moment before the refresh arrived. That listing overwrites the user's flags —
  a race this provider still does not resolve, and the next thing worth fixing
  here — but clearing the bit as well would take the row off the work list too,
  which turns a race into a change lost in silence rather than one retried.
  `a_listing_does_not_take_an_unsaved_change_off_the_work_list` pins that.
- **An empty change costs no connection, not merely no request.** The
  short-circuit for an empty `KeywordChange` already existed in
  `MailSync::set_keywords`, but underneath the store's connection check — so a
  folder full of unchanged mail failed to synchronise when offline. The store is
  now looked for only once there is something to say to it. Camel synchronises a
  folder every time it closes one, so this is the common path, not the edge.
- **The row's *after* is read back out of Camel's own two columns.** The before
  is the column `CamelJmapMessageInfo` keeps; the after is `flags_word` and
  `set_user_flags` run backwards, over the same bits and the same names, so a row
  nobody touched produces the set it was built from and therefore no change at
  all. `message_flags` is written as one loop over the same pairs `flags_word`
  uses rather than as a second list of them: a bit named in one and not the other
  would be a flag written to the server and never read back.
- **One row's failure does not stop the rest.** Every queued row is attempted and
  the first failure is what the vfunc reports. Stopping at the first would be
  cheaper on a dead network and wrong on a live one — a keyword one server
  refuses says nothing about the next message, and every row behind the refusal
  would stay queued behind a write that can never succeed. The cost, named in the
  module: a connection that has just gone away is discovered once per queued row.
- **A message another client destroyed is settled, not failed.** `NoSuchMessage`
  clears the bit and does not fail the synchronisation: a uid in a summary is a
  claim about the last listing, so the flag change is moot rather than refused.
  Reported, it would put an alert in front of the user about a message that is
  not there; left queued, it would retry a write that can never succeed. The row
  goes at the next refresh, which is where a message leaving a mailbox is noticed.
- **A row dirty for something that is not a keyword still leaves the list.**
  `CAMEL_MESSAGE_DELETED` is a local mark JMAP has no keyword for, so such a row
  produces an empty change — and the bit is cleared anyway, because a bit nothing
  can clear is a row retried on every synchronisation forever. Nothing is lost by
  it: `expunge_sync` will read the `DELETED` flag, which is untouched, not this
  bit.
- **`expunge` is ignored, and said so.** Camel's argument asks the folder to get
  rid of the messages marked deleted. In JMAP that is a mailbox change — taking
  the message out of its mailboxes, or destroying it, depending on what the
  account calls its trash — so it belongs with the increment that implements
  `expunge_sync`, not with a flag write. Documented in the module rather than
  quietly accepted.
- **The summary lock is not held across the request.** A synchronisation is one
  round trip per changed row, and a folder locked for the length of that is a
  message list that cannot be drawn while the user's last click is being saved;
  `camel_folder_summary_get` takes the lock itself for as long as it needs it. A
  refresh running alongside can renew the row's remembered set from a fresh
  listing, which the write then overwrites with what it established — the more
  recent of the two answers.

Not verified locally, as in the previous forty-three sessions: `reuse lint` and
`cargo deny` (neither binary is installed on this VM). The two new files —
`jmap-mail/src/synchronize.rs` and `jmap-mail/tests/synchronize.rs` — carry SPDX
`GPL-3.0-or-later` headers. `cargo fmt --check`, `cargo test --locked` and
`cargo clippy --all-targets --locked -- -D warnings` are clean on the default
member set and on the five EDS crates; `jmap-mail` is at 186 tests. One new
allowlist entry in `eds-sys`: `camel_folder_synchronize_sync`, so a test can call
the vfunc through Camel's own wrapper the way Evolution does.

Next in M5. **The refresh/write race** is the sharpest thing this increment
exposed and did not fix: `apply_listing` overwrites a row's flags with the
server's even when the user's own change to that row has not been sent yet. The
change is not lost — the row stays queued — but the flags it will be diffed from
are the server's, so the user's click is undone on screen and then never sent.
IMAPX solves it by not applying server flags to a row that is folder-flagged; the
same rule fits here and is a small, self-contained increment with an obvious red
test. **Bounding the cache** is still open from the forty-first session.
`CamelSubscribable` remains the smaller unblocked piece; `get_trash_folder_sync`
and `get_junk_folder_sync` are still a settings decision before they are a vfunc,
and that decision is now also what `expunge_sync` waits on. `Email/changes`
against a state kept on disk is what the empty `recent` list and the whole-mailbox
refresh are both still waiting for. Unexercised against a real `CamelSession`:
`service.rs`, which waits on M6 and M7. The README's architecture block still
lists only the round-1 crates.

## 2026-08-09 (forty-fifth session)

M5's twenty-fifth increment, and the fix for the race the previous one exposed:
a refresh arriving between the user's click and the synchronisation no longer
undoes the click. `update_message_info` now writes the listing with the row's
*outstanding* change replayed on top of it, instead of writing the listing whole.

Red first: two tests in `jmap-mail`'s `tests/synchronize.rs` against `jmap-mockd`
through a real `CamelStore` (both verified failing before the change and passing
after), three in `tests/message_info.rs` at the column level, and five in
`jmap-mail-sync`'s `tests/keywords.rs` for the two set operations the replay
rests on.

Decisions taken:

- **Replay, not refuse.** IMAPX's rule is often described as "do not apply server
  flags to a folder-flagged row", but refusing the listing outright would hide
  what another client did for as long as the row stayed queued — and a row whose
  write keeps failing stays queued forever. What IMAPX actually applies is the
  *difference* the server made, and the same idea written in this provider's
  terms is: the row becomes the listing patched by the change it is still waiting
  to send. `a_refresh_leaves_a_queued_row_carrying_both_changes` is the half that
  a refusal would fail.
- **The change replayed is exactly the change that will be sent.** Both are
  `KeywordChange::between(remembered, claimed)` over the row's own two columns, so
  there is one definition of "what this row still owes the server" and not two
  that could drift apart. `Keywords::patched` is the only new operation it needed.
- **The remembered set is renewed to the listing, never to what the row ends up
  claiming.** That is what keeps the next difference honest, and it is also what
  makes a change the server has meanwhile made *itself* settle: replayed onto a
  listing that already carries it, it changes nothing, the diff against the
  listing is empty, and the row leaves the work list instead of writing the same
  keyword on every synchronisation.
  `a_change_the_server_already_made_itself_settles_the_row` pins that; it passes
  under the old code too, and is here because it is what a naive fix breaks.
- **The dirty bit decides, not the two sets.** A row Camel does not hold as
  needing to reach the server takes the listing whole — the ordinary path, and
  the only thing that ever brings a row that has drifted back into line. A row
  whose sets differ with the bit clear is one nobody is waiting to send, and
  self-healing is worth more there than preserving a difference no one claims.
- **`hasAttachment` is taken from the listing on both paths.** It is the one bit
  of Camel's flags word that is not a keyword — RFC 8621 §4.1.1 has the server
  compute it — so `Keywords` cannot carry it and the replay cannot speak for it.
  Its own test, because losing it would have been silent.
- **`Keywords::split` is `Keywords::new` run backwards, in the sync crate.** The
  merged set has to become Camel's two columns again, and doing that in
  `jmap-mail` would have been a second copy of the keyword-to-flag table living a
  crate away from the first. Matched folded, so a server that shouts `$Seen`
  still marks the message read rather than labelling it.

Not verified locally, as in the previous forty-four sessions: `reuse lint` and
`cargo deny` (neither binary is installed on this VM). No new files, so no new
SPDX headers. `cargo fmt --check`, `cargo test --locked` and `cargo clippy
--all-targets --locked -- -D warnings` are clean on the default member set and on
the five EDS crates; `jmap-mail` is at 194 tests, `jmap-mail-sync` at 21 in
`tests/keywords.rs`. (`example-module`'s lib test still fails to link on this VM,
as before; it is not in either set.)

Next in M5. **Bounding the cache** is still open from the forty-first session.
`CamelSubscribable` remains the smaller unblocked piece; `get_trash_folder_sync`
and `get_junk_folder_sync` are still a settings decision before they are a vfunc,
and that decision is also what `expunge_sync` waits on. `Email/changes` against a
state kept on disk is what the empty `recent` list and the whole-mailbox refresh
are both still waiting for. One thing this increment does *not* settle: two
refreshes racing each other, or a refresh racing the write it replays around —
the summary lock is taken per row rather than across the walk, so the last writer
wins on a given row. That is the same shape IMAPX lives with and it is not worth
a lock across a round trip; it is written down here so it is a known cost rather
than an oversight. Unexercised against a real `CamelSession`: `service.rs`, which
waits on M6 and M7. The README's architecture block still lists only the round-1
crates.

## 2026-08-09 (forty-sixth session)

M5's twenty-sixth increment, and the one the cache module has been pointing at
since it was written: a cached message is now checked against the size its
summary row carries, so a truncated entry is fetched again instead of being
served as the message.

Red first: one test in `jmap-mail`'s `tests/message.rs` — the whole path, through
a real `CamelStore` against `jmap-mockd` — which failed with the body decoding to
`"Two lines, so the body is more than a header value"`, and six at the wrapper
level in `tests/cache.rs`. The three that assert the new rule were each verified
failing with the check disabled; the other three are the guards against the rule
being too strict.

Decisions taken:

- **MIME has no length, so the file has to be measured against something.** An
  entry is written by one `write_all` and closed, and a process killed between
  the two leaves a short file. Camel's parser reads one as a *complete* message
  with a truncated body and says nothing — so the symptom is a message that
  silently opens wrong every time it is opened, in preference to the download
  that would have been right. The number that closes it is the one the row
  already carries: RFC 8621 §4.1's `size`, defined as the octets of exactly the
  bytes the `blobId` references.
- **Shorter, not different.** Truncation produces a short file and nothing else,
  so `<` is the whole of the fault. An exact comparison would additionally catch
  a server whose `size` is a byte out in the other direction — by making every
  message it holds one that can never be cached, two round trips per open,
  forever. A mail client that tolerates an over-long entry is more usable than
  one that re-downloads everything a slightly wrong server serves.
- **Zero is not a claim.** It is what Camel's counter holds for a row that was
  never given a size — an `Email` that arrived without one, or a row read back
  from a summary database written before the column existed. Read as a claim it
  would be the claim every entry satisfies, which is harmless; `claimed()` names
  it so that the reason it is harmless is not the reason it is being relied on.
- **The check is at both ends.** `store` declines bytes it can see `load` would
  refuse, on the reasoning that already refuses an empty entry: an entry the
  cache will not serve is a syscall spent on producing a miss. What disagrees
  there is the server with itself rather than a file with a crash, so it gets a
  log line of its own — the visible symptom is a message downloaded again at
  every open.
- **A refused entry is dropped, not merely refused.** Nothing will ever serve
  it, the cache still has no bound of its own, and leaving it means the same
  critical at every open of that message. A fetch that succeeds writes it again;
  one that does not leaves the cache where it should be — empty of that message.
- **The size is read from the summary, not carried into the vfunc.** What
  Evolution clicks is a line of the message list, and the row behind that line is
  where everything already known about the message lives. A uid with no row —
  one a caller invented, or a folder not yet refreshed — claims nothing and is
  cached unchecked, exactly as before there was a check; the fetch is what
  decides whether such a uid means anything.
- **The mock was reporting the wrong `size`, and that is a finding rather than a
  test-fixture detail.** `seed_email` set `size` to the length of the *body*.
  RFC 8621 §4.1 defines it as "the number of octets in the file the user would
  download" — the raw data behind `blobId`, headers included. A client is
  entitled to check a download against it, so a mock that reported the body's
  length is one that teaches a client the check is useless. Fixed to the rendered
  message's length; no existing test depended on the old value.

Not verified locally, as in the previous forty-five sessions: `reuse lint` and
`cargo deny` (neither binary is installed on this VM). No new files, so no new
SPDX headers. `cargo fmt --check`, `cargo test --locked` and `cargo clippy
--all-targets --locked -- -D warnings` are clean on the default member set and on
the five EDS crates; `jmap-mail` is at 201 tests. (`example-module`'s lib test
still fails to link on this VM, as before; it is not in either set.)

Next in M5. **Bounding the cache** — expiry by age versus Evolution's own "empty
cache" — is still open from the forty-first session, and is now the only thing
`crate::cache` lists as missing. The other half of the atomicity problem is also
still open and is smaller than it looks: an entry is written by `write_all` and
close rather than by a write to a temporary name and a rename, so the window this
increment *detects* is one a rename would close outright. `CamelSubscribable`
remains the smaller unblocked piece; `get_trash_folder_sync` and
`get_junk_folder_sync` are still a settings decision before they are a vfunc, and
that decision is also what `expunge_sync` waits on. `Email/changes` against a
state kept on disk is what the empty `recent` list and the whole-mailbox refresh
are both still waiting for. Unexercised against a real `CamelSession`:
`service.rs`, which waits on M6 and M7. The README's architecture block still
lists only the round-1 crates.

## 2026-08-09 (forty-seventh session)

M5's twenty-seventh increment, at the sync layer: the `Email/set` a message
moved or copied into another folder becomes. `jmap-mail-sync` grows a `Filing`
and `MailSync::file_message`, which is what Camel's `transfer_messages_to_sync`
will spend when the folder side of it lands.

Red first: ten tests in a new `tests/mailboxes.rs`, four over the patch itself
and six through `jmap-mockd`. All ten failed to compile against the old crate;
the two that assert the server's own refusals were additionally verified failing
against the *new* client with the mock's new rule taken back out, which is where
they would have passed vacuously.

Decisions taken:

- **A copy and a move are the same request.** RFC 8621 has no `Email/copy` and
  no `Email/move`, because a JMAP mailbox is closer to a label than to a
  directory: §4.6 makes `mailboxIds` the set of mailboxes a message is in, so a
  copy adds a member and a move adds one and removes another. The message is one
  object either way, which is also why the cache keyed on the uid alone (the
  forty-second session) needs nothing doing to it here.
- **A move is one patch, not two requests.** RFC 8621 §4.6 spends a sentence on
  it: an `Email` in the mail store belongs to one or more `Mailbox`es. Removing
  the source first is therefore a request no server may accept, and adding the
  destination first leaves the message filed in both if the second request never
  happens — a copy the user did not ask for that nothing afterwards knows to
  clean up. One `Email/set` update is applied as one change, and RFC 8620 §5.3
  defines a `PatchObject` by its *result* rather than as a sequence, so there is
  no intermediate state with no mailbox in it for a server to refuse.
- **A move into the mailbox the message is already in is not a request.** It is
  the one filing that cannot be written down: the same pointer would have to be
  both `true` and `null`. Whichever won, the answer would be wrong, so `Filing`
  reports it empty and `file_message` sends nothing — the same shape
  `set_keywords` has for a change that changes nothing.
- **The mailbox a message came *from* is the caller's claim, not a question.** A
  Camel folder knows its own mailbox id, and confirming it would be a round trip
  spent re-reading what the summary the user clicked in already said. A `null`
  for a member that is not there removes nothing and is not an error, so a stale
  claim costs the message nothing.
- **A destination the account does not have is not `NoSuchMessage`.** The
  message is fine; the folder is the thing that is gone, which is what a folder
  Camel still shows after another client deleted it looks like. It stays the
  server's own `SyncError::Client` so that the user is told, while a uid the
  account no longer holds keeps the "ordinary, not a failure" judgement every
  other write here makes.
- **RFC 6901 escaping moved into `crate::pointer`.** `keywords/` and
  `mailboxIds/` are both JSON Pointers into a map whose keys came off the
  network, and a second copy of the escaping that fell behind would be a hole
  rather than a duplication. A mailbox id *cannot* contain `/` or `~` — RFC 8620
  §1.2 — but the id in hand is the server's word for that, and unescaped it
  would let a server choose which property of an `Email` this client patches.
  Its own test.
- **The mock was letting a message end up in no mailbox, and in mailboxes that do
  not exist — a finding, like the `size` one before it.** It checked
  `mailboxIds` only on creation and only for emptiness. So the two-request move
  this increment exists to avoid would have *worked* against it, and a client
  that botched a move would have been taught it was fine. `filed_somewhere` now
  holds the invariant over creation, over the result of an update, and over
  `onSuccessUpdateEmail`, refusing both halves with `invalidProperties`. No
  existing test depended on the old laxity.

Not verified locally, as in the previous forty-six sessions: `reuse lint` and
`cargo deny` (neither binary is installed on this VM). Two new files, both with
the SPDX GPL-3.0-or-later header. `cargo fmt --check`, `cargo test --locked` and
`cargo clippy --all-targets --locked -- -D warnings` are clean on the default
member set and on the five EDS crates; `jmap-mail-sync` is at 31 tests in
`tests/keywords.rs` and `tests/mailboxes.rs` together. (`example-module`'s lib
test still fails to link on this VM, as before; it is not in either set.)

Next in M5. **The folder half of this** — `transfer_messages_to_sync` on
`CamelFolderClass`, and with it what a move does to the source folder's summary
before the next refresh confirms it — is the increment that makes any of the
above reach Evolution; nothing the user does moves a message yet. Note that
`file_message` is one request per message where Camel hands its vfunc a list of
uids: one `Email/set` may carry many updates but applies them as one state
change, so a partly-failed transfer would have no way to say which messages
moved, and a request per message is the shape the caller has to report in
anyway. **Bounding the cache** is still open from the forty-first session, as is
the other half of the atomicity problem — an entry is written by `write_all` and
close rather than to a temporary name and renamed, and a rename would close the
window the forty-sixth session's size check only *detects*. `CamelSubscribable`
still wants `Mailbox/set`, which the client does not have yet.
`get_trash_folder_sync` and `get_junk_folder_sync` are still a settings decision
before they are a vfunc, and that decision is also what `expunge_sync` waits on.
`Email/changes` against a state kept on disk is what the empty `recent` list and
the whole-mailbox refresh are both still waiting for. Unexercised against a real
`CamelSession`: `service.rs`, which waits on M6 and M7. The README's
architecture block still lists only the round-1 crates.

## 2026-08-09 (forty-eighth session)

M5's twenty-eighth increment, and the folder half of the one before it:
`transfer_messages_to_sync` on `CamelFolderClass`. Dragging a message into
another folder now reaches the server, and the folder it left stops showing it.

Red first: ten tests in a new `jmap-mail/tests/transfer.rs`, eight of them
through Camel's own `camel_folder_transfer_messages_to_sync` wrapper and two
through the class pointer, for the two cases that wrapper settles before any
provider is asked. Eight failed against the old class — the two that passed did
so through `CamelFolder`'s generic implementation, which is what this overrides.
The flag-settling test was additionally verified failing with only that one call
taken back out, which is where it would otherwise have passed for free.

Decisions taken:

- **A move settles the row before it takes it away.** Camel keeps a change the
  user has made and not yet saved on the summary row, marked
  `CAMEL_MESSAGE_FOLDER_FLAGGED`, and `synchronize_sync` is the only thing that
  writes it. Removing the row would therefore drop it in silence — marking a
  message read and dragging it into another folder before anything synchronised
  is an ordinary sequence, and the destination would never learn of it either,
  because what a folder lists is what the server holds. So the move calls
  `crate::synchronize`'s own `push_row` first, which costs nothing at all for a
  row nobody changed: the diff is empty and no request is made.
- **The rows go now, not at the next listing.** A refresh would reach the same
  answer — a message that left the mailbox is one the next `Email/query` does not
  name — but "the next refresh" is a timer, and until it fires the message list
  would still be showing what the user just moved out of it.
- **A request per message, not one per transfer.** One `Email/set` could carry
  every selected message, and would be applied as one state change: a transfer
  that half succeeded would then be a single failure with no way to say which
  messages moved. Camel needs an answer per message — a row that landed must
  leave the folder and a row that did not must stay — so the walk is per uid and
  the first failure is reported once all of them have been tried, exactly as
  `synchronize_sync` does it.
- **A message another client deleted is reported here, unlike in a flag write.**
  `synchronize_sync` settles that case in silence because the write is a
  consequence the user never asked for. A transfer is something they did, so it
  becomes `CAMEL_FOLDER_ERROR_INVALID_UID` — what Evolution reads as "that
  message is gone" rather than as a reason to take the account offline. The row
  still goes: the message is not in this folder either.
- **The transferred uids are answered rather than left NULL.** IMAPX ignores that
  out-parameter, because its server mints a new uid in the destination and the
  copy command does not say what it is. JMAP has the opposite problem and no
  problem: RFC 8621 §4.1 gives an `Email` one immutable id per account, and
  filing it into another mailbox does not make a second object, so the answer is
  the uid the caller passed in. Allocated and filled the way Camel's own generic
  transfer does it — array sized up front, `NULL` for a message that did not
  land, every string one `g_free` releases — which is verifiably the convention
  its callers free by: the wrapper's own vee-folder path frees a nested call's
  array exactly that way.
- **`filing.is_empty()` guards the rows, not just the request.** Camel answers a
  transfer into the same `CamelFolder` itself, but two folder *objects* of one
  mailbox are a pair it cannot recognise, and a move that went nowhere must not
  take the rows away. Checked before anything, so it needs no connection either.
- **The destination is type-checked, the source is not.** GObject dispatched the
  call on the source's class; the destination is whatever the caller passed, and
  the wrapper deliberately picks the *destination's* class when it is a vtrash
  folder — so a folder of someone else's arriving here is a case Camel allows
  for. Reading a `JmapFolder` out of one would be undefined behaviour rather than
  a wrong answer.

Cross-store transfers are still Camel's generic `get_message` +
`append_message` path, which this provider has no `append_message_sync` for; a
drag from an IMAP account into a JMAP one therefore still fails, and fails in
Camel rather than here. It wants `Email/import`, which the client does not have.

Not verified locally, as in the previous forty-seven sessions: `reuse lint` and
`cargo deny` (neither binary is installed on this VM). Two new files, both with
the SPDX GPL-3.0-or-later header. `cargo fmt --check`, `cargo test --locked` and
`cargo clippy --all-targets --locked -- -D warnings` are clean on the default
member set and on the five EDS crates; `jmap-mail` is at 211 tests.
(`example-module`'s lib test still fails to link on this VM, as before; it is not
in either set.)

Next in M5. The `changed` signal a move emits is *not* asserted by this
increment's tests — the source folder's summary is, which is what the next
message list is drawn from, but the emission that updates one already on screen
is only the same one-line call `refresh_info_sync` makes. The harness for
watching it lives inside `tests/refresh.rs` (a main context per test, a
thread-local of emissions); lifting it into `tests/common` and asserting the
removal reaches a listener is a small increment of its own, and would pay for
itself in every folder vfunc after this. **Bounding the cache** is still open
from the forty-first session, as is the other half of the atomicity problem — an
entry is written by `write_all` and close rather than to a temporary name and
renamed. `CamelSubscribable` still wants `Mailbox/set`, which the client does not
have. `get_trash_folder_sync` and `get_junk_folder_sync` are still a settings
decision before they are a vfunc, and that decision is what `expunge_sync` waits
on — note that deleting mail is now one `Filing` away, since a trash folder is a
mailbox and moving into it is what this increment does. `Email/changes` against a
state kept on disk is what the empty `recent` list and the whole-mailbox refresh
are both still waiting for. Unexercised against a real `CamelSession`:
`service.rs`, which waits on M6 and M7. The README's architecture block still
lists only the round-1 crates.

## 2026-08-09 (forty-ninth session)

M5's twenty-ninth increment, in `jmap-mail-sync` and the two crates under it: a
mailbox can now be asked what *changed* rather than what it holds.
`MailSync::messages_since` turns one `Email/changes` into an answer a folder can
apply, and `MailSync::messages` finally comes back with the state such a question
is asked from.

Red first: twelve tests in a new `jmap-mail-sync/tests/updates.rs`, none of which
compiled against the old API. Once the API existed, two of the decisions below
were re-checked by breaking them on purpose — membership inferred instead of
re-read, and the ordering taken out — which failed three of the twelve.

Decisions taken:

- **The delta is present/absent, not created/updated/destroyed.**
  `Email/changes` reports on the account's *messages*; a folder is asking about
  one mailbox. JMAP files a message by changing its `mailboxIds`, which is an
  ordinary update to the message, so a delta naming one says only that something
  about it changed — never whether that something moved it into or out of the
  mailbox being refreshed. Every named message is therefore looked up with
  `mailboxIds` among its properties and sorted into the rows this mailbox holds
  and the uids it does not. A message moved *in* is not `created` and a message
  moved *out* is not `destroyed`, and a provider that believed either word would
  show mail that is not there and hide mail that is.
- **`destroyed` is the one word taken at face value.** A message that is gone is
  gone from every mailbox, and there is nothing left to look up. So is an id the
  delta named and `Email/get` did not answer for: it was destroyed between the
  two calls, and unlike a listing — which simply drops such an id — a delta has
  to report it, because the folder may well be holding a row for it.
- **The caller diffs, because the caller is the only side that knows.** A
  message moved into this mailbox and one whose flags changed while it sat here
  are the same delta on the wire; which of the two it is depends on whether
  there is already a row, and the rows are Camel's. So `present` carries whole
  summary rows rather than uids — a row that arrives by a delta has to be
  listable without a second fetch — and the folder decides add-or-update.
- **The state is read *before* the listing, at the cost of a round trip.** The
  `Email/get`s a listing makes carry a state of their own and using it would be
  free, but it is the state *after* the listing was taken: a message that
  arrived between the `Email/query` and the fetch is then one the query never
  named and no later delta will ever mention, because it changed before the
  state the delta is asked from. It would be missing until something forced a
  full listing again. Reading first has the opposite failure, which is not one —
  the next delta re-reports what the listing already has, and each such message
  is a row rewritten with what it already said. `Client::email_state` is that
  probe: `Email/get` naming no ids, which RFC 8620 §5.1 answers with the type's
  state and an empty list.
- **`mailboxIds` stays out of `SUMMARY_PROPERTIES`.** A listing already knows
  the answer — it asked `Email/query` for one mailbox — and neither
  `MessageSummary` nor a `CamelFolderSummary` row has anywhere to keep it. It is
  a question only a delta has, and only for as long as it takes to sort a
  message into one of two lists.
- **A state the server cannot calculate from lists the mailbox again**
  (`MessageUpdate::Relisted`), the judgement `folder_tree_since` already makes
  about the same condition: Camel has nowhere to report it to, so a folder that
  failed here would be one that never recovers.
- **Delta rows are sorted like listing rows** — oldest first by `receivedAt`,
  by uid where a server gave two messages the same time or none — because they
  are appended to the same summary and Camel numbers messages in the order they
  are added. Unsorted they would arrive in whatever order a `BTreeSet` of ids
  puts them, which is not a mail order at all.
- **The mock gained `deliver_email` and `destroy_email`**, the mail counterparts
  of `create_mailbox`/`destroy_mailbox`: `seed_email` deliberately does not bump
  state, so a seeded message predates every state a test asks from and can never
  appear in a `/changes` answer. A test that wants mail to *arrive* has to say
  so. `seed_email` and `deliver_email` now build the same message through one
  private helper and differ only in how it enters the store.

`JmapStore::messages` and `refresh_info_sync` were carried along to the new
signature; the vfunc drops the state with a comment naming what it is for. It is
*not* yet used for anything: keeping it means keeping it across a restart, which
is the summary's own on-disk header and the next increment. Nothing in Camel
calls `messages_since` yet, so this session moved no user-visible behaviour — it
built the half of it that can be tested without Camel, which is the half the
whole-mailbox refresh and the empty `recent` list were both waiting on.

Not verified locally, as in the previous forty-eight sessions: `reuse lint` and
`cargo deny` (neither binary is installed on this VM). One new file, with the
SPDX GPL-3.0-or-later header. `cargo fmt --check`, `cargo test --locked` and
`cargo clippy --all-targets --locked -- -D warnings` are clean on the default
member set and on the five EDS crates; `jmap-mail-sync` is at 115 tests, twelve
of them the new `tests/updates.rs`. (`example-module`'s lib test still fails to
link on this VM, as before; it is not in either set.)

Next in M5. **Keeping the state** is what turns this session's work into a
refresh that costs one round trip: `CamelFolderSummary` has an on-disk header a
provider may extend (`summary_header_load`/`summary_header_save` on
`CamelFolderSummaryClass`), and that is where a mailbox's `Email` state belongs.
With it, `refresh_info_sync` becomes messages_since-then-apply, and the `recent`
list `crate::changes` leaves empty can finally be filled — a delta knows which
rows are new, which a full listing cannot tell without keeping the previous one.
Note the summary's `apply_listing` reconciles a *whole* listing and a delta is
not one, so the folder side needs its own application path rather than a reuse of
that one. Still open from earlier sessions: **bounding the cache**; the other
half of the cache's atomicity problem (an entry is written by `write_all` and
close rather than to a temporary name and renamed); the `changed` signal a
transfer emits is still not asserted by a test, and lifting `tests/refresh.rs`'s
emission harness into `tests/common` is what that wants; `CamelSubscribable`
still wants `Mailbox/set`, which the client does not have; `get_trash_folder_sync`
and `get_junk_folder_sync` are still a settings decision before they are a vfunc,
and that decision is what `expunge_sync` waits on; cross-store transfers want
`Email/import` for an `append_message_sync`. Unexercised against a real
`CamelSession`: `service.rs`, which waits on M6 and M7. The README's architecture
block still lists only the round-1 crates.

## 2026-08-09 (fiftieth session)

M5's thirtieth increment, in `jmap-mail`: a folder now remembers the `Email`
state its last listing was taken at, and remembers it across a restart. That is
the one piece the previous session's `messages_since` was waiting on — a delta
is only cheaper than a listing if there is a state to ask it from, and a state
that lives in memory is gone exactly when it would have paid for itself, on the
first refresh of a session.

Red first: six tests — four in `tests/summary.rs`, two in `tests/refresh.rs` —
and each was checked by breaking the implementation on purpose afterwards. Not
installing the two vfuncs fails three, dropping the format-number check fails
one, taking out the `camel_folder_summary_touch` fails the round trip, and
dropping `set_summary_state` from the vfunc fails both refresh tests.

Decisions taken:

- **The state belongs to the summary, not to the folder.** It is a fact about
  the *rows* — it says what they are current as of — so it has to be stored
  where they are stored and read back when they are read back. Camel keeps one
  header record per folder beside the message rows and reserves a `bdata` field
  in it for what a provider has and Camel has not, which is the same
  arrangement `CamelMIRecord.bdata` already carries the keywords in.
- **A header's `bdata` is not a chain; a row's is.** `save`/`load` on a message
  info are handed a cursor the whole class chain walks in order, so
  `crate::message_info` appends to it. `summary_header_load` is handed the
  record and nothing else — there is no cursor to share — so the field belongs
  to the last class in the chain and this one writes it whole. Whatever was in
  it is still freed first, so that a base class which started using it would
  not leak once per save.
- **A format number in front of the state.** The case it is for is not a
  restart but a downgrade: a header written by a later version of this provider
  must not be read as a state by this one. A number this version does not know
  leaves the summary with no state, which costs one full listing — what a
  folder does today anyway — where misreading the field would cost the mailbox.
  A record with no `bdata` at all is the same answer, and is what every summary
  written before this increment is.
- **Setting the state touches the summary, and only when it changed.** Camel
  skips saving a summary it was not told had changed, and a refresh that found
  no new mail changes nothing else — so without the touch the state would never
  reach the disk. With an unconditional touch, every poll of every folder would
  rewrite the database for a value that did not move.
- **The state is recorded after the rows are applied, not before.** The two are
  not one transaction: a process that died in between having claimed the state
  first would come back holding the older rows and asking for changes *since*
  the newer state, and never hear about the ones in between. The other order
  re-reports what the rows already have, which is a rewrite of what they
  already said.
- **`guard_summary`, the summary's counterpart to `message_info`'s
  `guard_row`.** A vfunc body reaching the instance struct needs the type check
  in front of it, and the panic guard is the rule every `extern "C"` here
  follows.
- **`CamelFIRecord` named in `eds-sys`'s allowlist** beside `CamelMIRecord`.
  bindgen already emitted the struct as a field type; naming it brings the
  typedef, which is what the tests and the vfunc signatures read as.

`refresh_info_sync` now records what it used to drop, and its "what is not here
yet" section says what remains: `apply_listing` reconciles a *whole* listing, so
a delta handed to it would remove every row it did not mention. The folder side
needs an application path of its own, and that is the next increment — with it,
`refresh_info_sync` becomes messages_since-then-apply and the `recent` list
`crate::changes` leaves empty can finally be filled.

One test in this session is weaker than the others and is worth naming: the
`bdata`-is-NULL case cannot be made red by removing the NULL check, because
Camel's own `camel_util_bdata_get_number` tolerates a NULL cursor. It is red for
the *installation* of the vfunc — `header_load` in the test asserts the vfunc is
this provider's and not the inherited one, which is what tells "the record was
read and had nothing in it" from "nobody read the record" — and the check itself
stays as a guard rather than as tested behaviour.

Not verified locally, as in the previous forty-nine sessions: `reuse lint` and
`cargo deny` (neither binary is installed on this VM). No new files, so no new
SPDX headers. `cargo fmt --check`, `cargo test --locked` and `cargo clippy
--all-targets --locked -- -D warnings` are clean on the default member set and
on the five EDS crates; `jmap-mail`'s summary suite is at 25 tests and its
refresh suite at 6.

Still open from earlier sessions: **bounding the cache**; the other half of the
cache's atomicity problem (an entry is written by `write_all` and close rather
than to a temporary name and renamed); the `changed` signal a transfer emits is
still not asserted by a test, and lifting `tests/refresh.rs`'s emission harness
into `tests/common` is what that wants; `CamelSubscribable` still wants
`Mailbox/set`, which the client does not have; `get_trash_folder_sync` and
`get_junk_folder_sync` are still a settings decision before they are a vfunc,
and that decision is what `expunge_sync` waits on; cross-store transfers want
`Email/import` for an `append_message_sync`. Unexercised against a real
`CamelSession`: `service.rs`, which waits on M6 and M7. The README's
architecture block still lists only the round-1 crates.

## 2026-08-09 (fifty-first session)

M5's thirty-first increment, in `jmap-mail`: `apply_delta`, the path that puts
an `Email/changes` answer into a folder's summary — and with it Camel's fourth
list, `recent`, which every increment so far has deliberately left empty.

The whole point is what *silence* means. `apply_listing` reconciles a listing,
so a uid it does not name has left the mailbox; `Email/changes` answers for the
account, so a uid a delta does not name is one nothing was said about. Handing a
delta to `apply_listing` would empty a folder on the first refresh that found one
new message, which is why this is a second function and not an argument to that
one.

Red first: seven tests in `tests/summary.rs`, and each was checked afterwards by
breaking the implementation on purpose. Six mutations, every one caught by the
test written for it: dropping the `arrive` call, having `apply_message` call
every row new, dropping `remove_row`'s `check_uid` guard, ignoring `absent`,
reconciling the delta as if it were a listing, and leaving the pending-write bit
on an added row.

Decisions taken:

- **A delta may say a message is recent; a listing may not.** Camel's recent
  list is what runs the user's incoming filters, so putting a uid on it asks for
  that message to be filed, forwarded or deleted by the user's rules. A listing
  finds the whole mailbox and cannot tell an arrival from a message that was
  always there — its "added" is everything the user already had. A delta is
  asked from a state the folder itself recorded at its last refresh, so a
  message it names that the folder has no row for reached the mailbox since
  then. That is exactly what recent means, and it is the one honest answer this
  provider has ever had to the question.
- **Recent is a second call, not a replacement for added.** Checked against
  Camel rather than assumed: `camel_folder_change_info_recent_uid` appends to
  `uid_recent` and does *not* imply `_add_uid`, so an arrival recorded only as
  recent would be filtered and never drawn. `Changes::arrive` therefore sits
  beside `Changes::add` and both are called.
- **An absent uid this folder never held is not reported.** Most of what a delta
  calls absent was never here — the delta is account-wide, so a message the user
  filed in some other folder is on this folder's absent list too. A removal
  announced for a uid the message list never drew asks Camel to change nothing,
  and worse, makes a delta about somewhere else count as a change here, which is
  the test the refresh vfunc emits `changed` on. So `remove_row` checks for the
  row first and reports only what it really removed.
- **`apply_message` reports whether the row is new; it does not decide what that
  means.** The alternative was a boolean parameter telling it whether to mark
  arrivals, which would have put the listing-versus-delta judgement in the one
  function the two paths share. Returning the fact and letting `apply_delta` act
  on it keeps `apply_listing` unchanged — it ignores the return, which is the
  honest thing for a caller that cannot answer the question.
- **Absent is applied before present.** `messages_since` cannot produce a uid on
  both lists, so the order is not load-bearing for correctness; it matches
  `apply_listing`'s removals-then-rows so that the two read the same way.

Not verified locally, as in the previous fifty sessions: `reuse lint` and
`cargo deny` (neither binary is installed on this VM). No new files, so no new
SPDX headers. `cargo fmt --check`, `cargo test --locked` and `cargo clippy
--all-targets --locked -- -D warnings` are clean on the default member set and
on the five EDS crates; `jmap-mail`'s summary suite is at 32 tests.

Next in M5. **The join**, and it is now the only piece left of a cheap refresh:
every part exists — `messages_since` asks the question, the summary keeps the
state to ask it from across a restart, and `apply_delta` applies the answer —
and `refresh_info_sync` still asks `JmapStore::messages` for the whole mailbox
every time. That vfunc becomes: read the summary's state, ask for a delta when
there is one, and dispatch on `MessageUpdate`'s three answers — `Unchanged` to
nothing but a new state, `Changed` to `apply_delta`, `Relisted` to
`apply_listing`. The store needs a `messages_since` of its own beside
`messages`, and `refresh.rs`'s "what is not here yet" section names exactly this.

Still open from earlier sessions: **bounding the cache**; the other half of the
cache's atomicity problem (an entry is written by `write_all` and close rather
than to a temporary name and renamed); the `changed` signal a transfer emits is
still not asserted by a test, and lifting `tests/refresh.rs`'s emission harness
into `tests/common` is what that wants; `CamelSubscribable` still wants
`Mailbox/set`, which the client does not have; `get_trash_folder_sync` and
`get_junk_folder_sync` are still a settings decision before they are a vfunc,
and that decision is what `expunge_sync` waits on; cross-store transfers want
`Email/import` for an `append_message_sync`. Unexercised against a real
`CamelSession`: `service.rs`, which waits on M6 and M7. The README's
architecture block still lists only the round-1 crates.

## 2026-08-09 (fifty-second session)

M5's thirty-second increment, in `jmap-mail`: **the join**. `refresh_info_sync`
now asks what *changed* since the state its summary recorded, and dispatches on
`MessageUpdate`'s three answers instead of listing the whole mailbox every time.
`JmapStore::messages_since` is the new call beside `messages`, locked the same
way; `messages` stays, because a folder that has never listed has no "since" to
ask from and a refused delta has to fall back to something.

Every piece of this existed already — `messages_since` asks, the summary keeps
the state across a restart, `apply_delta` applies the answer — and the vfunc was
the one place they were not wired together. With it, Camel's fourth list is
filled for the first time: a delta names a message this folder has no row for,
which means it arrived since the state the folder recorded, which is what
`recent` means.

Red first, and the first test needed something the mock did not have. A listing
and a delta leave the folder holding exactly the same rows — the entire
difference between them is what went over the wire — so **the mock now records
the name of every method call it answers** (`ServerState::method_calls`,
`MockServer::method_calls()`), and the test asserts the second refresh asked
`Email/changes` and did *not* ask `Email/query`. Cheapness that is not asserted
is cheapness that quietly goes away. Two tests were red against the old vfunc
for the right reasons (`Email/changes` never asked; new mail not called recent);
three more are guards, and each was checked by breaking the implementation on
purpose:

- reconciling a delta as if it were a listing — caught by three tests, including
  the one where mail delivered to the *archive* must not empty the inbox;
- applying a relist as if it were a delta — caught by the recovery test, which
  destroys a message before planting an unusable state, so the listing that
  recovers has to be *reconciled* rather than merely written;
- never asking for a delta at all — caught by the two red tests above.

Decisions taken:

- **The first refresh is phrased as `Relisted`, not as a fourth case.** A folder
  with no state lists, and a listing *is* one of the three answers — mapping
  `store.messages` into `MessageUpdate::Relisted` means one dispatch below
  rather than two paths that must be kept in step.
- **A refused delta relists rather than fails.** That judgement is `jmap-mail-sync`'s
  and this vfunc inherits it; what is new here is the test for it, and the
  reason it matters at the Camel layer: `refresh_info_sync` has nowhere to
  report "your state is too old" to, so a folder that failed would be one that
  never comes back.
- **The mock records call names, not full requests.** Enough for "did it ask,
  and what did it ask", nothing that would make a test depend on argument
  shapes that are the client's business. Recorded before the call is answered,
  so a request that errored still counts as the round trip it was.

One arm is **not covered by a test, and honestly so**: `Unchanged` records the
state the delta came back with. Against this mock that is always the state the
folder asked with — an empty `Email/changes` here carries `sinceState` back
unchanged — so `set_summary_state`'s equality check makes the arm a no-op and
there is nothing observable to assert. It is written the way a server that
advances the state on an empty delta would need, and left untested rather than
tested against a mock behaviour that does not exist.

Also worth recording: the `recent` test logs a `camel-CRITICAL` about
`camel_session_get_filter_driver`. That is not a fault — it is Camel's own
`changed` handler reacting to a recent uid on a folder that carries
`CAMEL_FOLDER_FILTER_RECENT`, and the test's stand-in `CamelSession` does not
implement the vfunc. It is the first direct evidence the recent list reaches
the code path it exists for. Whether the user's filters then do the right thing
*needs human verification in real Evolution*.

Not verified locally, as in the previous fifty-one sessions: `reuse lint` and
`cargo deny` (neither binary is installed on this VM). No new files, so no new
SPDX headers. `cargo fmt --check`, `cargo test --locked` and `cargo clippy
--all-targets --locked -- -D warnings` are clean on the default member set and
on the five EDS crates; `jmap-mail`'s refresh suite is at 11 tests.

Next in M5. A refresh is now cheap and correct; what it still does not do is
**bound the catching up** — a folder that has been closed for a week gets a
delta naming every message the account touched, fetched one `Email/get` chunk
at a time, where relisting the one mailbox might be cheaper. That wants a
threshold and a test that a delta over it relists instead.

Still open from earlier sessions: **bounding the cache**; the other half of the
cache's atomicity problem (an entry is written by `write_all` and close rather
than to a temporary name and renamed); the `changed` signal a transfer emits is
still not asserted by a test, and lifting `tests/refresh.rs`'s emission harness
into `tests/common` is what that wants — the harness has now grown a fourth
list, which makes the case stronger; `CamelSubscribable` still wants
`Mailbox/set`, which the client does not have; `get_trash_folder_sync` and
`get_junk_folder_sync` are still a settings decision before they are a vfunc,
and that decision is what `expunge_sync` waits on; cross-store transfers want
`Email/import` for an `append_message_sync`. Unexercised against a real
`CamelSession`: `service.rs`, which waits on M6 and M7. The README's
architecture block still lists only the round-1 crates.

## 2026-08-09 (fifty-third session)

M5's thirty-third increment, across `jmap-mail-sync` and `jmap-mail`: **bounding
the catching up**. A refresh now asks how *much* a delta would cost before it
follows one, and lists the mailbox instead when catching up has stopped being
the cheap answer.

The gap the last session left. `Email/changes` answers for the whole account, so
a folder that has been closed for a fortnight is handed every message anyone
touched anywhere — each one an id that must be fetched before this mailbox can
say whether it holds it. `messages_since` followed that however long it was,
which turns the cheap path into the expensive one exactly when a user opens
Evolution after a holiday.

The rule is `catch_up_limit(held, objects_in_get) = held.max(objects_in_get)`,
and both halves are cost comparisons rather than tuning knobs:

- **`held` — the caller's row count.** Catching up fetches what the delta names;
  listing fetches the mailbox. The mailbox's size is not something this layer
  knows, and asking the server for it costs the round trip the bound exists to
  save — but the *caller* has it for free, and it is exactly the set a listing
  would fetch again. So `messages_since` grew a `held` parameter, threaded
  through `JmapStore::messages_since`, and `refresh_info_sync` fills it from the
  new `summary::summary_rows` (`camel_folder_summary_count`). It passes with the
  state, because the two are one fact: what the folder holds, and when it was
  true.
- **The floor of one `Email/get`.** A listing is never a single round trip — the
  state, then a query, then a `/get` per page — so a delta that fits in one
  `/get` is cheaper than any listing whatsoever. Without the floor an empty
  mailbox would relist itself every time the account was touched anywhere, which
  is the opposite of the point.
- **`destroyed` does not count.** Those ids are taken at the delta's word and
  never fetched, so a hundred of them cost what none do.

Red first, at both layers, and each new guard was checked by breaking the
implementation on purpose:

- `jmap-mail-sync` (four tests, `objects_in_get(2)` so the bound is reachable):
  the over-the-bound case was red for the right reason before the check existed.
  Dropping the floor breaks the "fits in one `/get`" test (and three older ones);
  counting `destroyed` breaks the "only reports gone" test; a limit of
  `usize::MAX` breaks the relist test.
- `jmap-mail` (two tests): a folder holding two rows relists when three messages
  moved elsewhere; the same folder holding four rows still follows the same
  three-message delta. The pair is what pins the number being passed down to the
  folder's *own* count — passing 0 breaks the second, passing `usize::MAX` breaks
  the first. Both assert on the wire (`Email/query` asked or not) rather than on
  the rows, because a delta and a listing leave the folder holding the same rows;
  `with_mail_on` is the new harness that builds the mock to order.

One existing test changed and it is worth being explicit about: 
`more_changed_messages_than_one_get_may_ask_about_are_fetched_in_several` seeds
its mailbox with five messages before delivering five more. It is about chunking
a delta across `Email/get` calls, and with the bound in place a folder holding
nothing would relist instead of chunking anything — so the setup gives it a
mailbox big enough for the delta to still be followed. The behaviour under test
is unchanged; what changed is that the folder now has rows, which is the only
state a real folder would be in when a delta of five arrives.

Decisions taken:

- **The bound lives in `jmap-mail-sync`, the measurement in `jmap-mail`.** The
  judgement is about JMAP round trips and belongs beside the code that makes
  them; the row count is a Camel fact and belongs beside the summary. Passing a
  wrong `held` can only ever cost round trips — it is a cost estimate, never an
  input to what rows come back — which is what makes it safe to ask a caller for.
- **The limit is relative, not a constant.** A fixed threshold would be wrong in
  the dangerous direction: a hundred-thousand-message inbox would relist itself
  for a four-hundred-message delta, which is far worse than the problem being
  fixed. Anchoring it to what the folder holds means the answer scales with the
  mailbox.
- **The mailbox size is not asked of the server.** `Email/query` with
  `calculateTotal`, or `Mailbox/get`'s `totalEmails`, would both be authoritative
  and both cost a round trip on the path that exists to avoid one. The folder's
  row count is the free estimate, and it is stale only in the direction that
  matters least.

Not verified locally, as in the previous fifty-two sessions: `reuse lint` and
`cargo deny` (neither binary is installed on this VM). No new files, so no new
SPDX headers. `cargo fmt --check`, `cargo test --locked` and `cargo clippy
--all-targets --locked -- -D warnings` are clean on the default member set and
on the five EDS crates; `jmap-mail`'s refresh suite is at 13 tests and
`jmap-mail-sync`'s updates suite at 16.

Next in M5. The cheap-refresh path is now complete end to end — ask what
changed, apply it, and stop following a delta that has outgrown the mailbox — so
the next tractable thing is one of the items that have been open for several
sessions rather than a new part of refresh. **Bounding the cache** is the
closest relative of this increment (unbounded on disk, and nothing evicts), and
the `changed` signal a transfer emits is still not asserted by a test, which
wants `tests/refresh.rs`'s emission harness lifted into `tests/common`.

Still open from earlier sessions: the other half of the cache's atomicity
problem (an entry is written by `write_all` and close rather than to a temporary
name and renamed); `CamelSubscribable` still wants `Mailbox/set`, which the
client does not have; `get_trash_folder_sync` and `get_junk_folder_sync` are
still a settings decision before they are a vfunc, and that decision is what
`expunge_sync` waits on; cross-store transfers want `Email/import` for an
`append_message_sync`. Unexercised against a real `CamelSession`: `service.rs`,
which waits on M6 and M7. The README's architecture block still lists only the
round-1 crates.

## 2026-08-09 (fifty-fourth session)

M5's thirty-fourth increment, across `jmap-proto`, `jmap-mock` and
`jmap-client`: **`Mailbox/set` — the folder the user asks the server to make.**

The gap this closes is one three earlier sessions have written down and none
has taken: `CamelSubscribable` wants `Mailbox/set` "which the client does not
have", and the same sentence is what `create_folder_sync`, `delete_folder_sync`
and `rename_folder_sync` have been waiting on. Nothing above the wire can be
written until the wire itself answers, so this increment is the protocol half
alone — the mock method and the three client calls — and deliberately stops
short of the Camel vfuncs that will sit on top of them.

What the mock now answers (RFC 8621 §2.5), written out rather than handed to
`setops::simple_set` for the reason `Email/set` is: every judgement here is
about the *rest* of the store rather than about the object in hand, and the
generic helper only validates a creation.

- **A name is unique among siblings and nowhere else.** Two folders called
  `2026` under one parent are two folders the user cannot tell apart; the same
  two under different parents are an ordinary way to file mail. Both halves are
  tested, and the second is what stops the check from being written as "unique
  in the account".
- **A `parentId` names a mailbox, and not one below the mailbox being moved.**
  The cycle walk goes up one step at a time from the proposed parent and is
  bounded by the size of the tree — a loop the mock did not put there is still a
  loop it must not hang on.
- **A role belongs to one mailbox.** An account with two inboxes is one where
  `folders.rs`'s role lookup picks whichever came first.
- **`id` is the server's.** A create that carries one is refused, on
  `ContactCard/set`'s reasoning: a backend offering a Camel folder path as a
  JMAP id is a mistake that would otherwise surface much later.
- **A destroy is refused by what is inside it** — `mailboxHasChild` and
  `mailboxHasEmail`, RFC 8621 §2.5's own two error types, added to `jmap-proto`
  as `mail::mailbox_set_error`. They are refusals rather than failures: what is
  inside the folder is the user's to decide about, and a backend has to be able
  to pass the distinction on unchanged. `onDestroyRemoveEmails` is not
  implemented, and the refusal is what says so.

Red first: nineteen tests in the new `jmap-client/tests/mail_folders.rs`, which
would not compile before the three client methods existed and then failed
against a mock that answered `unknownMethod`. Each guard was then checked by
breaking it on purpose — dropping the cycle walk fails the
moved-inside-itself test; making the sibling comparison forget the parent fails
`one_name_under_two_parents_is_two_folders`; letting a mailbox be its own
sibling (the `of` argument ignored) fails the unsubscribe test, because a patch
that leaves the name alone would otherwise collide with itself; removing either
destroy guard fails exactly its own test.

Decisions taken:

- **One working copy of the tree per request.** The checks run against a clone
  that the request updates as it goes, so a create is refused by a sibling made
  two entries earlier in the same call, and a destroy sees the mailbox a move
  earlier in the call reparented. Validating against the store as it was would
  let one request contradict itself.
- **A create answers with the mailbox, an update with nothing.** The id is the
  server's to hand out and a caller that cannot name the folder afterwards would
  have to go looking for it by a name that is unique only among siblings. The
  echoed object carries `totalEmails`/`unreadEmails` of 0 — the counts are
  derived rather than stored (see `mailbox_get`), and a folder made a moment ago
  holds nothing, so answering them beats sending the client back for a
  `Mailbox/get` to learn a number it could not fail to know.
- **`isSubscribed` defaults to true on a create.** RFC 8621 §2 leaves the
  default to the server; any other answer hides a folder the user has just asked
  for from a client that lists subscribed ones.
- **The client sends a `PatchObject`, not a whole `Mailbox`.** Same reason
  `email_update` does: most of what a `Mailbox` carries is the server's — the
  counts above all — and sending it back is a client telling a server what it
  has just been told.

Not verified locally, as in the previous fifty-three sessions: `reuse lint` and
`cargo deny` (neither binary is installed on this VM). One new file,
`jmap-client/tests/mail_folders.rs`, with the SPDX header. `cargo fmt --check`,
`cargo test --locked` and `cargo clippy --all-targets --locked -- -D warnings`
are clean on the default member set and on the five EDS crates.

Next in M5. The wire is now there for the three folder vfuncs that were blocked
on it, and the tractable next step is the smallest of them: **`CamelSubscribable`
on the store** — `subscribe_folder_sync`/`unsubscribe_folder_sync` are one
`Mailbox/set` update of `isSubscribed` each, and `folder_is_subscribed` is a
read of what `Mailbox/get` already returns. `create_folder_sync` and
`delete_folder_sync` are the next two after it, and both want a
`CamelFolderInfo` answer and the store's folder list kept in step; the refusals
this increment added (`mailboxHasChild`, `mailboxHasEmail`, a sibling's name)
are what those vfuncs must map onto a `CamelError` the user can act on.

Still open from earlier sessions: **bounding the cache** (unbounded on disk, and
nothing evicts); the other half of the cache's atomicity problem (an entry is
written by `write_all` and close rather than to a temporary name and renamed);
the `changed` signal a transfer emits is still not asserted by a test, which
wants `tests/refresh.rs`'s emission harness lifted into `tests/common`;
`get_trash_folder_sync` and `get_junk_folder_sync` are still a settings decision
before they are a vfunc, and that decision is what `expunge_sync` waits on;
cross-store transfers want `Email/import` for an `append_message_sync`.
Unexercised against a real `CamelSession`: `service.rs`, which waits on M6 and
M7. The README's architecture block still lists only the round-1 crates.

## 2026-08-09 (fifty-fifth session)

M5's thirty-fifth increment, across `jmap-mail-sync` and `jmap-mail`: **the
subscription a user toggles** — the `Mailbox/set` that carries it, and the
folder listing that has to agree with it afterwards.

This is the first half of what the previous session named as next:
`CamelSubscribable` on the store. The wire it was blocked on landed then
(`Mailbox/set` in the client); what is added here is the state on both sides of
it. The GInterface itself — `eds-sys`'s binding, the `interface_init` hook
`jmap-backend-core`'s `ObjectSubclass` does not have yet, and the three vfunc
slots — is deliberately left for the next session, because it is a change to the
subclassing scaffold rather than to the store, and doing it in the same commit
would mix a piece of shared machinery into a mail-provider increment.

- **`MailSync::set_subscribed(&Id, bool)`** — one `Mailbox/set` update of
  `isSubscribed`. One method rather than two, because `subscribe_folder_sync`
  and `unsubscribe_folder_sync` differ on the wire only in what they set it to.
- **`FolderTree::set_subscribed(&Id, bool) -> bool`**, the one edit the tree
  type offers, and by mailbox id rather than by path: a folder whose parent was
  renamed between the listing and the write has a different path and the same
  id.
- **`JmapStore::set_subscribed`** — the request, and then the edit to the
  listing the store holds.

Red first: six tests in the new `jmap-mail-sync/tests/subscriptions.rs`, nine in
the new `jmap-mail/tests/subscriptions.rs`, two more in
`jmap-mail-sync/tests/tree.rs`. Each guard was then checked by breaking it on
purpose — dropping the listing edit fails `the_held_listing_agrees_with_the_write`
and `a_tree_already_handed_out_is_not_edited_underneath_its_reader`; making the
tree walk not descend fails the nested-folder test; collapsing `NoSuchFolder`
into another variant fails `a_folder_the_account_no_longer_has_is_reported_as_missing`.

Decisions taken:

- **The store edits its own listing, and that is not cache-warming.**
  `CamelSubscribable` declares `folder_is_subscribed` as a *non-blocking*
  method — Evolution asks it once per folder while drawing the tree — so the
  held listing is the only thing that can ever answer it. A store that wrote the
  subscription to the server and left its listing saying the opposite would draw
  the tick straight back on, and keep doing so until something refreshed the
  tree.
- **The edit goes through `Arc::make_mut`.** A `CamelFolderInfo` forest is
  copied out of a borrowed tree; a tree that mutated underneath such a walk is
  the bug this rules out. A caller already holding the previous `Arc` keeps
  reading what it was handed, which a test pins.
- **The listing's `state` is left where it was.** The write did move the account
  on, so the next refresh finds a change and rebuilds — one listing more than
  strictly needed. The alternative is a store inventing a state string the
  server never handed it and then asking `Mailbox/changes` from it.
- **A store with nothing listed yet gains nothing.** A tree assembled from the
  single mailbox a write happened to name would be an account with one folder in
  it.
- **No "it is already subscribed" shortcut**, unlike `set_keywords`. The only
  thing that could answer that question is the folder listing, which is a cache
  another client's change makes wrong — in precisely the direction that would
  swallow the user's write. A round trip per tick in the subscription editor is
  the cheaper mistake.
- **`SyncError::NoSuchFolder`**, mapped to `StoreError::NoFolder` and so to
  `CAMEL_STORE_ERROR_NO_FOLDER`. A folder another client deleted while this one
  still lists it is ordinary; reported as a service error it would be a working
  account shown as broken. `NoFolder` carries a mailbox id here where it carries
  a path elsewhere — noted in the mapping, since the path the write came from
  may since have moved.

Not verified locally, as in the previous fifty-four sessions: `reuse lint` and
`cargo deny` (neither binary is installed on this VM). Two new files, both
carrying the SPDX header. `cargo fmt --check`, `cargo test --locked` and
`cargo clippy --all-targets --locked -- -D warnings` are clean on the default
member set and on the five EDS crates.

Next in M5. **The `CamelSubscribable` interface itself**, which is now the only
thing between this state and the vfuncs: `CamelSubscribable.*` on `eds-sys`'s
allowlist; an `interface_init` hook on `jmap-backend-core`'s `ObjectSubclass`,
whose `interfaces()` today registers every interface with a NULL init and so
cannot fill a vfunc slot (`CamelNetworkSettings`, the only interface implemented
so far, is all properties); then `folder_is_subscribed` as a read of the held
listing and the two sync vfuncs as calls to `JmapStore::set_subscribed`, each
emitting `camel_subscribable_folder_subscribed`/`_unsubscribed` the way IMAPX
does. `create_folder_sync` and `delete_folder_sync` come after that, and both
want a `CamelFolderInfo` answer and the store's folder list kept in step; the
refusals the mock learned two sessions ago (`mailboxHasChild`,
`mailboxHasEmail`, a sibling's name) are what those vfuncs must map onto a
`CamelError` the user can act on.

Still open from earlier sessions: **bounding the cache** (unbounded on disk, and
nothing evicts); the other half of the cache's atomicity problem (an entry is
written by `write_all` and close rather than to a temporary name and renamed);
the `changed` signal a transfer emits is still not asserted by a test, which
wants `tests/refresh.rs`'s emission harness lifted into `tests/common`;
`get_trash_folder_sync` and `get_junk_folder_sync` are still a settings decision
before they are a vfunc, and that decision is what `expunge_sync` waits on;
cross-store transfers want `Email/import` for an `append_message_sync`.
Unexercised against a real `CamelSession`: `service.rs`, which waits on M6 and
M7. The README's architecture block still lists only the round-1 crates.

## 2026-08-09 (fifty-sixth session)

The scaffold change the previous session named as the only thing left between
the subscription state it landed and the vfuncs that carry it: **an interface
whose vtable one of our types fills in**, in `jmap-backend-core`, and the
binding of the interface it exists for, `CamelSubscribable`, in `eds-sys`.

Deliberately not the vfuncs themselves. This is shared machinery plus an FFI
surface, and both are testable here on their own terms; the store's three slots
are a mail-provider increment and start from a scaffold that is already pinned.

- **`InterfaceDecl`**, what `ObjectSubclass::interfaces` now returns.
  `InterfaceDecl::defaults(gtype)` is exactly the old behaviour — a NULL
  `interface_init`, right for `CamelNetworkSettings`, which is all properties —
  and `InterfaceDecl::filled_by::<I>()` is new.
- **`InterfaceImpl`**, the `G_IMPLEMENT_INTERFACE` init function as a trait:
  an associated `Vtable` type, the interface's `GType`, and an
  `interface_init` handed a typed `*mut Vtable`. Implemented by a type *beside*
  the class rather than on it, because one class can fill several interfaces
  and a trait on the class could only describe one.
- **A guarded trampoline** between the two, like `class_init`'s: GObject calls
  it from inside `g_type_class_ref` while holding the type system's global
  lock, and a panic unwinding out of there aborts the process.
- **`CamelSubscribable.*` and `camel_subscribable_.*`** on `eds-sys`'s
  allowlist.

Red first: three tests in `jmap-backend-core/tests/subclass.rs`, two in
`eds-sys/tests/camel.rs`. Both new guards were then checked by breaking them on
purpose — with `filled_by` handing GObject a NULL init the dispatch test
segfaults, and with the `guard` taken out of the trampoline the panicking-init
test aborts the process instead of failing.

Decisions taken:

- **The old comment was wrong, and checking it is what this increment is built
  on.** `subclass.rs` claimed a type that needs to fill a vfunc slot "overrides
  them in `class_init`, where the interface struct is reachable through
  `g_type_interface_peek`". A throwaway probe says that does work today: the
  vtable *is* reachable from `class_init` and a slot written through it *does*
  survive. So this is not a bug fix. It is a choice of the documented mechanism
  over an ordering `gtype.c` happens to have — it base-initialises interface
  vtables before calling `class_init` and runs the `interface_init`s after —
  and nothing in GLib's documentation promises that ordering.
  The first version of the test asserted the peek returns NULL and failed,
  which is how the claim got checked at all; the doc comment now records what
  is actually true rather than what was assumed.
- **A caught panic leaves the vtable half-filled.** Putting the defaults back
  would mean copying a vtable whose size this code does not know. Logged and
  left, which for a slot the init never reached is the interface's own default.
- **`GTypePlugin` as the test interface.** Already used for the "an interface
  is added before the type is handed back" test, and it is the one interface a
  test can implement without also satisfying it — with the property that makes
  this increment testable at all: `g_type_plugin_use` dispatches through a slot
  with no default behind it, so the test drives GLib's own dispatch rather than
  reading the slot back. Reading it back would pass just as well if GLib never
  looked at that copy of the vtable.
- **Two `eds-sys` tests rather than a line in `tests/layout.rs`.** `g_type_query`
  reports nothing about an interface — asserted, so the gap is recorded rather
  than implied. What is checked instead is the contract: the `CamelStore`
  prerequisite, `CamelOfflineStore` *not* implementing it already, and no
  default behind any of the three methods. The last is why `defaults()` would be
  the wrong declaration here: a slot left NULL is a call through NULL from
  inside `camel_subscribable_folder_is_subscribed`, not a store that answers
  conservatively.

Not verified locally, as in the previous fifty-five sessions: `reuse lint` and
`cargo deny` (neither binary is installed on this VM). No new files, so no new
SPDX headers to get wrong. `cargo fmt --check`, `cargo test --locked` and
`cargo clippy --all-targets --locked -- -D warnings` are clean on the default
member set and on the five EDS crates.

Next in M5. **The store's three slots**, which now have nowhere left to hide: an
`InterfaceImpl` beside `JmapStore` with `CamelSubscribableInterface` as its
`Vtable`, `folder_is_subscribed` reading the held listing (non-blocking, so the
listing is the only thing that can answer it), and the two sync vfuncs calling
`JmapStore::set_subscribed`, each emitting
`camel_subscribable_folder_subscribed`/`_unsubscribed` with the
`CamelFolderInfo` for the folder the way IMAPX does. `create_folder_sync` and
`delete_folder_sync` come after that, and both want a `CamelFolderInfo` answer
and the store's folder list kept in step; the refusals the mock learned three
sessions ago (`mailboxHasChild`, `mailboxHasEmail`, a sibling's name) are what
those vfuncs must map onto a `CamelError` the user can act on.

Still open from earlier sessions: **bounding the cache** (unbounded on disk, and
nothing evicts); the other half of the cache's atomicity problem (an entry is
written by `write_all` and close rather than to a temporary name and renamed);
the `changed` signal a transfer emits is still not asserted by a test, which
wants `tests/refresh.rs`'s emission harness lifted into `tests/common`;
`get_trash_folder_sync` and `get_junk_folder_sync` are still a settings decision
before they are a vfunc, and that decision is what `expunge_sync` waits on;
cross-store transfers want `Email/import` for an `append_message_sync`.
Unexercised against a real `CamelSession`: `service.rs`, which waits on M6 and
M7. The README's architecture block still lists only the round-1 crates.

## 2026-08-09 (fifty-seventh session)

The three slots the previous session left with nowhere to hide: **`JmapStore`
fills `CamelSubscribableInterface`**, in a new `jmap-mail/src/subscribe.rs`.
The scaffold (`InterfaceDecl::filled_by`, `InterfaceImpl`) and the FFI surface
(`CamelSubscribable.*` on `eds-sys`'s allowlist) landed last session; the store's
write (`JmapStore::set_subscribed`, mailbox id and a listing edit) the session
before that. This is the joint between them.

- **`Subscribable`**, a unit type beside the store, is the `InterfaceImpl`:
  `CamelSubscribableInterface` as its `Vtable`, `camel_subscribable_get_type` as
  its `GType`, and an `interface_init` that fills all three slots.
- **`is_subscribed(store, path)`** — the non-blocking read, answered from
  `JmapStore::held_folders` and nothing else.
- **`set_subscribed(store, path, subscribed)`** — resolves the Camel path
  against the folder tree, writes, and hands back the folder as it now is, which
  is what the signal is built from.
- **`JmapStore::held_folders`** is new: the listing if there is one, no request
  and no connection needed. `JmapStore::folders(0)` was the near miss — it lists
  the account when it holds nothing, which from a non-blocking vfunc is a folder
  tree that stalls the UI thread once per row.
- `folders::tree_holding` became `pub(crate)`; the subscription write is its
  third caller.

Red first: nine tests added to `tests/subscriptions.rs`, and the two new guards
were then checked by breaking them on purpose — with `interfaces()` back to
empty the vtable peek returns NULL, and with `is_subscribed` reading
`folders(0)` the "makes no request" test fails.

Decisions taken:

- **`false` for a folder the store has never heard of**, and for a store with no
  listing at all. The question `folder_is_subscribed` asks is whether the *user*
  asked to see this folder; nothing in hand says they did. `true` would put a
  tick on a folder this store knows nothing about. IMAPX answers the same way,
  out of its mailbox table.
- **The path is resolved through `folders::tree_holding`**, so a mailbox another
  client created since the last listing is subscribable without a restart —
  identical to opening a folder by path. The cost is one `Mailbox/changes` on
  the path that was about to fail anyway; a hit costs nothing.
- **The answer is an owned `FolderInfo`, not a borrow of the tree.** The listing
  is edited by `JmapStore::set_subscribed` under its own lock, and handing out a
  borrow would mean holding that lock across the signal emission.
- **The announcement is a one-folder chain at depth `Some(0)`**, and the chain is
  kept and dropped by this function rather than handed over: Camel's signal only
  borrows it, which is why IMAPX frees it one line after emitting.
- **`StoreError::NoFolder` carries the path here**, where the store's own
  `set_subscribed` carries a mailbox id — this is the layer that still has the
  path Camel named.

**Not covered by a test, and this is the honest limit of the increment:** the
`g_signal_emit` at the end of `subscribe_folder_sync` and
`unsubscribe_folder_sync`. Emitting needs a store instantiated through a
`CamelSession`, and a probe confirmed what was suspected — `g_object_new` on the
store type without one leaves Camel logging four criticals and handing back
something `G_IS_OBJECT` rejects. The stores these tests use are
`JmapStore::detached` instances, which are deliberately not GObjects. So
everything up to the emission is driven (`folder_is_subscribed` through the
vtable, `set_subscribed` against the mock), and the two emission lines — the two
IMAPX has — are **not verified here**; they want a real `CamelSession`, which is
M6/M7 or the M9 functional tier.

Not verified locally, as in the previous fifty-six sessions: `reuse lint` and
`cargo deny` (neither binary is installed on this VM). One new file, carrying the
SPDX header. `cargo fmt --check`, `cargo test --locked` and `cargo clippy
--all-targets --locked -- -D warnings` are clean on the default member set and on
the five EDS crates.

Next in M5. **`create_folder_sync` and `delete_folder_sync`**, the last two
`CamelStoreClass` folder vfuncs with a mock behind them: both want a
`CamelFolderInfo` answer and the store's folder list kept in step, and the
refusals the mock learned four sessions ago (`mailboxHasChild`,
`mailboxHasEmail`, a sibling's name) are what they must map onto a `CamelError`
the user can act on. `rename_folder_sync` sits beside them. Worth pairing with
them: `get_folder_info_sync` still ignores `CAMEL_STORE_FOLDER_INFO_SUBSCRIBED`
and `SUBSCRIPTION_LIST`, which now have something to filter on and are what the
subscription editor passes.

Still open from earlier sessions: **bounding the cache** (unbounded on disk, and
nothing evicts); the other half of the cache's atomicity problem (an entry is
written by `write_all` and close rather than to a temporary name and renamed);
the `changed` signal a transfer emits is still not asserted by a test, which
wants `tests/refresh.rs`'s emission harness lifted into `tests/common`;
`get_trash_folder_sync` and `get_junk_folder_sync` are still a settings decision
before they are a vfunc, and that decision is what `expunge_sync` waits on;
cross-store transfers want `Email/import` for an `append_message_sync`.
Unexercised against a real `CamelSession`: `service.rs` and the two emissions
above, which wait on M6 and M7. The README's architecture block still lists only
the round-1 crates.

## 2026-08-09 (fifty-eighth session)

The two vfuncs the previous session named as next want a layer that does not
exist yet: **`MailSync::create_folder` and `MailSync::delete_folder`**, in
`jmap-mail-sync`. `jmap-client` learned `mailbox_create`/`mailbox_destroy` two
sessions ago and the mock learned the refusals five ago; this is the sync layer
between them, and it is where the one thing Camel needs and JMAP does not have —
the folder *path* — is built.

- **`create_folder(parent: Option<&FolderInfo>, name)` answers with a
  `FolderInfo`**, not an id. `camel_store_create_folder_sync` hands the
  `CamelFolderInfo` it gets straight to Evolution's folder tree, and the path is
  this crate's invention: `path::join(parent.path, encode_component(name))`, the
  same mapping a listing makes.
- **`delete_folder(&Id)`** is `Mailbox/set` destroy with no
  `onDestroyRemoveEmails`.
- **`folder_error`** is new and shared: the `notFound` → `SyncError::NoSuchFolder`
  mapping `set_subscribed` had inline, now the one place both mailbox writes ask
  it.

Red first: twelve tests in a new `tests/manage.rs`, all failing to compile
against a `MailSync` with neither method. The path guard was then checked by
breaking it on purpose — with the raw name in place of `encode_component(name)`,
`a_name_that_is_not_a_path_component_is_encoded` fails and nothing else does.

Decisions taken:

- **The answer is built from what was *sent*, not from what came back.** RFC
  8620 §5.3 lets a server return, for a created record, only the properties it
  set itself — so `name` and `parentId` may legitimately be absent from the
  response object, and a path read out of it would be empty against a perfectly
  correct server. The id is the property a create exists to learn and the one the
  RFC guarantees; everything else in the answer is the request plus arithmetic.
- **`parent` is a `FolderInfo` and not an `Id`.** The request needs the id and
  the answer needs the path, and only the caller's tree holds both.
- **`role` is `None` on a new folder, deliberately.** None is requested, and a
  role read back from the response would be this function assigning one outside
  `FolderTree::claim_roles`' arbitration — which is what keeps an account from
  showing two inboxes.
- **`isSubscribed` *is* read from the response**, defaulting to `true`. It is
  the one property here the client cannot work out, RFC 8621 §2 leaves the
  default to the server, and the other guess hides the folder Evolution was just
  told to make until the next listing.
- **`mailboxHasChild` and `mailboxHasEmail` get no variant of their own.** The
  test for a variant in `SyncError` is whether Camel has a *code* the layer above
  could map it onto — `NoSuchFolder` exists because `CAMEL_STORE_ERROR_NO_FOLDER`
  does. For these two the reason is prose either way, and it already survives the
  crate boundary intact inside `SyncError::Client(Error::Set(_))`; re-encoding
  the server's vocabulary into ours would only drop the description that came
  with it. Two tests pin that the distinction arrives, and one pins that neither
  is read as a missing folder.

**Not covered by a test, and the honest limit of the increment:** the "server
returns only what it set" case above. `jmap-mockd` echoes the whole mailbox in
its `created` map, so both readings of the response pass against it — the
decision is defensive, argued from the RFC, and unexercised. Making the mock
answer sparsely would be a change to a fixture every other test reads, and is
worth doing on its own if a second caller ever depends on the same rule.

Not verified locally, as in the previous fifty-seven sessions: `reuse lint` and
`cargo deny` (neither binary is installed on this VM). One new file, carrying the
SPDX header. `cargo fmt --check`, `cargo test --locked` and `cargo clippy
--all-targets --locked -- -D warnings` are clean on the default member set and on
the five EDS crates. (`example-module`, the vendored EDS sample, still fails
clippy on `manual_c_str_literals` as it has all along; it is not one of the six.)

Next in M5. **`create_folder_sync` and `delete_folder_sync` on `JmapStore`**, now
that both have a layer under them: each wants the store's held listing kept in
step — a create has to *add* its `FolderInfo` to the tree the way `set_subscribed`
edits one, or the folder disappears until the next refresh — and Camel wants
`folder_created`/`folder_deleted` emitted with the `CamelFolderInfo`, which is the
same signal problem the subscription vfuncs hit and the same
`CamelSession`-shaped limit. `rename_folder_sync` sits beside them and has no
`MailSync` method yet. Worth pairing: `get_folder_info_sync` still ignores
`CAMEL_STORE_FOLDER_INFO_SUBSCRIBED` and `SUBSCRIPTION_LIST`.

Still open from earlier sessions: **bounding the cache** (unbounded on disk, and
nothing evicts); the other half of the cache's atomicity problem (an entry is
written by `write_all` and close rather than to a temporary name and renamed);
the `changed` signal a transfer emits is still not asserted by a test, which
wants `tests/refresh.rs`'s emission harness lifted into `tests/common`;
`get_trash_folder_sync` and `get_junk_folder_sync` are still a settings decision
before they are a vfunc, and that decision is what `expunge_sync` waits on;
cross-store transfers want `Email/import` for an `append_message_sync`.
Unexercised against a real `CamelSession`: `service.rs` and the subscription
emissions, which wait on M6 and M7. The README's architecture block still lists
only the round-1 crates.

## 2026-08-09 (fifty-ninth session)

**`create_folder_sync` and `delete_folder_sync` on `JmapStore`** — the two
`CamelStoreClass` vfuncs the previous session's `MailSync::create_folder` and
`delete_folder` were the layer under. Two commits, one per crate.

- **`FolderTree::insert` and `FolderTree::remove`** (`jmap-mail-sync`). The
  tree's second and third edits, after `set_subscribed`, and they exist for the
  same reason it does: the server has just told the caller what changed, so
  re-listing the account to learn it would be asking a question already
  answered.
- **`JmapStore::create_folder` / `delete_folder`** and
  **`jmap_mail::manage`** — the path resolution, the two vfuncs, and the
  `camel_store_folder_created`/`_deleted` emissions.

Red first: seven tests in `jmap-mail-sync/tests/tree.rs` against a `FolderTree`
with neither method, then seventeen in a new `jmap-mail/tests/manage.rs` against
a store and a module that did not exist.

Decisions taken:

- **`insert` reads the parent out of the folder's own path** rather than taking
  it beside the folder. A folder's path *is* its position — its parent's path
  plus one encoded component, which is the invariant the tree already maintains
  — and a component can never contain the separator, so the last one splits it
  unambiguously. A parent passed separately could disagree with the path; there
  is no second version to disagree with.
- **A parent path the tree does not have inserts nothing**, rather than hanging
  the folder off the roots: drawing it at the top level of an account that has
  it somewhere else is worse than not drawing it until the next listing.
- **A sibling with the new folder's path is dropped.** The listing can be stale
  in exactly one direction the server will not correct — it can still hold a
  mailbox another client destroyed, whose name is then free for this create to
  reuse — and of two folders at one path, the server-confirmed one stays.
- **`remove` takes the subtree with it.** RFC 8621 §2.5 has the server refuse to
  destroy a mailbox with children, so a destroy that succeeded says the server
  had none, and the children this tree lists are ones another client removed
  first.
- **The create's `folder_name` is a mailbox name, not a path component.** JMAP
  files a mailbox under an explicit `parentId`, so unlike IMAPX there is no
  hierarchy to read out of the name: a `/` the user typed is a character of the
  name they chose, and the path is where it gets percent-encoded. Pinned by a
  test — "Bills/2026" becomes the path `Bills%2F2026`.
- **`mailboxHasChild`/`mailboxHasEmail` stay `StoreError::Client`**, which is
  the previous session's judgement carried through the last layer: Camel has no
  code to map them onto, and the server's sentence is what the user needs.
- **Camel does not emit these two signals for us.** Checked rather than assumed:
  disassembling the installed `libcamel-1.2.so.64` shows nothing in it calls
  `camel_store_folder_created`/`_deleted` outside `CamelVeeStore`, so the
  provider emits, as `subscribe.rs` already does for the subscription pair. The
  same disassembly settled the ownership question the create depends on — both
  emitters call `camel_folder_info_clone` and queue on the session, so the chain
  handed to the signal is only borrowed and the same one can be returned to
  Camel's caller.

**Not covered by a test, and the honest limits of the increment:**

1. The two emissions, for `subscribe.rs`'s reason, now stated more precisely:
   `camel_store_folder_created` starts at `camel_service_ref_session`, so a
   store without a `CamelSession` cannot emit at all, and these tests use
   `JmapStore::detached` instances.
2. **None of this is reachable from Evolution's UI yet, deliberately.**
   Evolution offers "New Folder"/"Delete Folder" for a store carrying
   `CAMEL_STORE_CAN_EDIT_FOLDERS`, and the store does not set that flag — the
   same flag also offers "Rename Folder", and `rename_folder_sync` is still
   NULL, so setting it today would put a menu item in front of the user that
   reaches a slot Camel refuses to call. A test pins the pairing: it fails the
   moment `rename_folder_sync` is filled in, as a reminder that the flag is then
   the thing to add. *Needs human verification in real Evolution once the flag
   is set.*

Not verified locally, as in the previous fifty-eight sessions: `reuse lint` and
`cargo deny` (neither binary is installed on this VM). Two new files, both
carrying the SPDX header. `cargo fmt --check`, `cargo test --locked` and
`cargo clippy --all-targets --locked -- -D warnings` are clean on the default
member set and on the five EDS crates (486 tests on the latter).

Next in M5. **`rename_folder_sync`** and, with it, `CAMEL_STORE_CAN_EDIT_FOLDERS`
— which is what turns this session's two vfuncs on. A rename is one
`Mailbox/set` update over `name` (and `parentId`, since Camel spells a move as a
rename to a path under another parent), and the work around it is the store's
held listing: every descendant's path changes, which is an edit `FolderTree` has
no method for yet. Worth pairing: `get_folder_info_sync` still ignores
`CAMEL_STORE_FOLDER_INFO_SUBSCRIBED` and `SUBSCRIPTION_LIST`.

Still open from earlier sessions: **bounding the cache** (unbounded on disk, and
nothing evicts); the other half of the cache's atomicity problem (an entry is
written by `write_all` and close rather than to a temporary name and renamed);
the `changed` signal a transfer emits is still not asserted by a test, which
wants `tests/refresh.rs`'s emission harness lifted into `tests/common`;
`get_trash_folder_sync` and `get_junk_folder_sync` are still a settings decision
before they are a vfunc, and that decision is what `expunge_sync` waits on;
cross-store transfers want `Email/import` for an `append_message_sync`.
Unexercised against a real `CamelSession`: `service.rs` and the four emissions
(two subscription, two folder-management), which wait on M6 and M7. The README's
architecture block still lists only the round-1 crates.

## 2026-08-09 (sixtieth session)

**The rename, in `jmap-mail-sync`** — the write and the tree edit under
`rename_folder_sync`, which the previous session named as the next increment.
Two commits, both in one crate; the `CamelStore` vfunc and
`CAMEL_STORE_CAN_EDIT_FOLDERS` are deliberately *not* here, for the reason at
the end.

- **`MailSync::rename_folder`**: one `Mailbox/set` update over `name` and
  `parentId`, answering with the folder's new Camel path.
- **`FolderTree::rename`**: the tree's third edit, after `set_subscribed` and
  `insert`/`remove`.

Red first: eight tests in `jmap-mail-sync/tests/manage.rs` against a `MailSync`
with no such method, then eleven in `tests/tree.rs` against a tree with no such
edit.

Decisions taken:

- **A rename and a move are one operation**, because they are one to the caller.
  Camel names a folder by path, so the folder's name and its parent both live in
  the string `rename_folder_sync` is handed, and it never says which of the two
  the user changed. RFC 8621 §2.5 puts both in one update.
- **Both properties are sent every time**, including a `parentId` of `null` for
  a folder moving up to the top level. Sending only what looks changed would
  need a before-picture, and the only one available is the caller's listing — a
  cache, which another client's move has already made wrong in exactly the
  direction that would swallow this write. Pinned by a test: a child moved to
  the top level ends up there, which a patch omitting `parentId` would not do.
- **The answer is the new path**, for the reason a create answers with a whole
  `FolderInfo`: the caller keys the folder by that string and cannot build it,
  because the mailbox-name-to-path-component encoding is private to this crate.
  Nothing else about a folder changes, so there is nothing else to report.
- **`display_name` is passed to the tree edit beside the path**, not read out of
  it. The path's last component is the name with the encoding applied and this
  crate has no decoder; the caller has the name it just sent. A test pins the
  pair — the path `and%2For` with the name `and/or`.
- **Descendant paths are rebuilt structurally**, from each child's own last
  component onto its parent's new path, rather than by swapping a prefix. It is
  the same reading of a path `insert` already makes, and it does not depend on
  the moved subtree's paths having been consistent beforehand. Iterative, so the
  walk is not one more thing a server-chosen depth turns into a stack.
- **A move into the folder's own subtree is refused**, and both refusals are
  checked *before* the subtree is lifted out: a folder taken out of the tree and
  then refused at its destination is one that has simply disappeared. The
  placement itself is delegated to `insert`, which is what makes the second
  lookup unable to fail without a panic being needed to say so.
- **A moved folder joins its new siblings at the end**, as a created one does.
  Sibling order is the server's (sortOrder, then name); this side has been told
  about one folder, not about where the account now sorts it. Stated in a test
  rather than left to be discovered.
- **`remove` became `take`** — the same walk with the folder handed back — since
  a move is a removal that puts the subtree down again somewhere else.

**Not covered by a test, and the honest limits of the increment:**

1. **Nothing in Evolution reaches this yet**, and one more increment is needed
   before it can: `rename_folder_sync` is still NULL on `CamelJmapStoreClass`,
   and `CAMEL_STORE_CAN_EDIT_FOLDERS` is still unset. The test in
   `jmap-mail/tests/manage.rs` that fails the moment the vfunc is filled in is
   still the reminder that the flag is then the thing to add. *Needs human
   verification in real Evolution once both land.*
2. The path the *store* will hand `FolderTree::rename` comes from Camel and its
   last component is whatever Evolution's rename dialog put there — not
   necessarily this crate's encoding of it. That is the layer above's problem to
   state and it is what the next increment has to decide: the name sent to the
   server and the path written into the tree have to be the pair this session's
   two methods take, or a renamed folder is one Camel cannot open.

Not verified locally, as in the previous fifty-nine sessions: `reuse lint` and
`cargo deny` (neither binary is installed on this VM). No new files, so no new
SPDX headers. `cargo fmt --check`, `cargo test --locked` and `cargo clippy
--all-targets --locked -- -D warnings` are clean on the default member set and
on the five EDS crates.

Next in M5. **`rename_folder_sync` on `JmapStore`, and the flag that turns all
three management vfuncs on.** The vfunc resolves the old path to a folder, reads
the new path's last component as the mailbox name (the decision limit 2 above
names), calls `MailSync::rename_folder`, applies `FolderTree::rename` to the
held listing and emits `camel_store_folder_renamed` — one more emission with
`subscribe.rs`'s `CamelSession` limit. Then
`CAMEL_STORE_CAN_EDIT_FOLDERS` on the store's flags, which is what puts New,
Delete and Rename Folder in front of the user. Worth pairing:
`get_folder_info_sync` still ignores `CAMEL_STORE_FOLDER_INFO_SUBSCRIBED` and
`SUBSCRIPTION_LIST`.

Still open from earlier sessions: **bounding the cache** (unbounded on disk, and
nothing evicts); the other half of the cache's atomicity problem (an entry is
written by `write_all` and close rather than to a temporary name and renamed);
the `changed` signal a transfer emits is still not asserted by a test, which
wants `tests/refresh.rs`'s emission harness lifted into `tests/common`;
`get_trash_folder_sync` and `get_junk_folder_sync` are still a settings decision
before they are a vfunc, and that decision is what `expunge_sync` waits on;
cross-store transfers want `Email/import` for an `append_message_sync`.
Unexercised against a real `CamelSession`: `service.rs` and the four emissions
(two subscription, two folder-management), which wait on M6 and M7. The README's
architecture block still lists only the round-1 crates.

## 2026-08-09 (sixty-first session)

**The rename, in the store** — `rename_folder_sync`, the third and last of the
folder-management vfuncs, which the previous session named as the next
increment. Two commits: the path mapping's other direction in `jmap-mail-sync`,
and everything else in `jmap-mail`.

- **`path::split`**, and `jmap_mail_sync::path` goes public for it alone.
- **`JmapStore::rename_folder`**: the write, the tree edit, and the renamed
  subtree the emission is built from.
- **`manage::rename_folder`**: the path Camel hands over, resolved.
- **`rename_folder_sync`** on the class.

Red first: sixteen tests in `jmap-mail/tests/manage.rs` against a store with no
such method.

Decisions taken:

- **The last component of the new path is read two different ways, and which one
  depends on whether it changed.** Unchanged, the folder keeps the name it has:
  Evolution builds the path for a drag and drop out of the folder's *existing*
  path, so the component that arrives is this crate's encoding of the name
  rather than the name, and there is no decoder to read it back with. Taking it
  as a name would rename `Bills/2026` to `Bills%2F2026` for the crime of being
  dragged. Changed, it is the name the user typed into the rename dialog,
  verbatim — the same reading `create_folder_sync` makes of `folder_name`,
  which is what makes the two vfuncs agree about what a name is.
- **The limit that leaves is stated in the module and pinned by a test rather
  than hidden**: a typed name this crate has to encode (one containing a `%`, a
  lone `.`) puts the folder at a path that is not the one Camel asked for,
  because the path is the encoding and the caller wrote the name unencoded. The
  name is the one the user asked for and the answer carries the real path, so
  what Evolution draws is right; anything above that remembered the requested
  path is out of step until the account is listed again. Refusing such a rename
  instead would be refusing a legal folder name.
- **Both paths are resolved against one look at the tree**, through
  `tree_holding`, whose predicate now asks for the folder *and* the new parent —
  so a parent another client created since the last listing is found rather than
  reported missing, which is the second look every other folder vfunc takes.
- **A missing new parent is `NoFolder` and nothing is written.** A folder moved
  under a parent that is not there is a folder nothing can reach.
- **The answer is the whole renamed subtree, not one folder.**
  `camel_store_folder_renamed`'s handler walks the children of what it is
  handed, and a rename moves every descendant's path — each of which is a key
  Camel opens a folder by — so the chain is built with no depth limit, unlike
  the create's and the delete's `Some(0)`.
- **`CAMEL_STORE_CAN_EDIT_FOLDERS` needs no line of ours**, contrary to what the
  previous two sessions expected. Camel's own `camel_store_init` sets it — the
  store's flags word is `VTRASH | VJUNK | CAN_EDIT_FOLDERS` on a store this
  provider has written nothing into, verified here on a real constructed store.
  A provider *opts out* of folder management by clearing the bit. So New and
  Delete Folder have been on offer since they landed, and Rename was a menu item
  over a NULL slot, which Camel refuses to call and the user sees as nothing
  happening; filling the slot is what fixes it. An explicit OR of a bit already
  set would have been a line nothing could ever observe, so there is none — a
  test pins the whole flags word instead, so that a Camel default changing under
  us is red rather than a menu item quietly going away.
- The verification method is worth recording: the flag test was written first
  and then run with the setting line commented out. It passed, which is what
  exposed the default. A test that passes without the code it is meant to test
  is not a test; the code went, the test stayed and grew an exact assertion.

**Not covered by a test, and the honest limits of the increment:**

1. **The emission is still unexercised**, now for all three vfuncs:
   `camel_store_folder_renamed` queues on the service's session, which the
   detached stores these tests use do not have. The three sync-side lines are
   written as every other provider writes them. *Needs human verification in
   real Evolution.*
2. **Nothing here has been driven from Evolution's menu.** Now that the flag
   turns out to have been set all along, the whole of folder management —
   create, delete, rename, move — is reachable by a user for the first time, and
   none of it has been tried in a real session. Renaming an *open* folder in
   particular touches Camel's own bookkeeping in
   `camel_store_rename_folder_sync`, which this VM has no source for and which
   was therefore not read. *Needs human verification in real Evolution.*
3. The `VTRASH | VJUNK` half of that flags word is Camel's default rather than a
   decision this provider has taken, and it is the one `get_trash_folder_sync`
   and `get_junk_folder_sync` still wait on: a JMAP account has mailboxes with
   `trash` and `junk` roles, and whether Evolution should show those or Camel's
   virtual folders is a settings question. It is now pinned by a test, so it is
   at least a visible default rather than an invisible one.

Not verified locally, as in the previous sixty sessions: `reuse lint` and `cargo
deny` (neither binary is installed on this VM). No new files, so no new SPDX
headers. `cargo fmt --check`, `cargo test --locked` and `cargo clippy
--all-targets --locked -- -D warnings` are clean on the default member set and
on the five EDS crates (499 tests on the latter).

Next in M5. With the three management vfuncs done, the store's remaining gap in
the folder listing is **`get_folder_info_sync` ignoring
`CAMEL_STORE_FOLDER_INFO_SUBSCRIBED` and `SUBSCRIPTION_LIST`** — a filter on the
tree rather than a different request, and the thing that makes the subscription
ticks the user sets actually change what the folder tree shows. It pairs with
the `FAST` flag, which asks for the tree without counts.

Still open from earlier sessions: **bounding the cache** (unbounded on disk, and
nothing evicts); the other half of the cache's atomicity problem (an entry is
written by `write_all` and close rather than to a temporary name and renamed);
the `changed` signal a transfer emits is still not asserted by a test, which
wants `tests/refresh.rs`'s emission harness lifted into `tests/common`;
`get_trash_folder_sync` and `get_junk_folder_sync` are still a settings decision
before they are a vfunc, and that decision is what `expunge_sync` waits on;
cross-store transfers want `Email/import` for an `append_message_sync`.
Unexercised against a real `CamelSession`: `service.rs` and the five emissions
(two subscription, three folder-management), which wait on M6 and M7. The
README's architecture block still lists only the round-1 crates.

## 2026-08-09 (sixty-second session)

**The ticks, in the listing** — `get_folder_info_sync` reads
`CAMEL_STORE_FOLDER_INFO_SUBSCRIBED` and `SUBSCRIPTION_LIST`, which the previous
session named as the next increment. One commit, entirely inside `jmap-mail`:
`Request::roots` becomes a `Cow`, and a filter behind it.

- **`SUBSCRIBED`** is what Evolution's folder tree adds for a store that is
  `CamelSubscribable`, so it is the flag that makes the tick the user sets in
  the subscription editor change what the folder tree draws. Until now the tick
  was written to the server, kept in the store's listing and answered to
  `folder_is_subscribed` — and then ignored by the only call that decides which
  folders appear.
- **`SUBSCRIPTION_LIST`** is the editor's own question — which folders are there
  to tick — and is answered with all of them. For JMAP that is the listing the
  store already holds: `Mailbox/get` returns every mailbox of the account with
  its `isSubscribed`, so there is no second, wider request to make, the way an
  IMAP store needs `LIST` beside `LSUB`.

Red first: twelve tests in `jmap-mail/tests/folders.rs` against a hand-built
tree with the ticks set explicitly, and two more through the vfunc against the
mock, nine of which failed.

Decisions taken:

- **An unticked folder with a ticked one below it stays in the answer.**
  `CamelFolderInfo` hangs a child off its parent, so there is no answer in which
  `Work/Invoices` is present and `Work` is not: dropping the unticked parent
  would drop mail the user explicitly asked to see. This is the same answer an
  IMAP server gives, which returns the unsubscribed parents `LSUB`'s children
  need.
- **Such a folder is not dressed up as anything else.** It keeps
  `subscribed: false`, so the listing does not put a tick in the editor the user
  never set, and it is deliberately *not* marked `CAMEL_FOLDER_NOSELECT`.
  Marking it was the obvious way to say "you are only seeing this because of
  what is under it", and `camel-enums.h` documents the flag as "the folder
  cannot contain messages" — which a JMAP mailbox the user unticked plainly can.
  A flag Camel acts on is not a place to put a hint. The visible cost is that
  unticking a folder that has a ticked child leaves it in the tree and openable;
  the alternative is lying to Camel about the mailbox.
- **The filter is applied to whatever `top` already chose**, rather than instead
  of it — a caller asking about one subtree of a filtered account means the
  filtered part of that subtree — and it leaves the depth alone: cutting to
  `RECURSIVE` stays `FolderInfoChain::from_forest`'s job.
- **`SUBSCRIPTION_LIST` outranks `SUBSCRIBED`** if a caller sets both. An editor
  showing only what is already ticked is one nothing new can be ticked in.
  Evolution passes them separately, so this is defensive rather than observed.
- **The filter and the depth cut differ in one visible way, and a test pins it.**
  `from_forest` deliberately leaves `CAMEL_FOLDER_CHILDREN` on a folder whose
  children the *depth* left out — they exist, and the expander is how the caller
  asks for them. Children the *ticks* left out are not part of this view at all,
  so the folder reports `NOCHILDREN`.
- **The filter is iterative**, for the reason `from_forest` gives: the depth of
  the tree comes from a `parentId` chain a server chose. Pre-order with each
  folder's parent recorded, then back the other way, so a folder's children are
  settled before the folder is. `FolderInfo` is rebuilt field by field rather
  than with `..folder.clone()`, which would clone the subtree its children were
  just chosen from only to throw it away.
- **`Request::roots` is now a `Cow`.** A subtree with folders taken out of it is
  not a subtree of the tree the store holds, so a filtered answer has to be
  built; an unfiltered one — every call Evolution makes for the message list —
  still borrows and allocates nothing.

**Not covered by a test, and the honest limits of the increment:**

1. **Nothing here has been driven from Evolution's folder tree.** That the tree
   passes `SUBSCRIBED` for a subscribable store, and the editor
   `SUBSCRIPTION_LIST`, is read from Camel's own documentation of the flags
   rather than observed. *Needs human verification in real Evolution.*
2. **The `NOSELECT` decision above is a judgement, not a measurement.** What
   Evolution draws for an unticked folder kept for its ticked child — and
   whether a user reads it as "unsubscribing did nothing" — is exactly the
   question a real session answers. *Needs human verification in real
   Evolution.*
3. `FAST` is still not read, and the module says why: it is documented as
   deprecated and "most backends will behave the same whether it is supplied or
   not", which is true of this one because JMAP puts the counts in the mailbox
   anyway. The pairing the previous session expected turned out not to exist.

Not verified locally, as in the previous sixty-one sessions: `reuse lint` and
`cargo deny` (neither binary is installed on this VM). No new files, so no new
SPDX headers. `cargo fmt --check`, `cargo test --locked` and `cargo clippy
--all-targets --locked -- -D warnings` are clean on the default member set and
on the five EDS crates (513 tests on the latter).

Next in M5. The store's folder surface is now complete enough that the gaps are
elsewhere: the most valuable next increment is **lifting `tests/refresh.rs`'s
emission harness into `tests/common`**, which is what the five unexercised
emissions (two subscription, three folder-management) and the transfer's
`changed` signal all wait on — every one of them is currently "two lines written
the way IMAPX writes them" rather than something a test has seen fire.

Still open from earlier sessions: **bounding the cache** (unbounded on disk, and
nothing evicts); the other half of the cache's atomicity problem (an entry is
written by `write_all` and close rather than to a temporary name and renamed);
`get_trash_folder_sync` and `get_junk_folder_sync` are still a settings decision
before they are a vfunc, and that decision is what `expunge_sync` waits on;
cross-store transfers want `Email/import` for an `append_message_sync`.
Unexercised against a real `CamelSession`: `service.rs` and the five emissions,
which wait on M6 and M7. The README's architecture block still lists only the
round-1 crates.

## 2026-08-09 (sixty-third session)

**The five emissions, watched at last — and one of them was a duplicate.** The
previous session named lifting `tests/refresh.rs`'s emission harness into
`tests/common` as the most valuable next increment, because five folder signals
and a transfer's `changed` were all "two lines written the way IMAPX writes
them" rather than anything a test had seen fire. The harness moved, and the
first thing it saw was a bug.

- **`tests/common/signals.rs`** holds `Context` (a main context per test, and
  the pump without which every emission test passes by observing silence) and
  two recorders: the folder's `changed`, lifted from `refresh.rs` unchanged, and
  the store's five folder signals, new. `refresh.rs` lost 159 lines and kept all
  thirteen tests.
- **`tests/emissions.rs`**, nine tests, is the first thing in this crate to
  drive a store signal end to end. It works because `tests/common`'s `Account`
  builds a real `CamelSession`: `camel_store_folder_created` and its four
  siblings queue the emission on `camel_session_ref_main_context`, so a detached
  store — what `tests/manage.rs` and `tests/subscriptions.rs` use, and why both
  of their headers said the emissions were out of reach — cannot emit at all.

**The finding: `camel_store_rename_folder_sync` emits `folder-renamed` itself.**
`manage.rs`'s header claimed the opposite — "Camel does not emit any of the
three signals for us … the emitters are called nowhere in libcamel outside
`CamelVeeStore`" — and that claim was wrong for exactly one of the three. Red,
reproducibly: two identical `folder-renamed` events per rename; one when our
line is commented out. So the line is gone, and with it the chain it built.

Decisions taken:

- **Rename is Camel's to announce, not ours.** A create and a delete that said
  nothing would leave every view of the account but one showing a stale tree —
  their wrappers call the vfunc and return. The rename wrapper does not: it
  renames the folders in the store's object bag and then emits, building the
  info by asking the store for the new path. Duplicating that is announcing one
  rename twice; matching the platform is what every other provider does.
- **The cost is stated, and pinned by two tests.** Camel asks for that info with
  `CAMEL_STORE_FOLDER_INFO_SUBSCRIBED`, because this store is subscribable — so
  the subscription filter the previous session built decides whether Camel says
  anything. Measured, not assumed: a subtree with nothing subscribed anywhere in
  it is renamed **silently**; an unsubscribed folder kept for a subscribed child
  is announced normally. The silent case is a subtree the folder tree the rename
  was invoked from is not drawing, and it is Camel's rule for every provider
  alike — but it is a real hole and there is now a test whose failure message
  says so, should Camel ever change its mind.
- **The four one-argument signals share one handler**, told apart by the static
  signal name passed as the user data. Five near-identical `extern "C"` bodies
  differing in a string literal is not clearer than one that reads the string.
- **`Context` must be pushed before the account is opened.** For the folder's
  `changed` the context that matters is the one current when `camel_folder_changed`
  runs; for the store's five it is the one the *session* captured at
  construction, which is earlier. Every test here pushes first, and the type
  says why.
- **A handler connected between the queueing and the pump still catches the
  emission**, because the emission happens when the idle source runs. Two tests
  therefore pump their setup away before they start watching.

**Not covered by a test, and the honest limits:**

1. **Nothing here has been driven from Evolution.** That the folder tree redraws
   from these signals, and that a duplicate `folder-renamed` would have been
   visible rather than merely wrong, is read from Camel's documentation and
   source behaviour, not observed. *Needs human verification in real Evolution.*
2. **A second finding, left for the next increment.** The fully-unsubscribed
   rename test prints
   `camel-WARNING: CamelJmapStore::get_folder_info() reported failure without
   setting its GError`. Camel's `CAMEL_CHECK_GERROR` treats a NULL return from
   `get_folder_info_sync` as a failure, while this provider documents NULL with
   no error as "an account with no folders" — which is what `camel_store_*`'s
   own callers read it as too. Both readings are defensible and the choice is
   not obvious enough to make in passing, so it is written down rather than
   patched: it is a console warning, not a behaviour break.
3. **The transfer's `changed` signal is still unasserted.** It was the other
   thing waiting on this harness; the harness is now there and the test is a
   small, self-contained next increment.

Not verified locally, as in the previous sixty-two sessions: `reuse lint` and
`cargo deny` (neither binary is installed on this VM). Two new files, both with
SPDX `GPL-3.0-or-later` headers. `cargo fmt --check`, `cargo test --locked` and
`cargo clippy --all-targets --locked -- -D warnings` are clean on the default
member set and on the five EDS crates; the jmap-mail suite was run three times
over for the main-context flakiness this harness exists to avoid.

Next in M5. The transfer's `changed` emission, then the `get_folder_info_sync`
NULL-versus-GError question above.

Still open from earlier sessions: **bounding the cache** (unbounded on disk, and
nothing evicts); the other half of the cache's atomicity problem (an entry is
written by `write_all` and close rather than to a temporary name and renamed);
`get_trash_folder_sync` and `get_junk_folder_sync` are still a settings decision
before they are a vfunc, and that decision is what `expunge_sync` waits on;
cross-store transfers want `Email/import` for an `append_message_sync`.
Unexercised against a real `CamelSession`: `service.rs`, which waits on M6 and
M7. The README's architecture block still lists only the round-1 crates.

## 2026-08-09 (sixty-fourth session)

**The transfer's `changed`, the last of the emissions nobody had watched.** The
previous session lifted the emission harness into `tests/common` and named this
as the small, self-contained next increment: `transfer_messages_to_sync` calls
`camel_folder_changed` when a move has taken rows away, and no test had ever
seen it fire. Five tests in `tests/transfer.rs` now do — and unlike last
session's rename, the code was right.

Red first, and reproducibly: with the emission disabled the two positive tests
fail with `left: []` against the expected `removed: ["E1"]`, and the three
silences pass — which is exactly the shape a guard against announcing too much
should have. The emission was restored before anything else.

- **`a_move_tells_camel_the_row_that_left`** — one emission, `removed` and
  nothing else. Asserted as a whole `Emission` rather than field by field, so
  the test also says that nothing was called added, changed, or *recent*: a uid
  on the recent list is a message the user's incoming filters would be run over,
  and a move is not an arrival.
- **`a_copy_says_nothing_about_the_folder_it_came_from`** — a copy takes no row
  away, so there is nothing to say. Announcing anyway would redraw a message
  list the user is reading, and lose their place in it, for a folder in which
  nothing happened.
- **`a_move_says_nothing_about_the_folder_it_arrives_in`** — the visible half of
  this file's standing decision not to build a row in the destination. Watched
  on the archive rather than the inbox, because the recorder is one list for
  whatever it is connected to: a test watching both folders could not tell which
  of them spoke.
- **`a_message_another_client_deleted_is_announced_as_its_row_goes`** — the one
  failure that still changes the folder. The message is not in this mailbox
  because it is not anywhere, so the row goes and the list is told, while the
  transfer is still reported as failed.
- **`a_transfer_that_failed_leaves_the_message_list_alone`** — every other
  failure. A disconnected store is the case Camel retries after reconnecting,
  and a folder that had announced the message as removed in the meantime would
  have taken it off the user's screen for the duration.

Decisions taken:

- **`Fixture::watching` takes the folder as a function, not a pointer.** The
  fixture has to exist before either of its folders does, and the ordering rule
  `common::signals::Context` documents pushes the context before even that. A
  `fn(&Fixture) -> *mut CamelFolder` is the smallest thing that lets one
  constructor serve both the source-side and the destination-side tests.
- **The setup is pumped away inside the constructor**, not in each test. The
  fixture refreshes the inbox, which queues a `changed` of its own, and a
  handler connected after the queueing still catches the delivery — so `watch`
  after `emissions` is the only order that answers what the *transfer*
  announced. Putting it in the constructor means no test can get it wrong.
- **Two of the five assert the whole `Emission` value**, three assert emptiness.
  Comparing the struct is shorter than four field assertions and says more,
  since it pins the three lists a transfer must leave alone.

**Not covered by a test, and the honest limits of the increment:**

1. **Nothing here has been driven from Evolution.** That the message list redraws
   from this signal — and in particular that the destination folder *not*
   announcing is invisible to a user, because they are looking at the folder
   they dragged into and Camel opens it fresh — is read from Camel's
   documentation rather than observed. *Needs human verification in real
   Evolution.*
2. **The multi-message partial transfer is still unwatched.** A selection where
   one message moves and another is already gone should announce both rows in
   one emission; the walk is written that way and every test here transfers a
   single uid. It is a fixture change (a second seeded message), not a code
   change, and it is the obvious next test in this file.
3. **Nothing asserts what the destination does at its next listing**, only that
   the transfer itself left it alone. `a_move_says_nothing_about_the_folder_it_arrives_in`
   checks the archive is still empty; that a refresh then finds the message is
   `tests/refresh.rs`'s subject and is not re-asserted here.

Not verified locally, as in the previous sixty-three sessions: `reuse lint` and
`cargo deny` (neither binary is installed on this VM). No new files, so no new
SPDX headers. `cargo fmt --check`, `cargo test --locked` and `cargo clippy
--all-targets --locked -- -D warnings` are clean on the default member set (337
tests) and on the five EDS crates (527); the jmap-mail suite was run three times
over for the main-context flakiness, 309 tests each time.

Next in M5. The `get_folder_info_sync` NULL-versus-GError question the previous
session wrote down and did not patch: Camel's `CAMEL_CHECK_GERROR` reads a NULL
return as a failure, this provider documents it as "an account with no folders",
and the fully-unsubscribed rename test prints a `camel-WARNING` because of it.
That is now the oldest undecided thing in the folder surface.

Still open from earlier sessions: **bounding the cache** (unbounded on disk, and
nothing evicts); the other half of the cache's atomicity problem (an entry is
written by `write_all` and close rather than to a temporary name and renamed);
`get_trash_folder_sync` and `get_junk_folder_sync` are still a settings decision
before they are a vfunc, and that decision is what `expunge_sync` waits on;
cross-store transfers want `Email/import` for an `append_message_sync`.
Unexercised against a real `CamelSession`: `service.rs`, which waits on M6 and
M7. The README's architecture block still lists only the round-1 crates.

## 2026-08-09 (sixty-fifth session)

**The selection, not the message.** The previous session named it: every test in
`tests/transfer.rs` transferred a single uid, while the vfunc is handed the
*list* the user had highlighted, and "a selection where one message moves and
another is already gone should announce both rows in one emission" was written
the right way and never watched. Four tests now cover the list, and the code was
right — no production change in this increment.

Red first, and twice over, because two different mistakes are in scope:

- With `break` after the first failure — the naive walk this module's header
  argues against — three of the four fail: the second message stays in the inbox
  (`{M1: true}` against `{M2: true}`), the reported array comes back
  `[None, None]` instead of `[None, Some("E2")]`, and the emission carries
  `["E1"]` instead of `["E1", "E2"]`.
- With the walk truncated to the first uid, all four fail, including the
  all-succeed case the `break` leaves green.

Both were restored before anything else; `git diff` on `src/transfer.rs` is
empty.

- **`every_message_of_a_selection_is_filed`** — the ordinary drag. Both messages
  end up in the archive alone and the inbox lists nothing.
- **`a_message_that_is_gone_does_not_hold_up_the_rest_of_the_selection`** — the
  module's "one request per message, not one per transfer" decision seen from
  outside. The first message is destroyed by another client; the second still
  moves, and the transfer is still reported as `INVALID_UID`.
- **`a_partial_transfer_reports_only_the_messages_that_landed`** — Camel reads
  the out-parameter by position, so a half-worked transfer leaves NULL in the
  slot of the message that did not land rather than a shorter array. A closed-up
  gap would tell the caller the wrong message had moved.
- **`the_rows_a_selection_left_behind_are_announced_together`** — one emission
  for the whole selection. `CamelFolderChangeInfo` is four *lists* precisely so
  that a drag of twenty messages redraws the message list once. Both rows are on
  it although only one message moved, which is where the announcement and the
  reported outcome deliberately disagree.

Decisions taken:

- **Two constructors over one parameterised fixture at every call site.**
  `Fixture::start()` seeds one message and `Fixture::selection()` two, both over
  a private `seeded(n)` and a `MESSAGES` table. Seeding two unconditionally
  would have been the smaller diff and would have quietly changed what fifteen
  existing tests assert — `listed(inbox).is_empty()` after a move is only true
  because the inbox held one message.
- **`Fixture::watching(context, folder)` became `Fixture::watched(self, …)`.**
  The old constructor hard-coded `Self::start()`, so a watched two-message
  fixture needed either a second near-identical constructor or a builder
  argument. Consuming `self` keeps the ordering rule the doc comment exists for
  — drain, then connect — inside one place, and reads as
  `Fixture::selection().watched(&context, |f| f.inbox)`.
- **`mailboxes_on_server` / `keywords_on_server` / `destroyed_elsewhere` take a
  uid.** A selection has no single "the message", and a zero-argument version
  beside a `_of(uid)` one would be two names for one question.

**Not covered by a test, and the honest limits of the increment:**

1. **Nothing here has been driven from Evolution.** That a drag of several
   messages reaches the vfunc as one call with one array — rather than as one
   call per message — is read from `camel_folder_transfer_messages_to_sync`
   rather than observed, and it is the premise of all four tests. *Needs human
   verification in real Evolution.*
2. **Two messages, not twenty.** The claims are about the shape of the answer
   (per-uid request, positional slots, single emission) and two uids is the
   smallest list that can distinguish them, but nothing here exercises a
   selection large enough for the per-message round trip to be a visible cost.
   Batching several uids into one `Email/set` while keeping a per-message answer
   is possible — RFC 8621 §5.3 reports `notUpdated` per id — and is a real
   optimisation this file's decision leaves on the table deliberately, not
   accidentally.
3. **The failing message is always the first.** A failure in the middle or at
   the end of a longer selection would exercise the same code, and the index
   arithmetic in `Reported::set` is the part a longer list would pin harder.

Not verified locally, as in the previous sixty-four sessions: `reuse lint` and
`cargo deny` (neither binary is installed on this VM). No new files, so no new
SPDX headers. `cargo fmt --check`, `cargo test --locked` and `cargo clippy
--all-targets --locked -- -D warnings` are clean on the default member set (337
tests) and on the five EDS crates (531); the jmap-mail suite was run three times
over for the main-context flakiness, 313 tests each time.

Next in M5. The `get_folder_info_sync` NULL-versus-GError question is still the
oldest undecided thing in the folder surface, and this session looked at it
before choosing the transfer work instead. What it found, so the next session
starts further along: the warning is
`CamelJmapStore::get_folder_info() reported failure without setting its GError`,
it comes from Camel's `CAMEL_CHECK_GERROR`, and exactly one test provokes it —
`emissions.rs`'s `a_rename_of_a_subtree_nothing_is_subscribed_to_is_announced_by_no_one`,
where Camel's own rename path asks with `SUBSCRIBED` and this store correctly
answers "nothing matches" as NULL with no error, which is what
`camel_store_get_folder_info_sync` documents as allowed. So the two readings are
Camel's macro against Camel's own prose, and settling it wants the EDS *source*
for `camel-store.c` — which this VM does not have, only the headers. That is the
blocker to clear first, by reading the release tarball rather than by guessing
at which branch of the wrapper the check sits in.

Still open from earlier sessions: **bounding the cache** (unbounded on disk, and
nothing evicts); the other half of the cache's atomicity problem (an entry is
written by `write_all` and close rather than to a temporary name and renamed);
`get_trash_folder_sync` and `get_junk_folder_sync` are still a settings decision
before they are a vfunc, and that decision is what `expunge_sync` waits on;
cross-store transfers want `Email/import` for an `append_message_sync`.
Unexercised against a real `CamelSession`: `service.rs`, which waits on M6 and
M7. The README's architecture block still lists only the round-1 crates.

## 2026-08-09 (sixty-sixth session)

**`Email/import`: the method a message that is already a message arrives by.**
The oldest item on the "still open" list that is a feature rather than a
decision: *cross-store transfers want `Email/import` for an
`append_message_sync`*. This session did the protocol half of it — RFC 8621 §4.8
in `jmap-proto`, in the mock, and in the client — and left the Camel vfunc for a
session of its own. Everything here is in `default-members`, so it is exercised
by the fast suite rather than only by the EDS one.

RFC 8621 §4.8 was read from the published text (`rfc-editor.org`) rather than
from memory, because the branch points in it are the whole design: what the
server *may* do with a duplicate, with a blob it cannot read, and with a
`receivedAt` it was not given.

Red first: the twelve tests in `jmap-client/tests/mail_import.rs` were written
against a proto and a client that compiled, and all twelve failed with
`unknownMethod` — the mock had no such method — before `dispatch.rs` learned the
arm. The ten unit tests in the new `jmap-mock/src/message.rs` came with the
parser they describe.

What landed:

- **`jmap-proto`**: `EmailImport`, `EmailImportRequest`, `EmailImportResponse`,
  and `email_import_error::INVALID_EMAIL`.
- **`jmap-mock`**: `Email/import`, and `message.rs` — the crate's only *parser*,
  since every other message in this server is written *out* of an `Email` rather
  than read into one.
- **`jmap-client`**: `email_import`, answering the `Email` the server made.
  `expect_created` was split so its inner half (`creation_outcome`) serves a
  method that carries `created`/`notCreated` without being a `/set`.

Decisions taken, each one a branch RFC 8621 §4.8 leaves to the server:

- **The blob is kept exactly as it arrived**, so the `blobId` answered is the one
  handed in and a download returns the uploaded octets byte for byte
  (`an_imported_message_downloads_as_the_bytes_it_went_up_as`). The RFC allows
  repairing a message and answering with a different blob; a mock that rewrote
  its input could not be used to test that what a client appended is what it
  later opens. The client's doc comment says to read the answered `blobId`
  anyway, because a real server may not be this one.
- **Duplicates are allowed** (`the_same_message_imported_twice_is_two_messages`).
  `alreadyExists` is a MAY, and the same fixture gets imported twice on purpose
  in a test account. Asserted, so that making the mock strict is a decision
  someone takes rather than a behaviour that drifts.
- **`receivedAt` is the one given, or the mock's fixed clock.** The RFC's default
  is the most recent `Received` header's date, which is a zone offset away from a
  `UtcDate` — calendar arithmetic that `jmap_proto::UtcDate` explicitly does not
  do. The provider that will drive this has the date already parsed (Camel hands
  it over as a `time_t`), so it will send `receivedAt` rather than have it
  guessed at. Written down in `message.rs` as a deliberate absence, not a TODO.
- **A missing property is a per-message refusal, a wrong-typed one fails the
  call.** Hence every field of `EmailImport` is `Option` although the RFC makes
  `blobId` and `mailboxIds` required: §4.8 wants "missing, wrong type, id not
  found" answered with an `invalidProperties` `SetError` for *that* message while
  the others still import, and a required field would turn one client mistake
  into a whole request that fails to parse. A wrong *type* still fails the call
  with `invalidArguments`, which is a deviation and is listed below.
- **`filed_somewhere` is reused rather than reimplemented**, so an import is held
  to exactly the `mailboxIds` rule a create and an update are: at least one
  mailbox, and every named mailbox must exist.
- **`invalidEmail` is the mock's answer to bytes that are not a message**, with
  the bar set where "not a message" is not a judgement call: not UTF-8, no header
  field at all, or a header block with a line that is neither a field nor a
  continuation. A body it cannot make sense of is never a reason — a body is
  opaque either way.

**Not covered by a test, and the honest limits of the increment:**

1. **No `append_message_sync` yet.** Nothing in `jmap-mail` calls
  `email_import`; a drag from an IMAP account into a JMAP one still fails inside
  Camel, and `transfer.rs`'s comment now says exactly that (the method exists,
  the vfunc does not). Until that vfunc is written this is protocol work with no
  user-visible effect, which is why no milestone tag is claimed for it.
2. **An imported message has no MIME tree.** No `bodyStructure`, `textBody`,
  `bodyValues` or `hasAttachment` is derived from the bytes, so an imported
  message and a seeded one answer differently to an `Email/get` that asks for
  body properties. Deriving them means a MIME parser, and a half-written one
  would make the mock a worse test subject than one that visibly has none. Our
  own provider reads a message from the raw blob, so nothing in this repository
  needs the tree — a real server would have it, and a test that needs it seeds.
3. **No RFC 2047 decoding**, matching the mock's writer, which emits display
  names without encoding them. A `Subject` with an encoded word imports as the
  encoded word.
4. **A wrong-typed `EmailImport` property fails the whole call** with
  `invalidArguments` instead of refusing the one message with
  `invalidProperties`, because the request is deserialized into a typed struct
  like every other method here. Faithful behaviour would need the `emails` map
  parsed entry by entry out of `Value`.
5. **An import's creation id is not back-referenceable.** `record_created_ids`
  only harvests ids from a `/set`, so `#creationId` in a later call of the same
  request cannot name an imported message. Nothing needs it (a submission
  references a draft an `Email/set` made), and adding it untested would be
  speculation.
6. **Nothing here has been driven from Evolution**, which is true of the whole
  session by construction: no EDS code changed except one comment.

Not verified locally, as in the previous sixty-five sessions: `reuse lint` and
`cargo deny` (neither binary is installed on this VM). Two new files —
`jmap-mock/src/message.rs`, `jmap-client/tests/mail_import.rs` — both with SPDX
`GPL-3.0-or-later` headers. `cargo fmt --check`, `cargo test --locked` and
`cargo clippy --all-targets --locked -- -D warnings` are clean on the default
member set (359 tests, up from 337: twelve integration tests and ten parser unit
tests) and on the five EDS crates (531).

Next in M5. `append_message_sync` is now unblocked and is the obvious next
increment: serialize the `CamelMimeMessage` Camel hands over, `upload_blob`,
`email_import` with the folder's mailbox id, the message's flags as keywords and
its date as `receivedAt`, and answer Camel the new uid. It is also what
`get_folder_info_sync`'s NULL-versus-GError question is *not* — that one is still
the oldest undecided thing in the folder surface and still wants the EDS source
for `camel-store.c`, which this VM does not have.

Still open from earlier sessions: **bounding the cache** (unbounded on disk, and
nothing evicts); the other half of the cache's atomicity problem (an entry is
written by `write_all` and close rather than to a temporary name and renamed);
`get_trash_folder_sync` and `get_junk_folder_sync` are still a settings decision
before they are a vfunc, and that decision is what `expunge_sync` waits on.
Unexercised against a real `CamelSession`: `service.rs`, which waits on M6 and
M7. The README's architecture block still lists only the round-1 crates.

## 2026-08-09 (sixty-seventh session)

**`MailSync::import_message`: the sync half of appending a message.** The
previous session put `Email/import` into the proto, the mock and the client, and
said the Camel vfunc wanted a session of its own. This is the layer between
them, and it is the crate's rhythm — the sync-layer increment, then the vfunc —
that the folder work has followed all week.

Red first: `jmap-mail-sync/tests/import.rs`'s nine tests were written against a
`MailSync` with no such method and failed to compile, and the five new
`date::tests` cases named a `utc_date` that did not exist. Green is the two
functions below.

What landed:

- **`date::utc_date`** — [`epoch_seconds`] run backwards. The first thing this
  crate writes a date *into*: Camel keeps `date_received` as a count of seconds
  and an import has to send a `UTCDate`.
- **`MailSync::import_message(mailbox, source, keywords, received_at) -> Id`** —
  an `upload_blob` announced as `message/rfc822`, then an `Email/import` naming
  the blob, the mailbox, the row's keywords and its date. Answers the id the
  server minted, which is the Camel uid.

Decisions taken:

- **The bytes go up as bytes.** The other way to add a message is an `Email/set`
  create out of `from`, `subject` and body values, which has the *server* build
  the message — right for composing a draft, wrong for one that already exists,
  because what comes back is not what went in and a signature over it stops
  verifying. `the_bytes_that_went_up_are_the_bytes_that_come_back_down` asserts
  the round trip through `message_source`, so a future change to either end that
  rewrote the message would be a red test rather than a broken signature.
- **`receivedAt` is sent rather than left to the server.** RFC 8621 §4.8's
  default is the most recent `Received` header's date or the time of the import,
  and either would date a message copied between accounts to the moment it was
  copied — sorting it to the wrong end of the folder. Camel has the date parsed
  already, so there is nothing to guess at.
- **An instant no `UTCDate` can spell is sent as no date at all**, not as a
  clamped one and not as a refusal: what the caller asked for is that the message
  be appended, and losing the message to save its timestamp is the worse trade.
  `utc_date` answers `None` outside years 1–9999 — a four-digit year is what RFC
  8620 §1.4 allows — and `a_date_no_utc_date_can_name_is_left_to_the_server`
  pins that an `i64::MAX` still imports.
- **A refusal stays `SyncError::Client`, including a mailbox the account does not
  have.** Not the `NoSuchFolder` the folder writes answer with, and the reason is
  structural rather than taste: those name a mailbox as the record being changed
  and get a `notFound` saying so, while an import names it inside `mailboxIds`,
  where a server reports an `invalidProperties` refusal of the *message* that is
  indistinguishable from the same refusal about the blob. Reading the server's
  prose to tell them apart would be worse than passing the sentence through.
- **The answer is the id and nothing else.** RFC 8620 §5.3 lets a server return
  only the properties it set, so the rest of a summary row is not there to read;
  the row is what the next refresh builds. `append_message_sync` asks for exactly
  the uid.
- **`Keywords`, not a string list.** The set the folder layer already holds, so
  the flags word maps through one path in both directions —
  `the_keywords_a_row_carries_go_up_with_the_message` reads the result back
  through `messages`, which is the whole round trip rather than a look at the
  request.

**Not covered by a test, and the honest limits:**

1. **Still no `append_message_sync`.** Nothing in `jmap-mail` calls this yet, so
   a drag from an IMAP account into a JMAP one still fails inside Camel. No
   milestone tag is claimed.
2. **No size check before the upload.** RFC 8620 §6.1's `maxSizeUpload` is in the
   session and `jmap_proto::Session` has no accessor for it; a message over the
   limit is therefore the server's HTTP refusal rather than a local one. Cheap to
   add when something needs the better message.
3. **The import is not conditional.** `EmailImportRequest` carries `ifInState`
   and this does not send one, for the reason every other write here gives: the
   state a folder holds is its listing's, and a conditional write would fail for
   any change to any other message in the account.
4. **A duplicate import is two messages**, which is the mock's documented MAY and
   not asserted again here — the client suite already pins it.
5. **Nothing driven from Evolution**, as with every session so far: no EDS code
   changed at all this time.

Not verified locally, as in the previous sixty-six sessions: `reuse lint` and
`cargo deny` (neither binary is on this VM). One new file,
`jmap-mail-sync/tests/import.rs`, with the SPDX `GPL-3.0-or-later` header.
`cargo fmt --check`, `cargo test --locked` and `cargo clippy --all-targets
--locked -- -D warnings` are clean on the default member set (372 tests, up from
359: nine integration tests and five date unit tests) and on the five EDS crates
(531, unchanged — nothing there was touched). `example-module` fails
`clippy --workspace` with 28 `manual_c_str_literals` errors; that is on master
already, unrelated to this work, and left alone.

Next in M5: `append_message_sync` itself, which now has both halves under it —
serialize the `CamelMimeMessage` Camel hands over (`camel_data_wrapper_write_to_stream_sync`
into a `CamelStreamMem`, the mirror of what `get_message_sync` parses), read the
flags and `date_received` off the `CamelMessageInfo` when there is one, call
`import_message`, and answer the uid through `appended_uid`. The open question
it will have to settle is what the folder does with its summary afterwards: the
same "the listing is the only thing that knows what a row should say" argument
`transfer.rs` makes about the destination folder applies here too.

Still open from earlier sessions: **bounding the cache** (unbounded on disk, and
nothing evicts); the other half of the cache's atomicity problem (an entry is
written by `write_all` and close rather than to a temporary name and renamed);
`get_folder_info_sync`'s NULL-versus-GError question, which still wants the EDS
source for `camel-store.c` that this VM does not have; `get_trash_folder_sync`
and `get_junk_folder_sync` are still a settings decision before they are a vfunc,
and that decision is what `expunge_sync` waits on. Unexercised against a real
`CamelSession`: `service.rs`, which waits on M6 and M7. The README's architecture
block still lists only the round-1 crates.

## 2026-08-09 (sixty-eighth session)

**`append_message_sync`: the message that arrives from outside the account.**
The previous session put `import_message` on `MailSync` and named this as what
comes next; both halves were already under it, so this is the Camel vfunc that
joins them — and the last of the four things a user does to a message that this
provider could not do.

Red first: `jmap-mail/tests/append.rs`'s nine tests were written against a
folder class with a NULL `append_message_sync` and all nine failed with "the
append failed: no error", which is what Camel's wrapper answers for a class that
has not filled the slot in. Confirmed as red by disabling the install line again
after the vfunc existed, rather than assumed.

What landed:

- **`eds-sys`: `camel_folder_append_message_sync`** on the allowlist, so a test
  drives the vfunc through the wrapper Evolution calls rather than through the
  class.
- **`jmap-mail/src/append.rs`** — serialise, upload, import, answer the uid.
- **`JmapStore::import_message`** — the connection-locked wrapper, read-locked
  like every other write on the store.
- **`crate::append::install_vfuncs`** from the folder's `class_init`, and
  `transfer.rs`'s "that path is not this provider's yet" paragraph is now false
  and was rewritten.

Decisions taken:

- **The message is written out by Camel's own emitter**, through
  `camel_data_wrapper_write_to_output_stream_sync` on its `CamelDataWrapper`
  face. This is `crate::message`'s parse decision turned around, and the
  consequence is worse in this direction: a provider that emitted headers itself
  would be a second MIME implementation whose disagreement with the first is
  *stored*, because what goes up is what the account holds from then on.
- **A `GMemoryOutputStream`, not a `CamelStreamMem`.** The destination is a
  buffer either way and the GIO object is the one Camel's stream class wraps; it
  also keeps three more Camel classes off the FFI boundary. The stream is
  *flushed* before its buffer is read — `GMemoryOutputStream` does not buffer,
  but the writer above it is free to wrap it, and a message truncated by whatever
  a filter was still holding would be silent corruption on the server.
- **Nothing is added to the folder's summary.** The same judgement `transfer.rs`
  makes about the destination of a drag, and for the same reason: what this side
  holds is a uid, and a row built from a uid alone is a message list line with no
  subject, sender or date.
  `an_appended_message_does_not_appear_in_the_folder_until_it_is_listed` pins
  both halves — empty right after the append, one row after a refresh.
- **The message is deliberately NOT put in the cache**, although the bytes and
  the uid are both in hand. RFC 8621 §4.8 lets a server repair a message rather
  than store it verbatim, so an entry written from this side could disagree with
  the account forever *and be served in preference to it*. One download the first
  time the message is opened is the cheaper mistake.
- **`date_received == 0` is "nothing known", not 1970.** It is what a
  `CamelMessageInfo` carries when nothing dated it, and sending it as `receivedAt`
  would file every such message at the epoch for good — an `Email` is immutable.
  `None` leaves the date to the server, which RFC 8621 §4.8 defines a default
  for. Anything else passes through, negatives included.
- **A NULL `CamelMessageInfo` is a case, not a defence.** Camel declares the
  argument nullable; the message then carries no keywords, which is the answer
  that cannot put a label on it that other clients would show.
- **A uid that cannot be spelled as a C string leaves `appended_uid` unset
  rather than failing the append.** Camel's callers read NULL as "the provider
  could not say", and the message is on the server either way — reporting a
  failure would have Evolution offer to send it again.

**Found while testing, and worth recording:** `camel_folder_append_message_sync`
**connects the service before it dispatches**. The disconnected-store test
therefore came back with the *reconnection's* `URL_INVALID` (these settings name
no server) instead of the vfunc's `NOT_CONNECTED`, and now goes through the class
pointer, the way `transfer.rs`'s two equivalents already do. This was observed,
not read out of the Camel source, which this VM does not have.

**Not covered by a test, and the honest limits:**

1. **No size check before the upload.** RFC 8620 §6.1's `maxSizeUpload` is in the
   session and `jmap_proto::Session` still has no accessor for it, so an
   oversized message is the server's HTTP refusal rather than a local message
   naming the limit. Unchanged from the previous session's note.
2. **`cancellable` is still not observed**, the gap every folder vfunc here
   documents: `Client` takes its `CancelFlag` when it is built. An append uploads
   the whole message, so it is now the longest request going the *other* way.
3. **A cross-store drag has not been driven end to end.** Camel's generic
   transfer path is `get_message` on the source folder and `append_message` on
   the destination, and both ends now exist — but a test of that needs a *second*
   store of another provider, which this crate's harness does not stand up. What
   is tested is the destination half, called the way Camel calls it.
4. **Nothing driven from Evolution**, as in every session so far.

Not verified locally, as in the previous sixty-seven sessions: `reuse lint` and
`cargo deny` (neither binary is on this VM). Two new files, `jmap-mail/src/append.rs`
and `jmap-mail/tests/append.rs`, both with the SPDX `GPL-3.0-or-later` header.
`cargo fmt --check`, `cargo test --locked` and `cargo clippy --all-targets
--locked -- -D warnings` are clean on the default member set (372 tests,
unchanged — nothing there was touched) and on the five EDS crates (540, up from
531: the nine new integration tests). `example-module` fails `clippy --workspace`
with 28 `manual_c_str_literals` errors; that is on master already, unrelated, and
left alone.

No milestone tag is claimed. M5's folder surface still has `expunge_sync` waiting
on the trash/junk settings decision, and `get_folder_info_sync`'s
NULL-versus-GError question still wants the EDS source for `camel-store.c` that
this VM does not have.

Next in M5: the trash/junk pair is now the thing most other work is queued
behind — `get_trash_folder_sync` and `get_junk_folder_sync` are a settings
decision (which mailbox role, and what a JMAP account with no trash does) before
they are vfuncs, and `expunge_sync` cannot be written until that is settled,
because deleting mail in JMAP is either an `Email/set` that files the message
into trash or one that destroys it, depending on the answer.

Still open from earlier sessions: **bounding the cache** (unbounded on disk, and
nothing evicts); the other half of the cache's atomicity problem (an entry is
written by `write_all` and close rather than to a temporary name and renamed);
`get_folder_info_sync`'s NULL-versus-GError question. Unexercised against a real
`CamelSession`: `service.rs`, which waits on M6 and M7. The README's architecture
block still lists only the round-1 crates.

## 2026-08-09 (sixty-ninth session)

**Trash and junk: the two folders Camel asks for by purpose, and the two virtual
ones it stops offering.** The previous session named this as the thing most other
work was queued behind, and it called it a settings decision. It is not one:
RFC 8621 §2 puts a `role` on the mailbox, so the account itself answers which
mailbox is the trash — where IMAP has to ask the user, because `\Trash` is a
convention and `CamelIMAPXSettings` carries a `use-real-trash-path` for exactly
that reason.

Red first: eight tests in `jmap-mail/tests/folders.rs` against a store that had
never overridden the two vfuncs. Seven failed with the inherited answers, and the
eighth — the NULL-instance guard — took the test binary down with SIGSEGV inside
Camel's own vTrash construction, which is what the inherited implementation does
with a store that is not there.

What landed:

- **`FolderRole::as_jmap`** in `jmap-mail-sync`, the inverse of `from_jmap`, with
  a round-trip test. What wants it is an error message that can name the role
  Camel asked for.
- **`StoreError::NoInbox` became `StoreError::NoRole(FolderRole)`.** The variant
  was already this case with one role hard-coded into it; three roles is when
  that stops being tidy.
- **`crate::folders::open_by_role`**, and `get_inbox_folder_sync`,
  `get_trash_folder_sync`, `get_junk_folder_sync` as three names for it. The
  inbox implementation was the body of this function already.
- **`crate::store`'s `instance_init` clears `CAMEL_STORE_VTRASH` and
  `CAMEL_STORE_VJUNK`.**

Decisions taken:

- **The trash is the mailbox holding the `trash` role, and nothing else.** The
  inherited implementation answers with Camel's vTrash — a search across the
  account for messages flagged `CAMEL_MESSAGE_DELETED` — and that flag is local
  to this client: `message_info.rs`'s `FLAGS_FROM_JMAP` has said since it was
  written that JMAP has no deleted keyword. So the virtual folder holds exactly
  the messages *this* Evolution deleted and not the ones the user's phone did,
  while the account's own trash sits next to it under its own name. Junk is the
  same decision with a weaker version of the same argument, and it is worth
  writing down that it is weaker: `$junk` *is* a JMAP keyword, so a vJunk folder
  would not be empty — it would be a second spam folder disagreeing with the
  first about what spam is.
- **The virtual folders are turned off, not just outvoted.** Measured rather than
  assumed: `camel_store_get_flags` on a constructed store is `0x23` —
  `VTRASH | VJUNK | CAN_EDIT_FOLDERS`, Camel's defaults — and with the first two
  set, `camel_store_get_folder_info_sync` appends `.#evolution/Trash` and
  `.#evolution/Junk` to *every* listing the store answers with. Overriding the
  getters and leaving the flags would be an account showing the user two trash
  folders and two junk folders.
  `the_listing_offers_no_virtual_trash_or_junk_beside_the_accounts_own` pins the
  wrapper's whole answer, and `tests/manage.rs`'s flags test — which a previous
  session wrote to fail if this bit ever moved, saying it was what the trash
  pair still waited on — is updated to the new word rather than loosened.
- **Unconditionally, and not per account.** A JMAP account whose server assigns
  no `trash` role gets no trash folder rather than a virtual one. The alternative
  is a folder tree whose shape changes when a listing arrives, and a store whose
  flags say one thing before the first `Mailbox/get` and another after.
- **Neither vfunc creates the mailbox.** `Mailbox/set` from inside a getter would
  be the provider inventing a folder in the user's account on the way to
  answering a question about one.
- **A role no mailbox claims is NULL *with* an error**, although Camel documents
  the return as "NULL on error or if no such folder exists". The store knows
  which of the two it is, and all three by-purpose vfuncs now answer alike.
- **The folder is opened by path through `camel_store_get_folder_sync`**, the
  judgement `get_inbox_folder_sync` already made: the answer has to come out of
  the store's folder bag, or Evolution ends up with two `CamelFolder`s over one
  mailbox.

**Found while testing, and worth recording:** Camel's wrapper does **not** mark
the folder a store hands back — `camel_folder_get_flags` on the trash this
provider answers with is `HAS_SUMMARY_CAPABILITY` and nothing else, so
`CAMEL_FOLDER_IS_TRASH` is off. `folder.rs` guessed the opposite when it decided
not to set that bit from the role, and that comment is now corrected and the
flags word is pinned by the two role tests. Setting the bit belongs with the
increment that makes a delete *file* the message into the trash, because that is
where its consequence is observable; it is not set here.

Also noticed, unrelated and not fixed: every `Account::open()` in the tests logs
`CamelJmapStore does not implement CamelServiceClass::get_name()`. The service
has no display name of its own, which is what Evolution puts in a progress
message.

**Not covered by a test, and the honest limits:**

1. **Deleting a message still does not reach the server.** This increment says
   *which* mailbox is the trash; nothing yet moves a message into it. Evolution's
   delete sets `CAMEL_MESSAGE_DELETED` locally, `crate::synchronize` does not read
   that bit, and `expunge_sync` is still NULL. So the interim state is a trash
   folder that shows what the server put there and not what this client deleted
   — which is the same information the user had before, minus the virtual folder
   that showed the local marks. That is the next increment, and it is the one the
   IS_TRASH question above waits on too.
2. **`CAMEL_STORE_REAL_JUNK_FOLDER` is deliberately not set.** IMAPX sets it when
   the user configures a real junk path; what reads it is Evolution's own junk
   handling, which this VM cannot exercise, and setting a flag whose effect
   cannot be observed here would be a claim rather than a change.
3. **Nothing driven from Evolution**, as in every session so far. What a real
   Evolution does with an account whose trash is a server mailbox — the icon, the
   "Empty Trash" menu item, the delete key — is unverified here.

Not verified locally, as in the previous sixty-eight sessions: `reuse lint` and
`cargo deny` (neither binary is on this VM). No new files, so no new SPDX
headers. `cargo fmt --check`, `cargo test --locked` and `cargo clippy
--all-targets --locked -- -D warnings` are clean on the default member set (373
tests, up from 372: the role-name round trip) and on the five EDS crates (548, up
from 540: the eight new integration tests).

No milestone tag is claimed. M5's folder surface is now complete except
`expunge_sync`, which is the next increment rather than a blocked one — the
question it waited on is answered above.

Next in M5: **deleting mail.** `synchronize_sync` reading `CAMEL_MESSAGE_DELETED`
and filing those messages into the trash mailbox, `expunge_sync` destroying what
is already in the trash, and the `CAMEL_FOLDER_IS_TRASH` bit that tells Camel the
folder it is looking at is the one where delete means destroy.

Still open from earlier sessions: **bounding the cache** (unbounded on disk, and
nothing evicts); the other half of the cache's atomicity problem (an entry is
written by `write_all` and close rather than to a temporary name and renamed);
`get_folder_info_sync`'s NULL-versus-GError question; no size check before an
upload (`maxSizeUpload` has no accessor on `jmap_proto::Session`); `cancellable`
observed nowhere, because `Client` takes its `CancelFlag` when it is built.
Unexercised against a real `CamelSession`: `service.rs`, which waits on M6 and
M7. The README's architecture block still lists only the round-1 crates.

## 2026-08-09 (seventieth session)

**Expunging a folder: the first time `CAMEL_MESSAGE_DELETED` reaches the
server.** The previous session named this as the next increment and left the
interim state precisely: a trash folder showing what the server put there and
not what this client deleted, because Evolution's Delete key marks a summary row
with a bit JMAP has no keyword for and `synchronize_sync` deliberately produced
no keyword change for it. This is the vfunc that reads the mark.

Red first: ten tests in a new `jmap-mail/tests/expunge.rs` against a folder class
that had never filled in `expunge_sync`. Nine failed — Camel's wrapper answers
`TRUE` for a class with that slot NULL, so every one of them was a folder
reporting the user's deletions as carried out and destroying nothing. The tenth,
`synchronising_without_expunge_leaves_the_deleted_rows_alone`, passed before the
implementation and is meant to: it is the negative control for the `expunge`
argument, and a version of this work that ignored the argument entirely would
still pass it. Five more tests in a new `jmap-mail-sync/tests/expunge.rs` covered
the protocol half first.

What landed:

- **`Client::email_destroy`** — `Email/set` with a `destroy`, shaped like
  `mailbox_destroy` beside it, with `notFound` staying an `Error::Set` so the
  caller can tell "another client got there first" from "the server refused".
- **`MailSync::expunge_message(uid, mailbox)`**, which reads before it writes,
  and a private `mailboxes::out_of` for the patch it may send.
- **`JmapStore::expunge_message`**, read-locked like every other write.
- **`crate::expunge`**, the vfunc and the walk, with `expunge_folder` exposed to
  the crate so `synchronize_sync` reaches the same code.
- **`synchronize_sync` honours its `expunge` argument**, after the keyword walk
  rather than before it.
- **`camel_folder_expunge_sync` added to eds-sys's allowlist**, so the tests
  drive the wrapper Evolution calls rather than the class pointer.

Decisions taken:

- **An expunge is one of two different writes, chosen per message from a read.**
  This is the whole of the increment's difficulty and it is a mismatch between
  the two models rather than caution. Camel's vfunc asks a *folder* to get rid of
  the messages marked deleted in it; in IMAP that is unambiguous, because a
  message is in one mailbox and removing it from the mailbox is removing it. RFC
  8621 §4.6 makes `mailboxIds` a set, so the same message may be in the inbox and
  in a folder the user filed it into, and the two candidate writes say different
  things: `Email/set` **destroy** takes the message out of the account, which is
  right for a message this mailbox is the last home of and data loss for one the
  user also filed elsewhere — emptying the trash would take their copy in "Work"
  with it; `Email/set` **update** with `mailboxIds/<this>: null` takes it out of
  this mailbox only, which is right for the second case and a request any server
  keeping §4.6's invariant refuses for the first, because it would leave the
  message filed nowhere. Nothing on the Camel side can tell the cases apart: a
  summary row records the mailbox it was listed from and was never told about any
  other. So `mailboxIds` is read first — one `Email/get` of one property — and
  the write chosen from the answer. It is a round trip per message on top of the
  write, and the alternative is a provider that either loses mail or cannot empty
  a trash.
- **A message that is not in this mailbox at all is no work.** A uid is a claim
  about the last listing, so another client can have moved the message out while
  Evolution held the folder open; destroying it on the strength of where it *was*
  would be deleting mail from a stale row. Removing a member that is already
  absent would be harmless (RFC 8620 §5.3) and would also be a request that says
  nothing.
- **Membership is the member being present, whatever its value.** RFC 8621 §4.6
  gives every value in the set as `true`, so a `false` from a server that spelled
  absence out is still a mailbox naming the message — and counting it is the
  reading that cannot turn into a destroy.
- **The work list is the flag and not `get_changed`.** `synchronize_sync` walks
  `camel_folder_summary_get_changed`, which is the rows Camel has not written back
  to its *database*; a row marked deleted before the last synchronisation is not
  on it while being exactly what an expunge is for. So this walks the whole
  summary and tests the bit, which is what Camel's own providers do.
- **A message another client destroyed is not a failure**, and its row goes
  anyway — the judgement `transfer` and `synchronize` already make, and here it is
  additionally the outcome the expunge wanted.
- **The rows go now rather than at the next listing**, announced in one
  `camel_folder_changed` — `transfer`'s decision, for its reason.
- **`synchronize_sync`'s `expunge` argument is honoured, and after the keyword
  walk.** A row about to be destroyed may still carry an unsaved change of the
  user's (marking a message read and deleting it before anything synchronised is
  ordinary), and the keyword walk is what clears the marks that would otherwise be
  retried. Doing the expunge first would drop the change.

**Not covered by a test, and the honest limits:**

1. **The read and the write are not one atomic step.** A client that files this
   message into a second mailbox between the `Email/get` and the destroy loses it.
   That is the window every unconditional `Email/set` has; closing it would need a
   server-side "destroy if in no other mailbox", which JMAP does not have.
   `ifInState` does not help — the state that would matter is the message's own
   membership, and a conditional on the account's `Email` state would fail for any
   change to any other message.
2. **Nothing yet *files* a delete into the trash.** Evolution's Delete key still
   marks the row locally, and this increment destroys such a message rather than
   moving it to the account's trash mailbox. Whether that is what a real Evolution
   asks for depends on how it treats a store whose `get_trash_folder_sync` answers
   with a real folder and whose `CAMEL_STORE_VTRASH` is clear — the state the
   previous session established — and that cannot be observed on this VM. Two
   possibilities and both stay open: Evolution may move the message itself through
   `transfer_messages_to_sync` (in which case this vfunc is only ever reached in
   the trash, which is exactly right), or it may only set the bit (in which case a
   delete in the inbox followed by an expunge destroys mail the user may have
   expected to find in the trash). **Needs human verification in real Evolution.**
   Until it is answered, `CAMEL_FOLDER_IS_TRASH` is still not set from the role —
   the question `folder.rs` has carried for two sessions — because what reads it
   is the same code path that cannot be exercised here.
3. **`Email/set` destroy is per message**, so emptying a large trash is one
   `Email/get` and one `Email/set` per message. Consistent with the per-message
   decision `transfer` documents and for its reason (an answer per message is what
   the caller must report), but it is a real cost that a batched destroy could cut
   for the subset that needs no read.

Not verified locally, as in every session so far: `reuse lint` and `cargo deny`
(neither binary is on this VM). Three new files, each with the SPDX
`GPL-3.0-or-later` header. `cargo fmt --check`, `cargo test --locked` and
`cargo clippy --all-targets --locked -- -D warnings` are clean on the default
member set (378 tests, up from 373: the five new `jmap-mail-sync` ones) and on
the five EDS crates (558, up from 548: the ten new integration tests).

No milestone tag is claimed. M5's delete surface is now half done — the server
side of an expunge exists and the Evolution side of a delete is the open
question above.

Next in M5: **answering limit 2** — either from a reading of Evolution's own
source or from a human running it — and then `CAMEL_FOLDER_IS_TRASH` and, if the
answer needs it, filing a delete into the trash rather than destroying it.

Still open from earlier sessions: **bounding the cache** (unbounded on disk, and
nothing evicts); the other half of the cache's atomicity problem (an entry is
written by `write_all` and close rather than to a temporary name and renamed);
`get_folder_info_sync`'s NULL-versus-GError question; no size check before an
upload (`maxSizeUpload` has no accessor on `jmap_proto::Session`); `cancellable`
observed nowhere, because `Client` takes its `CancelFlag` when it is built; and
the store still implements no `CamelServiceClass::get_name()`, which every
`Account::open()` in the tests logs. Unexercised against a real `CamelSession`:
`service.rs`, which waits on M6 and M7. The README's architecture block still
lists only the round-1 crates.

## 2026-08-09 (seventy-first session)

**The message that is too big to send, refused before it is sent.** The next
item the previous session named — whether Evolution's Delete key files a message
into the trash or only marks the row — cannot be answered on this VM and is
still waiting on a human running real Evolution, so this session took the
tractable item beside it: the size check `append.rs` had carried as a written-out
gap since the append landed ("RFC 8620 §6.1's `maxSizeUpload` is in the session
object and nothing here reads it").

Red first, and red by not compiling: eight new tests naming
`Session::max_size_upload`, `Error::TooLarge` and the mock's `size_upload`
builder, none of which existed.

What landed:

- **`Session::max_size_upload()`** in `jmap-proto`, shaped like
  `max_objects_in_get` beside it, `None` for a server that names no limit.
- **`Error::TooLarge { size, limit }`** in `jmap-client`, and the check in
  `Client::upload_blob` that produces it before any request is made.
- **The mock advertises and enforces its own limit**: `size_upload(bytes)`,
  `no_size_upload()`, `DEFAULT_SIZE_UPLOAD` (50 MB), and a `/upload/` that
  answers an over-limit body with RFC 8620 §6.1's
  `urn:ietf:params:jmap:error:limit` naming `maxSizeUpload`.
- **`StoreError::Client(Error::TooLarge)` maps to `CAMEL_FOLDER_ERROR_INVALID`**
  rather than falling through to `CAMEL_SERVICE_ERROR_INVALID`.
- Tests: two in `jmap-proto` (the accessor and the absent property), three in
  `jmap-client` (over, exactly at, and a server naming no limit), two in a new
  `jmap-mock/tests/upload.rs`, one in `jmap-mail-sync`, one in `jmap-mail`.

Decisions taken:

- **The check is in `upload_blob`, not in the append.** An upload is the one
  request whose body is the whole message, so it is the one place where finding
  out by asking costs the user real time — minutes of progress bar over a
  domestic uplink for an answer that was already in the session document. Putting
  it in the client also covers the other caller of `upload_blob` that exists
  today (`send_email`'s attachments) and every one that comes later, rather than
  being a check `crate::append` remembered to make.
- **A server that names no `maxSizeUpload` is sent the data.** RFC 8620 §2
  requires the property, so this is a server out of spec, and the two available
  answers are to invent a limit or to send. An invented limit would refuse
  uploads the server would have taken, and would put *this crate's* number in
  front of the user as the account's. Pinned by
  `a_server_that_names_no_upload_limit_is_sent_the_message` and by the mock's
  `no_size_upload()`, which omits the property rather than sending `null` — a
  `null` would be a server naming a limit of nothing, which is a different
  server.
- **Larger than, not at least.** RFC 8620 §6.1 refuses what is *larger* than the
  limit, so a message of exactly `maxSizeUpload` goes up; asserted on both sides,
  in the client and against the mock, because an off-by-one here is a refusal the
  user cannot work around by deleting a byte.
- **`TooLarge` carries both numbers rather than a sentence.** The layer above has
  to be able to say "this account takes at most N" — and a future one may decide
  to send a large attachment as a link instead — so flattening the numbers into
  prose that has to be parsed back out would be losing them at the boundary. The
  same reason `SyncError` keeps `jmap_client::Error` whole.
- **The Camel code is the folder's, not the service's.** `CAMEL_SERVICE_ERROR` is
  what Evolution reads to decide an account is unusable, and an account with a
  size limit is not broken — the *message* is what could not be used, which is
  the judgement `append.rs`'s serialisation failure already makes with
  `CAMEL_FOLDER_ERROR_INVALID`. The sentence carries the limit; the code only has
  to not lie about whose fault it is.
- **The mock enforces the number it advertises**, the rule `objects_in_get`
  already follows: a mock that advertised a limit and took anything would let a
  client that never reads the session document pass. Tested over a raw socket in
  `jmap-mock/tests/upload.rs`, deliberately — `jmap-client` now refuses such an
  upload locally, so a test going through it could never reach that code.

**Not covered by a test, and the honest limits:**

1. **The limit is checked against the session document as it was when the client
   connected.** `Client` holds the session it fetched; a server that lowers
   `maxSizeUpload` mid-session will still refuse an upload this side thought was
   fine, and the answer then is the server's `limit` error rather than the local
   one. Re-fetching the session before every upload would cost a round trip on
   the request that least needs another one, and the failure mode it would close
   is a server changing its limits under a running client.
2. **Only `maxSizeUpload` is read.** `maxSizeRequest`, `maxCallsInRequest` and
   `maxConcurrentUpload` are all in the same capability and all still unread; a
   method call over `maxSizeRequest` is refused by the server exactly as an
   oversized upload used to be. `maxObjectsInGet` is the one other limit this
   code honours.
3. **Nothing tests the Camel error against real Evolution.** The domain and code
   are asserted here, but which of them Evolution actually branches on for a
   failed append is the same open question every error mapping in this provider
   has, and it is a *needs human verification* one.

Still open from earlier sessions, unchanged by this one: **whether Evolution's
Delete key files into the trash or only marks the row** (the previous session's
limit 2, and `CAMEL_FOLDER_IS_TRASH` behind it) — **needs human verification in
real Evolution**; bounding the cache; the cache entry written by `write_all`
rather than to a temporary name and renamed; `get_folder_info_sync`'s
NULL-versus-GError question; `cancellable` observed nowhere; no
`CamelServiceClass::get_name()`; `service.rs` unexercised against a real
`CamelSession`; and the README's architecture block still listing only the
round-1 crates.

Not verified locally, as in every session so far: `reuse lint` and `cargo deny`
(neither binary is on this VM). One new file, `jmap-mock/tests/upload.rs`, with
the SPDX `GPL-3.0-or-later` header. `cargo fmt --check`, `cargo test --locked`
and `cargo clippy --all-targets --locked -- -D warnings` are clean on the default
member set (386 tests, up from 378) and on the five EDS crates (559, up from
558).

No milestone tag is claimed; M5's open questions are the ones listed above.

Next in M5: still **answering the delete-versus-trash question** — from a reading
of Evolution's own source or from a human running it — and then
`CAMEL_FOLDER_IS_TRASH` and, if the answer needs it, filing a delete into the
trash rather than destroying it.
