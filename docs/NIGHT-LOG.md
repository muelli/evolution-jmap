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
