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
