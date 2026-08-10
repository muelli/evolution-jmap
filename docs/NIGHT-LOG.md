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

## 2026-08-09 (seventy-second session)

**The name Camel had for this account, which was NULL.** The delete-versus-trash
question the last two sessions named is still one only a human running real
Evolution can answer, so this session took another item off the standing list:
the store implemented no `CamelServiceClass::get_name()`.

That slot is not optional decoration. `camel_service_get_name` is
`g_return_val_if_fail (class->get_name != NULL, NULL)`, so every sentence Camel
writes about the account — "Cannot get folder … from store …", the progress the
user watches, the line an error dialog puts the failure on — was being written
about NULL, with a critical logged beside it.

Red first: eight tests naming `jmap_mail::service::describe`, which did not
exist, and two of them going through `camel_service_get_name` on a real
`Account`, which answered `None` until the slot was filled.

What landed:

- **`describe(host, port, user, brief)`** in `jmap-mail`'s `service.rs`: the
  whole decision as a pure function, so the naming can be tested without a
  GObject.
- **`get_name` installed** in `install_vfuncs`, over a `name_of` that reads the
  three fields off `camel_service_ref_settings` and hands back a `g_strdup` the
  caller frees.
- **`server::network()`**, the `CAMEL_IS_NETWORK_SETTINGS` check lifted out of
  `ServerConfig::from_settings` so both readers share it rather than each
  spelling it, and `take_string` made `pub(crate)` for the same reason.

The names: `JMAP server <host>` brief, `JMAP service for <user> on <host>[:port]`
full, `JMAP service on <host>[:port]` when the account names no user, and
`JMAP account` when it names no server.

Decisions taken:

- **The port is in the long form and not the short one.** Camel documents
  `brief` as a short description for the folder tree and the other as "complete
  and mostly unambiguous". IMAPX drops the port from both, but IMAP accounts
  differing only in port are exotic and JMAP ones are not — JMAP is HTTP, and a
  local server beside a test one on the same host is this repo's own daily
  setup. Two accounts Camel cannot tell apart in an error message is exactly
  what the unambiguous form exists to prevent.
- **An unconfigured account is named `JMAP account`, not `JMAP server `.** Camel
  asks for the name long before anything has configured the service — the
  settings object's properties are `G_PARAM_CONSTRUCT`, so a fresh account has a
  host of `""` — and a sentence about a server with the host left off is worse
  than a sentence that does not mention one.
- **The host as the account spells it, not as the wire does.** `name_of` reads
  `dup_host` where `ServerConfig` reads `dup_host_ensure_ascii`: nothing
  connects with this string, and an account in an internationalised domain
  should be described in the name its owner typed rather than in punycode.
  Pinned by `an_internationalised_host_is_named_as_the_account_spells_it`.
- **A panic answers with a name rather than with NULL.** This is the one vfunc
  here with no `GError` out-parameter and no failure value; the caller drops
  whatever comes back into the middle of a message. So the guard's fallback is
  the unconfigured name, and the critical it logs is where the bug is reported.
- **English and untranslated**, like the provider's own name and description.
  There is no catalogue under this module's translation domain yet, and calling
  into one that does not exist would not make the strings translated.

**Not covered by a test, and the honest limits:**

1. **Nothing here has been seen in Evolution.** That these strings read well in
   a folder tree, an error dialog and a progress bar is a judgement about a UI
   this VM cannot run — *needs human verification in real Evolution*. What is
   verified is that Camel asks and is answered, and what the answer is.
2. **`get_name` is not the account's display name.** Evolution shows the
   `ESource` display name in the folder tree; this string is what *Camel*
   substitutes into its own messages. The two can disagree, and until M6/M7
   exist there is no source to compare against.
3. **The name is read fresh on every call**, which is what makes it follow a
   reconfigured account (asserted), and also means it is read under whatever
   lock the caller holds. `camel_service_ref_settings` is a property read; no
   caller of `get_name` in Camel holds anything this re-enters.

Still open from earlier sessions, unchanged by this one: **whether Evolution's
Delete key files into the trash or only marks the row** — **needs human
verification in real Evolution**; bounding the cache; the cache entry written by
`write_all` rather than to a temporary name and renamed; `get_folder_info_sync`'s
NULL-versus-GError question; `cancellable` observed nowhere; `maxSizeRequest`,
`maxCallsInRequest` and `maxConcurrentUpload` still unread; `service.rs`
unexercised against a real `CamelSession`; and the README's architecture block
still listing only the round-1 crates.

Not verified locally, as in every session so far: `reuse lint` and `cargo deny`
(neither binary is on this VM). No new files, so no new SPDX headers.
`cargo fmt --check`, `cargo test --locked` and `cargo clippy --all-targets
--locked -- -D warnings` are clean on the default member set (386 tests) and on
the five EDS crates (567, up from 559).

No milestone tag is claimed; M5's open questions are the ones listed above.

Next in M5: still **answering the delete-versus-trash question**, and after it
either `CAMEL_FOLDER_IS_TRASH` or the cache items above, which are the tractable
ones left that need no display server.

## 2026-08-09 (seventy-third session)

**The Stop button, wired to nothing since the first folder listing.** Every
sync vfunc in the mail provider is handed a `GCancellable`, and every one of
them named it `_cancellable` and ignored it. The one place cancellation reached
was the connect, through a `CancelFlag` taken from the *authentication's*
`CancelBridge` and built into the `Client` — and that bridge is disconnected the
moment `authenticate_sync` returns. So a user stopping a refresh that was
fetching a large mailbox, or a message download, was pressing a button attached
to nothing, in a provider where those are the two longest operations there are.

Worse than a gap: a flag can be set and never unset. A cancellation arriving in
the window between `open_mail` returning and the vfunc returning latched the
flag the store's whole connection was built around — an account that refuses
every operation for the rest of the session, curable only by reconnecting.

Red first: seven tests in `jmap-client` naming a `CancelScope` that did not
exist, two in `jmap-backend-core` naming an `observe` that did not, and six in
`jmap-mail` calling vfuncs through their class pointers with an already-stopped
`GCancellable` and expecting `G_IO_ERROR_CANCELLED`.

What landed:

- **`CancelScope` in `jmap-client`'s transport**: a `CancelFlag` installed as
  the cancellation of every request *the calling thread* makes, restored to
  whatever it was when the scope drops.
- **`Client::cancel_for_request`**: the scope if the thread installed one, and
  otherwise the flag the client was built with.
- **`observe(cancellable)` in `jmap-backend-core`**: a `CancelBridge` and a
  scope in one value, which is the whole of what a vfunc has to hold.
- **Sixteen vfuncs in `jmap-mail`** now hold one for the length of their call —
  the refresh, the message fetch, the listing, the folder opens, create/delete/
  rename, transfer, synchronise, expunge, append, both subscription writes — and
  `open_mail` no longer takes a flag at all.

Decisions taken:

- **A thread, not a resettable shared flag.** The note this session inherited
  (in `jmap-backend-book`) imagined "a resettable flag shared between the client
  and a per-operation `CancelBridge`". That design is wrong for this provider and
  the reason is already written in `folder.rs`: Camel drives one store from
  several threads at once — a refresh and two message opens are three operations
  in flight. One resettable slot on the client would let one operation's Stop
  cancel another's request, and one operation's reset clear another's Stop.
  These are *blocking* vfuncs, so the operation being cancelled is the one the
  calling thread is inside, and a thread is inside exactly one at a time. A
  thread-local is that fact, not a trick.
- **The scope outranks the flag the client was built with**, rather than either
  cancelling. Precedence is what un-latches: the operation's own cancellable is
  the more specific statement, and a client-wide flag that fired once must not
  get to veto every operation afterwards. Pinned by
  `an_operation_that_was_not_cancelled_runs_under_a_client_flag_that_latched`.
- **A NULL cancellable installs nothing.** GIO's NULL means "this call cannot be
  cancelled"; installing a never-firing flag for it would *hide* the
  cancellation of the operation this one is nested inside, and vfuncs here do
  nest — `open_by_role` calls `camel_store_get_folder_sync`, which reaches
  `get_folder_sync`.
- **The connection carries no flag.** `open_mail` builds a plain `Client`: what
  stops the connect is the scope `authenticate_sync` installed, which is also
  what stops every operation after it. The latch cannot come back.
- **Tests go through the class pointer, not through Camel's wrappers.**
  `camel_folder_refresh_info_sync` and friends check the cancellable themselves
  before dispatching, so a test through the wrapper would pass whether or not
  this provider observed anything at all.

**Not covered by a test, and the honest limits:**

1. **No test cancels a request in flight.** Every test here stops the
   cancellable *before* the call, which is what EDS and Camel produce for an
   operation stopped while queued, and what `g_cancellable_connect` fires
   immediately for. Cancellation is checked between requests and by the ureq
   transport before it sends — it does not abort a socket that is already
   blocked in `read`, so a Stop during a slow download waits for that one
   response. Naming it because it is the difference between "stops soon" and
   "stops now", and closing it is a transport change (libsoup, or ureq with a
   read deadline), not this one.
2. **Six of the sixteen vfuncs are covered behaviourally**; the other ten hold
   the same single line and are covered by the mechanism's own tests. A vfunc
   added later that forgets the line is not caught by anything.
3. **Nothing here has been seen in Evolution.** That the Stop button in a real
   session reaches these cancellables is Camel's business and is not verified
   here — *needs human verification in real Evolution*.
4. **`jmap-backend-book` and `jmap-backend-cal` are untouched.** Their vfuncs
   still observe nobody, and their clients still carry a connect-time flag with
   the same latch. The mechanism they need now exists; their docs were updated
   to say so and to say what is left, which is one line per vfunc plus tests.

Still open from earlier sessions, unchanged by this one: **whether Evolution's
Delete key files into the trash or only marks the row** — **needs human
verification in real Evolution**; bounding the cache; the cache entry written by
`write_all` rather than to a temporary name and renamed; `get_folder_info_sync`'s
NULL-versus-GError question; `maxSizeRequest`, `maxCallsInRequest` and
`maxConcurrentUpload` still unread; `service.rs` unexercised against a real
`CamelSession`; and the README's architecture block still listing only the
round-1 crates.

Not verified locally, as in every session so far: `reuse lint` and `cargo deny`
(neither binary is on this VM). Two new files, `jmap-client/tests/cancellation.rs`
and `jmap-mail/tests/cancellation.rs`, both with the SPDX `GPL-3.0-or-later`
header. `cargo fmt --check`, `cargo test --locked` and `cargo clippy
--all-targets --locked -- -D warnings` are clean on the default member set (393
tests, up from 386) and on the five EDS crates (575, up from 567).

No milestone tag is claimed; M5's open questions are the ones listed above.

Next in M5: the cancellation line for the book and calendar backends' vfuncs is
now a small, tractable item; after that the cache items above, or the
delete-versus-trash question if a human has answered it.

## 2026-08-09 (seventy-fourth session)

**The same Stop button, on the address book and the calendar.** Last session
wired cancellation through the sixteen Camel mail vfuncs and left the two EDS
backends exactly where they were: every vfunc but `connect_sync` named its
`GCancellable` `_cancellable` and ignored it. A first sync of a large address
book — the longest operation either backend has, and the one a user is most
likely to stop — was a Stop button attached to nothing.

And the same latch: the only flag either client carried came from the
*connect's* `CancelBridge` and was built into the `Client`, so it belonged to
the account rather than to the operation. A connect the user managed to stop
left behind a client that refused every request for the rest of the session,
curable only by reconnecting.

Red first: twelve tests, six per backend, calling the vfuncs through their
class pointers with an already-stopped `GCancellable` and expecting
`G_IO_ERROR_CANCELLED`. Ten failed (the vfuncs cheerfully did the work); the
two NULL-cancellable tests passed from the start, which is what they are for.

What landed:

- **`with_connection` observes**, in both backends. `let _cancel =
  observe(cancellable)` sits in the one shared helper every connected vfunc
  goes through — the listing, the changes call, the load, the save and the
  remove — rather than being repeated five times.
- **`connect_with` in `jmap-backend-core` installs a scope** instead of handing
  a `CancelFlag` to the client builder, and `open_book`/`open_calendar` build a
  plain `Client`. The connection now carries no cancellation of its own.
- The two backends' `backend.rs` module docs lost their "what is not wired up
  yet" sections, which were the note this session was working from.

Decisions taken:

- **The `observe` goes in `with_connection`, not at the top of each vfunc.**
  The mail provider repeats the line sixteen times because its vfuncs have no
  shared entry point; these do. Putting it there makes reaching the connection
  and being cancellable the same act, so a vfunc added later cannot get the
  first without the second — which is exactly the gap the mail crate still has
  (its note names it: "a vfunc added later that forgets the line is not caught
  by anything").
- **`disconnect_sync` deliberately does not observe.** It makes no request; it
  drops the connection. Refusing it because the user pressed Stop would leave
  the backend holding a socket EDS believes is closed, and dropping it is what
  the caller asked for either way. It is the one vfunc that does not go through
  `with_connection`, and the helper's doc comment says why.
- **The offline check stays after the observe**, so an operation with no
  connection *and* a stopped cancellable still reports
  `E_CLIENT_ERROR_REPOSITORY_OFFLINE`. That is the code that makes
  `EBookMetaBackend`/`ECalMetaBackend` serve their cache; reporting CANCELLED
  for a backend that is merely disconnected would be a worse answer to the more
  important question.
- **The client carries no flag, as in the mail provider.** Same reasoning:
  precedence alone would not un-latch it, because `Client::cancel_for_request`
  falls back to the built-in flag whenever the vfunc installed nothing — which
  is every NULL cancellable, and EDS passes those.

**Not covered by a test, and the honest limits:**

1. **The latch removal is not pinned by a red test, and cannot easily be.** The
   bridge is disconnected when `connect_with` returns, so cancelling the
   cancellable afterwards does nothing either way; the only way to latch the
   flag is to cancel *during* the connect, which is a race the mock server
   offers no hook for. What pins it now is a signature: `open_book` and
   `open_calendar` no longer take a `CancelFlag`, so there is nothing to latch.
   Saying so plainly rather than writing a test that would pass before the fix.
2. **No test cancels a request in flight**, as in the mail provider. Every test
   here stops the cancellable before the call — which is what EDS produces for
   an operation stopped while queued, and what `g_cancellable_connect` fires
   immediately for. A Stop during a slow response still waits for that
   response; closing that is a transport change, not this one.
3. **Nothing here has been seen in Evolution.** That EDS's Stop reaches these
   cancellables is EDS's business — *needs human verification in real
   Evolution*.

Not verified locally, as in every session so far: `reuse lint` and `cargo deny`
(neither binary is on this VM). Two new files,
`jmap-backend-book/tests/cancellation.rs` and
`jmap-backend-cal/tests/cancellation.rs`, both with the SPDX `GPL-3.0-or-later`
header. `cargo fmt --check`, `cargo test --locked` and `cargo clippy
--all-targets --locked -- -D warnings` are clean on the default member set (393
tests, unchanged) and on the five EDS crates (587, up from 575).

No milestone tag is claimed. `example-module` still fails clippy on
`manual_c_str_literals` — pre-existing, untouched by this session, and outside
both sets the rules require green.

Still open from earlier sessions, unchanged by this one: **whether Evolution's
Delete key files into the trash or only marks the row** — **needs human
verification in real Evolution**; bounding the mail cache; the cache entry
written by `write_all` rather than to a temporary name and renamed;
`get_folder_info_sync`'s NULL-versus-GError question; `maxSizeRequest`,
`maxCallsInRequest` and `maxConcurrentUpload` still unread; `service.rs`
unexercised against a real `CamelSession`; and the README's architecture block
still listing only the round-1 crates.

Next in M5: the mail cache items, or the delete-versus-trash question if a
human has answered it.

## 2026-08-09 (seventy-fifth session)

**The mail cache stops growing.** `cache.rs` had carried a "what is not here
yet" note since it was written: nothing removed an entry that was merely old, so
an account's message cache grew by every message the user had ever opened and
stopped growing when the disk was full. The note called the missing piece a
settings question rather than a mechanism one, and that turned out to be the
useful half of the work.

Red first: two tests. One stores two messages, opens a second cache over the
same directory with a bound of nothing, opens one of the two — and expects the
*other* to be gone. One does the same with the default bound and expects it
still to be there, which is the test that separates a bound from
`camel_data_cache_clear`. The first failed (the entry was still on disk); it was
re-checked against a build with `expire_enabled` forced to FALSE afterwards, so
what it pins is the mechanism rather than the API.

What landed:

- **`camel_data_cache_set_expire_access`**, set in `MessageCache::open` along
  with an explicit `set_expire_enabled(TRUE)`. Thirty days.
- **`MessageCache::open_bounded(directory, Duration)`** — the mechanism takes
  the bound, `open` supplies the policy constant.
- The stale "what is not here yet" note replaced by what the bound is and what
  it is not, and the two `no bound of its own` asides elsewhere in the file
  corrected.

Decisions taken:

- **atime, not mtime — `set_expire_access` rather than `set_expire_age`.** An
  `Email` is immutable (RFC 8621 §4.1), so a cache entry's mtime is the moment
  it was downloaded and never changes again; a bound on it would drop the
  message the user reads every week on the same schedule as the one they read
  once. atime is the weaker signal — `relatime` updates it once a day,
  `noatime` never — but both fail in the conservative direction against a bound
  of a month: on `noatime` the atime stays at the write, so the access bound
  quietly degrades into the age bound, which is the one we would otherwise have
  chosen.
- **A constant, not a setting, and that is the settings question answered.**
  Camel has nowhere to put this: `CamelOfflineSettings`'s `limit-by-age` /
  `limit-unit` / `limit-value` govern which messages are *downloaded* for
  offline use, not how long a downloaded one is kept, and reading it as the
  latter would silently make an account's offline window double as its cache's.
  A knob of our own is a field in an account editor, which is M7.
- **Thirty days**, on the ground that when the bound is wrong it costs exactly
  one `Email/get` and one blob download for a message the user came back to —
  which is what every open cost before the cache existed, so the bound's failure
  mode is the behaviour it replaced.
- **The test drives Camel's sweep rather than forging a file's atime.** Camel
  expires lazily: a bucket is swept when a lookup lands in it, at most once an
  hour per `CamelDataCache` instance, and the key being looked up is skipped. So
  the test needs a *second* key in the same one of the sixty-four buckets, and a
  *fresh* cache instance to do the sweeping (the first one's hourly slot is
  already spent by the second `store`). `M1` and `M2` share a bucket; that is a
  fact about `g_str_hash`, not a promise, so `share_a_bucket` asserts it and a
  Camel that rehashed would fail the tests with a sentence instead of quietly
  making them prove nothing.

**Not covered by a test, and the honest limits:**

1. **This is not a quota.** A cache is only ever as small as its bound makes it,
   not as small as a number of megabytes, and an account nobody opens is an
   account nothing is swept from — Camel sweeps on lookup. A real size cap would
   be ours to write over `camel_data_cache_foreach_remove`, and the number it
   enforced is the question this bound was chosen to avoid needing answered.
2. **Thirty days is not verified against anything.** No user has said it is
   right; it is a defensible first number with its cost written down.
3. **Nothing here has been seen in Evolution** — *needs human verification in
   real Evolution*, like everything else on this surface.

Found while reading Camel's source, and worth recording: EDS **3.62** grew
`camel_data_cache_add_atomic` / `commit_atomic` / `discard_atomic`, which write
an entry under a temporary name and rename it into place. That is exactly the
open item this log has carried as "the cache entry written by `write_all` rather
than to a temporary name and renamed" — so the answer is a version bump rather
than code of ours, and 3.52 (what this builds against) has neither call. The
size check stays the answer here. Noted in the module docs.

Not verified locally, as in every session so far: `reuse lint` and `cargo deny`
(neither binary is on this VM). No new files, so no new SPDX headers. `cargo fmt
--check`, `cargo test --locked` and `cargo clippy --all-targets --locked -- -D
warnings` are clean on the default member set (393 tests, unchanged) and on the
five EDS crates (589, up from 587).

No milestone tag is claimed.

Still open from earlier sessions, unchanged by this one: **whether Evolution's
Delete key files into the trash or only marks the row** — **needs human
verification in real Evolution**; `get_folder_info_sync`'s NULL-versus-GError
question; `maxSizeRequest`, `maxCallsInRequest` and `maxConcurrentUpload` still
unread; `service.rs` unexercised against a real `CamelSession`; and the README's
architecture block still listing only the round-1 crates. The atomic-write item
is now answered rather than open (see above), and the cache bound is done.

Next in M5: `maxCallsInRequest` is the smallest remaining protocol item — the
client builds two-call requests in `search_emails` and `send_email`, and a
server that advertised a limit of one would fail both. After that,
`get_folder_info_sync`'s NULL-versus-GError question, or the README.

## 2026-08-09 (seventy-sixth session)

**The two-call requests nobody had asked the server about.** The client chains
two method calls into one request in exactly two places — `Email/query` +
`Email/get` through a `#ids` back-reference, and `Email/set` +
`EmailSubmission/set` through a `#draft` creation reference — and neither had
ever read `maxCallsInRequest`. RFC 8620 §3.2 refuses an over-long request
*whole*, with `urn:ietf:params:jmap:error:limit`: a server that takes one call
at a time would have answered neither the read nor the send, so the round trip
the chain saves would have cost the user the mail.

Red first, in three layers:

- `evolution-jmap-proto`: two tests for a `Session::max_calls_in_request()` that
  did not exist — `Some(32)` off the RFC fixture, `None` for a session with the
  property stripped.
- `evolution-jmap-mock`: `calls_in_request(n)` / `no_calls_in_request()` on the
  builder, advertised **and enforced**, exactly as `objects_in_get` and
  `size_upload` already are. A test pins the enforcement itself (a two-call
  `Core/echo` request to a one-call server is a 400 `…:error:limit` and neither
  call runs) so the client tests below cannot pass against a permissive mock.
- `evolution-jmap-client`: four tests over the two chains, two of which failed
  with precisely that 400.

What landed:

- **`Session::max_calls_in_request()`**, the third accessor of its shape next to
  `max_objects_in_get` and `max_size_upload`, and `None` for a silent server for
  the same reason as those two.
- **`Client::takes_calls_in_one_request(calls)`** — asked *before* a chain is
  built, not after a refusal.
- **`email_query_then_get`** falls back to a query followed by an `Email/get`
  naming the ids it answered; **`send_email`** falls back to creating the draft
  and then submitting it by its real id. Both keep the chained form as the
  default. The `/get` order-restoration is now `in_query_order`, shared by both
  paths, and the submission request is now built by one `submission_request`
  helper so the two forms differ in one argument rather than in a copied struct.
- **`ServerState::api_requests`** / `MockServer::api_requests()` — a round-trip
  counter. `method_calls` counts calls, which is the same for both paths; only
  this can tell one request carrying two calls from two carrying one each, and
  it is what stops "always split" from passing the tests.

Decisions taken:

- **Split, rather than fail with a good error.** A `TooLarge`-style refusal was
  the other option and is what `upload_blob` does — but an upload that is too
  big cannot be made smaller by the client, whereas a request that is too long
  can always be sent as several. Refusing would have made a whole class of
  server unable to read or send mail over a limit that costs one round trip to
  respect.
- **The chain stays the default, and not only for speed.** Split, there is a
  window between the two requests: a message the query found may be destroyed
  before the `/get`, and it comes back one short rather than as an error; a
  draft exists alone in Drafts before its submission. Written into both doc
  comments rather than left to be rediscovered.
- **`#submission` stays a creation reference in the split path.**
  `onSuccessUpdateEmail` is keyed by the *submission*, which the second request
  creates itself — only the `emailId` had to become a real id. The test asserts
  the draft still lands in Sent with `$seen`, which is the part that would have
  gone quietly missing.
- **An empty query costs one request, not two.** The chained form's `/get`
  travels with the query whether or not anything matched; the split form would
  otherwise spend a whole round trip fetching nothing.

**Not covered by a test, and the honest limits:**

1. **No real server was asked.** Everything here is against the mock, which now
   enforces the limit it advertises — but a real server's refusal wording, and
   whether it counts calls the way this does, is unverified. Stalwart is the
   place to check that (the integration track), not this VM.
2. **The split `Email/get` can still be too long.** It names every id the query
   answered, so a query returning more than `maxObjectsInGet` fails there. That
   is true of the chained form too — the server resolves the back-reference to
   the same ids — so this change neither introduces nor fixes it; `jmap-mail-sync`
   is where that limit is already respected, by chunking. Worth doing in the
   client one day.
3. **A server naming a limit of 0** is refused everything, split or not. Out of
   spec, and nothing here pretends to rescue it.
4. **Nothing here has been seen in Evolution** — *needs human verification in
   real Evolution*, like the rest of this surface.

Not verified locally, as in every session so far: `reuse lint` and `cargo deny`
(neither binary is on this VM). One new file,
`jmap-client/tests/call_limits.rs`, with the SPDX `GPL-3.0-or-later` header.
`cargo fmt --check`, `cargo test --locked` and `cargo clippy --all-targets
--locked -- -D warnings` are clean on the default member set (402 tests, up from
393) and on the five EDS crates (589, unchanged — the mail provider goes through
the client, so it inherits this without a line of its own changing).

No milestone tag is claimed.

Still open from earlier sessions, unchanged by this one: **whether Evolution's
Delete key files into the trash or only marks the row** — **needs human
verification in real Evolution**; `get_folder_info_sync`'s NULL-versus-GError
question; `service.rs` unexercised against a real `CamelSession`; and the
README's architecture block still listing only the round-1 crates.
`maxCallsInRequest` is now read; `maxSizeRequest` and `maxConcurrentUpload`
still are not.

Next in M5: `maxSizeRequest` is the sibling of what was done here and the harder
one — it bounds the *bytes* of a request, which for `Email/import` and a large
`Email/set` is a number the client would have to measure before sending rather
than count. Otherwise `get_folder_info_sync`'s NULL-versus-GError question, or
the README.

## 2026-08-09 (seventy-seventh session)

**The octets nobody had counted.** `maxCallsInRequest` was read last session;
`maxSizeRequest` — its sibling, counting octets where that one counts calls —
was not, and the mock advertised a hardcoded 10 MB it never enforced. RFC 8620
§2 refuses an over-long request on its *bytes*, before it is a request at all,
with `urn:ietf:params:jmap:error:limit`. The one call this client builds whose
length is the user's mailbox rather than the client's choice is `Email/get`
naming a list of ids: a folder of ten thousand messages is a list of ten
thousand ids, and over the limit the client got none of them.

Red first, in three layers, the same three as last session:

- `evolution-jmap-proto`: two tests for a `Session::max_size_request()` that did
  not exist — `Some(10_000_000)` off the RFC fixture, `None` for a session with
  the property stripped.
- `evolution-jmap-mock`: `size_request(n)` / `no_size_request()` on the builder,
  advertised **and enforced** — the hardcoded `maxSizeRequest` in the session
  document is gone, replaced by the configured one. Enforcement happens on
  `body.len()` before `serde_json::from_slice`, because a server counting octets
  has not parsed anything yet and cannot have run any of the calls inside. A
  test pins that (a 400 `…:error:limit` with `"limit": "maxSizeRequest"`, and
  `Core/echo` never runs), sent through `UreqTransport` rather than the client,
  since the client now refuses such a request itself and a client-side assertion
  there would have been about the client twice.
- `evolution-jmap-client`: four tests over `email_get`, two of which failed.

What landed:

- **`Session::max_size_request()`**, the fourth accessor of its shape, `None`
  for a silent server for the same reason as the other three.
- **`Client::api_call` refuses an oversized request without sending it**, with a
  new `Error::RequestTooLarge { size, limit }`. This is the backstop under every
  caller that builds its own envelope.
- **`Client::email_get` splits a long id list across several requests.** Each
  chunk gets its call id first, then the request naming *no* ids is serialized
  once to measure everything whose length is not the id list; a JSON array grows
  by exactly each element's serialized length plus one comma between them, so
  that single measurement places every boundary, and places it on the same count
  the server will — the bytes measured are the bytes `api_call` sends. No
  estimate, no slack constant.
- `email_query_then_get`'s split path and `jmap-mail-sync`'s chunked catch-up
  both go through `email_get`, so both inherit this without a line of their own
  changing.
- **`ServerState::api_requests` is now incremented at the top of `handle_api`**,
  before the body is parsed rather than after. A request refused on its size is
  still a round trip the client spent, which is exactly what a test asserting
  "nothing was sent" needs to be able to see. The comment that already claimed
  this is now true of the malformed-body path too.

Decisions taken:

- **Split, rather than fail with a good error** — the same call as last
  session's, and for the same reason: an upload that is too big cannot be made
  smaller by the client, but a request that is too long can nearly always be
  sent as several. `Error::RequestTooLarge` is what is left when it cannot: a
  single id so long that a call naming only it is still over the limit. A call
  naming one id cannot be made into two, and that is where splitting ends.
- **`RequestTooLarge` is its own variant, not `TooLarge`.** The two differ in
  what the caller can do — and in `jmap-mail` they differ in the GError: the
  upload one is a `CAMEL_FOLDER_ERROR_INVALID` because one message is what could
  not be used, while this one reaches the wildcard and is reported as a service
  error, because a server handing out ids too long for the request size it
  itself named is the account being inconsistent. Written into the comment at
  that arm rather than left to be rediscovered.
- **The whole list stays the default.** Splitting is only what the limit forces:
  between two requests another client may destroy a message the first named, and
  it comes back one short rather than as an error. A server naming no limit is
  sent the list whole, like the other three limits.
- **`maxObjectsInGet` and `maxSizeRequest` do not imply each other**, so the
  count chunking in `jmap-mail-sync` is not made redundant by this and was left
  alone: ids may be up to 255 characters (RFC 8620 §1.2), so a list well inside
  one limit can be well outside the other.

**Not covered by a test, and the honest limits:**

1. **No real server was asked.** The mock now enforces what it advertises, but
   whether a real server counts the same octets — body only, as RFC 8620 §2
   says, and not headers or a decompressed length — is unverified. A server that
   counts more than the body will still refuse a request this client measured as
   fitting. Stalwart is where to check that, not this VM.
2. **Only `Email/get` splits.** Every other call this client builds is a fixed
   shape whose length does not grow with the user's data, so `api_call`'s
   refusal is the whole answer for them — but `Email/set` with many `update`
   entries would be the next one to grow, and nothing here splits it. It is not
   built that way today.
3. **The test's limit is 220 octets**, chosen because an `Email/get` naming no
   ids is 150 here. That is a measured number written into a comment, not a
   number a server would use, and a change to the capability list would move it.
   The test asserts the split happened rather than how many requests it took, so
   it fails loudly rather than silently stopping exercising the split.
4. **Nothing here has been seen in Evolution** — *needs human verification in
   real Evolution*, like the rest of this surface.

Not verified locally, as in every session so far: `reuse lint` and `cargo deny`
(neither binary is on this VM). One new file,
`jmap-client/tests/request_size.rs`, with the SPDX `GPL-3.0-or-later` header.
`cargo fmt --check`, `cargo test --locked` and `cargo clippy --all-targets
--locked -- -D warnings` are clean on the default member set (410 tests, up from
402) and on the five EDS crates (589, unchanged — the mail provider reads mail
through the client, so it inherits the splitting without a line of its own
changing).

No milestone tag is claimed.

Still open from earlier sessions, unchanged by this one: **whether Evolution's
Delete key files into the trash or only marks the row** — **needs human
verification in real Evolution**; `get_folder_info_sync`'s NULL-versus-GError
question; `service.rs` unexercised against a real `CamelSession`; and the
README's architecture block still listing only the round-1 crates.
`maxConcurrentUpload` is the last core limit still unread — and, unlike the
three now read, it is about concurrency rather than about a number to compare
against, so it means nothing to a blocking client that makes one request at a
time. Worth writing down as answered-by-design rather than carrying as open.

Next in M5: `get_folder_info_sync`'s NULL-versus-GError question, or the README's
architecture block. The core limits are done.

## 2026-08-09 (seventy-eighth session)

**The folders Send / Receive never checked.** `CamelStoreClass` has one
non-blocking slot, `can_refresh_folder` — "Returns if this folder (param info)
should be checked for new mail or not" — and this provider had never filled it.
The inherited answer is one line, `(info->flags & CAMEL_FOLDER_TYPE_MASK) ==
CAMEL_FOLDER_TYPE_INBOX`: the inbox and nothing else. Every provider in the
tree overrides it — IMAPX, EWS, local, POP3, NNTP, Evolution's own RSS store —
and this one did not, so a JMAP account's Send / Receive checked the inbox and
left every other folder's counts stale until the user clicked it. On JMAP that
is worse than on IMAP, because server-side filing is ordinary: mail the user
cares about often never touches the inbox at all.

Red first, three tests through the wrapper Evolution calls:

- `send_receive_checks_the_inbox_and_the_folders_the_user_ticked` — a listing
  with all four cases in it (inbox; a ticked folder; an unticked one; an
  unticked folder kept in the answer only because a ticked one sits below it),
  walked the way `get_folders` in Evolution's `mail-send-recv.c` walks one,
  asking `camel_store_can_refresh_folder` about each info. Failed with
  `["Inbox"]` against `["Inbox", "Lists", "Work/Invoices"]`.
- `the_inbox_is_checked_even_when_the_user_unticked_it` — the half of the
  inherited rule that is kept rather than replaced.
- `the_inherited_answer_checks_nothing_but_the_inbox` — the same listing put
  through `CamelOfflineStoreClass`'s inherited slot, so a Camel that widened
  its own default fails here with a sentence instead of leaving this provider
  carrying an override nobody needs.

What landed: `folders::refreshable(flags)` — the inbox, plus every folder
carrying `CAMEL_FOLDER_SUBSCRIBED` — and the `can_refresh_folder` trampoline
that installs it, in `folders::install_vfuncs` beside the other five.

Decisions taken:

- **Subscription is the line, and no setting is added.** RFC 8621 §2 defines
  `isSubscribed` as "has the user indicated they wish to see this Mailbox" —
  the same user and the same intent that "check this folder for new mail" is
  about. IMAPX needs `check-all`/`check-subscribed` settings because `LIST` and
  `LSUB` are separate round trips and an IMAP account's subscriptions are often
  nobody's idea of the folders worth checking; `Mailbox/get` returns every
  mailbox with its `isSubscribed` in the one call the listing already made, so
  the answer is in hand and costs nothing. A "check all folders regardless"
  setting is a property to add later, not a reason to guess at one now.
- **An unticked folder kept only for a ticked child answers no.** It is a
  folder the user chose not to see; the ticked folder below it is asked about
  separately and answers yes. This is the same asymmetry `folders::ticked`
  already documents, now visible in a second place.
- **`CAMEL_FOLDER_NOSELECT` is deliberately not tested for**, unlike in EWS.
  This store never sets it — `ticked` explains why an unticked ancestor is
  still a selectable mailbox — and Evolution's `get_folders` filters `NOSELECT`
  itself in the walk that asks the question, so a test here would be a second
  answer to one the caller has already given.
- **The vfunc reads neither the store nor the network.** The slot is documented
  as non-blocking and Camel asks it once per folder while walking a forest it
  already holds; a provider that reached for account state here would turn one
  Send / Receive into a round trip per folder.

**The long-standing `get_folder_info_sync` NULL-versus-GError question is now
answered, and the answer is "NULL with no error is right".** This log has
carried it since the sixty-third session as needing the EDS source this VM does
not have; the source was fetched from GNOME GitLab at tag 3.52.3, which is the
`camel-1.2` this VM has installed. Three pieces of evidence, all primary:

1. `camel_store_get_folder_info_sync` **exempts `SUBSCRIBED` from the check**:
   `if ((flags & CAMEL_STORE_FOLDER_INFO_SUBSCRIBED) == 0) CAMEL_CHECK_GERROR
   (...)`. Camel itself treats a subscription-filtered listing that comes back
   empty as legitimate.
2. IMAPX, the reference provider, does exactly what this one does.
   `get_folder_info_offline` ends in `camel_folder_info_build (folders, top,
   '/', TRUE)`, and `camel_folder_info_build` returns NULL — with no error —
   for an empty array. A `top` naming nothing is NULL-and-no-error there too.
3. The warning this repo saw came from `store_rename_folder_thread`, which uses
   `CAMEL_CHECK_LOCAL_GERROR` **without** the `SUBSCRIBED` exemption its
   sibling has. That is an inconsistency in Camel, and the consequence —
   `camel_store_folder_renamed` not being emitted for a subtree with nothing
   subscribed in it — is already pinned by
   `a_rename_of_a_subtree_nothing_is_subscribed_to_is_announced_by_no_one`.

So the console warning is a diagnostic false positive that IMAPX trips too, and
nothing changes in this provider. Also checked while the source was in hand:
`camel_store_get_folder_info_sync`'s vTrash/vJunk relist branch is dead for this
store, which clears `CAMEL_STORE_VTRASH | CAMEL_STORE_VJUNK` in `instance_init`.
The item is struck from the open list.

**Not covered by a test, and the honest limits:**

1. **Nothing here has been seen in Evolution.** That Send / Receive now visits
   the subscribed folders, and that their counts update in the folder tree as a
   result, is read from `mail-send-recv.c` at 3.52.3 — `get_folders` is its one
   caller, and `mail-folder-cache.c` lists with `FAST | RECURSIVE | SUBSCRIBED`
   before it — not observed. *Needs human verification in real Evolution.*
2. **The cost is not measured.** An account with a hundred subscribed folders
   now gets a hundred folder refreshes per Send / Receive where it got one.
   That is the behaviour every other provider has with `check-all` set, and it
   is the point of the change, but no one has run it against an account that
   size — nor against a real server, where each refresh is an `Email/query`.
3. **Only 3.52.3 was read.** The rule is a test on a flags word, so it is not
   version-fragile in the way a struct layout is, but `store_can_refresh_folder`
   changing in a later EDS is exactly what
   `the_inherited_answer_checks_nothing_but_the_inbox` exists to catch.

Not verified locally, as in every session so far: `reuse lint` and `cargo deny`
(neither binary is on this VM). No new files, so no new SPDX headers.
`cargo fmt --check`, `cargo test --locked` and `cargo clippy --all-targets
--locked -- -D warnings` are clean on the default member set (410 tests,
unchanged — nothing there was touched) and on the five EDS crates (592, up from
589).

No milestone tag is claimed.

Still open from earlier sessions, unchanged by this one: **whether Evolution's
Delete key files into the trash or only marks the row** — **needs human
verification in real Evolution**; `service.rs` unexercised against a real
`CamelSession`; and the README's architecture block still listing only the
round-1 crates. `get_folder_info_sync`'s NULL-versus-GError question is answered
above and no longer open.

Next in M5: the README's architecture block is the last item this log is
carrying that can be done here. Beyond it, the remaining `CamelStoreClass` slots
are worth a look now that the source is available — `initial_setup_sync`
(EWS uses it to write folder ids into the ESource at first connect, which is
M6/M7 territory), `synchronize_sync`, and `get_can_auto_save_changes`.

## 2026-08-09 (seventy-ninth session)

**The half of M5 that leaves the account.** The roadmap names three objects for
M5 — `CamelJmapStore`, `CamelJmapFolder`, `CamelJmapTransport` — and the third
does not exist: `provider.rs` leaves `CAMEL_PROVIDER_TRANSPORT` at
`G_TYPE_INVALID`, with a comment saying naming a type there before there is one
is a crash the first time a user hits Send. This session did the protocol half
of that transport, at the layer where it can be tested against `jmap-mockd`:
`MailSync::send_message`, so that the Camel object a later session registers has
something to call.

Red first, seven tests in `jmap-mail-sync/tests/send.rs`, all failing to compile
against a `send_message` that did not exist. Two of them were then re-checked
against a deliberately broken implementation, because a compile failure is a weak
red: with the `onSuccessUpdateEmail` patch dropped the two filing tests fail on
"still a draft after being sent", and with the envelope dropped the envelope test
fails with the header address in place of the envelope one.

What landed:

- `jmap-client`: `Client::submit_email` — `EmailSubmission/set` for a message the
  account already holds, naming it by id rather than by a `#draft` creation
  reference. `submission_request` gained an `envelope` parameter; the two
  existing callers inside `send_email` pass `None`, which is what they were
  sending before.
- `jmap-mail-sync`: `send::Outgoing` (source bytes, identity, envelope, staging
  mailbox, optional destination) and `MailSync::send_message`, which imports the
  message into the staging mailbox as a `$draft` and submits it with the patch
  that files it where sent mail is kept.

Decisions taken:

- **Bytes, not properties.** Sending goes through `Email/import` over an
  uploaded blob rather than the `Email/set` create `Client::send_email` uses.
  What Evolution's composer hands a transport is a finished MIME document, and a
  client that took it apart into JMAP properties for the server to write out
  again would send a different message than the one it was given — one whose
  signature no longer verifies. That is `import_message`'s judgement, made again
  for the message the user just wrote, and it is why `send_email` could not
  simply be reused.
- **The envelope is a field of its own.** RFC 8621 §7 lets the server derive an
  envelope from the message's headers, which is right for a message the client
  composed and wrong for one it was handed: a `Bcc` recipient is a recipient with
  no header. Camel hands `send_to_sync` the recipients as their own argument, so
  they travel as their own field the whole way down, and the test that pins this
  uses addresses that appear in no header of the message.
- **Staged, then filed, in the server's own transaction.** The message is
  imported into Drafts and moved to Sent by `onSuccessUpdateEmail` (RFC 8621
  §7.5) rather than by a second `Email/set` of ours. A message imported straight
  into Sent would be one sitting in Sent that may never go out; a move made by a
  follow-up request would be a message that has gone out and still claims to be
  an unsent draft if the client dies in between.
- **A refused submission leaves the draft behind, deliberately.** The import
  succeeded, so the user's message exists, unsent, in the mailbox unsent messages
  live in — which is where they would look for it. Destroying it to keep the
  account tidy would throw away work on behalf of a server that said no. Pinned
  by `an_identity_the_account_does_not_have_sends_nothing`, which asserts the
  draft is still there and still a draft.
- **`$seen` is set at import, not in the patch.** The sender has read every word
  of their own message, and setting it at import means the message left behind by
  a refusal is not also an unread one. `$draft` is the only keyword the patch
  clears.
- **Not chained into one request**, although RFC 8620 §5.3 would let the
  submission name the import's creation id. The upload has to happen first
  either way — `Email/import` takes a blob id — so the chain saves one round trip
  of three, and buys it by making "the account would not take this message" and
  "this message did not go out" indistinguishable to the caller. Those two are
  exactly what the user needs told apart.
- **`destination` is optional and no mailbox is invented for it.** An account
  with no Sent role, or one where Evolution saves its own copy, passes `None` and
  the message stays where it was staged — still not a draft, because it has been
  sent. Which mailbox is Drafts and which is Sent is the caller's lookup, out of
  the folder tree it already holds; `send_message` making that lookup would be a
  `Mailbox/get` per send.

**Not covered by a test, and the honest limits:**

1. **There is still no `CamelJmapTransport`,** and the provider's transport slot
   is still `G_TYPE_INVALID` — so Evolution cannot send through a JMAP account
   yet. Nothing in this session changes what the user sees. What it changes is
   that the next session's Camel object has a tested call to make.
2. **Nothing here has been seen against a real server.** The mock accepts a
   submission by recording it in an outbox; it has no MTA, so "the mail went out"
   is untested by construction, and so is a server that refuses a `From` that
   disagrees with the identity — RFC 8621 §7 requires that check and the mock
   does not make it.
3. **Which identity to submit through is not decided anywhere yet.** `Outgoing`
   takes one; nothing picks it. `Client::identities` exists and is unused. That
   is the transport's job — Evolution knows the account's configured address —
   and it is the first thing the next session has to answer.
4. **`sendAt`, `undoStatus` and delayed sending are untouched.** The submission
   is created with neither, which is immediate send.

Not verified locally, as in every session so far: `reuse lint` and `cargo deny`
(neither binary is on this VM). Two new files, `jmap-mail-sync/src/send.rs` and
`jmap-mail-sync/tests/send.rs`, both with the SPDX `GPL-3.0-or-later` header.
`cargo fmt --check`, `cargo test --locked` and `cargo clippy --all-targets
--locked -- -D warnings` are clean on the default member set (417 tests, up from
410) and on the five EDS crates (592, unchanged — nothing there was touched).

No milestone tag is claimed.

Still open from earlier sessions, unchanged by this one: **whether Evolution's
Delete key files into the trash or only marks the row** — **needs human
verification in real Evolution**; `service.rs` unexercised against a real
`CamelSession`; and the README's architecture block still listing only the
round-1 crates.

Next in M5: `CamelJmapTransport` itself — the `CamelTransport` subclass, its
`send_to_sync` turning Camel's `CamelAddress` arguments into the envelope above,
the identity lookup, and the provider's transport slot. It is the largest single
object left in M5 and wants a session of its own.

## 2026-08-09 (eightieth session)

**The two arguments that are not in the message.** Last session left
`CamelJmapTransport` as the next thing and called it a session of its own. It is
still that; this session took the first piece of it that can be finished and
tested to the end here — `jmap-mail/src/envelope.rs`, the SMTP envelope read out
of the two `CamelAddress` arguments `CamelTransportClass::send_to_sync` is
handed, in the shape `Outgoing::envelope` already takes.

It is a module rather than a few lines inside a vfunc that does not exist yet
because it is the one part of sending that is a pure reading of Camel's objects:
no connection, no request, no GObject of ours, and therefore ten tests that run
against real `CamelInternetAddress` objects and assert on the whole answer.

Red first: `jmap-mail/tests/envelope.rs`, failing to compile against a module
that did not exist. Because a compile failure is a weak red, two of the tests
were then re-checked against a deliberately broken implementation — with the
unusable-recipient refusal turned into a skip,
`a_recipient_with_no_address_is_refused_rather_than_dropped` fails with a
two-recipient envelope where a three-recipient list went in; with the
`CamelInternetAddress` type check dropped,
`an_address_that_is_not_an_internet_address_is_refused` fails with `NoSender` in
place of `NotInternet("sender")`.

Decisions taken:

- **Refuse, never shorten.** Every failure in this module is a refusal to send
  at all. The alternative to refusing an entry with no addr-spec is dropping it,
  and a submission with a shorter `rcptTo` is a perfectly valid submission —
  nothing below this point could notice, so the user would be told the message
  went and one recipient would never hear from them. `UnusableRecipient` carries
  the position and the display name so the sentence can name who.
- **The display names go, and nothing else does.** RFC 5321's `RCPT TO` takes an
  addr-spec and RFC 8621 §7's `EnvelopeAddress` has one field, so the name has
  nowhere to go — it is in the headers, which are uploaded verbatim. The list is
  not deduplicated (whether a repeated `RCPT TO` delivers twice is the server's
  rule, and editing the list would be quietly changing who the user addressed),
  not reordered, and not completed from the headers.
- **No fallback to the `From` header.** An absent sender is a refusal rather
  than a lookup: which identity a message goes out as is the caller's decision,
  and a transport that read one out of the message would be choosing who the
  mail is from. A sender listed with a display name and no address is the same
  refusal — `MAIL FROM:<>` is the null reverse-path a bounce is sent with.
- **The first sender wins.** SMTP's reverse-path is one address; every Camel
  transport reads entry zero and every caller in Evolution passes exactly one.
  Refusing a list of two would refuse a send Evolution cannot produce, over a
  rule SMTP applies to the transaction and not to the client. Pinned so the
  choice is visible rather than accidental.
- **The type is checked although Camel's own transports do not check it.**
  `send_to_sync` is declared over `CamelAddress`, which has other subclasses, and
  reading one of those through `camel_internet_address_get` is undefined
  behaviour rather than an empty answer. The check is ordered *before* the
  emptiness test, so a wrong-typed argument is reported as wrong-typed and not
  as an absent sender — which is what the test asserts, since the two are
  otherwise indistinguishable from the outside.
- **NULL is empty, not an error of its own.** A NULL `from` answers `NoSender`
  and a NULL recipient list answers `NoRecipients`: absent is absent, however
  Camel spells it. Only a non-NULL argument of the wrong class is
  `NotInternet`.
- **`CAMEL_SERVICE_ERROR_INVALID`,** in the transport's own domain, since a
  `CamelTransport` is a `CamelService`. Deliberately not `UNAVAILABLE`, which is
  what Evolution reads to put an account offline: nothing is wrong with the
  account or the connection, and this send would fail identically against a
  working server.

**Not covered by a test, and the honest limits:**

1. **There is still no `CamelJmapTransport`,** and `provider.rs` still leaves
   the transport slot at `G_TYPE_INVALID`. Nothing in this session changes what
   the user sees; Evolution still cannot send through a JMAP account. What it
   changes is that the vfunc a later session writes has its address handling
   done and pinned.
2. **No caller yet, so no test that the envelope reaches the wire.** The
   round trip from `read_envelope` through `Outgoing::envelope` into
   `EmailSubmission/set` is two tested halves with nothing joining them, and
   the join is the transport's `send_to_sync`.
3. **`camel_internet_address_get`'s behaviour is taken from the running
   Camel,** not from reading 3.52's source, which is not on this VM. The tests
   exercise it directly — an entry added with an empty address really does come
   back as one — so the assumption is checked rather than assumed, but only
   against the EDS installed here.
4. **Nothing about `out_sent_message_saved`, identity selection, or the
   staging/destination mailbox lookup.** Those are the transport's, and item 3
   of the previous session's list (which identity to submit through) is still
   unanswered.

Not verified locally, as in every session so far: `reuse lint` and `cargo deny`
(neither binary is on this VM). Two new files, `jmap-mail/src/envelope.rs` and
`jmap-mail/tests/envelope.rs`, both with the SPDX `GPL-3.0-or-later` header.
`cargo fmt --check`, `cargo test --locked` and `cargo clippy --all-targets
--locked -- -D warnings` are clean on the default member set (417 tests,
unchanged — nothing there was touched) and on the five EDS crates (602, up from
592).

No milestone tag is claimed.

Still open from earlier sessions, unchanged by this one: **whether Evolution's
Delete key files into the trash or only marks the row** — **needs human
verification in real Evolution**; `service.rs` unexercised against a real
`CamelSession`; and the README's architecture block still listing only the
round-1 crates.

Next in M5: the rest of `CamelJmapTransport` — the `CamelTransport` subclass and
its own connection, `send_to_sync` joining `envelope` to
`MailSync::send_message`, the identity lookup, the Drafts/Sent lookup out of the
folder tree, and the provider's transport slot. The addresses are no longer part
of that work.

## 2026-08-09 (eighty-first session)

**Which identity the message goes out as.** Item 3 of the seventy-ninth
session's list — "which identity to submit through is not decided anywhere yet"
— answered: `jmap-mail-sync/src/identity.rs` and `MailSync::identity_for`, the
lookup from the address Camel hands a transport to the id `Outgoing::identity`
takes. It is the second of the transport's pieces that can be finished and
tested to the end here, after the envelope, and it leaves `send_to_sync` itself
as the only part of sending with nothing under it.

Red first: `jmap-mail-sync/tests/identity.rs`, nine tests against a live mock,
failing to compile against a method that did not exist. A compile failure is a
weak red, so each rule was then re-checked against a deliberately broken
implementation — dropping the `.rev()` fails
`the_first_of_two_identities_with_the_same_address_wins`; reading the wildcard
as a prefix (`local.starts_with('*')`) fails
`an_identity_whose_local_part_only_begins_with_a_star_is_not_a_wildcard`;
matching the domain with `ends_with` fails
`a_wildcard_identity_covers_only_its_own_domain` on `notexample.com`; swapping
the two `MatchKind` variants so the wildcard outranks the exact address fails
the preference test and the case test. The same treatment for the Camel-side
mapping: flattening `NoIdentity` into a client error fails the crate-boundary
test, and reporting it as `UNAVAILABLE` fails the code table.

Decisions taken:

- **The wildcard is RFC 8621 §6's and nothing wider.** An identity whose local
  part is the single character `*` covers its domain; `*alice@example.com` is an
  ordinary address with an unusual name. A server hosting a whole domain
  publishes only the wildcard form, so a client comparing whole strings would
  tell such an account it cannot send at all — and one reading `*` as a prefix
  would send Bob's mail through Alice's identity.
- **Exact beats wildcard, and the first of equals wins.** The identity that has
  the address outright carries the user's name and signature and is what the
  server writes `From` from; the wildcard is the account's fallback. Among
  identities that match equally the first in the server's order wins, pinned so
  that retries of one message do not pick up a different signature each time.
- **Case is folded, in both halves, ASCII only.** The domain is
  case-insensitive by DNS. The local part is case-*sensitive* per RFC 5321 §2.4
  — but this is not a relay: both spellings are the user's own address on their
  own account, and refusing to send because the server wrote the identity with a
  capital would be a failure with nothing behind it. The safety argument is that
  RFC 8621 §7 has the *server* check the message's `From` against the identity,
  so a generous match here can only ever produce a refusal, never mail leaving
  as somebody else. Unicode case is deliberately not folded: the fold is
  language-dependent and two addresses that fold together are not reliably the
  same mailbox.
- **The envelope sender, not the `From` header.** The address matched is what
  Evolution filled in from the account the user chose, whereas a `From` can be a
  list address or a second author. The header check is the server's, and
  pre-empting it here would be this crate guessing at a policy it cannot see.
- **No identity is `SyncError::NoIdentity(String)`, a variant of its own.**
  Nothing failed — the server was never asked to send anything — and the
  alternative to refusing is submitting through whatever the account happens to
  have first, which is sending the user's message as somebody else. It carries
  the address because that is the part the user recognises and the only part
  they can act on. At the Camel boundary it becomes `StoreError::NoIdentity` and
  `CAMEL_SERVICE_ERROR_INVALID`, the code `envelope.rs`'s refusals use, and
  deliberately not `UNAVAILABLE`: a retry cannot help, and `UNAVAILABLE` is what
  Evolution reads to put an account offline.
- **One `Identity/get` per lookup, nothing cached.** A send is not a hot path,
  the user changes their identities elsewhere, and an identity list held across
  a session goes wrong quietly — by submitting through an identity the account
  no longer has, or by failing to find one it has just gained.

**Not covered by a test, and the honest limits:**

1. **Still no `CamelJmapTransport`,** and `provider.rs` still leaves the
   transport slot at `G_TYPE_INVALID`. Nothing in this session changes what the
   user sees; Evolution still cannot send through a JMAP account.
2. **Nothing joins the three tested halves.** `read_envelope` → `identity_for`
   → `MailSync::send_message` is a chain with no caller, and the caller is
   `send_to_sync`.
3. **An identity the server returns without an id** is reported as a protocol
   violation, and that path has no test: the mock always mints an id and there
   is no hook to seed one without. It is one `ok_or_else` over the `Option<Id>`
   `Identity` carries.
4. **No test against a server that publishes a wildcard identity for real.**
   The mock takes whatever `email` it is seeded with, so `*@example.com` is our
   own fixture rather than an observed server's behaviour. Stalwart's
   `Identity/get` has not been looked at.
5. **Nothing decides what happens when the account has several identities and
   the user's chosen address matches none** *at the Evolution level* — the
   refusal is correct here, but whether Evolution's composer can even produce
   such an address for a JMAP account is a question for the account-setup work.

Not verified locally, as in every session so far: `reuse lint` and `cargo deny`
(neither binary is on this VM). Two new files, `jmap-mail-sync/src/identity.rs`
and `jmap-mail-sync/tests/identity.rs`, both with the SPDX `GPL-3.0-or-later`
header. `cargo fmt --check`, `cargo test --locked` and `cargo clippy
--all-targets --locked -- -D warnings` are clean on the default member set (428
tests, up from 417) and on the five EDS crates (603, up from 602).

The new `SyncError` variant made two exhaustive matches fail to compile —
`jmap-mail-sync/tests/source.rs` and `jmap-mail/src/connect.rs`. Both were
written exhaustively on purpose, so that a new variant forces a decision instead
of falling into a wildcard; the decision was taken in both places rather than
the match loosened.

No milestone tag is claimed.

Still open from earlier sessions, unchanged by this one: **whether Evolution's
Delete key files into the trash or only marks the row** — **needs human
verification in real Evolution**; `service.rs` unexercised against a real
`CamelSession`; and the README's architecture block still listing only the
round-1 crates.

Next in M5: `CamelJmapTransport` itself — the `CamelTransport` subclass and its
connection, `send_to_sync` joining `read_envelope`, `identity_for` and
`MailSync::send_message`, the Drafts/Sent lookup out of the folder tree, and the
provider's transport slot. Both of its pure pieces are now done.

## 2026-08-09 (eighty-second session)

**The second service the account has.** `jmap-mail/src/transport.rs`:
`CamelJmapTransport`, the `CamelTransport` subclass Camel instantiates for the
sending half of a JMAP account, with a connection of its own. It is the first
of the four things the previous session left on the transport's list — the
subclass and its connection — and it is the one that had to come first, because
`send_to_sync` is a slot on this type's class and there was no class.

Red first: `jmap-mail/tests/transport.rs`, nine tests against a type that did
not exist. A compile failure is a weak red, so every rule was re-checked against
a deliberately broken implementation: not installing the service vfuncs fails
`the_transport_names_the_account_it_sends_through` (`camel_service_get_name`
answers NULL for a class that filled none of the slot in); leaving
`settings_type` inherited fails both that test and
`the_transport_is_configured_through_the_accounts_settings_class`; making the
`Connected` impl's `hold_connection` a no-op fails all four connection tests;
and giving the type `camel_service_get_type` as its parent fails
`the_transport_is_a_camel_transport`.

Decisions taken:

- **Its own connection, not a view of the store's.** Camel gives an account two
  services and no supported way for either to reach the other:
  `camel_session_get_service` needs a uid the transport does not carry, and the
  pairing of the two lives in Evolution's `EMailAccountStore`, above Camel. So
  the transport opens a second HTTP client against the same server. That is one
  more connection than JMAP needs and the only shape Camel offers; what it buys
  is that neither service's disconnect can take the other's away, which is the
  case that matters — Evolution disconnects a store on its own schedule, and a
  message in the outbox should not lose what it was going to go out over.
- **One slot, and no folder listing.** A transport lists nothing. The two
  mailboxes sending needs — where the message is staged, where it is filed
  afterwards — are a read over the connection at send time and not state this
  object keeps, so the type has exactly the connection in it.
- **The account's settings class, not an inherited one.** Same account, same
  server: a transport that inherited `CamelSettings` would have no host, no port
  and no user, and `e_source_camel_configure_service` would have nowhere to
  write what the user typed.
- **The four `CamelService` vfuncs are written once and installed on both
  services.** `service::install_vfuncs` is now generic over a new
  `service::Connected` trait — three methods, all about where the connection
  that opening the account produced is put — and `authenticate` is generic with
  it. Connecting is the same operation on either service, and the alternative
  was a second copy of the connect/authenticate/disconnect trio in which
  Camel's re-prompt rule (only an `ERROR` verdict carries a `GError`) could be
  got right in one place and wrong in the other.
- **The provider's transport slot stays `G_TYPE_INVALID`.** A registered
  transport whose `send_to_sync` is NULL is an account that offers to send and
  fails with a GLib critical, which is worse than an account that does not offer
  to send. `tests/provider.rs` already pins the slot as invalid with that
  reasoning written out; it is left exactly as it was.

**Not covered by a test, and the honest limits:**

1. **Still no `send_to_sync`,** so nothing in this session changes what the user
   sees: Evolution still cannot send through a JMAP account. What it changes is
   that the object the vfunc goes on exists, is connected the same way the store
   is, and has somewhere to read a connection from.
2. **`install_vfuncs::<T>` mis-parameterised is not caught by any test.**
   Installing the store's vfuncs on the transport's class would have every
   connect on that service read a `CamelTransport` as a `CamelJmapStore` — a
   cast the compiler cannot object to. The guard is that both `class_init`s pass
   `Self` rather than a named type, which is a convention and not a check; a
   test would need a `CamelSession` to drive the vfuncs through Camel, and the
   read it would catch is undefined behaviour rather than a wrong answer.
3. **The vfuncs are exercised only through `authenticate`,** as the store's are.
   `connect_sync` and `disconnect_sync` on a real transport need a
   `CamelSession` that authenticates, which is `EMailSession` over a source
   registry on the session bus — the same gap `service.rs` has had since it was
   written. `get_name` is the one vfunc tested through Camel, because it needs
   no session.
4. **Nothing about two sends at once.** The `RwLock` is documented as letting a
   second send proceed while the first uploads, and no test provokes it: there
   is no send yet to run twice.

Not verified locally, as in every session so far: `reuse lint` and `cargo deny`
(neither binary is on this VM). Two new files, `jmap-mail/src/transport.rs` and
`jmap-mail/tests/transport.rs`, both with the SPDX `GPL-3.0-or-later` header.
`cargo fmt --check`, `cargo test --locked` and `cargo clippy --all-targets
--locked -- -D warnings` are clean on the default member set (428 tests,
unchanged — nothing there was touched) and on the five EDS crates (612, up from
603).

Making `authenticate` generic broke one call in `tests/cancellation.rs`, where
`&store` on a `Box<JmapStore>` no longer coerces; it is `&*store` now. No
behaviour changed.

No milestone tag is claimed.

Still open from earlier sessions, unchanged by this one: **whether Evolution's
Delete key files into the trash or only marks the row** — **needs human
verification in real Evolution**; `service.rs` unexercised against a real
`CamelSession`; and the README's architecture block still listing only the
round-1 crates.

Next in M5: `send_to_sync` itself — the vfunc joining `read_envelope`,
`MailSync::identity_for` and `MailSync::send_message`, the Drafts/Sent lookup
out of a folder tree read over the transport's connection, the
`CamelMimeMessage` written out to the bytes that get imported, the
`out_sent_message_saved` out-parameter, and — once all of that is there and
tested — the provider's transport slot. It is the last piece of sending with
nothing under it.

## 2026-08-09 (eighty-third session)

**Where an outgoing message waits, and where it is filed.** The two mailbox ids
`Outgoing` carries — `staging` and `destination` — found in the account rather
than named by the caller, because Camel's `send_to_sync` is handed a message and
two address lists and nothing about folders at all. `jmap-mail-sync`:
`send::OutgoingMailboxes` with the decision in it, and `MailSync::outgoing_mailboxes`
with the round trip. It is the third of the four things the transport still
needs before `send_to_sync` can be written, and the last one that is decidable
without a `CamelMimeMessage` in hand.

Red first: `jmap-mail-sync/tests/staging.rs`, six tests against a method and an
error variant that did not exist. A compile failure is a weak red, so every rule
was re-checked against a deliberately wrong implementation. Dropping the
`destination == staging` collapse fails
`an_account_with_no_drafts_waits_in_the_mailbox_it_will_be_filed_in`; adding an
Inbox fallback fails `an_account_with_nowhere_to_put_an_outgoing_message_cannot_send`;
preferring Sent over Drafts for staging fails three of the six. The
name-versus-role rule is pinned by construction — the fixture seeds decoys
called "Drafts" and "Sent" with no role at all, *before* the German mailboxes
that carry the roles, so both a name match and a first-mailbox-wins match pick
the wrong pair.

Decisions taken:

- **Drafts, then Sent, then a refusal.** Drafts is where unsent mail belongs, so
  a submission the server refuses leaves the message where the user would look
  for it. An account with no Drafts stages in Sent — the message is going there
  anyway, and the only thing lost is that a refused send leaves something in
  Sent that never went out, still marked `$draft` because `accepted_patch` only
  clears the keyword on success. An account with neither is one this crate will
  not send from.
- **Not the Inbox.** The tempting fallback and the wrong one: the Inbox is where
  the *server* delivers, and importing the user's own outgoing mail into it
  manufactures arrivals they then have to sort out — for a message that may then
  be refused. The refusal names something the user can act on, and it costs
  nothing, because it happens before the upload.
- **By role, never by name.** RFC 8621 §2 puts a `role` on a mailbox for exactly
  this, and it is the only thing that identifies one across servers and
  languages. Same judgement as the trash lookup two sessions back.
- **No destination when it is the staging mailbox.** Not an optimisation but
  `Filing::is_empty`'s rule: a message cannot be filed out of a mailbox into
  that same mailbox, and asking for it would put a pointer that is both `true`
  and `null` in one patch. It is also the answer Camel's
  `out_sent_message_saved` out-parameter wants — whether this provider has
  already saved the sent copy somewhere the user will find it.
- **One `Mailbox/get` per send, caching nothing.** `identity_for`'s decision
  made again and for its reasons: a folder tree held across a session goes wrong
  quietly, by staging into a mailbox another client has deleted or by not
  finding the Sent folder the user has just made. A send already costs an
  upload. Deliberately *not* the tree the store keeps from its own listing —
  the transport is a separate `CamelService` with no pointer to the store, and
  reaching for one would invent a relationship Camel does not have.
- **`SyncError::NoOutgoingFolder` carries nothing,** because the missing thing
  is a mailbox that does not exist and so has no id or path to name. In
  `jmap-mail` it maps to a new `StoreError::NoOutgoingFolder` reported as
  `CAMEL_SERVICE_ERROR_INVALID` — like `NoIdentity`, and deliberately not the
  store's `NO_FOLDER` that `NoRole` uses, since the service answering is a
  transport rather than a store and there is no folder Camel asked for.

**Not covered by a test, and the honest limits:**

1. **Still no `send_to_sync`,** so nothing here changes what the user sees.
   Three of the four pieces the transport needs now exist and none of them has a
   caller.
2. **Nothing checks the pair against a real server's roles.** The mock takes
   whatever `role` string it is seeded with, so "an account with no Drafts" is
   our own fixture. Whether Stalwart or Fastmail ever publish an account without
   a Drafts mailbox has not been looked at; the refusal is a judgement about a
   case that may not occur in practice.
3. **The Sent-as-staging fallback leaves a refused message in Sent,** marked
   `$draft`. That is distinguishable from sent mail by anything reading
   keywords, and it is *not* distinguishable in Evolution's message list, which
   shows Sent without a draft column. Nobody has looked at what that looks like
   to a user, because there is no send yet to produce it.
4. **`out_sent_message_saved` has no code behind it.** The `destination` this
   produces is what it should be derived from, and the derivation is next
   session's, in the vfunc.

Not verified locally, as in every session so far: `reuse lint` and `cargo deny`
(neither binary is on this VM). One new file, `jmap-mail-sync/tests/staging.rs`,
with the SPDX `GPL-3.0-or-later` header. `cargo fmt --check`, `cargo test
--locked` and `cargo clippy --all-targets --locked -- -D warnings` are clean on
the default member set (434 tests, up from 428) and on the five EDS crates (612,
unchanged — nothing there gained a test).

The new `SyncError` variant made two exhaustive matches fail to compile again —
`jmap-mail-sync/tests/source.rs` and `jmap-mail/src/connect.rs`, the same two as
last session, both written exhaustively so that a new variant forces a decision.
Both were decided rather than loosened.

No milestone tag is claimed.

Still open from earlier sessions, unchanged by this one: **whether Evolution's
Delete key files into the trash or only marks the row** — **needs human
verification in real Evolution**; `service.rs` unexercised against a real
`CamelSession`; and the README's architecture block still listing only the
round-1 crates.

Next in M5: `send_to_sync` itself. What is left under it is the
`CamelMimeMessage` written out to bytes — `append.rs` already does exactly that
in a private `serialize`, which wants lifting out of the folder's error domain
before a transport can share it — and then the vfunc joining it to
`read_envelope`, `identity_for`, `outgoing_mailboxes` and `send_message`, the
`out_sent_message_saved` out-parameter, and the provider's transport slot.

## 2026-08-09 (eighty-fourth session)

**The bytes a message goes up as, and the line endings they did not have.**
`jmap-mail/src/mime.rs`: `write_message`, lifted out of `append.rs`'s private
`serialize` so that a `CamelTransport` with no folder in the call can share it,
plus `Unwritable`, which is a write failure that has no error domain yet — the
caller names one. Along the way the round-trip test found something: what Camel's
emitter writes is Camel's *internal* form, with bare LF line endings, so every
message this provider has been importing since the append landed went up
malformed. `write_message` now converts, and `append.rs` is its first caller.

Red first: `jmap-mail/tests/mime.rs` (5 tests, real `CamelMimeMessage`s) and
seven unit tests beside the implementation. The line-ending pair was red on
behaviour rather than on compilation — 7 bare LFs in a 9-line message. Five
deliberately wrong implementations were then checked against the suite, and each
is caught by exactly the test that should catch it: dropping the conversion
fails the RFC 5322 test and the whole-message test; inserting a CR before *every*
LF fails `a_line_that_already_ended_crlf_does_not_gain_a_second_cr` and the unit
test; reading one byte less than the stream holds fails
`a_message_larger_than_one_buffer_is_written_out_whole`; synthesising an error
even when Camel set one fails `a_failure_the_writer_explained_...`; hardcoding
the folder's domain fails `the_same_failure_is_reported_in_the_transports_domain...`.

Decisions taken:

- **CRLF is added here or nowhere.** Camel's own transports put a
  `CamelMimeFilterCrlf` between the message and the socket; this provider is a
  transport *and* an importer, and both of its callers put the bytes somewhere
  that outlives the call. RFC 5322 §2.1 defines a line as CRLF-terminated, RFC
  8621 §4.8 imports "an RFC 5322 message", and RFC 5321 §2.3.8 forbids a bare LF
  in what an SMTP server is handed — which is what an `EmailSubmission` hands one
  eventually. Bare LFs mean a DKIM signature computed over different bytes than
  the recipient verifies, and a body a strict relay may cut short.
- **The same rule Camel's filter applies, minus the dots.** A CR is inserted
  before an LF that has none; a lone CR is left alone, because it is not a line
  ending; `CRLF_MODE_CRLF_DOTS`'s leading-dot stuffing is deliberately *not*
  done, since it is an SMTP wire escape and would end up inside an imported
  message.
- **Written in Rust rather than reached for through `CamelStream`.** The filter
  is a `CamelStream`-era API this crate otherwise has no use for, and taking it
  would mean four new symbol families in `eds-sys`'s allowlist for a rule that is
  four lines and whose edge cases are directly testable. Not a MIME operation —
  no second MIME implementation is created by it.
- **`Unwritable` carries the failure and not the domain.** Camel's own `GError`
  is passed through untouched, as before. What is new is the *unexplained*
  refusal: it used to be a `CAMEL_FOLDER_ERROR` written inside `append.rs`, which
  is right for a folder and wrong for a transport Camel never asked a folder of.
  `into_gerror(domain, code)` takes it from the caller and keeps the sentence,
  because the thing that went wrong is the same either way.
- **It owns the error it holds.** `Unwritable` frees the `GError` on drop and
  `into_gerror` forgets itself, so a caller that drops a failure rather than
  reporting it does not leak the message it never showed anyone.

**Not covered by a test, and the honest limits:**

1. **No `CamelMimeMessage` here is one its own writer refuses,** so the failure
   path is unit-tested from a constructed `Unwritable` rather than end to end.
   Nothing was found that makes Camel's emitter fail on demand without also
   being undefined behaviour.
2. **The CRLF exposure is Camel's own, unchanged:** a part whose content is raw
   8-bit or binary rather than transfer-encoded has its LFs rewritten too —
   exactly as it would going out through `camel-smtp-transport`, which is the
   comparison this follows, but it is a rewrite of body bytes and it is worth
   knowing about.
3. **No real server has seen either form.** The mock stores blobs verbatim, so
   what is pinned is that the bytes are CRLF-ended, not that Stalwart or Fastmail
   would have rejected the LF-ended ones. The RFCs are the argument; interop is
   untested.
4. **Still no `send_to_sync`,** so the transport that this was lifted out for
   still has no caller. Four of its four pieces now exist.

Not verified locally, as in every session so far: `reuse lint` and `cargo deny`
(neither binary is on this VM). Two new files, `jmap-mail/src/mime.rs` and
`jmap-mail/tests/mime.rs`, both with the SPDX `GPL-3.0-or-later` header. `cargo
fmt --check`, `cargo test --locked` and `cargo clippy --all-targets --locked --
-D warnings` are clean on the default member set (434 tests, unchanged — nothing
there was touched) and on the five EDS crates (624, up from 612).

No milestone tag is claimed.

Still open from earlier sessions, unchanged by this one: **whether Evolution's
Delete key files into the trash or only marks the row** — **needs human
verification in real Evolution**; `service.rs` unexercised against a real
`CamelSession`; and the README's architecture block still listing only the
round-1 crates.

Next in M5: `send_to_sync` itself, which now has nothing missing under it — the
vfunc joining `read_envelope`, `write_message`, `identity_for`,
`outgoing_mailboxes` and `send_message`, the `out_sent_message_saved`
out-parameter derived from whether a destination mailbox was needed, and the
provider's transport slot that stays `G_TYPE_INVALID` until it exists.

## 2026-08-09 (eighty-fifth session)

**The message that leaves the account, and the second copy Evolution would have
kept of it.** `jmap-mail/src/send.rs`: `send_to_sync`, the vfunc M5 has been
building towards, joining `read_envelope`, `write_message`, `identity_for`,
`outgoing_mailboxes` and `send_message`; `JmapTransport::send_message` beside it
for the account-side half of that under one hold of the connection; and the
provider's transport slot, filled at last, so a JMAP account is now an account
Evolution can send from.

Red first: `jmap-mail/tests/send.rs` (13 tests, real `CamelMimeMessage`s and
`CamelInternetAddress`es through `camel_transport_send_to_sync`), three new
assertions in `jmap-mail-sync/tests/staging.rs`, and the flipped transport-slot
test in `jmap-mail/tests/provider.rs`. All red before any of it existed.

**The plan the last session left was wrong, and the tests are why.** It said the
`out_sent_message_saved` out-parameter would be "derived from whether a
destination mailbox was needed" — that is, `destination.is_some()`. It is not
derivable from that. `OutgoingMailboxes::destination` is `None` in two opposite
cases: the account that stages *in* Sent because it has no Drafts, where the
copy is saved exactly where the user looks for sent mail, and the account with
only a Drafts, where it is not. Camel documents the parameter as "the transport
saved the message into its Sent folder — do not copy it there yourself", and
Evolution appends a copy of its own when told `FALSE`. So the planned derivation
would have given the first kind of account **two of every message it sends**.
`OutgoingMailboxes` gained a `saves_sent_copy` field instead, which is true
exactly when the account has a Sent mailbox, and
`an_account_with_no_drafts_waits_in_the_mailbox_it_will_be_filed_in` and
`an_account_whose_only_outgoing_mailbox_is_sent_has_already_saved_the_copy` are
the two tests that pin the distinction. The misleading paragraph in
`OutgoingMailboxes::of` that the plan came from is corrected in the same commit.

Decisions taken:

- **Everything that can be refused is refused before the upload.** The envelope
  and the written-out message both come before the connection is looked for,
  which is `crate::envelope`'s own argument honoured: the alternative is a
  refusal made *after* the import, which leaves a draft in the user's account
  for a send they were told did not happen. Four tests assert the staging
  mailbox is still empty after a refusal, not only that the right code came
  back.
- **The envelope, not the headers, names the identity.** `identity_for` is
  asked about `envelope.mail_from`, which is what Evolution filled in from the
  account the user chose, and `the_identity_is_the_one_the_envelope_sender_names`
  is a message whose `From` header has no identity behind it and whose envelope
  sender does. RFC 8621 §7 has the server check the header against the identity
  named; that check is the server's, and pre-empting it here would refuse sends
  that are ordinary.
- **One read lock across all three account-side operations.** Taking it per
  operation would let a `disconnect_sync` land between the import and the
  submission — a message in the account, no submission, and a failure reported.
  It stays a *read* lock, which is what the `RwLock` on the transport was for:
  Evolution's outbox hands a transport several messages in a row.
- **The out-parameter is written before anything can fail, as well as on
  success.** `camel_transport_send_to_sync` clears it on the way in, so nothing
  going through the wrapper can tell the two apart — which is exactly why
  `a_failed_send_says_no_copy_was_saved` dispatches the class slot directly with
  the parameter pre-set to `TRUE`. Without that test the defensive clear was
  dead code; it was added after a deliberately-wrong implementation survived.
- **A message Camel will not write out is reported in the service's domain**
  here and in the folder's in `append.rs`. `Unwritable::into_gerror` takes the
  domain from the caller for this reason, which is what the previous session
  lifted it out for; there is no folder in a `send_to_sync` call to blame.
- **No retry, ever.** A submission the server refused is a message safe in the
  staging mailbox and a sentence for the user. Trying again would risk sending
  twice what could not be proved unsent once.
- **The provider slot is only filled because the class installs the vfunc.**
  `tests/provider.rs` now asserts both together: a transport type whose
  `send_to_sync` is NULL is worse than no transport type, because Camel's
  wrapper is a `g_return_val_if_fail` and the user meets it by pressing Send.

Six deliberately wrong implementations were checked against the suite and each
is caught by the test that should catch it: `destination.is_some()` for the
saved copy fails both the sync-level and the Camel-level Sent-only tests;
always-`true` fails the Drafts-only test; not clearing the out-parameter fails
the direct-dispatch failure test; `envelope: None`, which lets the server derive
the envelope from the headers, fails the Bcc test.

**Not covered by a test, and the honest limits:**

1. **No real Evolution has pressed Send.** What is tested is the vfunc against
   `jmap-mockd` through Camel's own wrapper, with a transport constructed by
   `g_initable_new` rather than by `camel_session_add_service`. That
   `e_mail_session_send_to` reads `out_sent_message_saved` the way its
   documentation says, and that an account configured through the source
   registry reaches this vfunc at all, is **needs human verification in real
   Evolution**.
2. **No real server has accepted a submission from this code.** The mock records
   the `EmailSubmission/set` and applies `onSuccessUpdateEmail`; whether
   Stalwart or Fastmail accept the same request is untested.
3. **Cancellation during a send is not tested here.** The `observe` scope is
   installed exactly as every other vfunc's and `tests/cancellation.rs` covers
   that machinery, but no test presses Stop mid-upload on a transport.
4. **The connection is held under a read lock across three round trips**, which
   is deliberate, but nothing tests two concurrent sends over one transport.

Not verified locally, as in every session so far: `reuse lint` and `cargo deny`
(neither binary is on this VM). Two new files, `jmap-mail/src/send.rs` and
`jmap-mail/tests/send.rs`, both with the SPDX `GPL-3.0-or-later` header. `cargo
fmt --check`, `cargo test --locked` and `cargo clippy --all-targets --locked --
-D warnings` are clean on the default member set (434 tests, unchanged — the
staging tests gained assertions, not cases) and on the five EDS crates (637, up
from 624).

No milestone tag is claimed: M5's acceptance also wants the provider exercised
through a real Evolution, and the two limits above are exactly that gap.

Still open from earlier sessions, unchanged by this one: **whether Evolution's
Delete key files into the trash or only marks the row** — **needs human
verification in real Evolution**; `service.rs` unexercised against a real
`CamelSession`; and the README's architecture block still listing only the
round-1 crates.

Next in M5: the surface is now feature-complete against the mock — store,
folders, summaries, bodies, flags, moves, appends, folder management,
subscriptions and sending. What is left is not another vfunc but the two things
no test here can reach: a manual test recipe for the mail provider, like
`docs/manual-test-book-backend.md` has for the address book, and a run against a
real server.

## 2026-08-09 (eighty-sixth session)

**What one JMAP login fans out into.** M5's surface is feature-complete against
the mock and what is left of it needs a real Evolution and a real server, so
this session started M6 at the only end of it that a test on this machine can
reach: the decision, taken from the session document at `/.well-known/jmap`,
of which JMAP account serves mail, which serves contacts, which serves
calendars, and whether the mail one can also send. New crate
`rust/crates/jmap-collection-sync` (`evolution-jmap-collection-sync`, lib
`jmap_collection_sync`), sibling to `jmap-book-sync`/`jmap-cal-sync`/
`jmap-mail-sync` and, like them, free of GObject and the EDS headers, so it is
in `default-members` and `cargo test` covers it — `cmake/Rust.cmake`'s
`rust-test` target is a plain `cargo test` and needed no change.

Red first, and recorded as red: `CollectionLayout::from_session` was stubbed to
resolve nothing and all 9 unit tests in `src/layout.rs` failed; the 3
mock-driven tests in `tests/layout.rs` went with them. Then green, then five
deliberately wrong implementations checked against the suite — trusting
`accountCapabilities` alone, trusting `capabilities` alone, inferring from
shared accounts, taking the first of several candidates, and accepting
submission in any account — each caught by exactly the test named for it.

Decisions taken:

- **A service needs both statements, because the two failure modes are
  different sentences to the user.** `capabilities` is what the server
  implements and `accountCapabilities` is what this account offers, and they
  are not the same claim. Believing the account alone gives a child source
  whose every refresh names a capability in `using` that the server answers
  with `unknownCapability` — which fails the whole request, not the one call.
  Believing the server alone gives one whose every refresh is an
  `accountNotFound`. Neither is a folder worth creating, so both must say yes.
- **`primaryAccounts` is honoured as given; absence is inferred from, but only
  where there is nothing to guess.** Where the server names an account for a
  capability that is the answer, including when the account is not the user's
  own — a server that designates a shared account as primary has said something
  deliberate. Where it names none, the account is inferred only when exactly one
  account with `isPersonal` offers the capability. Two of them and the answer is
  none: a collection fanned out to the wrong mailbox is worse than one that
  reports it could not tell, because the user cannot see which JMAP account a
  folder came from and would have to be told by us.
- **A submission capability in another account is no transport, not a second
  one.** `EmailSubmission` names an `emailId` (RFC 8621 §7) and ids are scoped
  to an account, so submitting through account B a message uploaded into
  account A is not a thing the protocol can express. `MailService::can_send` is
  therefore true only when submission resolves to the *same* account as mail;
  false is a receive-only account, which is a usable account.
- **`isReadOnly` is carried through rather than acted on here.** It is the whole
  account's flag, not one collection's, and what a read-only account should
  become on the EDS side — a child source marked read-only, or no child at all —
  is a question for the code that creates sources, not for the code that reads
  the document.
- **`is_empty()` is a distinct answer.** A login can authenticate and offer
  nothing this backend can use (a server with only `…:jmap:core`, an account
  with its capabilities stripped). That is a sentence for the user, not an empty
  account tree to leave them puzzling over.
- **Turning a layout into `ESource`s is deliberately not in this crate.** That
  half needs the headers (`e_collection_backend_new_child`, the
  `[Mail Account]`/`[Mail Transport]`/`[Address Book]`/`[Calendar]` extensions)
  and it is the half no test here can verify.

**Not covered by a test, and the honest limits:**

1. **There is no collection backend yet.** Nothing has turned a
   `CollectionLayout` into an `ESource`, nothing has been loaded by
   `evolution-source-registry`, and M6 is not started beyond this decision
   layer. What is tested is a reading of a document.
2. **The inference fallback is untested against any real server.** `jmap-mockd`
   always writes a full `primaryAccounts`, so the sole-personal-account path is
   exercised only by hand-written session documents. It exists as a defence
   against servers that omit entries RFC 8620 §2 lets them omit; whether
   Stalwart or Fastmail ever do is unknown here.
3. **Whether Evolution wants mail as children of the collection source or as
   properties on it** — one `[Mail Account]` child plus `[Mail Identity]` and
   `[Mail Transport]` siblings, which is what the other collection backends
   appear to do — is an EDS structural fact this crate does not answer and the
   next increment must check against the installed 3.52 headers rather than
   guess at.
4. **The mail identity's address is not derived from `username`.** The session's
   `username` is credentials-shaped, not necessarily an address; the account's
   `Identity` objects are the authoritative source and `jmap-mail-sync`'s
   `identity` module already reads them. Which of them a `[Mail Identity]`
   child should be written from is left to the increment that writes one.

Not verified locally, as in every session so far: `reuse lint` and `cargo deny`
(neither binary is on this VM). Four new files — the crate's `Cargo.toml`,
`src/lib.rs`, `src/layout.rs` and `tests/layout.rs` — each with the SPDX
`GPL-3.0-or-later` header. `cargo fmt --check`, `cargo test --locked` (446
tests, up from 434) and `cargo clippy --all-targets --locked -- -D warnings` are
clean on the default member set, and clippy is clean on the five EDS crates,
which this change does not touch.

No milestone tag: M5 still wants the provider exercised through a real
Evolution, and M6 has one decision layer and no backend.

Next in M6: the EDS side — a `jmap-backend-collection` crate subclassing
`ECollectionBackend`, its `populate` creating the children this layout warrants,
and `dup_resource_id`/`create_resource` for the address books and calendars
enumerated per account. The structural question in limit 3 is the first thing to
settle, from the headers.

## 2026-08-09 (eighty-seventh session)

**How many address books is "the address book".** The previous session left M6
with a decision layer that answers *which account* serves contacts and
calendars, and a note that the next thing to settle was structural and had to
come from the installed 3.52 headers rather than from a guess. This session
settled one half of that and built on it.

From the headers, and not guessed at: `e_collection_backend_new_child` takes a
*resource id* and hands back the child `ESource` — so a backend does not invent
child uids, it names resources and EDS assigns them. And
`e_collection_backend_list_mail_sources` sits beside
`list_contacts_sources`/`list_calendar_sources`, all three returning children —
so mail is a child of the collection, not a set of properties on it. That
answers the "children or properties" half of the previous session's limit 3.
The other half — *which* extension sits on *which* of the mail children (the
`[Mail Identity]`/`[Mail Submission]` pairing in particular) — is Evolution
convention, not a header fact, and there is no reference `.source` file
installed on this VM to check it against; it stays open and is **not** claimed.

The increment itself is the other question a populate has to answer, and one
that is entirely testable here: an account is not one address book, it is
however many `AddressBook`s and `Calendar`s the server lists (RFC 9610 §2,
draft-ietf-jmap-calendars §4), and Evolution shows one source per collection.
New module `jmap-collection-sync/src/resources.rs`: `Resource` (id, name,
is_default) and `Fanout` (the layout plus the two collection lists), with
`Fanout::discover(&Client)` doing the session read and the two listings.

Red first, and recorded as red: with `discover` stubbed to return empty vectors,
5 of the 6 mock-driven tests in `tests/resources.rs` failed (the sixth,
"a login without calendars is never asked for them", passes vacuously against a
stub — noted rather than papered over). Then green, then the deliberately wrong
implementations checked against the suite: dropping the subscription filter,
sorting by name alone, and listing regardless of what the layout resolved are
each caught by exactly the test named for them.

Decisions taken:

- **A listing is sent only for a capability the layout resolved.** Not to save
  a round trip: RFC 8620 §3.3 has a server answer a `using` naming a capability
  it does not advertise with `unknownCapability`, and that fails the *whole
  request*, not the one call. So asking a contacts-less server for its address
  books anyway would not return a short answer, it would return nothing at all —
  and the calendars in the same request with it. Two mock tests assert on
  `method_calls()` that the request is never sent, which is the only place this
  is visible; the returned value looks the same either way.
- **`isSubscribed == Some(false)` is dropped; an absent `isSubscribed` is not.**
  The property means the user has said they do not want this collection, and
  creating a child for it puts a calendar they removed back in the sidebar at
  every populate. But the property is optional in both specifications, so
  silence is the shape of a plain server, not a refusal — reading it as one
  would empty the sidebar of every server that omits it.
- **A collection with no `id` is dropped rather than pointed somewhere.**
  `[Resource] Identity` is how a child names its collection, and the book
  backend already treats a missing one as "the account's default" — so keeping
  an id-less collection would show the wrong address book under the right name,
  which is worse than showing nothing.
- **A collection the server named nothing is shown under its id.** `name` is
  required by both specifications, so this is a server out of spec; the
  alternative to a fallback is a blank row in Evolution's sidebar that the user
  cannot tell from another blank row.
- **The order is `sortOrder`, then name, then id.** The first two are what a
  JMAP client is told to do with `Mailbox` and both collection objects copy the
  property; the id tie-break exists only so that two identically named
  collections come back in the same order every populate. A child list that
  reshuffles between runs is one EDS is handed as a changed account.
- **The error type is `jmap_client::Error` rather than a new wrapper.**
  Everything that can fail here is one of its calls, and the GObject layer
  already maps that type onto Evolution's codes; a wrapper with one variant
  would be ceremony. If a second failure kind appears, that is when it earns a
  `FanoutError`.

**Not covered by a test, and the honest limits:**

1. **There is still no collection backend.** Nothing turns a `Fanout` into
   `ESource`s, nothing has been loaded by `evolution-source-registry`, and the
   mail-children structure above is settled only as far as the headers state it.
2. **Nothing here decides what a *read-only* account's collections become.**
   `ServiceAccount::read_only` is carried through as before and still acted on
   nowhere; per-collection rights (a shared calendar one may read and not write)
   are a `myRights`-shaped question neither collection object is being read for
   yet.
3. **`Calendar` and `AddressBook` are read only for the five properties a child
   source is made from.** Colour, description and the calendar's participant
   identity are ignored; `ESourceCalendar` has a `color` property and mapping it
   is a later increment, not an oversight to rediscover.
4. **Untested against any real server.** `jmap-mockd` answers `AddressBook/get`
   and `Calendar/get` in full and never paginates them; whether Stalwart or
   Fastmail sort, subscribe or default differently is unknown here.

Not verified locally, as in every session so far: `reuse lint` and `cargo deny`
(neither binary is on this VM). Two new files — `src/resources.rs` and
`tests/resources.rs` — each with the SPDX `GPL-3.0-or-later` header;
`evolution-jmap-client` moved from a dev-dependency to a dependency of the
crate. `cargo fmt --check`, `cargo test --locked` (457 tests, up from 446) and
`cargo clippy --all-targets --locked -- -D warnings` are clean on the default
member set, and clippy is clean on the five EDS crates, which this change does
not touch.

No milestone tag: M5 still wants the provider exercised through a real
Evolution, and M6 has two decision layers and no backend.

Next in M6: the EDS side — a `jmap-backend-collection` crate subclassing
`ECollectionBackend`, whose `populate` calls `e_collection_backend_new_child`
once per `Resource` and once for the mail account, and whose `dup_resource_id`
reads the id back out of `[Resource] Identity`. The open question above — which
extension sits on which mail child — has to be answered there, and this VM
cannot answer it; it needs a real Evolution account to compare against, and the
increment that writes it must be marked *needs human verification*.

## 2026-08-09 (eighty-eighth session)

**The name a child is created under, and why it is not the JMAP id.** The
previous session ended with a `Fanout` — which account serves what, and which
address books and calendars are in it — and the note that the next thing is the
EDS side. This session did the half of that which is decidable *here*: what a
`populate` makes of a fan-out, up to but not including the `ESource` calls.

The pairing the header states, and the whole reason this increment exists:
`e_collection_backend_new_child (backend, resource_id)` takes a resource id and
hands back the child source, and `ECollectionBackendClass.dup_resource_id` has
to return that same string for that same child. That is how EDS knows, on the
*next* populate, that a collection already has a source. Get the string wrong
and every populate creates a second source for a collection that already has
one.

New module `jmap-collection-sync/src/children.rs`: `ChildKind`, `Child`
(resource id, kind, display name, account id, collection id, is_default,
read_only), `Fanout::children()` and `parse_resource_id()`.

Red first, and recorded as red: with `children()`, `parse_resource_id()` and
`ChildKind::resource_id()` stubbed, 7 of the 11 unit tests failed. The other 4
assert *absence* (no children for a mail-only login, no mail children, a foreign
resource id is not read as ours) and so pass vacuously against a stub — noted
rather than papered over; they are load-bearing only against the real
implementation, which is why the mutation checks below matter more for them.

Then green, then three deliberately wrong implementations checked against the
suite: dropping the kind prefix, dropping the duplicate filter, and pointing the
calendar children at the contacts account are each caught by exactly the tests
named for them.

Decisions taken:

- **A resource id is `addressbook:<id>` / `calendar:<id>`, not the bare JMAP
  id.** Ids in JMAP are scoped to an account *and an object type* (RFC 8620
  §1.2), so an `AddressBook` and a `Calendar` may both be called `a` — on a
  server that numbers its objects from one, that is the expected case rather
  than a corner one. The resource id namespace is flat, being every child of
  this one collection, so the bare id would have the calendar resolve to the
  address book's child: the account comes up one source short, with a calendar's
  data reached through a contacts backend.
- **Parsing splits at the first colon and keeps the rest whole.** The id charset
  in RFC 8620 §1.2 has no colon in it, but nothing at this layer is in a
  position to insist on that, and a server that sends one should get a
  wrong-*looking* source rather than one silently pointed at another collection.
  A round-trip test covers an id containing a colon for exactly this.
- **An unparseable resource id is `None`, which the vfunc will turn into
  `NULL`.** `dup_resource_id` is called for children this backend may never have
  created; answering for one is claiming a source that belongs to another
  collection backend.
- **A collection the server listed twice becomes one child, first listing
  winning.** Two children under one resource id are not two sources — the second
  `new_child` resolves to the first one's source — so the second child is at
  best ignored and at worst overwrites the first one's display name. First-wins
  also keeps the child list independent of which duplicate the server sent last.
- **Each child carries its own `accountId`.** `CollectionLayout` resolves
  contacts and calendars independently and they can land in different accounts;
  a child that carries the collection's "the" account fails every call it makes.
- **`ServiceAccount::read_only` now reaches something.** It is copied onto each
  child of *that* account only — the previous session's limit 2, closed on the
  account-level half. Per-collection `myRights` is still not read, so a child
  that is not marked read-only is not thereby known to be writable; the doc
  comment on the field says so.
- **The mail children are deliberately absent.** Whether the mail account,
  identity and transport sources come from this backend's populate or from the
  account-setup module (M7), and which of `[Mail Account]`/`[Mail Identity]`/
  `[Mail Submission]`/`[Mail Transport]` sits on which, is Evolution convention;
  the installed 3.52 headers do not state it and this VM has no reference
  account to read it off. A guess here would produce a child list whose tests
  only confirm the guess. There is a test asserting the absence, so that anyone
  adding mail children has to settle the question first rather than discover it.

**Not covered by a test, and the honest limits:**

1. **Still no collection backend.** Nothing calls `e_collection_backend_new_child`
   with these strings, nothing writes an `ESource` keyfile from a `Child`, and
   nothing has been loaded by `evolution-source-registry`. `Child` is the input
   that layer will take; that it is the *right* input is argued from the header,
   not demonstrated.
2. **`ChildKind` has no task-list variant.** JMAP has no task collection in the
   calendars draft this repo tracks, and `ECalMetaBackend` sources come in three
   flavours (events, tasks, notes). If tasks arrive, the prefix set grows, and
   old resource ids must keep parsing — the parse is closed against unknown
   prefixes precisely so that a future prefix is a `None` rather than a
   mis-typed child.
3. **The resource id is not known to be constrained by EDS.** The header takes a
   `const gchar *` and says nothing more; whether EDS puts it in a filename or a
   keyfile key (which would rule out some characters) is not visible from the
   headers here, and the colon is chosen on the assumption that it is opaque.
   Worth re-checking against the EDS source before the backend crate lands.
4. **Untested against any real server.** The mock's ids are the mock's.

Not verified locally, as in every session so far: `reuse lint` and `cargo deny`
(neither binary is on this VM). Two new files — `src/children.rs` and
`tests/children.rs` — each with the SPDX `GPL-3.0-or-later` header.
`cargo fmt --check`, `cargo test --locked` (470 tests, up from 457) and
`cargo clippy --all-targets --locked -- -D warnings` are clean on the default
member set, and clippy is clean on the five EDS crates, which this change does
not touch.

Noticed in passing, not fixed: `cargo clippy --workspace --all-targets --
-D warnings` fails on `example-module` with 26 pre-existing
`manual_c_str_literals` warnings. CI is unaffected — both `.github/workflows/ci.yml`
and `.gitlab-ci.yml` run clippy without `--workspace`, so it sees the default
member set — and example-module is scaffolding, so this is a note rather than a
finding.

No milestone tag: M5 still wants the provider exercised through a real
Evolution, and M6 has three decision layers and no backend.

Next in M6: the backend crate itself — `ECollectionBackend` subclassed in
`jmap-backend-collection`, `populate` calling `e_collection_backend_new_child`
once per `Child` and writing the `[Address Book]`/`[Calendar]`/`[Resource]`
extensions, `dup_resource_id` reading the id back off the child source through
`parse_resource_id`. Limit 3 above is the first thing to check there, from the
EDS source rather than the headers. The mail-children question stays open and
the increment that answers it must be marked *needs human verification*.

## 2026-08-09 (eighty-ninth session)

**What has to be set on a child source, read off EDS's own source rather than
its headers.** The previous session ended with `Child` — the resource id a
populate names each collection under — and three open limits, the first of them
being that nothing writes an `ESource` from a `Child` and the third being that
the resource id's constraints were assumed rather than known ("worth re-checking
against the EDS source before the backend crate lands"). This session fetched
that source — EDS 3.52.4 `src/libebackend/e-collection-backend.c` and
`e-webdav-collection-backend.c`, the version this VM has headers for — and did
the half of the next layer the source makes decidable.

New module `jmap-collection-sync/src/child_source.rs`: `Connection` (where the
collection source says its server is), `Setting` (one `(group, key, value)` to
set on a child, named as both the keyfile and `e_source_get_extension` name it),
`ChildKind::extension()`, `Child::settings()` and `resource_id_for()` — the
`dup_resource_id` half.

Red first, and recorded as red: with `settings()`, `resource_id_for()` and
`extension()` stubbed, 6 of the 8 new unit tests failed. The other two assert
*absence* — that no child writes its own `Enabled`/`Parent`, and that a source
this backend did not create has no resource id — so they pass against a stub
that returns an empty vector and `None`; noted rather than papered over. The
mock-based test added afterwards was also run against the stub first and fails
there ("every child is written with an identity").

What the EDS source settled, each of which is now a test or a decision:

- **Limit 3 is closed: the resource id is opaque.** `collection_backend_new_user_file()`
  names the child's `.source` file after a freshly generated uid, not after the
  resource id, and the resource id is only ever a `GHashTable` key compared with
  `g_strcmp0`. The colon in `addressbook:<id>` was the open question; it is safe.
- **But it is not persisted, and a `NULL` answer destroys data.** On each start
  `collection_backend_load_resources()` re-reads the cached `.source` files and
  asks `dup_resource_id` what each one is. A file whose answer is `NULL` — or a
  duplicate of one already seen — is put on `remove_redundant` and **unlinked**.
  So the resource id must be a *total* function of a child's own properties, and
  every child this crate describes must round-trip. That is what the round-trip
  test over both kinds, including two kinds sharing one id, is for.
- **`[Resource] Identity` stays the bare JMAP id.** EDS's own WebDAV collection
  backend writes `"contacts::" + url` into the identity and leaves
  `dup_resource_id` at its default, which returns it verbatim — it has to,
  because a WebDAV URL alone does not say which kind it is. A JMAP child does
  say, in the extension it carries, and that is the same pair EDS keys
  `collection_backend_child_is_contacts()`/`…_is_calendar()` off. Since
  `jmap-backend-core`'s `SourceConfig` already reads `[Resource] Identity` as
  *the JMAP object id*, and `docs/examples/jmap-mock.source` says `Identity=Ab1`,
  prefixing it here would leave one field with two spellings — one written by
  this backend, one by hand. The prefix lives in the resource id, which is
  derived from (extension, identity) instead of stored.
- **A child inherits nothing of the connection.** `collection_backend_child_added()`
  binds `oauth2-support`, and the display name for mail children, and that is the
  whole list; the WebDAV backend copies user and auth method onto each child by
  hand for exactly this reason. Since the JMAP backends are handed the *child*
  source and read the server off it, a child without `[Authentication] Host` is
  one whose every operation fails with "the account does not name a JMAP server".
  Host, Port, User and auth Method are copied, each omitted rather than written
  empty when the account does not state it.
- **`[Security] Method` is written even when it says `tls`.** `SourceConfig`
  reads an absent `[Security]` as TLS, so omitting it is the one omission that
  would silently *upgrade* a child past its own account — and against
  `jmap-mockd`, past working at all.
- **`Enabled` and `Parent` are not written.** `collection_backend_new_source()`
  sets the parent before the child is handed out, and
  `collection_backend_bind_child_enabled()` *binds* `enabled` to
  `ESourceCollection:contacts-enabled`/`:calendar-enabled` (and the collection's
  own `enabled`). Writing either would be overwritten, and would put the user's
  "don't show this account's contacts" choice in two places. There is a test
  asserting the absence.
- **`read_only` is not a setting.** No `ESource` property says a collection is
  read-only; writability is a runtime answer the book and calendar backends
  give. The field stays a fact for the backend to act on.

**Not covered by a test, and the honest limits:**

1. **Still no collection backend.** Nothing calls `e_collection_backend_new_child`,
   nothing calls `e_source_get_extension`, nothing has been loaded by
   `evolution-source-registry`. That the settings list is *complete* is argued
   from EDS's source and from what `SourceConfig` reads, not demonstrated.
2. **`[Authentication] Method` is copied on the WebDAV backend's authority.**
   Whether EDS consults the child's method or the collection's when it decides
   how to obtain credentials was not traced through
   `ESourceCredentialsProvider`; copying it matches what the one in-tree
   collection backend does, and a child that answered differently from its
   account would at worst be prompted differently.
3. **`[Offline] StaySynchronized` is not written**, so children get EDS's
   default. Whether a collection's children should default to staying
   synchronised is a product decision, not a protocol one.
4. **The part-enabled question is untouched.** `ECollectionBackendParts` and
   `e_collection_backend_get_part_enabled()` exist, and a populate should
   probably not create address books for an account whose `contacts-enabled` is
   off. EDS binds the child's `enabled` either way, so this is about not
   creating sources rather than about hiding them — the next increment's
   question, deliberately not bundled into this one.

Not verified locally, as in every session so far: `reuse lint` and `cargo deny`
(neither binary is on this VM). One new file — `src/child_source.rs` — with the
SPDX `GPL-3.0-or-later` header. `cargo fmt --check`, `cargo test --locked`
(479 tests, up from 470), `cargo clippy --all-targets --locked -- -D warnings`
and `RUSTDOCFLAGS=-D warnings cargo doc` are clean on the default member set;
the five EDS crates are untouched and depend on nothing that changed.

No milestone tag: M6 still has no backend.

Next in M6: the backend crate itself — `ECollectionBackend` subclassed in
`jmap-backend-collection`, `populate` calling `e_collection_backend_new_child`
once per `Child` and applying `Child::settings`, `dup_resource_id` answering
from (extension, `[Resource] Identity`). Limit 4 above — whether a disabled part
should suppress the children rather than only their `enabled` flag — is the
first thing to settle there. The mail-children question stays open and the
increment that answers it must be marked *needs human verification*.

## 2026-08-09 (ninetieth session)

**What a part the user switched off means — and, the half that can lose data,
what it does *not* mean.** The previous session's limit 4: `ECollectionBackendParts`
and `e_collection_backend_get_part_enabled()` exist, and a populate "should
probably not create address books for an account whose `contacts-enabled` is
off" — left deliberately unsettled. This session settled it, both halves, off
EDS 3.52.4's `e-collection-backend.c` and `e-webdav-collection-backend.c`.

New module `jmap-collection-sync/src/parts.rs`: `Parts` (the three flags
`ESourceCollection` carries), `Parts::from_collection`, `wants`, `any`,
`Fanout::listed` and `Fanout::is_obsolete`. `Fanout` gained a `parts` field and
`Fanout::discover` a `parts` argument; `CollectionLayout` gained `account_for`
and `serves`.

Red first, and recorded as red: with `from_collection`, `wants`, `any`, `listed`
stubbed and `is_obsolete` written the naive way ("not in the child list"), 8 of
the 10 new unit tests failed. The two that passed are noted rather than papered
over — `a_source_that_says_nothing_about_its_parts_has_all_of_them` passes
against a stub returning `ALL`, and `a_collection_the_server_no_longer_lists_is_obsolete`
is precisely what the naive `is_obsolete` does; it is there because the *other*
tests must not be satisfied by never removing anything. The two mock-based tests
were run against the ungated `discover` first and fail there
(`["AddressBook/get", "Calendar/get"]`).

What was decided, and why:

- **A switched-off part is not asked about.** `discover` sends
  `AddressBook/get` only when the contacts part is on, `Calendar/get` only when
  the calendar part is on — the same gate `EWebDAVCollectionBackend` puts in
  front of its discovery, which returns before contacting anything when neither
  part is enabled. Cheaper by a request, and on an account that has to
  authenticate, by a credential prompt for data the user said they did not want.
- **So its children are not created either**, rather than created and
  immediately switched off by EDS's `enabled` binding. The user sees the same
  thing; this way it costs no `.source` file. `Fanout::children` holds the same
  line a second time so a hand-built fan-out cannot route around it.
- **But a switched-off part is not deleted — and here this backend parts company
  with EDS's WebDAV one.** `EWebDAVCollectionBackend` fills its "previously
  known" table with children of *both* kinds and then discovers only the enabled
  ones, so with contacts off every address book child is a leftover and is
  `e_source_remove_sync`'d: uid, `.source` file and offline cache gone to a
  checkbox, and re-ticking it rediscovers them as brand new sources. That looks
  like an oversight in EDS rather than a design (the error path has an explicit
  "prevent lost of already known calendars when the discover failed" guard; the
  part-disabled path has no equivalent), and EDS's own answer to a disabled part
  — `collection_backend_bind_child_enabled()` binding the child's `enabled` to
  it, with `SYNC_CREATE` when the part is off — is "the children exist and are
  off", not "the children are gone".
- **So `is_obsolete` is true in exactly one case**: the resource id parses as
  ours, its kind was actually *listed* (`Fanout::listed` — the part is on *and*
  the login resolved an account for it), and the listing did not contain it.
  A resource id we did not write, a child of a switched-off part, and a child of
  a kind the login stopped advertising are all kept. `Fanout` carries the
  `parts` it was discovered under so that what was asked and what may be
  concluded cannot drift apart in the caller.
- **A failed discovery removes nothing** — by construction rather than by a
  check: `discover` returns `Err` and there is no `Fanout` to ask.
- **`Parts::from_collection` mirrors `e_collection_backend_get_part_enabled()`'s
  two rules**: a disabled collection source has no enabled parts whatever its
  extension says, and a source with no `[Collection]` extension has all of them
  (EDS returns `TRUE` there). Encoding it here keeps the GObject layer a
  three-getter read.
- **`Parts::mail` is carried but drives nothing yet.** No mail children exist to
  gate, and the mail service costs no request of its own. It is in the struct
  because a `Parts` with two of EDS's three parts is a trap for whoever writes
  the mail children.

**Not covered by a test, and the honest limits:**

1. **Still no collection backend.** Nothing calls
   `e_collection_backend_get_part_enabled`, `e_source_collection_get_contacts_enabled`
   or `e_source_remove_sync`; `Parts` and `is_obsolete` are the decision, not the
   act. That the three getters map onto these three fields is argued from EDS's
   source, not demonstrated.
2. **Diverging from the WebDAV backend is a judgement.** If Evolution or a user
   somewhere *relies* on disabling contacts pruning the sources, this keeps
   sources they expect to be gone (switched off, but present in the cache
   directory). The trade is data preservation against tidiness, and it is
   reversible in one function.
3. **A collection that disappears while its part is off is kept indefinitely**,
   and is only removed the first populate after the part comes back on. That is
   the intended consequence of 3 above, but it means the cache can hold children
   for collections that no longer exist.
4. **The part-enabled gate is per-populate, not reactive.** EDS re-runs populate
   when the collection source changes, so ticking a part on should discover it;
   nothing here proves that, since nothing here is driven by EDS yet.

Not verified locally, as in every session so far: `reuse lint` and `cargo deny`
(neither binary is on this VM). One new file — `src/parts.rs` — with the SPDX
`GPL-3.0-or-later` header. `cargo fmt --check`, `cargo test --locked` (491
tests, up from 479) and `cargo clippy --all-targets --locked -- -D warnings` are
clean on the default member set; the five EDS crates are untouched and depend on
nothing that changed.

Correction to the last three sessions' sign-off: `RUSTDOCFLAGS=-D warnings cargo
doc` is **not** clean on master, and was not before this session either — it
fails on two pre-existing private-intra-doc-links, `jmap-ical`'s
`syntax.rs:28` (`Component::write_into`) and `jmap-mock`'s `state.rs:301`
(`Transaction`). Verified by stashing this session's work and re-running. Left
alone here rather than folded into an unrelated commit; no CI job runs `cargo
doc`, so this is a local-check claim to fix, not a red pipeline.
`cargo doc -p evolution-jmap-collection-sync` is clean.

No milestone tag: M6 still has no backend.

Next in M6: the backend crate itself — `ECollectionBackend` subclassed in
`jmap-backend-collection`, `populate` reading `Parts::from_collection` off the
collection source, calling `e_collection_backend_new_child` per `Child`,
applying `Child::settings`, removing the children `Fanout::is_obsolete` names,
and `dup_resource_id` answering from (extension, `[Resource] Identity`). The
mail-children question stays open and the increment that answers it must be
marked *needs human verification*.

## 2026-08-09 (ninety-first session)

**The name EDS knows a child by — and the one vfunc whose wrong answer deletes
a file.** M6's decision layer has been complete for a session
(`jmap-collection-sync`: layout, resources, children, child_source, parts) and
the next thing needed was the backend crate. This session started it, with the
vfunc that has to work before `populate` can be written at all: EDS loads the
cached `.source` files and asks each one's resource id *before* it calls
`populate`, so a populate written first would be running against a child list
that had already been mis-loaded — and, worse, mis-*pruned*.

New crate `rust/crates/jmap-backend-collection` (out of `default-members`, added
to CMake's `rust-test-eds`), with two modules:

- `resource_id.rs` — `resource_id_of(*mut ESource)`: which of `[Address Book]`
  and `[Calendar]` the source carries, `[Resource] Identity`, and
  `jmap_collection_sync::resource_id_for` over the two. `KIND_EXTENSIONS` pairs
  each EDS `#define` with the literal `jmap-collection-sync` spells it as.
- `backend.rs` — `JmapCollectionBackend`, an `ECollectionBackend` subclass whose
  `class_init` installs a `dup_resource_id` trampoline, panic-guarded, returning
  a `g_strdup`ed string or NULL.

Red first, and recorded as red: against stubs (`KIND_EXTENSIONS` empty,
`resource_id_of` returning `None`, no `class_init`), 5 of 9 in
`tests/resource_id.rs` and 3 of 5 in `tests/backend.rs` failed. The failures are
worth quoting because they are the bug the increment exists to prevent: with the
slot left at EDS's default, an address book source carrying `Identity=X1`
answered `"X1"` and a `[Mail Account]` source carrying `Identity=A1` answered
`"A1"` — EDS's `collection_backend_dup_resource_id()` returns the identity
verbatim and asks no questions about the kind. The four resource-id tests that
passed against the stub are the "not claimed" ones (a foreign source, an empty
identity, a NULL source, a child with no `[Resource]`); a stub that claims
nothing satisfies them, and they are there so the *other* tests cannot be
satisfied by claiming everything.

What was decided, and why:

- **The resource id carries the kind; the stored `Identity` does not.** EDS's
  default is enough for `EWebDAVCollectionBackend` only because it writes
  `"contacts::" + url` into `[Resource] Identity` itself. This backend writes the
  bare JMAP id there, because that is the field `jmap-backend-core`'s
  `SourceConfig` — and the hand-written `docs/examples/jmap-mock.source` — already
  read as the object to fetch. JMAP ids are unique per data type, not per account
  (RFC 8620 §1.2), so an address book and a calendar may both be `X1`, and under
  the inherited vfunc the second of them to load is "redundant":
  `collection_backend_load_resources()` keeps the first and **deletes the second's
  cache file**. Overriding is not a refinement here, it is the difference between
  two children and one.
- **The identity is tested for before it is read.** `e_source_get_extension()`
  creates the extension it is asked for, and this vfunc is called on every
  `.source` in the cache directory — including other backends' — so reaching
  straight for `[Resource]` would hand an empty one to each. EDS's own default
  guards it the same way. `a_child_with_no_identity_is_not_claimed_and_is_not_given_one`
  asserts the absence afterwards, not just the `None`.
- **A panic becomes NULL, and NULL deletes the file.** There is no better
  answer available: `dup_resource_id` has no `GError` and no "ask me again"
  sentinel. The guard logs a critical, which is the only trace such a bug can
  leave; this is written down in the trampoline rather than left implied.
- **A source carrying both kind extensions reads as an address book.** Chosen
  rather than derived — this backend never writes one — and it is the same
  precedence `collection_backend_child_added()` applies. Returning `None` there
  was the alternative and is not obviously better: both outcomes end in a
  deleted file, and one of them at least round-trips.

Also pinned, in `eds-sys/tests/layout.rs`: `ECollectionBackend`/`…Class` against
`g_type_query`, and the four vfunc slots the backend will override
(`populate`, `dup_resource_id`, `child_added`, `child_removed`). Both passed on
first run — they are pins on a surface the bindings already generated correctly,
not a fix — but `ECollectionBackend` is the one class whose vfuncs
`evolution-source-registry` dispatches *itself* rather than in a factory
subprocess, so a layout drift there misfires in the process that owns every
account.

**Not covered by a test, and the honest limits:**

1. **Still no `populate`, and so still no fan-out.** Nothing calls
   `e_collection_backend_new_child`, nothing applies `Child::settings`, nothing
   removes what `Fanout::is_obsolete` names. This session made the *reading*
   half sound; the writing half is the next increment and the larger one.
2. **No module entry point, so nothing is loadable yet.** The crate builds an
   rlib only — a `module-jmap-backend.so` exporting nothing is a file
   `evolution-source-registry` would dlopen and learn nothing from. The cdylib
   and the `cmake/Backends.cmake` install rule land with `e_module_load`.
3. **Never driven by EDS.** Every call goes through the class struct from a
   *detached* instance (zeroed parent bytes), which is sound for this vfunc
   precisely because it never touches the backend — but it means "EDS calls this
   with the sources from the cache directory" is read off
   `e-collection-backend.c`, not demonstrated. A real instance needs an
   `ESourceRegistryServer` and a running `evolution-source-registry`, which this
   VM does not have.
4. **The mail children question is still open**, and it now has a second edge:
   `resource_id_of` answers `None` for a `[Mail Account]` source, which is
   correct today (this backend creates none) and would become a deletion the day
   it does. Whoever adds mail children has to add them to `KIND_EXTENSIONS` in
   the same commit.

Not verified locally, as in every session so far: `reuse lint` and `cargo deny`
(neither binary is on this VM). Four new files, each with the SPDX
`GPL-3.0-or-later` header. `cargo fmt --check`, `cargo test --locked` (491
tests on the default members, unchanged — the new crate is not among them) and
`cargo clippy --all-targets --locked -- -D warnings` are clean, as is
`cargo test`/`clippy` over the six EDS crates (`eds-sys`, `jmap-backend-core`,
`jmap-backend-book`, `jmap-backend-cal`, `jmap-mail`, `jmap-backend-collection`
— 14 new tests). `RUSTDOCFLAGS=-D warnings cargo doc -p jmap-backend-collection`
is clean; the two pre-existing private-intra-doc-link failures in `jmap-ical`
and `jmap-mock` noted last session are untouched.

No milestone tag: M6 has a backend now, but not one that fans anything out.

Next in M6: `populate` — reading `Parts::from_collection` off the collection
source's `[Collection]` extension and its `Connection` off `[Authentication]`/
`[Security]`, calling `Fanout::discover`, `e_collection_backend_new_child` per
`Child`, applying `Child::settings` to each, and `e_source_remove_sync` on the
children `Fanout::is_obsolete` names. It cannot be driven by EDS here either, so
the increment that writes it must be marked *needs human verification in real
Evolution*.

## 2026-08-09 (ninety-second session)

**What the account says before anything is asked of the server.** Last session
made the *child* half of the collection backend's `ESource` reads sound
(`resource_id_of`, and the `dup_resource_id` slot it fills). This session did the
other half and the other source: the collection source itself — the account EDS
hands the backend, which is the whole description of what to populate.

New module `rust/crates/jmap-backend-collection/src/collection_source.rs`, two
functions and a struct:

- `parts_of(*mut ESource) -> Parts` — `e_source_get_enabled` and, when the source
  has a `[Collection]` extension, its three flags, through
  `Parts::from_collection`.
- `server_of(*mut ESource) -> Result<Server, SourceError>` — `[Authentication]`
  host/port/user/method and `[Security]` secure, validated through
  `jmap_backend_core::source::origin`.
- `Server { origin, connection }` — the origin *this* backend fetches
  `/.well-known/jmap` from and the `Connection` each child repeats, out of one
  read of one source.

Red first, and recorded as red: against a stub returning `Parts::ALL` and a
canned well-formed `Server`, 8 of the 11 tests in `tests/collection_source.rs`
failed. The three that passed against the stub are the ones a canned answer
satisfies — "a source that says nothing has all parts", "a well-formed account
reads back", "the origin and the children agree" — and they are there so the
other eight cannot be satisfied by answering everything the same way.

What was decided, and why:

- **Two functions, not one.** They fail differently and are wanted at different
  moments. An account with every part switched off has nothing to populate and
  must not be reported broken merely because its host field is also empty:
  `populate` asks `parts_of` first, returns when `Parts::any` is false, and only
  then needs a server. Folding the two into one `Result` would turn a
  switched-off account into an error dialog.
- **But the origin and the children's connection come out of one read.** This
  backend contacts the server itself, and every child assembles its own origin
  at the far end from the fields `Child::settings` copied into it. Two reads of
  one source are two chances to disagree, and a disagreement is an account that
  discovers its collections from one server and fetches them from another. Hence
  `Server` carrying both, and `the_server_this_backend_contacts_is_the_one_its_children_are_given`
  holding the origin against what `Child::settings` actually writes.
- **The host rules are `jmap_backend_core::source::origin`'s, reached rather than
  repeated.** They matter twice here: this backend is the first thing to contact
  the server, and it is what *writes* the host into the children. A child
  re-validates what it was handed, but by then the string has been written into
  one `.source` file per collection — so `evil.example.com/x` and plain HTTP to a
  non-loopback host are refused here, before any child exists.
- **Nothing in this module creates an extension.** `e_source_get_extension()`
  creates the extension it is asked for, and the source in question is the user's
  *account* — the file EDS writes back to disk. So `[Collection]`, `[Security]`
  and `[Authentication]` are each tested for before they are read, and three
  tests assert the *absence* afterwards rather than only the value. The absences
  are the documented answers: no `[Collection]` is `Parts::ALL` (what
  `e_collection_backend_get_part_enabled()` answers), no `[Security]` is TLS (the
  same rule `SourceConfig` applies, and the one whose omission would silently
  downgrade every hand-written account and every child of it at once), and no
  `[Authentication]` is `MissingHost` — which an empty one would have produced
  anyway, minus the edit to the user's file.
- **A port nobody named stays unnamed.** The keyfile writes 0 for "not set", and
  passing it on would give the children `Port=0` and this backend an origin
  asking for port zero, rather than the scheme's default.

**Not covered by a test, and the honest limits:**

1. **Still no `populate`, so still no fan-out.** This is its input, not its
   body: nothing yet calls `Fanout::discover`, `e_collection_backend_new_child`
   or `e_source_remove_sync`. That remains the next and larger increment.
2. **Still no module entry point**, so nothing is loadable — the crate is an
   rlib. The cdylib and the `cmake/Backends.cmake` install rule land with
   `e_module_load`, as noted last session.
3. **Never driven by EDS.** Every source here is built with
   `e_source_new_with_uid` and the EDS setters, which is what EDS itself does
   for a source read from a keyfile — so the extension machinery is real — but
   "EDS hands the backend this source" is still read off `e-collection-backend.c`
   rather than demonstrated. A real one needs `evolution-source-registry` on the
   session bus, which this VM does not have.
4. **`e_collection_backend_get_part_enabled()`'s rules are taken from last
   session's reading of the EDS source**, not from EDS at runtime. If it has a
   third rule, this reads an account slightly wrong in a direction no test here
   can see.

Not verified locally, as in every session so far: `reuse lint` and `cargo deny`
(neither binary is on this VM). Two new files, each with the SPDX
`GPL-3.0-or-later` header. `cargo fmt --check`, `cargo test --locked` (491 tests
on the default members, unchanged — this crate is not among them) and
`cargo clippy --all-targets --locked -- -D warnings` are clean, as is
`cargo test`/`clippy` over the six EDS crates (`jmap-backend-collection` now 25
tests, up from 14). `RUSTDOCFLAGS=-D warnings cargo doc -p jmap-backend-collection`
is clean.

No milestone tag: M6 can now read an account, and still cannot fan one out.

Next in M6: `populate` itself — `parts_of`, then `server_of`, then
`Fanout::discover` against that origin, `e_collection_backend_new_child` per
`Child` with `Child::settings` applied, and `e_source_remove_sync` for each
cached child `Fanout::is_obsolete` names. It cannot be driven by EDS here, so
the increment that writes it must be marked *needs human verification in real
Evolution*.

## 2026-08-09 (ninety-third session)

**Onto the source, not just off it.** Every `ESource` read this crate does now
has a counterpart tested against a real source — but nothing had yet *written*
one. `Child::settings` says what a child is as `(group, key, value)` triples,
which is a description of a keyfile and proves nothing about an `ESource`;
`resource_id_of` and `SourceConfig::from_source` are tested against sources
built by hand with the EDS setters, which proves nothing about what this backend
writes. This session is the join between them.

New module `rust/crates/jmap-backend-collection/src/child_source.rs`:

- `apply(*mut ESource, &[Setting]) -> Result<(), UnwritableSetting>` — every
  setting a child is described by, onto the property it names: the source's own
  `display-name`, `ESourceBackend:backend-name` under the extension that *is*
  the child's kind, `ESourceResource:identity`, the four `[Authentication]`
  fields and `[Security] Method`.
- `EXTENSIONS` — the three non-kind keyfile groups paired with EDS's
  `E_SOURCE_EXTENSION_*` constants, the same two-spellings-of-one-string table
  as `KIND_EXTENSIONS` and held against the `#define`s by the same kind of test.
- `UnwritableSetting` — `UnknownProperty` and `WrongType`, with `Display`.

Red first, and recorded as red: against a stub that returned `Ok(())` and wrote
nothing, 7 of the 8 tests in `tests/child_source.rs` failed. The one that passed
is `every_setting_a_child_can_be_described_by_is_one_this_writes`, which a
do-nothing `apply` satisfies by construction — it is there to catch a setting
`jmap-collection-sync` grows later, not to be red today. (The ninth test, the
`EXTENSIONS`/`#define` pairing, was added after green as a pin, like
`tests/resource_id.rs`'s.)

Four of the four `EXTENSION_*` group constants in
`jmap-collection-sync::child_source` became `pub` for this; they were already
the strings that crate writes into its `Setting`s, so nothing changed but who
can name them.

What was decided, and why:

- **An unknown setting is refused, not skipped.** Skipping is the worse failure
  by a wide margin: the child is still created, still looks like an address book
  of this account, and is missing whichever one property makes it work —
  `[Resource] Identity`, whose absence makes EDS *delete* the child's cache file
  on the next start, or `[Authentication] Host`, whose absence sends every
  request the address book backend makes to no server. The settings are a closed
  set produced by this project's own crate, so an `UnwritableSetting` means that
  crate grew a setting this module was not taught to write — a red test here
  rather than a broken account there.
- **`Child::settings` is gone through, not around.** The obvious shortcut is to
  take a `&Child` and a `&Connection` and call the typed setters directly. That
  would leave two descriptions of what a child is, one tested as data in
  `jmap-collection-sync` and one tested as behaviour here, free to drift.
- **`e_source_get_extension` is called *for* the thing the read side avoids.**
  Both readers in this crate go out of their way not to call it, because it
  creates the extension it cannot find. Here creating them is the point: giving
  the source `[Address Book]` is exactly what makes it an address book to
  `collection_backend_child_is_contacts()` and to the factory that loads it.
  What it must not do is create the *other* kind's, so nothing reaches for an
  extension a setting did not name, and
  `a_child_carries_the_extension_of_its_own_kind_and_not_the_other` asserts the
  absence rather than only the presence.
- **`[Security] Method` is written as the string, and read back as the
  boolean.** `Child::settings` writes "tls"/"none"; the JMAP backends read the
  derived `ESourceSecurity:secure`. Those are the same question only if EDS
  spells its secure method the way that crate does — so `apply` calls
  `e_source_security_set_method()` rather than `…_set_secure()`, and the test
  reads `e_source_security_get_secure()` back. It agrees, which is now a fact on
  the record rather than a reading of EDS's source.
- **A NUL in a display name truncates rather than fails.** It is the one setting
  whose value is server data, a JSON string may carry an escaped NUL and a C
  string may not, and refusing the write would mean refusing the child. Handled
  by `jmap_backend_core::error::cstring_lossy`, which is what every other string
  crossing this boundary already uses.
- **A `Port` that is not a number is `WrongType`, not a silently unset port.**
  `Child::settings` can only produce a `u16`'s decimal form, so this is
  unreachable through the intended caller; the conversion is nevertheless this
  module's, so its failure is too.

**Not covered by a test, and the honest limits:**

1. **Still no `populate`, so nothing calls this yet.** It is the second of the
   two halves that one needs (the account read landed last session); what is
   left is the body — `Fanout::discover`, `e_collection_backend_new_child`,
   `apply` per child, `e_source_remove_sync` for the obsolete ones.
2. **The source written here is never handed back to EDS.** These are
   `e_source_new_with_uid` sources with a NULL D-Bus object, which is what EDS
   itself builds from a keyfile — so the extension machinery and the property
   setters are real — but "EDS writes this child out and reads it back on the
   next start" is still read off `e-collection-backend.c`. A real round trip
   needs `evolution-source-registry` on the session bus, which this VM has not
   got. **Needs human verification in real Evolution.**
3. **`apply` leaves a half-written child behind on error.** Deliberate — there
   is nothing to roll back to, since the source is fresh — but it means the
   caller's only correct answer to an `UnwritableSetting` is to abandon the
   child, and there is no caller yet to hold to that.

Not verified locally, as in every session so far: `reuse lint` and `cargo deny`
(neither binary is on this VM). Two new files, each with the SPDX
`GPL-3.0-or-later` header. `cargo fmt --check`, `cargo test --locked` (491 tests
on the default members, unchanged) and `cargo clippy --all-targets --locked --
-D warnings` are clean, as is `cargo test`/`clippy` over the six EDS crates
(`jmap-backend-collection` now 34 tests, up from 25).
`RUSTDOCFLAGS=-D warnings cargo doc` is clean for `jmap-backend-collection` and
`jmap-collection-sync`.

No milestone tag: M6 can now read an account and write a child, and still cannot
fan one out.

Next in M6: `populate` itself — `parts_of`, then `server_of`, then
`Fanout::discover` against that origin, `e_collection_backend_new_child` per
`Child` with `child_source::apply` over its settings, and `e_source_remove_sync`
for each cached child `Fanout::is_obsolete` names. It cannot be driven by EDS
here, so the increment that writes it must be marked *needs human verification
in real Evolution*.

## 2026-08-09 (ninety-fourth session)

**The half of a populate that deletes.** `populate` is two loops — one that
creates children and one that removes them — and only the second can be written
without settling how a collection backend gets its credentials. It is also the
one that destroys data, so it went first.

New module `rust/crates/jmap-backend-collection/src/removal.rs`:

- `obsolete(&Fanout, &[*mut ESource]) -> Vec<*mut ESource>` — of the children
  this collection already has, the ones this populate must remove. The decision
  is `Fanout::is_obsolete`'s; what is here is the join to the `ESource`, which is
  `resource_id_of` per child and nothing else.
- `remove_obsolete(&Fanout, &[*mut ESource]) -> Vec<NotRemoved>` — the same
  choice, carried out with `e_source_remove_sync()`, reporting rather than
  raising.
- `NotRemoved { resource_id, message }` — a child that could not be removed and
  what EDS said about it.

Red first, and recorded as red: against a stub returning empty vectors, 5 of the
7 tests in `tests/removal.rs` failed. The two that passed are
`a_child_of_a_part_the_user_switched_off_is_kept` and
`a_source_this_backend_did_not_write_is_never_removed` — a populate that removes
nothing keeps everything, so they are satisfied by construction. They are there
because they are the two ways this module can delete a user's offline cache, and
they have to stay green as it grows a caller.

What was decided, and why:

- **Every source judged is one this backend wrote, by the code that writes it.**
  The test sources are built by `child_source::apply` from a `Child`, not shaped
  by hand. `Fanout::is_obsolete` is tested in `jmap-collection-sync` against
  resource id *strings* and `apply` is tested here against the properties it
  writes; neither covers the join, and the join is the whole risk — a resource id
  this backend cannot read back off a source it wrote itself is not a mislabelled
  sidebar row, it is `e_source_remove_sync()` on a child that should have been
  kept, or a child kept forever that should have gone.
- **A source with no resource id of ours is never removed.** `None` from
  `resource_id_of` means a child of another collection backend, one written by a
  future version of this one, or a hand-edited file. "I cannot read this" and
  "this is obsolete" are indistinguishable from the removal's side and only one
  of them is recoverable, so the unreadable child is left alone.
- **A failed removal is reported, not raised, and does not stop the ones after
  it.** `ECollectionBackendClass::populate` returns `void`: there is no `GError`
  to fill and nobody to hand one to. So `remove_obsolete` removes what it can and
  hands back one `NotRemoved` per refusal for the caller to log; the next
  populate finds the same children and asks again, which is the whole of the
  recovery available. Abandoning the loop at the first refusal would leave the
  sidebar half-cleaned with no error anywhere to say so.
- **The resource id is read once, not twice.** `named_obsolete` keeps the id it
  judged, so the name a child is removed under is the name it was judged under.
- **A `TRUE` that also set a `GError` frees it; a `FALSE` that set none still
  produces a report.** Neither is EDS's behaviour, both are cheap, and the second
  is the difference between a silent non-removal and a logged one.

**Verified, not read off a header:** `e_source_remove_sync()` on a source with no
D-Bus object fails rather than blocking or aborting — it answers `FALSE` with
`Data source "…" is not removable`. That is the one branch of the call this
machine can drive, and it happens to be exactly the branch a populate has to
survive. The tests assert the message is non-empty rather than its wording,
which is translated.

**Not covered by a test, and the honest limits:**

1. **The success branch of `e_source_remove_sync` is never taken here.** Every
   source in the suite is detached, so every removal is refused. That a removal
   which EDS *accepts* takes the child, its `.source` file and its cache — and
   that this module then reports nothing — is read off EDS, not observed.
   **Needs human verification in real Evolution.**
2. **Still no caller.** `children` will come from
   `e_collection_backend_list_contacts_sources()` and `…_list_calendar_sources()`,
   which need a live `ESourceRegistryServer`. Nothing in this crate can produce
   that list yet, so the contract "these are the children of this collection" is
   this module's precondition rather than something it checks.
3. **The piece between the account and the fan-out is still unsettled**, and it
   is what blocks `populate` as a whole: `Fanout::discover` wants a connected
   `Client`, and when EDS makes credentials available to a *collection* backend
   (as against the book and calendar backends, which are handed an
   `ENamedParameters`) is not something this VM can be made to demonstrate. That
   is the next session's first question, and it is a reading-and-deciding
   question before it is a code one.

Not verified locally, as in every session so far: `reuse lint` and `cargo deny`
(neither binary is on this VM). Two new files, each with the SPDX
`GPL-3.0-or-later` header. `cargo fmt --check`, `cargo test --locked` (491 tests
on the default members, unchanged) and `cargo clippy --all-targets --locked --
-D warnings` are clean, as is `cargo test`/`clippy` over the six EDS crates
(`jmap-backend-collection` now 41 tests, up from 34).
`RUSTDOCFLAGS=-D warnings cargo doc` is clean for `jmap-backend-collection`.

No milestone tag: M6 can now read an account, write a child and remove one, and
still cannot fan one out.

Next in M6: the credentials question above, and then `populate` itself —
`parts_of`, `server_of`, a client, `Fanout::discover`, an
`e_collection_backend_new_child` plus `child_source::apply` per `Child`, and
`removal::remove_obsolete` over the children EDS lists. It cannot be driven by
EDS here, so the increment that writes it must be marked *needs human
verification in real Evolution*.

## 2026-08-09 (ninety-fifth session)

**Where a collection backend's credentials come from.** The previous session
stopped on it and called it "a reading-and-deciding question before it is a code
one", so this session read the headers and then wrote the answer down as a
module with tests.

The answer is that `populate` is *not* where the fan-out happens.
`ECollectionBackendClass::populate` returns `void`, is handed no credentials and
has nowhere to put a prompt. What EDS gives a collection backend instead is its
grandparent's vfunc, `EBackendClass::authenticate_sync`:

```c
ESourceAuthenticationResult (*authenticate_sync) (EBackend *backend,
                                                  const ENamedParameters *credentials,
                                                  gchar **out_certificate_pem,
                                                  GTlsCertificateFlags *out_certificate_errors,
                                                  GCancellable *cancellable,
                                                  GError **error);
```

and the loop is: a `populate` that needs the server calls
`e_backend_schedule_credentials_required()`, `evolution-source-registry` resolves
the password (libsecret, OAuth2, or a prompt through Evolution), and calls back
into `authenticate_sync` with an `ENamedParameters` — the same shape
`connect_sync` on the book and calendar backends is handed. **Read off the
installed 3.52 headers, not guessed:** `e-backend.h` declares the vfunc and the
three `e_backend_credentials_required*` entry points, and
`e-webdav-collection-backend.h` declares
`e_webdav_collection_backend_discover_sync (…, const ENamedParameters *credentials,
gchar **out_certificate_pem, …) -> ESourceAuthenticationResult` — EDS's own
collection backend has exactly this signature for exactly this reason. So the
credentials never come from this crate and never from a config file, and the
fan-out belongs inside the call that receives them.

New module `rust/crates/jmap-backend-collection/src/authenticate.rs`:

- `authenticate_with(source, credentials, cancellable, error, fan_out) ->
  ESourceAuthenticationResult` — that vfunc minus the instance.
- `Login { server, parts, credentials }` — everything a fan-out needs, out of
  one read of the account and one set of credentials from EDS.

`fan_out` is a closure because it is the only part that needs a live
`ECollectionBackend`: `e_collection_backend_new_child()` and
`e_collection_backend_list_*_sources()` are instance methods, and none of the
decisions above are about children at all.

Red first, and recorded as red: against a stub returning `ERROR` and calling
nothing, all 12 tests in `tests/authenticate.rs` failed (0 passed). Green now.

What was decided, and why:

- **Parts are read before the server, and that order is the test that pins it.**
  An account with every part switched off is `ACCEPTED` — not `ERROR`, which
  would put a dialog in front of someone for an account they deliberately turned
  down, and not `REJECTED`, which would discard a password that was never tried
  — and it is accepted *without the fan-out running*, so nothing is contacted.
  Asking for the host first would report a half-written account as broken the
  moment its owner unticked the last part. This is the ordering
  `collection_source`'s module comment already documented; here it is enforced.
- **`ESourceAuthenticationResult` is not a status code, it is what Evolution
  does next.** `REQUIRED` is the prompt; anything else for an account with no
  password yet is an account that can never be completed. `REJECTED` discards
  the stored password; answering it for a 403 or a server that is down asks
  someone to fix something a password cannot fix, forever. The 401-and-only-401
  rule is *not* restated here — it is `ConnectError::auth_result`'s, in
  `jmap-backend-core`, reached through `ConnectError::from(jmap_client::Error)`,
  because `connect_sync` answers the same question with the same enum and a rule
  like that written twice is a rule corrected once.
- **A `GError` on every non-`ACCEPTED` path and on none of the accepting ones.**
  GLib's convention read against an enum whose only success is `ACCEPTED`. EDS
  reads the out-parameter whatever the result was, so a stale error is how an
  account that is fine gets reported as broken.
- **`out_certificate_pem` / `out_certificate_errors` are deliberately not
  filled in.** They are how a backend invites Evolution to offer "trust this
  certificate?". TLS here is `ureq`'s and the system trust store's, and a
  certificate this code cannot see is one it must not invite anyone to accept.
  The cost is honest and small: a self-signed JMAP server fails with an error
  rather than a trust dialog.
- **The cancellable is observed for the length of the fan-out and no longer** —
  `jmap_backend_core::cancel::observe`, same as every other vfunc. A test drives
  an already-cancelled `GCancellable` through the call and asserts both halves:
  the fan-out sees `jmap_client::transport::observed()` cancelled, and after the
  call this thread observes nothing again. A flag that outlived the call would
  belong to the *account*, and an authenticate someone stopped would leave every
  later request on the thread refusing.
- **An empty stored password is sent, not prompted for.** `marshal::password`'s
  rule, now pinned at this layer too: reading it as absent would prompt, and a
  user who answers the prompt with nothing would be prompted again forever.

**Not covered by a test, and the honest limits:**

1. **The vfunc slot is not installed.** `class_init` still only overrides
   `dup_resource_id`; wiring `authenticate_sync` means writing the fan-out body,
   which needs the instance. Until then `authenticate_with` has no caller, like
   `removal::remove_obsolete` before it.
2. **That EDS actually calls `authenticate_sync` on a collection backend after
   `e_backend_schedule_credentials_required()`** is read off the 3.52 headers
   and EDS's own WebDAV collection backend, not observed — this VM has no
   `evolution-source-registry` on a session bus. **Needs human verification in
   real Evolution.**
3. **Nothing here talks to a server.** The fan-out is a closure in every test,
   so what is verified is the classification and the plumbing, not
   `Fanout::discover` against `jmap-mockd` through this path.

Not verified locally, as in every session so far: `reuse lint` and `cargo deny`
(neither binary is on this VM). Two new files, each with the SPDX
`GPL-3.0-or-later` header. `cargo fmt --check`, `cargo test --locked` (491 tests
on the default members, unchanged) and `cargo clippy --all-targets --locked --
-D warnings` are clean, as is `cargo test`/`clippy` over the six EDS crates
(`jmap-backend-collection` now 53 tests, up from 41).
`RUSTDOCFLAGS=-D warnings cargo doc` is clean for `jmap-backend-collection`.
`jmap-backend-collection` gained two dependencies it did not have —
`evolution-jmap-client` (for `Credentials`) and `gio-sys` (for `GCancellable`).

No milestone tag: M6 can now read an account, write a child, remove one, and say
what it authenticates as — and still cannot fan one out.

Next in M6: the fan-out body itself, which is now the only thing left before a
`populate` exists — `Fanout::discover` against `Login::server`, an
`e_collection_backend_new_child` plus `child_source::apply` per `Child`, and
`removal::remove_obsolete` over `e_collection_backend_list_contacts_sources()`
and `…_list_calendar_sources()`. Then the two vfunc slots: a `populate` that
schedules credentials, and the `authenticate_sync` that does the work. None of
that can be driven by EDS here, so the increment that writes it must be marked
*needs human verification in real Evolution*.

## 2026-08-09 (ninety-sixth session)

**The fan-out body.** The one thing M6 could not do: read an account, write a
child and remove one, and never actually turn *one login* into a set of
children. `jmap-backend-collection/src/fan_out.rs` is that, and it is the last
piece of M6 that is not a vfunc slot.

New module, ~10 tests in `tests/fan_out.rs`:

- `Collection` — an `unsafe trait` holding the four `ECollectionBackend`
  instance methods a fan-out needs and nothing else of a GObject:
  `new_child`, `is_new_child`, `publish`, `existing_children`.
- `fan_out(collection, login)` — `Client::connect` against `Login::server`,
  `Fanout::discover` under `Login::parts`, then the rest.
- `apply_fanout(collection, fanout, connection)` — the fan-out minus the
  network, so a hand-built `Fanout` can drive every shape.
- `adopt(collection, resource_id, settings)` — one child: created, written in
  full, exported only if new.
- `Populated` — what the fan-out did, as the log line a `void`-returning
  `populate` can write: `children`, `uncreated`, `abandoned`, `not_removed`.

Red first, and recorded as red: against stubbed `apply_fanout`/`adopt` bodies
(returning `Populated::default()` and `Adopted::Uncreated`, calling nothing) 8
of the 10 tests failed. The two that passed are the two that assert *nothing*
happens — a switched-off part and an unreachable server — which is what a stub
does by construction. Green now, all 10.

**The protocol was read off EDS's own source, not guessed.** `/tmp` on this VM
still had `e-collection-backend.c` and `e-webdav-collection-backend.c` from an
earlier session, and they answer three things the header does not:

1. `e_collection_backend_new_child()` is `(transfer full)` — "drawn from a cache
   of previously used sources indexed by @resource_id" — and "the returned data
   source **should be passed to `e_source_registry_server_add_source()`** to
   export it over D-Bus". So creating a child and exporting it are two calls,
   and a child that is only created is a child Evolution cannot see. That was
   the single biggest thing this module could have got silently wrong.
2. EDS's own `EWebDAVCollectionBackend` makes that second call under exactly one
   condition, `if (is_new)` — `e_collection_backend_is_new_source()`. A child
   drawn from the cache was already exported by the `populate` that claimed it.
   Copied rather than invented, and it is `is_new_child` in the trait.
3. `e_collection_backend_claim_all_resources()` belongs to `populate`, not here.
   It is what makes an account's address books appear in the sidebar *offline*,
   before a password exists, and it has to happen whether or not a fan-out ever
   runs. So this module does not touch it; the `populate` slot will.

What was decided, and why:

- **The existing children are listed before a single new one is created**, and
  that order is a test (`the_children_a_collection_has_are_listed_before_any_new_one_is_created`,
  asserting `existing_children` is the first trait call). A list taken *after*
  the additions would contain children this same fan-out had just created. They
  would not be judged obsolete — they are in the fan-out by construction — but
  then what keeps them safe is an accident of what `Fanout::is_obsolete` happens
  to answer rather than of what was asked. EDS's WebDAV backend snapshots its
  `known_sources` before discovery for the same reason.
- **Nothing half-written is exported.** `adopt` writes every setting before it
  publishes any of it; a setting it cannot write means the child is dropped
  unexported. That is `child_source`'s rule followed to its consequence: the two
  properties whose absence matters are `[Resource] Identity`, whose absence makes
  EDS delete the child's cache, and `[Authentication] Host`, whose absence points
  the child at no server — and a child Evolution never sees has neither problem.
  For a child drawn from the cache the damage is already done and all this can do
  is report it; that is the honest limit of a write with no transaction.
- **One child EDS refuses costs that child and no other.** `new_child` answers
  NULL when it cannot claim a resource (it warns and returns NULL). One row
  missing from the sidebar is not the same failure as an account missing, so the
  loop continues and the resource id goes in `Populated::uncreated`.
- **The error type is `jmap_client::Error`, and it is the connection's only.**
  Everything that fails per child is in `Populated`, because a login that worked
  is not a failure because one address book of it could not be written — and the
  layer above (`ConnectError::from`) is what turns a connection failure into the
  enum Evolution re-prompts on. A test drives a dead port and asserts the
  collection was not touched at all: a populate whose server is down must not be
  the populate that empties the sidebar.
- **The instance is a trait for the same reason it was a closure.** Four methods,
  so the decisions above are testable against a real `jmap-mockd`, real
  `ESource`s built by `e_source_new_with_uid`, the same `child_source::apply` the
  backend calls and the same `resource_id_of` the `dup_resource_id` vfunc answers
  with. What is stubbed is the part that needs a session bus, and only that part.
- **Reference counting is spelled out in the trait's contract.** Both EDS getters
  behind it are `(transfer full)`, so the fan-out consumes every reference it is
  handed — `new_child`'s after the write and the publish, `existing_children`'s
  after the removals. The test collection takes a `g_object_ref` before returning
  each pointer, which is both what EDS does and what keeps the sources alive for
  the assertions.

**Not covered by a test, and the honest limits:**

1. **The two vfunc slots are still not installed.** `class_init` overrides only
   `dup_resource_id`. `populate` (claim the cached children, then
   `e_backend_schedule_credentials_required`) and `authenticate_sync` (run
   `fan_out` with a `Collection` the instance implements) are what is left of M6,
   and both need a live `ECollectionBackend`.
2. **`e_source_registry_server_add_source` is not bound yet.** `eds-sys` allows
   `e_collection_backend_.*` but not `e_source_registry_server_.*`, so the
   `publish` method's real body — and the `ESourceRegistryServer` the
   `e_collection_backend_ref_server()` hands back — arrive with the slot that
   needs them. The trait is where that call is *documented*; it is not yet a call
   this crate makes. **Needs human verification in real Evolution** that a child
   published this way appears in the sidebar.
3. **`Adopted::Abandoned` cannot be reached through `apply_fanout` today.**
   `Child::settings` is a closed set that `child_source::apply` can write all of;
   the abandoned path is reachable the moment `jmap-collection-sync` grows a
   setting this crate was not taught to write, which is precisely when the child
   must not be exported. That is why `adopt` takes `settings` as a parameter: the
   test hands it an unparseable `[Authentication] Port` directly and asserts
   nothing was published.
4. **Every removal in the tests fails**, as in `tests/removal.rs`: these sources
   have no D-Bus object, so `e_source_remove_sync` refuses. What the tests pin is
   which children the fan-out *asked* to remove, via `Populated::not_removed` —
   the successful-removal branch stays unreachable on this machine.

Not verified locally, as in every session so far: `reuse lint` and `cargo deny`
(neither binary is on this VM). Two new files, each with the SPDX
`GPL-3.0-or-later` header. `cargo fmt --check`, `cargo test --locked` (491 tests
on the default members, unchanged) and `cargo clippy --all-targets --locked --
-D warnings` are clean, as is `cargo test`/`clippy` over the six EDS crates
(`jmap-backend-collection` now 63 tests, up from 53).
`RUSTDOCFLAGS=-D warnings cargo doc` is clean for `jmap-backend-collection`.
`jmap-backend-collection` gained one dev-dependency, `evolution-jmap-mock`.

No milestone tag: M6 can now read an account, authenticate it, and fan one login
out into children — and still has no vfunc slot for EDS to reach any of it
through.

Next in M6: the two slots, and they are the whole of what is left. `populate`
first, since it is the one that runs offline —
`e_collection_backend_freeze_populate`/`thaw_populate` around a chain-up, then
`claim_all_resources` + `new_child` + `add_source` per cached child, then
`e_backend_schedule_credentials_required` when any part is on. Then
`authenticate_sync`, which is `authenticate_with` with a closure that calls
`fan_out` against a `Collection` implemented over the instance. Both need
`e_source_registry_server_.*` in `eds-sys`'s allowlist, and neither can be
driven by EDS here, so the increment that writes them must be marked *needs
human verification in real Evolution*.

## 2026-08-09 (ninety-seventh session)

**The `populate` slot, installed.** The first of the two vfunc slots M6 was
missing, and the one that runs *offline*: `jmap-backend-collection/src/populate.rs`
plus the slot in `class_init`. An account's address books and calendars now have
a path from their cached `.source` files to the sidebar that does not go through a
login.

New module, 10 tests in `tests/populate.rs`, and three more in
`tests/collection_source.rs` for the one account field a populate needs:

- `Populating` — an `unsafe trait` holding the seven `ECollectionBackend`/
  `EBackend` calls a populate makes and nothing else of a GObject: `freeze`,
  `thaw`, `chain_up`, `claim_all_resources`, `publish`, `request_credentials`,
  `authenticate_anonymously`.
- `populate(collection, parts, user) -> Option<Restored>` — the vfunc body minus
  the instance. `None` is a populate that lost the freeze.
- `Restored` — what it did, for the log line a `void`-returning vfunc can write:
  `children`, `unidentified`, `asked`.
- `Asked` — which of EDS's two "authenticate me" calls was made, if either.
- `collection_source::user_of` — whom the account authenticates as, read without
  `server_of` being involved.
- `backend.rs`: the `populate` slot, a `Live` struct implementing `Populating`
  over the real instance (one line per method), `parent_class()` for the
  chain-up, and `debug_print` onto EDS's own `e_source_registry_debug_print`
  channel.

Red first, and recorded as red: against a stub `populate` returning
`Some(Restored::default())` and calling nothing, 9 of the 10 tests failed. The
one that passed is `every_reference_the_claim_handed_over_is_given_back`, which a
stub that claims nothing satisfies by construction — the same shape as last
session's two.

**One decision deliberately departs from EDS's own WebDAV backend, and it is the
main thing this session got right.** `EWebDAVCollectionBackend::populate` calls
`e_collection_backend_new_child()` for each claimed source before exporting it,
and last session's plan (in this log) said to copy that. Reading
`e-collection-backend.c` through says not to:

- `e_collection_backend_claim_all_resources()` *empties* the `unclaimed_resources`
  table it draws from — "previously used sources can only be claimed once".
- `collection_backend_claim_resource()`, which is all `new_child` is, looks in
  exactly two places: that now-empty table, and the backend's `children` table.
  The claimed sources are not in `children` either — that table is filled from
  the `child-added` signal, and nothing has exported them yet.
- So a `new_child()` after the claim finds neither and takes the third branch:
  `collection_backend_new_user_file` + `collection_backend_new_source`, i.e. a
  brand-new `EServerSideSource` with a fresh uid, recorded in `new_sources`. The
  WebDAV backend then passes the **claimed** source to `add_source` and
  unreferences the new one, so the only trace of it is
  `e_collection_backend_is_new_source` answering TRUE for a uid nothing holds.

Copying that would mean minting and discarding one source per cached child per
populate. So this populate exports the claimed source directly, which is what
`claim_all_resources()`'s own documentation asks for ("export the remaining
instances with `e_source_registry_server_add_source()`").

**And the pairing that `new_child` might have looked necessary for happens
anyway**, verified end to end in EDS's source rather than assumed. Fetched
`e-source-registry-server.c` from GNOME's GitLab (`master`; the 3.52 branch name
404s) to close the last link:
`e_source_registry_server_add_source` → emits `source-added` →
`collection_backend_source_added_cb` recognises a source whose parent is its own
collection and emits `child-added` → `collection_backend_child_added` →
`collection_backend_children_insert`. And `collection_backend_ref_child_source`
walks that same `children` table asking `dup_resource_id` about each entry. So
the fan-out's later `new_child(resource_id)` finds this child and reuses it
instead of creating a second source for the collection.

Other decisions, and why:

- **The freeze is a debt, not a lock.**
  `e_collection_backend_freeze_populate` is `return !g_atomic_int_add (&count, 1)`
  — it increments whatever it answers — so the populate that *lost* the race
  still owes a thaw, which is why EDS spells the guard
  `if (!freeze) { thaw (); return; }`. Both halves are a test: the loser calls
  `freeze`, `thaw` and nothing else, and the counter is left where the winner put
  it. And the thaw is a `Drop` impl rather than a statement at the end, because
  the panic guard in front of the vfunc cannot undo a freeze — a panic between
  the two would silence *this account's* populate for the life of the process.
  There is a test that panics inside `publish` and asserts the counter came back
  to zero.
- **The cached children are exported whatever the account's parts say.** A child
  of a switched-off part is dormant, not gone: EDS binds each child's `enabled`
  to the account's part flag, so withholding it would make it vanish from the
  sidebar *and* leave its resource id unclaimed — and the next populate that
  found the part switched back on would create a fresh source with a fresh uid
  beside the cached file. That is the same destruction `Fanout::is_obsolete`
  refuses to do, reached from the other side.
- **Credentials are asked for only when contacts or calendars is on**, which is
  EDS's WebDAV condition and, here, the honest one: this backend creates no mail
  children yet, so a mail-only account would spend a password prompt to produce
  nothing anyone can see.
- **A password is asked for only when the account names a user.** Otherwise
  `e_backend_schedule_authenticate (backend, NULL)`, because
  `jmap_backend_core::connect::credentials` reads an account with no user as
  anonymous *on purpose* — asking for a password there would prompt someone who
  needs none and then drop what they typed. `user_of` reads an empty `User=` as
  no user for the same reason: `read_string` already does, and the two spellings
  must not decide differently. That is a test.
- **`user_of` and not `server_of`.** A populate needs one field of
  `[Authentication]` and must not fail on the host: an account with a user and a
  broken host has to reach `authenticate_sync`, which is the vfunc that has a
  `GError` to say so through. Tested with a host `server_of` refuses.
- **A claimed source this backend cannot name is dropped unexported and
  counted.** Unreachable through EDS, which only caches a source
  `dup_resource_id` answered for — and defined anyway, because exporting it would
  put a child in the sidebar that no resource id can be paired with again (see
  `ref_child_source` above), so every later populate would recreate it. Same
  shape as `Adopted::Abandoned`.
- **`eds-sys` needed no change.** Last session's note that
  `e_source_registry_server_.*` was not allowlisted is wrong: the existing
  `e_source_.*` pattern already matches it, and
  `e_source_registry_server_add_source`, `e_collection_backend_claim_all_resources`,
  `_freeze_populate`, `_thaw_populate`, `_ref_server`,
  `e_backend_schedule_credentials_required`, `e_backend_schedule_authenticate`
  and `E_SOURCE_CREDENTIALS_REASON_REQUIRED` were all already in the generated
  bindings. Checked in `bindings.rs` rather than assumed.

**Not covered by a test, and the honest limits:**

1. **The vfunc body itself cannot be driven here.** `populate`'s first act is
   `e_collection_backend_freeze_populate` on the instance, so unlike
   `dup_resource_id` it cannot be driven from `JmapCollectionBackend::detached()`
   — that would be undefined behaviour, and the `detached` doc comment now says
   so. What `tests/backend.rs` can hold is the slot: two new tests assert it is
   installed, that it is not EDS's placeholder, and that the pointer the chain-up
   walk finds is the placeholder and not our own. **Needs human verification in
   real Evolution** that a JMAP account's cached address books and calendars
   appear in the sidebar offline and that a password is asked for exactly once.
2. **`Live`'s seven method bodies are unverified**, by construction: they are the
   part that needs a session bus. Each is one EDS call, with the ownership spelled
   out in a SAFETY comment — the `(transfer full)` list from
   `claim_all_resources` (`g_list_free`, not `_full`, and one `g_object_unref` per
   source once it has been published) and the `(transfer full)` server from
   `ref_server`.
3. **`Restored.unidentified` reaches `log_critical`, and the rest reaches EDS's
   debug channel**, which is silent unless `SOURCE_REGISTRY_DEBUG` is set. That
   is all a `void` vfunc has.

Not verified locally, as in every session so far: `reuse lint` and `cargo deny`
(neither binary is on this VM). Two new files, each with the SPDX
`GPL-3.0-or-later` header. `cargo fmt --check`, `cargo test --locked` (491 tests
on the default members, unchanged) and `cargo clippy --all-targets --locked --
-D warnings` are clean, as is `cargo test`/`clippy` over the six EDS crates
(`jmap-backend-collection` now 78 tests, up from 63).
`RUSTDOCFLAGS=-D warnings cargo doc` is clean for `jmap-backend-collection`.

No milestone tag: M6's `authenticate_sync` slot is still missing, so EDS can
reach the offline half of this backend and not the fan-out.

Next in M6, and it is the last of it: `authenticate_sync` on
`EBackendClass` — `authenticate_with` (already written and tested) with a closure
that calls `fan_out` against a `Collection` implemented over the instance, the
same way `Live` implements `Populating` here. Every EDS call it needs
(`e_collection_backend_new_child`, `_is_new_source`, `_list_contacts_sources`,
`_list_calendar_sources`, `e_source_registry_server_add_source`) is already in
the bindings, and it needs one more `parent_class()` — `EBackendClass`'s, not
`ECollectionBackendClass`'s, since that is where the slot lives. It cannot be
driven here either, so the increment that writes it is *needs human verification
in real Evolution* too, and after it M6 is a module entry point and a CMake
target away from being installable.

## 2026-08-09 (ninety-eighth session)

**The `authenticate_sync` slot, installed — the last vfunc M6 was missing.**
`populate` (last session) asks EDS for the account's credentials; this is where
EDS comes back with them, and so where the fan-out finally happens. Everything
underneath it was already written and tested — `authenticate_with`
(`src/authenticate.rs`, 12 tests) decides who gets contacted and which failure
becomes a second password prompt, `fan_out` (`src/fan_out.rs`, 10 tests) turns one
login into children — so what this session added is the two ends: the class-struct
slot, and the one implementation of `fan_out::Collection` that is not a test's.

Five tests, red first:

- `jmap-backend-core`: `trampoline::guard_value` and three tests for it. The
  existing guards pick the failure value themselves — FALSE, NULL — and
  `authenticate_sync` cannot be guarded that way: four of
  `ESourceAuthenticationResult`'s five values are failures and they mean
  *different things to the user* (prompt again, distrust the stored password,
  give up). So the caller names the fallback and the guard still sets the
  `GError`, which is the only part of a failed authentication a person can read.
  The vfunc passes `E_SOURCE_AUTHENTICATION_ERROR`, deliberately not `REJECTED`:
  a panic in this code is not the user's password being wrong, and answering
  `REJECTED` would make EDS throw a probably-correct password away and ask again
  on every retry.
- `jmap-backend-collection/tests/backend.rs`: two more slot tests, of the kind
  the other two vfuncs already have — but with a sharper reason, verified against
  the running EDS and its source rather than assumed.

**Why the slot needs a test at all, and this one most of all.** Fetched
`e-backend.c` from GNOME's GitLab to check what an *uninstalled* override would
leave in place. `e_backend_class_init` installs three defaults, and
`authenticate_sync`'s is one line with a comment saying why: "the default
implementation just reports success, it's for backends which do not use (nor
define) authentication routines". It returns `E_SOURCE_AUTHENTICATION_ACCEPTED`
without contacting anything. So an override written but not installed is not a
backend that fails to log in — it is one EDS believes logged in: the account goes
CONNECTED, no fan-out ever runs, no credentials are ever asked for again, and
there is no error, no prompt and no log line anywhere in it. That is the third
and worst of this crate's three inherited defaults (`dup_resource_id` answers the
bare identity, `populate` is a placeholder), and the reason `class_init` is held
against the parent class three times now.

The second test is about the offset rather than the value.
`authenticate_sync` is the first slot this crate writes into a half of the class
struct whose layout it does not own — bindgen's `EBackendClass`, two levels up,
between `GObjectClass` and `ECollectionBackendClass`'s own vfuncs. A wrong offset
there does not fail to compile; it silently overwrites a neighbouring slot with a
function of a different signature, which is a call through a bad pointer the first
time EDS uses it. EDS fills in exactly two neighbours — `get_destination_address`
and `prepare_shutdown` — so the test asserts both are still pointer-identical to
the grandparent's after `class_init` ran.

Other decisions, and why:

- **`Live` implements both traits now**, `Populating` and `Collection`, which is
  what makes one struct the whole instance side of this crate. Two things they
  share were factored out rather than written twice: `Live::export`, since
  `publish` is the same `ref_server` / `add_source` / `unref` in both (the log
  line's prefix is the parameter), and `drain`, since
  `claim_all_resources` and `list_{contacts,calendar}_sources` are all
  `(transfer full)` `GList`s freed with `g_list_free` and *not* `_full`. Checked
  `e-collection-backend.c`'s own doc comment for the listing calls rather than
  trusting the header: "the sources returned in the list are referenced for
  thread-safety… free the returned #GList itself with g_list_free()".
- **`new_child` refuses a resource id with an interior NUL** instead of using
  `cstring_lossy` like the rest of this crate's C-string conversions. Truncating
  there would not fail — it would silently ask EDS for a *different* resource and
  pair this collection's child with it. NULL is already a documented answer
  (`Adopted::Uncreated`, reported and logged), so the wrong child is the only
  outcome worth avoiding. Reachable only from a server that puts a NUL in a JMAP
  id, since every other resource id in this crate was read back out of a C string.
- **`existing_children` is contacts + calendars and never mail**, which is
  `Collection`'s documented contract reached from the implementing side: this
  backend creates no mail children, so it has no opinion about them and must not
  remove them.
- **A fan-out's per-child failures never reach the `GError`.** `uncreated`,
  `abandoned` and `not_removed` each become a critical naming the resource id;
  the result stays `ACCEPTED`. Turning one unwritable address book into a failed
  authentication would take the whole account offline over one collection — and
  EDS would then ask for the password again, which fixes none of the three.

**Not covered by a test, and the honest limits:**

1. **The vfunc body cannot be driven here**, like `populate`'s: its first act is
   `e_backend_get_source` on the instance, so `JmapCollectionBackend::detached()`
   is not sound for it (that doc comment already said so). What `tests/backend.rs`
   holds is the slot. **Needs human verification in real Evolution** that adding a
   JMAP account prompts for the password exactly once and that its address books
   and calendars appear in the sidebar afterwards.
2. **`Live`'s four new method bodies are unverified**, by construction — the part
   that needs a session bus. Each is one EDS call with the ownership spelled out
   in a SAFETY comment.
3. **`report_fan_out` is untested**, like `populate`'s equivalent: it is four
   `format!`s and a channel choice, and capturing GLib criticals to assert on them
   would test the harness rather than the decision.

Not verified locally, as in every session so far: `reuse lint` and `cargo deny`
(neither binary is on this VM). No new files, so no new SPDX headers.
`cargo fmt --check`, `cargo test --locked` (491 tests on the default members,
unchanged) and `cargo clippy --all-targets --locked -- -D warnings` are clean, as
is `cargo test`/`clippy` over the six EDS crates (`jmap-backend-collection` now 80
tests, `jmap-backend-core` 68). `RUSTDOCFLAGS=-D warnings cargo doc` is clean for
both changed crates.

No milestone tag. M6's Rust is now complete — all three vfuncs installed — but the
backend is not yet loadable: there is no `e_module_load` entry point registering
the type with `evolution-source-registry`, and no CMake target installing the
`.so` where the registry looks for it. Until then nothing of M6 has ever run
inside EDS, which is exactly the state the roadmap's rules say must not be tagged.

Next, and the last of M6: the module entry point plus the CMake target.
`evolution-source-registry` loads an `ECollectionBackendFactory`, not a backend,
so the entry point needs a factory subclass beside the one written here — its
`backend_factory_get_type`, its `factory_name`, and whichever registration call
3.52 actually uses, which has to be read off EDS's own source before it is
written (`e_collection_backend_factory_get_type` is in the bindings; nothing that
registers one is, so the current guess is that the registry finds them by walking
`g_type_children`, and that is a guess). Then `add_cargo_cdylib` installing into
the registry's module dir found with `pkg_check_variable`. That is the first
increment of M6 whose acceptance is a manual recipe rather than a test, so it
belongs with a documented `.source` keyfile like M3's and M4's.

## 2026-08-09 (ninety-ninth session)

**The module entry point, the factory, and the install rule — M6's Rust reaches
`evolution-source-registry`.** Everything before this session was code no
process outside `cargo test` could ever have run: the backend type existed, its
three vfuncs were installed, and there was no `.so` for the registry to open and
nothing in it to find if there had been. This session is the four pieces that
close that: an `ECollectionBackendFactory` subclass, the two `e_module_*`
symbols, `crate-type = ["cdylib", "rlib"]`, and an `add_cargo_cdylib` installing
`module-jmap-backend.so` into `pkg-config --variable=moduledir libebackend-1.2`.

Thirteen tests, red first (9 in `tests/factory.rs`, 4 in `tests/recipe.rs`):
`jmap-backend-collection` is now 93 tests, up from 80.

**The open question from last session is answered, and the guess was wrong.**
The note left behind said the registry probably "finds factories by walking
`g_type_children`, and that is a guess". Read `e-collection-backend-factory.c`
and `e-source-registry-server.c` from EDS 3.52.3 rather than guessing again:

- `e_collection_backend_factory_class_init` sets
  `EExtensionClass.extensible_type = E_TYPE_SOURCE_REGISTRY_SERVER`. The server
  is an `EExtensible`, so its own `e_extensible_load_extensions` instantiates one
  of every registered subclass. **No registration call exists, and none is
  needed** — inheriting from `ECollectionBackendFactory` *is* the registration.
  `the_factory_is_an_extension_of_the_registry_server` pins the inheritance,
  because a factory that lost it (deriving from `EBackendFactory` directly, say)
  would register cleanly, pass every other test in the file, and never be
  constructed.
- The lookup is `e_data_factory_ref_backend_factory (server, backend_name,
  "Collection")` against `collection_backend_factory_get_hash_key`, which builds
  `"<factory_name>:Collection"`. So the key is `jmap:Collection`, and
  `tests/factory.rs` asserts that string through EDS's own `get_hash_key` rather
  than by reading our own field back — the only test here that crosses into
  compiled EDS code and so the only one that would notice the class struct's
  fields having moved under a bindgen that still agrees with itself.

**Every field this factory installs has a *working* default under it, which is
the whole reason the tests are worth writing.** `factory_name` defaults to
`"none"` and `backend_type` to `E_TYPE_COLLECTION_BACKEND`. Neither is an error:
the first is an account the registry files under `none:Collection` and never
finds; the second passes `new_backend`'s own
`g_type_is_a (backend_type, E_TYPE_COLLECTION_BACKEND)` check and builds EDS's
own do-nothing collection backend — an account that appears in the sidebar,
connects, fans out to nothing, and reports nothing anywhere. That is the same
shape of hazard as the three inherited vfunc defaults from last session, so the
tests name the defaults (`EDS_DEFAULTS`) and assert *away* from them rather than
merely towards the right value.

Other decisions, and why:

- **`prepare_mail` is deliberately not overridden**, and there is a test that it
  is still the parent's. It is the hook a vendor backend fills an account's mail
  host/port/security into, and this backend creates no mail children at all — so
  an override would be an opinion about mail in the class struct that no code
  backs up. That is also the one part of M6's roadmap text still unbuilt, and it
  is now written down in the crate's own docs rather than only here.
- **The layout guard is three pointer comparisons, not two.** `factory_name` and
  `backend_type` are written into the parent's half of the class struct, between
  `EBackendFactoryClass` and `prepare_mail`; a wrong offset there compiles and
  then calls through a bad pointer. So `get_hash_key` and `new_backend` on the
  near side and `prepare_mail` on the far side are all asserted still
  pointer-identical to the parent's — plus an assertion that EDS installs a
  `prepare_mail` at all, since comparing two `None`s would say nothing.
- **`g_type_create_instance` and not `g_object_new`** in the hash-key test.
  `EExtension:extensible` is `G_PARAM_CONSTRUCT_ONLY`, and GObject sets every
  construct property during construction whether or not it was supplied — so a
  bare `g_object_new` hands `extension_set_extensible` a NULL and earns a
  critical from its `E_IS_EXTENSIBLE` assertion. Harmless (the assertion returns
  early and the field would have been NULL anyway) but not something to leave in
  a green run, where it sits next to real criticals and under
  `G_DEBUG=fatal-criticals` would abort. Creating the instance directly skips
  property defaults and `constructed`, which is what GObject's own
  `g_object_new_internal` does before it sets any; neither `EExtension` nor
  `EBackendFactory` overrides `constructed`, and `g_object_unref` still runs the
  normal dispose/finalize chain that ends in the paired `g_type_free_instance`.
- **`module-jmap-backend.so`, and the name is for humans.** Unlike
  `libebookbackend<name>.so` and `libcamel<protocol>.so`, nothing derives this:
  the registry dlopens every file in its module directory regardless of name.
  `module-*` is what every in-tree registry module is called
  (`module-google-backend.so`, `module-cache-reaper.so`), and the `-backend`
  suffix is there to distinguish it from M7's `module-jmap-configuration.so`,
  which lives in Evolution's module directory instead.
- **The recipe's keyfile is a file with tests on it**, as M3's and M4's are:
  `docs/examples/jmap-mock-collection.source` is loaded through
  `e_server_side_source_new` — the registry's own call, no bus needed — and
  `tests/recipe.rs` asserts the origin, the anonymous connection, the parts
  switched on, and that the `[Collection] BackendName` is the string the factory
  answers to. Plus the check the other two recipes have: the ini block quoted in
  the prose is byte-identical to the file.
- **`MailEnabled=false` in the documented account**, and that is not cosmetic:
  `mail` is one of the three bits `Parts::any` is a disjunction over, so a recipe
  that switched on a part nothing serves would be documenting an account whose
  populate contacts a server on behalf of children it will never create.

**Not covered by a test, and the honest limits:**

1. **Nothing here has run inside a real `evolution-source-registry`.** The
   install-check ctest proves `module-jmap-backend.so` lands in the directory
   `libebackend-1.2` reports and exports both entry points, and
   `tests/factory.rs` drives `e_module_load` through a stand-in `GTypeModule`
   the way `EModule` would. What neither can do is construct a backend —
   `new_backend` passes the server itself, so a real registry is required.
   **Needs human verification in real Evolution**, per
   `docs/manual-test-collection-backend.md`: the account appears, its address
   book and calendar appear under it, restarting the registry does not duplicate
   them, and switching a part off removes them.
2. **The credentials round trip is still unexercised end to end.** The
   documented account is anonymous on purpose; the `User=` + `--basic` variant is
   in the recipe as the way to reach it, and it is the half of
   `authenticate_sync` no test on this VM can drive.
3. **This VM has no `registry-modules` directory at all** (only
   `camel-providers` and `ui-modules` exist under
   `/usr/lib/evolution-data-server`), which is why the install check stages into
   a `DESTDIR` rather than proving the real directory is writable. The recipe
   documents `EDS_REGISTRY_MODULES` as the no-sudo path, read off
   `e-source-registry-server.h`.

Not verified locally, as in every session so far: `reuse lint` and `cargo deny`
(neither binary is on this VM). Two new Rust files, both with SPDX headers; the
two new `docs/` files are covered by `REUSE.toml`'s `docs/**` annotation.
`cargo fmt --check`, `cargo test --locked` (491 tests on the default members,
unchanged) and `cargo clippy --all-targets --locked -- -D warnings` are clean,
as is `cargo clippy`/`test` over the six EDS crates. Full `ctest` in a fresh
build tree: 6/6, including the new `install-collection-backend`.
`RUSTDOCFLAGS=-D warnings cargo doc` is clean for the changed crate. The new
tests were mutation-checked: commenting out both `class_init` assignments turns
exactly three of them red.

No milestone tag, deliberately. M6's Rust is complete and its module is
installable, but two of its acceptance criteria are not met: the roadmap asks
for a fan-out to **mail** as well as book and cal, and nothing of M6 has been
observed working inside a running registry. Tagging it would be claiming both.

Next, and in this order: the mail child (an `ESource` triple — account,
identity, transport — plus the `prepare_mail` override that configures them,
which is the piece that finally joins M5's Camel provider to an account), and
then M6 is a candidate for a tag as soon as a human has walked the recipe.

## 2026-08-09 (hundredth session)

**`prepare_mail`, and the answer to a question this crate had been guessing at
since `children.rs` was written: mail sources are not children of a collection
backend.** Last session's log named "the mail child (an `ESource` triple —
account, identity, transport — plus the `prepare_mail` override)" as the next
item, on the assumption that populate would create the triple. It would not have
worked, and the reason is in EDS's own source rather than in its headers.

Six tests, red first, in `tests/prepare_mail.rs`; `tests/factory.rs`'s layout
guard rewritten. `jmap-backend-collection` is now 99 tests, up from 93.

**What was read, and what it settled.** `children.rs` said the shape of the mail
children was "Evolution convention rather than anything the installed headers
state, and this machine has no reference account to read it off". This VM has a
network, so the reference was fetched rather than guessed: `libebackend/
e-collection-backend.c` and `e-collection-backend-factory.c` from EDS 3.52.3,
`modules/google-backend/module-google-backend.c` and `modules/yahoo-backend/`,
evolution-ews's `src/EWS/registry/e-ews-backend{,-factory}.c`, and a sparse
checkout of evolution 3.52.3's `src/`.

- **A collection backend's cached children cannot be mail sources.**
  `collection_backend_load_resources()` reads every `.source` file in the
  backend's cache directory, asks `dup_resource_id` what each one is, and
  **deletes** the file when the answer is `NULL`. So keeping mail sources there
  would require `dup_resource_id` to claim them — and
  `google_backend_dup_resource_id` chains up for `[Calendar]`, `[Memo List]`,
  `[Task List]` and `[Address Book]` and returns `NULL` for everything else, i.e.
  for exactly the mail extensions. EWS answers with its own folder id, which the
  mail sources do not carry. Neither backend creates them.
- **They are children of the *account*, not resources of the *backend*.** The
  three sources live in the registry's own source directory with `Parent` set to
  the collection's uid, written by the setup UI. `child_added`,
  `collection_backend_bind_child_enabled` (mail children bind to
  `mail-enabled`) and `e_collection_backend_list_mail_sources()` all still find
  them; only the cache directory does not hold them.
- **So `prepare_mail` is the whole of a collection factory's say in mail.** The
  inherited implementation does the vendor-independent wiring — the account's
  `identity-uid`, the identity's `[Mail Submission] transport-uid`, and the
  extension that makes each source recognisable. The vendor part is naming the
  service: Google writes `imapx`/`smtp` plus host, port and security into an
  `ESourceCamel`; evolution-ews writes the single name `ews` on the account and
  the transport and nothing else, because an EWS account's server comes from the
  collection. JMAP is the EWS shape, with one name on both sources — M5's
  provider registers one `CamelProvider` carrying a store type *and* a transport
  type, since JMAP submits over the session it reads through.

`children.rs`'s conservative decision therefore stands and is now verified
rather than assumed; its `the_mail_account_is_not_one_of_these_children` test
keeps meaning what it said.

**Decisions, and why:**

- **The host, port and security method are deliberately not written.** Google can
  write them because they are constants of the vendor. Here they are the user's,
  and they already live on the collection source — which this vfunc is not
  handed. It gets the three mail sources and the *factory*, nothing else. Same
  for `[Mail Identity] Address`: `Identity/get` states it, but this runs before
  anything has connected, and a guessed identity is a wrong `From:`.
- **The name is checked against `libcameljmap.urls`, not against a constant.**
  `the_name_written_is_the_protocol_camel_dlopens_the_provider_for` reads the
  file out of the source tree with `include_str!` rather than adding a
  `jmap-mail` dependency — that crate links Camel and this one does not. It is
  the same check `jmap-mail`'s own `tests/provider.rs` makes from the other
  side, and CTest's `install-camel-provider` checks the same file is installed.
- **The vfunc is driven through `e_collection_backend_factory_prepare_mail`**,
  EDS's public wrapper, not by calling our function. That is what makes the test
  cross the class struct: the wrapper reads the slot at the offset the *parent*
  believes it is at, one past the two fields `class_init` already wrote. It is
  the only test in this crate that both writes and dispatches through that
  struct.
- **`tests/factory.rs`'s layout guard changed shape rather than being deleted.**
  It used to assert `prepare_mail` was still pointer-identical to the parent's,
  which is now false by design. It asserts the opposite — the slot changed, and
  the `reserved` array past it did *not*, on our class and on EDS's — so a write
  landing one slot too far is still red.
- **No `e_source_mail_*_get_type()` calls before the chain-up**, and this is a
  correction to a belief the crate already held. `crate::child_source` makes such
  calls with a comment saying `e_source_get_extension` cannot find an
  unregistered type. The premise is right and the conclusion is not:
  `e_source_class_init` ends with a `g_type_ensure` of every built-in extension,
  all four mail ones included, and any source reaching either function is a live
  `ESource`, so that has already run. The calls were written here first, then
  removed when the mutation test showed nothing noticed; the comment now records
  the fact instead. **Follow-up:** `child_source.rs`'s five equivalent calls are
  dead for the same reason and its comment overstates their necessity — left
  alone tonight to keep this increment one thing, and they are harmless.

**Mutation-checked, since the tests and the code were written close together.**
Removing `factory.prepare_mail = Some(...)` turns
`the_mail_account_is_served_by_the_jmap_camel_provider`,
`the_transport_is_the_same_provider_and_not_a_second_one` and
`writing_the_fields_left_the_parent_vfuncs_alone` red. Removing the chain-up
turns `chaining_up_left_the_three_sources_pointing_at_each_other` red, and
nothing else — which is the point of it being a separate test.

**Not covered by a test, and the honest limits:**

1. **`e_collection_backend_factory_prepare_mail` has no caller.** Not in
   evolution-data-server 3.52.3, not in evolution 3.52.3 — grepped both trees.
   It is public API that vendor backends implement and that an account-setup
   path is expected to call; evolution-ews implements it anyway, and so does
   this. So the tests check the vfunc, not that anything reaches it. When M7
   creates the three sources it will be M7 that calls it.
2. **Nothing yet creates a JMAP account's mail sources**, which is why
   `docs/manual-test-collection-backend.md` still documents `MailEnabled=false`,
   with the reason rewritten to say *creates* rather than *wires*.
3. **Still no run inside a real `evolution-source-registry`**, unchanged from
   last session and still the reason M6 carries no completion tag.

Not verified locally, as in every session: `reuse lint` and `cargo deny`
(neither binary is on this VM). One new source file and one new test file, both
with SPDX headers. `cargo fmt --check`, `cargo test --locked` (491 tests on the
default members, unchanged) and `cargo clippy --all-targets --locked -- -D
warnings` are clean, as is `cargo clippy`/`test` over the six EDS crates. Full
`ctest` in a fresh build tree: 6/6. `RUSTDOCFLAGS=-D warnings cargo doc` clean
for the changed crate.

No milestone tag. M6's roadmap text asks for a fan-out to mail as well as book
and cal; what this session established is that the fan-out is not where mail
comes from, so the criterion belongs to M7 and the question of whether M6 is
done is now only the one that was already open — that none of it has been
observed inside a running registry.

Next: M7 is the natural continuation — it is what creates the three sources this
vfunc fills in — but it is GUI/config code this VM cannot verify, so it should be
approached as conservatively as the roadmap's rules demand. The tractable
alternatives are the `child_added` binding EWS uses to keep a child's host, user
and method following the collection's (this backend copies once at populate
instead, so an account whose server changes leaves stale children), and the
`child_source.rs` cleanup noted above.

## 2026-08-09 (hundred-and-first session)

**`child_added`: a child's connection follows its account, instead of being a
copy taken once and never looked at again.** Last session's log named this as
one of the two tractable items left in M6 — "the `child_added` binding EWS uses
to keep a child's host, user and method following the collection's (this backend
copies once at populate instead, so an account whose server changes leaves stale
children)" — and it is what this session did.

Ten tests, red first, in `tests/child_added.rs`; two more in `tests/backend.rs`
for the slot. `jmap-backend-collection` is now 111 tests, up from 99.

**The bug it fixes has no symptom.** `child_source::apply` writes the account's
`Connection` onto a child at the moment the child is created, and nothing copies
it again: a populate only writes children it *creates*, and a child that already
exists is claimed from the cache untouched. So an account whose host, port, user
or TLS setting the user edits afterwards keeps address books and calendars that
name the old one. Nothing about such a child looks wrong — it has a host, it
connects, it authenticates — it is simply talking to last week's server, or over
plain text against an account that has since been given TLS.

**What was read.** `libebackend/e-collection-backend.c` (3.52.3) and
evolution-ews's `src/EWS/registry/e-ews-backend.c`, both fetched rather than
guessed, plus `libedataserver/e-source-security.c` and the `e_binding_bind_property`
doc comment in `e-data-server-util.c`.

- EDS's own `collection_backend_child_added` binds only `display-name` (and only
  for mail children) and `oauth2-support`. Nothing of the connection.
- `ews_backend_child_added` binds the collection's `[Authentication]` **host**,
  **user** and **method** onto each child's, with `G_BINDING_SYNC_CREATE`, for
  every child that has an `[Authentication]` group — it does not filter for mail
  — and then chains up. That is the shape adopted here.
- `e_binding_bind_property` is EDS's own "thread safe variant of
  `g_object_bind_property()`", `(transfer none)`. Used rather than the GLib call
  it wraps, and `e_binding_.*` added to `eds-sys`'s function allowlist for it;
  the sources a registry binds are touched from more than one thread.
- `e_source_security_set_method` notifies **both** `method` and `secure`. That is
  what makes binding the boolean sound, which is what this does — `secure` is the
  property every JMAP backend actually reads (`SourceConfig::from_source`), so an
  account set to some third method spelling reaches its children as the answer
  they will act on rather than as a string they would have to agree about.

**Decisions, and why:**

- **Five properties, not three.** EWS binds host, user and method; this binds
  those plus `[Authentication] port` and `[Security] secure` — exactly the five
  fields a `Connection` is, so what `apply` writes once and what `child_added`
  keeps true afterwards are the same list. EWS can leave port and TLS out
  because an EWS account's server comes from its URL; a JMAP account's is these
  fields.
- **A group is bound only when *both* sources already have it** — a deliberate
  deviation from EWS, which fetches the collection's `[Authentication]`
  unconditionally. `e_source_get_extension` *creates* what it cannot find, and on
  the collection that means writing a group into the user's own account file,
  which `collection_source.rs` goes out of its way never to do; on the child it
  would mean this backend editing a source belonging to another part of
  Evolution. A source with no `[Authentication]` names no host anyway, so there
  is nothing a binding could carry to it.
- **The chain-up goes first**, which is the other way round from EWS. The
  parent's `child_added` is what puts the child in the backend's own table — and
  so what makes `e_collection_backend_list_*_sources` know about it, which is
  what the next fan-out's removal pass reads. A panic in our binding must not
  cost the child that.
- **One-way.** The binding carries the account to the child and never back; a
  child that could write to the account could, through it, rewrite every other
  child.
- **Mail sources are bound too, and that is wanted.** `child_added` fires for
  every source parented to the collection, the mail account and transport
  `prepare_mail` fills in included. They reach the same server as the address
  books, so they should follow the same fields — and the "both sides have the
  group" rule is what keeps that from turning into this backend inventing groups
  on sources it did not write. `tests/child_added.rs` covers the mail-shaped
  cases: a source with `[Authentication]` and no `[Security]` follows the host
  and is not given a `[Security]` group, and one with neither is left alone
  entirely.

**Red first, and mutation-checked.** The ten new tests were run against a
`follow_collection` whose body returned immediately: six failed on the
propagation assertions, four (the negative-space ones) passed, which is what
they should do against a function that does nothing. Removing
`vfuncs.child_added = Some(child_added)` from `class_init` turns
`class_init_replaces_the_default_child_added_rather_than_leaving_it` red and
nothing else, which is why that test exists — an uninstalled override here is
not a backend that breaks, it is one whose children quietly stay stale.

`the_bound_properties_exist_on_the_extensions_they_are_named_under` is the
guard for the failure mode particular to this module: a property name is a
string on `e_binding_bind_property`, so a misspelling is a `g_critical` at
runtime and a binding that was never made — the same silent staleness the module
exists to remove. It asks `g_object_class_find_property` rather than relying on
some other test happening to exercise that property.

**Not covered by a test, and the honest limits:**

1. **The vfunc itself is still only checked at the slot.** Calling it needs a
   live `ECollectionBackend`, and so a running `evolution-source-registry`;
   `follow_collection` is where the decisions are, and that is driven against
   real `ESource`s. Unchanged from `populate` and `authenticate_sync`.
2. **That EDS writes a bound child back to disk is asserted only in the manual
   recipe**, not by a test — `EServerSideSource` is what persists a changed
   child, and there is none here. `docs/manual-test-collection-backend.md` gained
   the step: edit the account's `.source`, restart the registry, and every child
   file names the new value.
3. **A child bound twice** — if EDS ever emitted `child-added` twice for one
   source — would carry two identical bindings. Harmless (both write the same
   value) and the same in evolution-ews, but not something this code prevents.
4. **Still no run inside a real `evolution-source-registry`**, unchanged, and
   still the reason M6 carries no completion tag.

Not verified locally, as in every session: `reuse lint` and `cargo deny`
(neither binary is on this VM). Two new source files, both with SPDX headers.
`cargo fmt --check`, `cargo test --locked` (default members, unchanged) and
`cargo clippy --all-targets --locked -- -D warnings` are clean, as is
`clippy`/`test` over the five EDS crates this touches;
`RUSTDOCFLAGS=-D warnings cargo doc` clean for the changed crate. `example-module`
— the pre-existing C-plus-Rust scaffold, in no default set and untouched here —
fails `clippy -D warnings` on `manual_c_str_literals` and did so before this
session too.

No milestone tag. Nothing about M6's open question changed: none of it has been
observed inside a running registry.

Next: the two items left over are `child_source.rs`'s five dead
`e_source_*_get_type()` calls, whose comment overstates their necessity (noted
last session, still harmless), and M7 — which is what creates the mail sources
`prepare_mail` fills in and `child_added` now binds, and which is GUI/config code
this VM cannot verify.

## 2026-08-09 (hundred-and-second session)

**M7 opens with the account itself: a new crate `jmap-config`, and the
collection `ESource` a setup commits.** Last session's log named M7 as the
natural continuation — "it is what creates the mail sources `prepare_mail` fills
in and `child_added` now binds" — with the caveat that it is GUI/config code
this VM cannot verify. That caveat is what decided the shape of this increment:
the parts of M7 that *decide* anything are `ESource` writes, and an `ESource`
can be built and read back in a plain test with no display, no session bus and
no Evolution. So they come first, and the `EMailConfigServiceBackend` subclass
that calls them comes after. The smaller the part of M7 that is GUI, the more of
M7 is actually checked rather than merely compiled.

Thirteen tests, red first, in `rust/crates/jmap-config/tests/account.rs`. New
crate, kept out of `default-members` like the other five that need the headers,
and added to CMake's `rust-test-eds` target.

**Everything in this repository so far reads an account; nothing wrote one.**
Every backend test starts from a `.source` keyfile written by hand, because in
Evolution the account file is the setup UI's to write and this project had none.
`account::apply` is that write, and it is the exact inverse of
`jmap-backend-collection`'s `collection_source` — which is why the tests write
with this crate and read back with *that* one. `collection_source` is tested
against sources built by hand with the EDS setters and would go on passing if
this crate wrote the host into the wrong group; this crate could asseverate that
it wrote what it meant to and be equally blind. The join is the thing: an
account the setup commits has to be an account the registry's backend
recognises, and a gap there is not a failed operation — it is an account that
appears in the sidebar, produces no child, and leaves nothing in any log.

**Decisions, and why:**

- **Every field is written every time, including the absent ones** — NULL for a
  string the account does not have, 0 for a port it does not name. This is the
  opposite of `child_source::apply`, which is handed a *fresh* child and may
  leave alone a property it has nothing to say about. A setup commits onto a
  source that already says something: an account being edited says the old
  server. So "the user cleared the login name" has to reach the file as an empty
  `User=`, and a conditional write would leave the old one there — an account
  the user made anonymous that goes on asking libsecret for a password under a
  name they deleted. `committing_an_account_that_dropped_its_user_clears_the_one_that_was_there`
  is the test for it, and it is the one both mutants below trip.
- **`[Authentication] Method` has no unset state, and the test says so rather
  than pretending otherwise.** Checked directly against the installed EDS, not
  assumed: a fresh `ESourceAuthentication` already reads `"none"`, and both NULL
  and `""` set it *back* to that string. So an `Account` with `auth_method:
  None` reads back as `Some("none")` — the right meaning, since "none" is what
  EDS's credentials provider resolves to the ordinary password impl, but not the
  identity. The round-trip assertions name it explicitly instead of quietly
  choosing an input that hides it. (It applies to children too: the conditional
  `Method` write in `Child::settings` is a distinction the keyfile cannot hold.)
- **The writer is not the security gate, and must not quietly become one.**
  Writing an account with TLS off and a public host succeeds; `server_of` then
  refuses it with `InsecureTransport`, because the `origin` rules every backend
  shares allow plain text for loopback and nothing else.
  `a_public_host_in_the_clear_is_written_faithfully_and_refused_by_the_reader`
  pins both halves. An account the writer silently "fixed" to TLS would be a
  file that disagrees with what the user was shown. Telling the user *before*
  they commit is `check_complete`'s job and is not written yet — noted here so
  it is not mistaken for done.
- **`[Security]` is written as the method string, read back as the boolean**,
  the same way `child_source` does it and for the same reason: the keyfile holds
  the string, so the string is the spelling that has to be right, and a test
  that reads `ESourceSecurity:secure` back is what catches it when it is not.
- **`BackendName` is held against the factory's registered name by a test.** It
  is not a description: the registry files each collection factory under
  `"<factory_name>:Collection"`, so a value that does not match is not an error
  anywhere. `jmap-backend-collection` is a dev-dependency of this crate for
  exactly this — the tests link it, the library must not, which is also why the
  doc links to it are paths into the generated documentation rather than
  intra-doc links.
- **What is deliberately not written:** `DisplayName` (the assistant's page
  sets it; writing it here would rename the account on every commit), `Enabled`
  (the user's answer to "show this account"), and the three mail sources — which
  are separate *sources*, not more groups in this one, and are the next
  increment.

**Mutation-checked.** Replacing the unconditional `set_user` with a
`if let Some(user)` turns exactly one test red, the edit-an-account one; the
same substitution on `set_port` turns the same one red and nothing else. Both
are the mutants the "every field, every time" decision exists to stop, and
before that test was written neither was caught by anything.

**Two tests were wrong about the world and were corrected, not the code.** The
first draft expected `http://jmap.example.com:8443` to round-trip and
`auth_method: None` to come back as `None`. Neither is true of EDS or of this
project's own `origin` rules, and finding that out is what the round-trip
against the real reader is for.

**Not covered by a test, and the honest limits:**

1. **Nothing calls `apply` yet.** There is no `EMailConfigServiceBackend`
   subclass and no `module-jmap-configuration.so`; the crate is an rlib on
   purpose, since a cdylib with no entry point would install a file no host ever
   opens. So this is verified as a function, not as a thing Evolution does.
2. **No account has been created through Evolution's UI** — the milestone's
   actual acceptance, and not something this VM can do. M7 carries no completion
   tag and will not until someone runs it.
3. **The mail sources still do not exist**, so
   `docs/manual-test-collection-backend.md` still documents `MailEnabled=false`.
   Unchanged from last session; this increment moved the boundary of what is
   written, not of what exists.

Not verified locally, as in every session: `reuse lint` and `cargo deny`
(neither binary is on this VM). Four new files, all with SPDX headers.
`cargo fmt --check`, `cargo test --locked` (491 tests on the default members,
unchanged) and `cargo clippy --all-targets --locked -- -D warnings` are clean,
as are `clippy`/`test` over the seven EDS crates — `jmap-config`'s 13 included.
`RUSTDOCFLAGS=-D warnings cargo doc` clean for the new crate. `example-module`
— the pre-existing C-plus-Rust scaffold, in no default set and untouched here —
still fails `clippy -D warnings` on `manual_c_str_literals`, as it did before.

No milestone tag.

Next: the three mail sources — `[Mail Account]`, `[Mail Identity]`,
`[Mail Transport]` — written the same way and read back through
`prepare_mail`'s vfunc, which is the other end of the same pipe and the reason
that vfunc has had no caller. After that, the module and the
`EMailConfigServiceBackend` subclass, which is where the part this VM cannot
verify begins.

## 2026-08-09 (hundred-and-third session)

**The three mail sources an account's mail is: `jmap-config`'s `mail` module.**
The continuation last session named, and the reason
`jmap-backend-collection`'s `prepare_mail` vfunc has had no caller. Twelve
tests, red first, in `rust/crates/jmap-config/tests/mail.rs`; the crate stays an
rlib out of `default-members`, and CMake's `rust-test-eds` target already runs
`-p jmap-config`, so nothing had to be wired.

**This time the join runs the other way round.** `tests/account.rs` writes with
this crate and reads back with somebody else's reader. Here there is no reader
of ours at all — the mail sources are read by Evolution and by Camel — and
instead there are two *writers* of the same three sources:
`mail::apply`, which runs in Evolution's process where the user's answers are,
and `prepare_mail`, which runs in `evolution-source-registry` where the
collection factory is. Neither can stand in for the other: the vfunc is handed
the three sources and nothing else (which is why it writes no address), and
Evolution's process has no factory instance to call it on. So
`the_setup_writes_the_services_the_registry_side_vfunc_would_write` runs both
over blank sources and compares. Nothing else would notice the vfunc going
stale — it has no caller in evolution-data-server 3.52.3 or evolution 3.52.3 —
and it is the implementation a later Evolution reaching that hook would get.

**What is written, and why each:**

- **`Parent`, on all three.** This is what makes them *this account's* mail:
  `e_collection_backend_list_mail_sources()` finds them by walking the account's
  children, and `collection_backend_bind_child_enabled()` binds each one's
  `enabled` to the account's `mail-enabled` on the same walk. An unparented mail
  source is not a broken account — it is a second, top-level account in the
  sidebar that no "receive mail for this account" switch reaches. Evolution's
  assistant is *believed* to set it too; it is written here anyway, because a
  writer that produces a complete account only when called from one particular
  caller is one whose output depends on something none of its tests can see.
- **The service name on the account and the transport** — `jmap` on both, the
  same string `prepare_mail` writes and the one line of `libcameljmap.urls`,
  because JMAP submits over the session it reads through.
- **The two links** — the account's `identity-uid` and the identity's
  `[Mail Submission] transport-uid`. A link written to the wrong uid is not a
  failure at commit time: it is a `From:` from some other account, or a send
  through some other account's server.
- **`[Mail Identity] Address`, from the same string as `[Collection] Identity`.**
  EDS keeps the address in two places; that the two agree is the setup's
  business and nobody else's, and
  `the_address_mail_is_sent_from_is_the_identity_the_account_claims` asserts the
  equality rather than each half against a literal.
- **Every field every time**, as in `account::apply` and for the same reason: a
  commit lands on sources that already say something, and an address left behind
  because the writer had nothing to add is the `From:` of every message sent
  afterwards.

**Deliberately not written:** `Enabled` (bound to the account's `mail-enabled`
by the collection backend on every load, so a value written here is one the
registry overwrites — which is also why the sources are written whether or not
`Parts::mail` is on: a switch needs something to switch), and
`[Mail Identity] Name`, which is the assistant's identity page's and which an
`Account` does not carry.

**Mutation-checked**, three mutants, each caught by tests that did not exist
before: dropping the `e_source_set_parent` loop turns three red; pointing the
account's `identity-uid` at the transport turns three red; dropping the
transport's backend name turns two red, one of them the vfunc comparison.

**The honest limit, and it is a large one: the mail account still names a
provider but no server.** Host, port, security and user reach a Camel service
through an `ESourceCamel` extension generated for the provider's *own* settings
type — `e_source_camel_generate_subtype()` takes that GType — so writing them
needs `jmap-mail`'s `CamelJmapSettings`, and therefore Camel, which this crate
does not link. That is the next increment and the reason M7 carries no
completion tag. Also unchanged from last session: nothing calls any of this
(there is no `EMailConfigServiceBackend` subclass and no
`module-jmap-configuration.so`), no account has been created through Evolution's
UI, and `docs/manual-test-collection-backend.md` still documents
`MailEnabled=false` — the recipe gains nothing from sources that cannot connect.

Not verified locally, as in every session: `reuse lint` and `cargo deny`
(neither binary is on this VM). Two new files, both with SPDX headers.
`cargo fmt --check`, `cargo test --locked` (491 tests on the default members,
unchanged) and `cargo clippy --all-targets --locked -- -D warnings` are clean,
as are `clippy`/`test` over the seven EDS crates — `jmap-config`'s 25 included.
`RUSTDOCFLAGS=-D warnings cargo doc` clean for the crate.

No milestone tag.

Next: the Camel settings group on the mail account, which is where M7 stops
being writable without Camel — either `jmap-config` gains a `jmap-mail`
dependency, or the settings subtype is generated where the provider already
lives. Then the module and the `EMailConfigServiceBackend` subclass, which is
where the part this VM cannot verify begins.

## 2026-08-09 (hundred-and-fourth session)

**The server an account's mail is reached at: `jmap-config`'s `apply_server`.**
The increment last session named as the reason M7 carries no tag — and the first
thing to say about it is that last session's account of *why* it was hard was
wrong. Six tests, red first, in `rust/crates/jmap-config/tests/mail.rs`; the
crate stays an rlib out of `default-members` and gains `jmap-mail` as a
**dev**-dependency only.

**The correction.** The claim was that writing host, port, user and security
needs `jmap-mail`'s `CamelJmapSettings` GType, and therefore Camel, because those
values live in the `ESourceCamel` extension generated from it. They do not live
there. `e-source-camel.c` carries a table of six properties it binds to *other*
extensions — `[Authentication]`'s `host`, `method`, `port`, `user`,
`[Security]`'s `method`, `[Offline]`'s `stay-synchronized` — and
`g_object_class_list_properties` is what fills the generated group, so exactly the
five a setup has answers for are the ones excluded from it. Writing the server is
therefore four ordinary `ESource` setters and no Camel at all; what is left in
`[JMAP Backend]` is `CamelStoreSettings`' and `CamelOfflineSettings`' inherited
defaults, which are the user's to change and not a setup's to invent. The library
still links no Camel. The *tests* link it, because the settings object is the only
place to ask an account the question the store will ask it.

**What is written, and why on both services:** host, port and user out of the
same `Connection` the collection was written from, on the mail account and on the
transport. Camel splits an account into a store and a transport with no pointer
between them and configures each from its own `ESource`; an unwritten transport is
an account that receives and cannot send, found out the first time the user
presses Send. The host being *the same string* as the collection's is
load-bearing beyond tidiness:
`e_util_can_use_collection_as_credential_source` compares exactly those two
values to decide whether a child shares the account's password, so a host that
disagreed — or one left blank while the collection has one — is a second password
prompt for the same server. That rule is now asserted rather than trusted, which
is the one line added to `eds-sys`'s allowlist (named exactly, not `e_util_.*`).

**`[Authentication] Method` is written as nothing, deliberately.** On a
collection it names the EDS credentials provider impl; on a mail source
`ESourceCamel` also binds it to `CamelNetworkSettings:auth-mechanism`, where it
names a SASL mechanism. `jmap-mail` passes a NULL mechanism to
`camel_session_authenticate_sync` because JMAP authenticates over HTTP and
advertises none, so the absent value is the true one — written rather than left
alone, so a mechanism from a previous commit does not survive as this account's
authentication type in the editor.

**`[Security] Method` is not `"tls"` on a mail source, and the mutation test is
what established what that costs.** EDS spells encryption `"tls"`; a mail source's
copy of the same key is additionally read as a `CamelNetworkSecurityMethod`
**enum nick**, so the string written there is `ssl-on-alternate-port` — TLS from
the first byte, which is what HTTPS is, and the spelling Evolution's own server
settings page writes back through the same binding. The first version of the test
asserted the origin `jmap-mail` assembles and the `"tls"` mutant **passed it**: on
a failed nick lookup `e_binding_transform_enum_nick_to_value` returns FALSE and
the binding sets nothing, so the settings object keeps the property's default,
which in EDS 3.52 is `STARTTLS_ON_STANDARD_PORT` — a TLS method. So an account
written as `"tls"` connects fine today, by way of a default nobody chose, while
telling the account editor a setting the user did not pick; and it becomes a
refusal to connect (`origin` allows plaintext only to loopback) the day Camel's
default moves. The doc comment claiming silent plaintext was corrected to that,
and the test now asserts the enum value, not only the URL it produces.

**Mutation-checked**, four mutants, each caught: `"tls"` for the nick (one red,
the reason above); the host write dropped (five red); the loop narrowed to the
account so the transport gets no server (three red, one of them the credential
test); the unencrypted branch written as encrypted (one red — and Camel's own
default being a TLS method is exactly why that direction needs its own
assertion).

**The honest limits.** Unchanged and still the whole of M7's remainder: nothing
calls any of this — there is no `EMailConfigServiceBackend` subclass and no
`module-jmap-configuration.so` — and no account has been created through
Evolution's UI, which is the milestone's actual acceptance and not something this
VM can do. So M7 carries no completion tag. What did change is that an account
this crate commits is now one a store could open and a transport could send
through, with nothing left for the caller to remember;
`docs/manual-test-collection-backend.md` still documents `MailEnabled=false`,
because the recipe would have to hand-write three more `.source` files to have
any mail sources at all, which is its own increment.

Not verified locally, as in every session: `reuse lint` and `cargo deny`
(neither binary is on this VM) — no new files this time, so no new SPDX headers
either. `cargo fmt --check`, `cargo test --locked` (491 tests on the default
members, unchanged) and `cargo clippy --all-targets --locked -- -D warnings` are
clean, as are `clippy`/`test` over the seven EDS crates — 784 tests,
`jmap-config`'s 31 (was 25) included. `RUSTDOCFLAGS=-D warnings cargo doc` clean
for the crate.

No milestone tag.

Next: the module and the `EMailConfigServiceBackend` subclass, which is where
the part this VM cannot verify begins — and where
`e_source_camel_generate_subtype` will have to be called for real, since
nothing has loaded `libcameljmap.so` in Evolution's process at the point an
account is committed.

## 2026-08-09 (hundred-and-fifth session)

**What a setup refuses to commit: `jmap-config`'s `complete::check`.** Ten
tests, red first, in `rust/crates/jmap-config/tests/complete.rs`; a new
`src/complete.rs`; the crate stays an rlib out of `default-members` and gains no
dependency.

**Why this and not the module.** Last session named the module and the
`EMailConfigServiceBackend` subclass as next, and that is still next — but the
subclass has two separable halves, and only one of them is verifiable on this
machine. `check_completeness` is a vfunc whose *decision* is ordinary Rust over
an `Account`: whether the answers so far are ones an account may be written
from. Taking that half first means that when the widget code lands, the part of
it that could be wrong in a way no test would notice is the plumbing, not the
rules. That is this crate's stated strategy applied one step further rather than
a detour around the hard part.

**Two rules, and both are grounded in code that already exists.** The server is
checked by calling `jmap_backend_core::source::origin` — the same function
`server_of` and every backend's connect path calls — and keeping its
`SourceError`: a missing host, a host that is not a bare host name, and
plaintext to anywhere that is not loopback. Nothing is restated, because a rule
spelled out twice is a rule to fix twice. The identity is checked for being an
address at all, since it is written into `[Mail Identity] Address` and is
therefore the `From:` of everything the account sends.

**The one line of translation, and the test that pins it.** `origin` takes the
absent-or-non-empty host `read_string` produces from a keyfile; a setup has an
entry the user has not filled in, which is `""`. Mapping the one to the other is
the only place the two sides do not share code, so `""` is reported as
`MissingHost` and not as `InvalidHost("")`. The last test is the join and the
reason the file links the collection backend: eight servers, each committed with
`account::apply` and read back with `server_of`, asserting the setup's verdict
and the registry's are the *same* verdict — accepted by both, or refused by both
for the identical `SourceError`. A check that accepted what the registry rejects
would be an account that fails everywhere except in the dialog it was typed into.

**Two things deliberately not checked, both documented as such in the code.**
A missing user name is not a fault: `credentials()` turns it into an anonymous
connection, which is how `jmap-mockd` and a local development server are
reached, so insisting on one would refuse to commit the account this project is
developed against. And an account with mail, contacts and calendars all switched
off is not a mistake at commit time — `mail::apply` already writes the three
sources either way precisely because the parts are switches, and a switch the
user can flip later is not an incomplete answer. Refusing either would have been
a rule invented by the checker rather than one the rest of the code has.

**Whitespace is refused rather than trimmed**, for the same reason every writer
here writes verbatim: `apply` would commit `" vera@example.com"` unchanged. An
entry holding *only* whitespace is reported as `MissingIdentity` — the
unanswered question it looks like — rather than as an address that fails to
parse. `is_address` is deliberately not an RFC 5322 parser and says so: the cost
of a wrong answer is asymmetric, so anything it is unsure about it accepts and
leaves for the server to reject at login.

**Mutation-checked**, four mutants, each caught: the whitespace rule dropped
(one red); the empty host passed to `origin` as `Some("")` (two red, one of them
the join test, which is the case for the translation existing); a second `@`
allowed (one red); the identity's emptiness tested without `trim` (one red).

**The honest limits are unchanged and are still all of M7's remainder.** Nothing
calls any of this: there is no `EMailConfigServiceBackend` subclass and no
`module-jmap-configuration.so`, and no account has been created through
Evolution's UI, which is the milestone's actual acceptance and not something
this VM can do. So M7 carries no completion tag.
`docs/manual-test-collection-backend.md` still documents `MailEnabled=false`.

Not verified locally, as in every session: `reuse lint` and `cargo deny`
(neither binary is on this VM). Two new files, both with SPDX headers.
`cargo fmt --check`, `cargo test --locked` (491 tests on the default members,
unchanged) and `cargo clippy --all-targets --locked -- -D warnings` are clean,
as are `clippy`/`test` over the EDS crates — 794 tests, `jmap-config`'s 41
(was 31) included. `RUSTDOCFLAGS=-D warnings cargo doc` clean for the crate.
Pre-existing and untouched: `example-module` does not build on this VM (26
`manual_c_str_literals` clippy errors and a link failure) — confirmed against a
stashed tree, so it is the machine and not this change, and it is why the
workspace-wide runs above exclude it.

No milestone tag.

Next: the module and the `EMailConfigServiceBackend` subclass — the widgets and
the plumbing, now that the rules they enforce are decided and tested here. That
is where the part this VM cannot verify begins, and where
`e_source_camel_generate_subtype` will have to be called for real.

## 2026-08-09 (hundred-and-sixth session)

**The account a setup starts from: `jmap-config`'s `defaults::from_identity`.**
Ten tests, red first, in `rust/crates/jmap-config/tests/defaults.rs`; a new
`src/defaults.rs`; the crate stays an rlib out of `default-members` and gains no
dependency.

**Why this before the module, again.** Last session named the module and the
`EMailConfigServiceBackend` subclass as next, and they still are. But that
subclass has a third decision in it that is ordinary Rust over an `Account` —
`setup_defaults`, what the *Receiving Email* page says before the user has typed
anything into it — and taking it now leaves the subclass with widgets and
plumbing and no rules, which is the split this crate has been working to. The
same reasoning as `complete::check`, one step further, and the last of the three
halves that could be checked on a machine with no display.

**For JMAP a default server is not a guess.** For IMAP, deriving `example.com`
from `vera@example.com` is usually wrong — mail servers live at `imap.` and
`mail.` and at names no rule produces, which is what autoconfig databases exist
for. RFC 8620 §2.2 removes the problem: a client that knows only the address
fetches `https://<domain>/.well-known/jmap`, and the server answers with where
its session, API and download URLs really are. So the domain is not a guess at
where the server is, it is the address the protocol specifies for asking, and
the account this writes names exactly that. That is the whole reason this crate
can offer a working default where an IMAP setup would need a lookup table.

**The login name, and the rule that is not broken by it.** `account` says the
identity is deliberately *not* derived from `[Authentication] User` — the two
are equal often enough to be assumed and different often enough for the
assumption to be wrong. That rule is about what is committed and still holds:
`apply` writes whatever the entry says and never looks at the identity. Here the
two are related the only way they safely can be, as an offer sitting in an entry
the user can edit. The alternative default was an empty entry, which is worse
than wrong: an account with no user name is an *anonymous* connection — a
legitimate state, and the one `jmap-mockd` is reached by — so leaving it blank
would offer this project's development configuration to everybody else.

**What is left unanswered, and why each.** The port: 443 is right and writing it
down would still be wrong, because an account that names a port keeps naming it
if the scheme changes underneath. The auth method: what the server offers is
something a session document answers, not something a dialog guesses before it
has connected. The display name: not this crate's to write at all. And nothing
here reaches the network — `setup_defaults`'s neighbours run on every keystroke,
and asking a server belongs in the assistant's lookup step, which is still a
later increment.

**Both joins are tested, and they are the point of the function.** A default
`check` would refuse is a *Next* button greyed out on a page the user has not
touched — so `check(&from_identity("vera@example.com"))` is `Ok`. A default
whose server the registry reads back as some other server is a well-known probe
aimed somewhere the address never named — so the account is committed with
`account::apply` and read back with the collection backend's `server_of`, which
must answer `https://example.com`. A string that is not an address yet leaves
the server entry blank rather than half-filled, and `check` reports the address,
which is the entry the user is in.

**Mutation-checked**, six mutants, each caught: `split_once` for `rsplit_once`
(one red); the default written as insecure (three red, including both joins);
the empty local part accepted so `@example.com` becomes a server (one red); the
port written down as 443 (two red); the empty address offered as a login name
(one red); calendars off by default (one red). One redundancy the mutants found
was removed rather than tested around: an empty domain and an absent one produce
the same empty entry, so `domain_of` no longer distinguishes them.

**The honest limits are unchanged and are all of M7's remainder.** Nothing calls
any of this: there is no `EMailConfigServiceBackend` subclass and no
`module-jmap-configuration.so`, and no account has been created through
Evolution's UI, which is the milestone's actual acceptance and not something
this VM can do. So M7 carries no completion tag.
`docs/manual-test-collection-backend.md` still documents `MailEnabled=false`.

Not verified locally, as in every session: `reuse lint` and `cargo deny`
(neither binary is on this VM). Two new files, both with SPDX headers.
`cargo fmt --check`, `cargo test --locked` (491 tests on the default members,
unchanged) and `cargo clippy --all-targets --locked -- -D warnings` are clean,
as are `clippy`/`test` over the EDS crates — 804 tests, `jmap-config`'s 51
(was 41) included. `RUSTDOCFLAGS=-D warnings cargo doc` clean for the crate.
Pre-existing and untouched: `example-module` does not build on this VM, which is
why the workspace-wide runs exclude it.

No milestone tag.

Next: the module and the `EMailConfigServiceBackend` subclass. All three of its
decisions — what an account is written as, what it starts from, and whether it
may be committed — are now decided and tested in plain Rust, so what is left is
the widgets and the plumbing, and that is where the part this VM cannot verify
begins.

## 2026-08-09 (hundred-and-seventh session)

**The headers the setup module is written against: a new `evo-sys` crate.**
Three tests, red first (as a missing crate, then as a missing symbol), in
`rust/crates/evo-sys/tests/layout.rs`; a bindgen `build.rs` one class wide; one
line added to `eds-sys`'s function allowlist. The crate stays out of
`default-members` and is run by CMake's `rust-test-eds` target with the others.

**Why this and not the module.** The last three sessions each took one of
`EMailConfigServiceBackend`'s decisions — what an account is written as, what it
starts from, whether it may be committed — because each was ordinary Rust over
an `ESource` and so could be *tested* here, and each ended saying the module was
next. It is, and this is the first half of it: before a vfunc can be overridden,
the class it lives in has to exist in Rust, and it did not. Nothing in this
repository had ever crossed into Evolution's own libraries; everything so far
stops at EDS.

**Two sys crates, not one.** `eds-sys` could have grown the Evolution headers
the way it grew Camel's, and that would have been wrong: the backends M3–M6
install are loaded by `evolution-source-registry` and the data factories, which
are EDS processes that never link GTK, while M7's module is loaded by Evolution,
which does. One crate would put GTK, WebKit and Evolution's four private
libraries behind every address book backend that only ever wanted `ESource`. So
`evo-sys` is its own crate with its own `links` key, and the split is by *host
process*, which is the thing that actually differs.

**The blocklist is the whole design.** `eds-sys` blocks `G[A-Z].*` and
re-exports the gtk-rs sys crates, because a regenerated `GObject` has the right
layout and the wrong identity. The same argument applies one library up and with
a second owner: `EMailConfigServiceBackend` is an `EExtension` and hands out
`ESource`s, `CamelProvider`s and `CamelSettings`, all of which `eds-sys` already
generated from the very same headers. Blocked here, re-exported with a single
`pub use eds_sys::*` — which brings the GLib ones along, since `eds-sys`
re-exports those in turn — they stay one type, so the module's vfuncs can be
written in terms of `jmap-config` and `jmap-mail`, which read and write those
objects already. `tests/layout.rs` asserts the join rather than assuming it: the
`g_type_parent` of the class must be *`eds-sys`'s* `e_extension_get_type`.

**GTK is the exception, and deliberately opaque.** `GtkBox` appears in these
headers only as the container `insert_widgets` is handed, and there is no
`gtk-sys` to re-export it from: GTK 3's sys crate is frozen at the 0.18
generation and depends on `glib-sys` 0.18 while this workspace is on 0.22, so
depending on both would put two incompatible `GObject`s in one process —
precisely the failure the blocklist exists to prevent. The pointer types are
`c_void` for now, and the widget calls M7 eventually needs will be generated
from these same headers rather than borrowed from a second ecosystem. It also
keeps `GtkWidgetClass` and the hundred vfuncs under it out of a binding surface
nobody reads.

**One thing tried and removed.** `build.rs` first emitted an explicit
`-Wl,-rpath` for each of pkg-config's link paths, on the reasoning that
Evolution's libraries live in `/usr/lib/evolution` rather than in the system
directory and a binary that does not record that will not start. Both `.pc`
files already carry `-Wl,-R<libdir>` in their `Libs:` for that exact reason, and
the `pkg_config` crate forwards it — dropping the loop and checking with
`readelf -d` leaves the directory in the test binary's `RUNPATH`, and the loop
had only been duplicating the entry. The comment now records the check rather
than the code.

**Mutation-checked, and the useful mutants were to the header, not the code.**
Making the structs `opaque_type` proves nothing — bindgen keeps the size — so
the drift was simulated the way it will really arrive, with a patched copy of
`e-mail-config-service-backend.h` on the include path ahead of the system one. A
vfunc added before `commit_changes`, as a future Evolution would: class size 216
against the runtime's 208, one red. The instance's private pointer removed: two
red, the size and the `EExtension`-plus-a-pointer assertion. `backend_name`
moved behind the first vfunc, which changes no size at all: the offset test
alone, 152 against 144 — which is the case for that third test existing, since
Evolution finds a backend by `strcmp`ing that field against a Camel protocol
name and would otherwise be reading a function pointer as a string.

**Also fixed:** `jmap-config`'s docs called the vfunc `check_completeness` in
five places. It is `check_complete`; the header is now in the tree to check
against.

**The honest limits, which this does not change.** A layout test is not a
working module: there is still no `EMailConfigServiceBackend` subclass, no
`module-jmap-configuration.so`, and no account has been created through
Evolution's UI, which is M7's actual acceptance and not something this VM can
do. What is new is only that the class can now be named from Rust — and that if
a future Evolution reshapes it, this fails as a red test rather than as a
misplaced vfunc pointer in the process the user types their password into. So
M7 carries no completion tag. `docs/manual-test-collection-backend.md` still
documents `MailEnabled=false`.

Not verified locally, as in every session: `reuse lint` and `cargo deny`
(neither binary is on this VM). Five new files, all with SPDX headers.
`cargo fmt --check`, `cargo test --locked` (491 tests on the default members,
unchanged) and `cargo clippy --all-targets --locked -- -D warnings` are clean,
as are `clippy`/`test` over the EDS crates — 807 tests, `evo-sys`'s 3 included.
`RUSTDOCFLAGS=-D warnings cargo doc` clean for the new crate. The CI image
installs `evolution-dev` and CMake already `REQUIRED`s `evolution-shell-3.0` and
`evolution-mail-3.0`, so nothing new has to be present for this to build there.
Pre-existing and untouched: `example-module` does not build on this VM, which is
why the workspace-wide runs exclude it.

No milestone tag.

Next: the subclass itself — `class_init` filling in `backend_name` with the
Camel protocol name and the vfuncs delegating to `jmap-config`'s three
functions, and the `e_module_load` that registers it into Evolution's module
directory. That is the part this VM can compile and cannot verify.

## 2026-08-10 (hundred-and-eighth session)

**The class Evolution talks to: `EMailConfigServiceBackendJmap`.** Six tests,
red first, in `rust/crates/jmap-config/tests/backend.rs`; the subclass in
`src/backend.rs`; `evo-sys` added as a dependency of `jmap-config`. The class
carries two things and no more — the `backend_name` the *Receiving Email* page
finds it by, and a `new_collection` that answers the account a new JMAP setup
starts as.

**Why only two.** `backend_name` is not a vfunc but it is what makes the backend
exist at all: the page `strcmp`s it against each Camel provider's protocol, and
NULL — which is what the abstract parent leaves there — is a JMAP entry that
never appears in the account type list. `new_collection` is the other one whose
decision was already made and tested: Evolution's own answers NULL, which is
right for POP3 and, for a provider that fans out to contacts and calendars, an
account committed as a lone mail source with nothing behind it. The remaining
four slots are left inherited *and said so in the crate docs*:
`insert_widgets` and `setup_defaults` need the `EMailConfigServicePage` this
extension extends, and `check_complete`/`commit_changes` need the account read
back out of the collection source — the inverse of `account::apply`, which this
crate does not have yet and which is the next increment.

**`get_selectable` is left alone deliberately, not forgotten.** Its inherited
implementation answers "yes, unless this provider is both a store and a
transport, in which case only on the receiving page" — and `jmap-mail`'s
provider registers both types, so the inherited answer is already right.
An unconditional override would offer JMAP a second time in the *Sending Email*
combo as an account type the user can pick and then not configure.

**The one real decision: what `new_collection` writes.** evolution-ews writes a
single property here (the collection backend name) and leaves the rest to
`setup_defaults`. This writes the whole of `defaults::from_identity("")` — the
same account with the one field that needs an address left empty — because the
fields `setup_defaults` would fill are not neutral when absent.
`[Collection] MailEnabled` and its two siblings read *false* when unwritten, so
a collection carrying only a backend name reads back, through the registry's own
`parts_of`, as a JMAP account with mail, contacts and calendars all switched
off: not what the dialog shows, and a difference that would only ever surface as
an account with no children. Two of the six tests go red if that write is
dropped, which is how it was checked rather than argued.

**A linking bug the first run found, and its fix.** `jmap-config`'s test binary
linked and then would not start: `libevolution-mail.so.0: cannot open shared
object file`. Evolution's libraries live in `/usr/lib/evolution`, and the
`-Wl,-R` its `.pc` files carry for exactly that reason reaches only the package
whose build script emitted it — Cargo passes a build script's `-l`/`-L` on to
every crate downstream but scopes `rustc-link-arg` to that package. Last
session's note that the rpath "comes out in the test binary's RUNPATH" was true
of `evo-sys`'s own tests and of nothing else. So `evo-sys` now publishes its
link directories as `cargo:libdirs`, which Cargo hands to dependents of a crate
with a `links` key as `DEP_EVOLUTION_SHELL_LIBDIRS`, and `jmap-config` has a
small `build.rs` that turns them back into `-Wl,-rpath`. Checked with
`readelf -d` on the new test binary, not assumed. The same mechanism is what the
installed `module-jmap-configuration.so` will need.

**One assertion corrected against the installed EDS rather than against
intent.** The empty identity `new_collection` writes reads back as *absent*, not
as an empty string: EDS's setters strip what they are given and store nothing
for what is empty afterwards. That is the reading that matters — it is also what
the registry finds in a keyfile with no `Identity=` line — and the test says so
with the reason.

**What this is not.** A registered class is not a module and not a working
setup: nothing calls `e_module_load`, no `module-jmap-configuration.so` is
built or installed, no widget exists, and no account has been created through
Evolution's UI, which is M7's actual acceptance and not something this VM can
do. M7 carries no completion tag, and this needs human verification in real
Evolution when there is something to verify.

Not verified locally, as in every session: `reuse lint` and `cargo deny`
(neither binary is on this VM). Three new files, all with SPDX headers.
`cargo fmt --check`, `cargo test --locked` (491 tests on the default members,
unchanged) and `cargo clippy --all-targets --locked -- -D warnings` are clean,
as are `clippy`/`test` over the EDS crates — 813 tests, `jmap-config`'s 57
(was 51) included. `RUSTDOCFLAGS=-D warnings cargo doc` clean for the crate.
Pre-existing and untouched: `example-module` does not build on this VM, which is
why the workspace-wide runs exclude it.

No milestone tag.

Next: `account::read`, the inverse of `account::apply` — the account read back
out of the collection source the widgets edit. It is ordinary Rust over an
`ESource`, so it can be tested here, and it is what both `check_complete` and
`commit_changes` are waiting on.

## 2026-08-10 (hundred-and-ninth session)

**`account::read`, the inverse of `account::apply`.** Seven tests, red first
(they did not compile: there was no `read`), in
`rust/crates/jmap-config/tests/account.rs`; the function in `src/account.rs`.
It is the account the widgets are filled from and the one `check_complete` and
`commit_changes` are handed — the last piece those two vfuncs were waiting on,
which is why the crate docs that said "does not exist yet" now say what is
actually left (the vfunc plumbing).

**It is total where the registry's reader is fallible, and that is the point.**
`collection_source::server_of` answers a `Result` and reports `MissingHost`,
because a backend about to open a connection has no use for an account without
one. `read` answers an `Account` and never fails: it is asked on every keystroke
of a dialog the user is still filling in, where "no host yet" is the ordinary
state. The verdict stays with `complete::check`, which already exists and
already calls the same shared `origin` — so this adds a reader, not a second
opinion about what a good account is.

**The one deliberate disagreement with the registry's reader, and why.**
`parts_of` folds the source's own `Enabled` flag in — a disabled account has no
parts to populate — and `read` does not. `enabled` is not a field of `Account`,
`apply` writes all three switches every time, and the two together mean a `read`
that answered `Parts::NONE` for a hidden account would show three cleared check
boxes and then *commit* them: "hide this account for now" turned into
permanently losing which parts it offered. The test asserts both answers side by
side on one source (`parts_of` says `NONE`, `read` says what was written) so the
divergence is pinned rather than latent.

**Every other absent-group rule is the collection backend's, restated nowhere.**
No `[Collection]` is `Parts::ALL`, through `Parts::from_collection` itself; no
`[Security]` is TLS, because reading the `secure` property of a group that is
not there answers FALSE and a dialog that offered *that* back would switch TLS
off on a hand-written account; no `[Authentication]` is the empty host and no
user. And nothing here calls `e_source_get_extension` without
`e_source_has_extension` first — this is handed the user's own account file, and
a read that left three groups behind would turn opening the account editor and
pressing Cancel into a write. That has its own test.

**What does not round-trip, and is asserted not to.** `auth_method: None` comes
back as `Some("none")`, because `ESourceAuthentication:method` has no unset
state — the same fact `tests/account.rs` already pinned for the registry's
reader. `read` reports what the source says rather than mapping "none" back to
`None`: the alternative is this crate and the collection backend disagreeing
about one string in one keyfile. The host is the only field translated at all —
the keyfile's NULL becomes the empty string an unfilled entry holds, which is
`complete::check`'s single line of translation read backwards.

**What this is not.** Still no module, no `e_module_load`, no
`module-jmap-configuration.so`, no widget, and no account created through
Evolution's UI — M7's actual acceptance, which this VM cannot do. M7 carries no
completion tag. `read` itself is ordinary Rust over an `ESource` and is tested
here; the vfuncs that will call it are not, and will need human verification in
real Evolution.

Not verified locally, as in every session: `reuse lint` and `cargo deny`
(neither binary is on this VM); no new files this time, so no new SPDX headers
either. `cargo fmt --check`, `cargo test --locked` (491 tests on the default
members, unchanged) and `cargo clippy --all-targets --locked -- -D warnings` are
clean, as are `clippy`/`test` over the EDS crates — 820 tests, `jmap-config`'s
64 (was 57) included. The one ignored test is a pre-existing `ignore` doctest in
`jmap-backend-core`'s `instance::Slot`. `RUSTDOCFLAGS=-D warnings cargo doc`
clean for the crate. Pre-existing and untouched: `example-module` does not build
on this VM, which is why the workspace-wide runs exclude it.

No milestone tag.

Next: the `check_complete` vfunc — the first slot that is pure plumbing now that
both halves of its answer exist (`account::read` and `complete::check`).
Overriding it is `class_init` work of the kind `backend.rs` already does for
`new_collection`, so it can be compiled and its trampoline tested here; what it
cannot show is the greyed-out *Next* button, which stays human verification.

## 2026-08-10 (hundred-and-tenth session)

**The `check_complete` vfunc.** Six tests, red first (they did not compile:
there was no `is_complete`), in `rust/crates/jmap-config/tests/backend.rs`; the
slot and its two functions in `src/backend.rs`. It is the first vfunc since
`new_collection` and the one both halves of whose answer already existed —
`account::read` for the account the collection source says, `complete::check`
for whether that account may be committed — so what landed is the plumbing and
one composition, not a new decision.

**What the inherited slot does, read rather than remembered.** The test says
Evolution's own `check_complete` accepts anything, and that claim was checked
against the installed `libevolution-mail.so` instead of asserted from memory:
the parent class's slot points at `endbr64; mov eax,1; ret`. Unconditional
TRUE. Left inherited it is not a missing feature anyone sees — it is an
assistant whose *Next* is sensitive over an account with no address and no
server, committed and then failing in `evolution-source-registry`, which is the
failure `complete`'s module comment exists to argue against.

**Which source the vfunc asks about, and the note that has to survive to
`insert_widgets`.** The collection, through
`e_mail_config_service_backend_get_collection`. evolution-ews asks its
`CamelEwsSettings` instead, because that is what its entries are bound to; both
are defensible and this crate has picked the collection everywhere — it is what
`account::apply` writes, what `account::read` reads, and the one description of
an account the collection backend reads back in another process. The
consequence is a constraint on a vfunc that does not exist yet: `insert_widgets`
must bind its entries to that same source, or this one is answering questions
about a source nobody is editing. It is written down in `backend.rs` next to the
vfunc rather than left to be rediscovered.

**A refusal that is silent, twice, for two different reasons.** A NULL
collection answers FALSE and logs nothing: the only way to get one is a
`new_collection` that failed, which already logged a critical where the failure
happened, and a second copy per keystroke would bury the original. An account
that is merely unfinished also logs nothing, because `Incomplete`'s text is
written for the person who typed the answer and this vfunc has nowhere to put it
— it returns a boolean. The status label that will carry it is `insert_widgets`'
to add. Adding a debug-level log helper to `jmap-backend-core` was considered
and dropped: it would have been a second crate's surface, untested, for a line
nobody is currently reading.

**What this makes worse before it makes it better, said plainly.** With
`check_complete` installed and neither `setup_defaults` nor `insert_widgets`
written, a JMAP account in the assistant would have *Next* greyed out and no way
to un-grey it: nothing fills in an address, and no address is exactly what this
refuses. That is not a regression — no module loads this class, so nothing
reaches the dialog at all — and it is the right order, since the alternative is
a setup that commits accounts it knows are broken. `backend.rs` says so in its
own words under "The state this leaves the dialog in".

**What this is not.** Still no module, no `e_module_load`, no
`module-jmap-configuration.so`, no widget, and no account created through
Evolution's UI — M7's actual acceptance, which this VM cannot do. The vfunc body
itself is not tested either: driving it needs a live `EMailConfigServiceBackend`,
and the detached instance the `new_collection` test uses would reach
`e_mail_config_service_backend_get_collection`'s `E_IS_...` assertion — a
critical in a green run, and a path the class docs already call undefined
behaviour. What is tested is `is_complete`, over real `ESource`s: the account
`new_collection` offers is refused (for the identity, which is the page the user
is on), a finished account is accepted and reads back through the registry's own
reader as `https://jmap.example.com:8443`, plaintext to a remote server is
refused and plaintext to localhost is not, and a NULL collection commits
nothing. M7 carries no completion tag and this needs human verification in real
Evolution.

Not verified locally, as in every session: `reuse lint` and `cargo deny`
(neither binary is on this VM); no new files this time, so no new SPDX headers
either. `cargo fmt --check`, `cargo test --locked` (491 tests on the default
members, unchanged) and `cargo clippy --all-targets --locked -- -D warnings` are
clean, as are `clippy`/`test` over the EDS crates — 826 tests, `jmap-config`'s
70 (was 64) included. The one ignored test is a pre-existing `ignore` doctest in
`jmap-backend-core`'s `instance::Slot`. `RUSTDOCFLAGS=-D warnings cargo doc`
clean for the crate. Pre-existing and untouched: `example-module` does not build
on this VM, which is why the workspace-wide runs exclude it.

No milestone tag.

Next: `commit_changes`, the last vfunc whose decision is already written — the
account `account::read` gives, written back with `account::apply` and fanned out
to the three sources `mail::apply` writes. Like this one it is plumbing over
tested parts, and like this one what a test here can drive is the composition
rather than the vfunc Evolution dispatches; the registry call it has to make to
turn the scratch collection into a real account is the part to look at closely.

## 2026-08-10 (hundred-and-eleventh session)

**The `commit_changes` vfunc.** Five tests, red first (they did not compile:
there was no `commit`), in `rust/crates/jmap-config/tests/backend.rs`; the slot,
the vfunc and `commit` in `src/backend.rs`, plus one visibility change —
`mail::apply_server` is now `pub`, because it turns out to be the whole of what a
`commit_changes` is in a position to write.

**What the vfunc actually has to do, which is much less than expected.** The
plan carried over from the last session was "the account `account::read` gives,
written back with `account::apply` and fanned out to the three sources
`mail::apply` writes". Reading Evolution 3.52's own code rather than working from
that plan cut it down to one line of writing. `e_mail_config_assistant_commit`
queues the collection and the three mail sources itself and calls
`e_source_registry_create_sources` — so the vfunc creates nothing and saves
nothing. `EMailConfigSummaryPage`'s own `commit_changes` sets all three sources'
`Parent`, the account's `identity-uid` and the identity's `transport-uid`, which
is the same wiring `mail::apply` does. The assistant writes the service name into
each scratch source before any backend sees it — that is how the candidate was
picked. The identity page writes the address. What is left over, and what nobody
else can write because JMAP asks for a server once on the *account* rather than
on the mail page, is `[Authentication]` and `[Security]` on the mail source. That
is `apply_server`, and that is the vfunc.

So `mail::apply` is *not* called from the vfunc, and stays the account's writer:
the vfunc is handed one backend holding one scratch source, not three.

**One backend per page, and the empty collection that has to be refused.**
Evolution instantiates this class once for the *Receiving Email* page and once
for *Sending Email*, and `constructed` calls `new_collection` on each — so the
sending instance holds a scratch collection of its own that no widget fills in
and that the assistant never queues. A commit that simply copied would put an
empty host onto the transport source, and an empty host is not an unwritten one:
it reads back as an account that names a server. So `commit` writes only for a
collection `is_complete` accepts — the same question `check_complete` answers,
true of the receiving instance (that is what let *Next* be pressed) and false of
the sending one. Silent in both directions, for the reasons already written down
under `check_complete`.

**The gap this leaves, which is real and is the next item.** On the assistant's
path the transport source therefore ends up with the service name `jmap` and no
server, and JMAP submission needs one. Nothing in the dialog can fix it: the
sending page is hidden for a store-and-transport provider (`e-mail-config-
assistant.c` hides it when `CAMEL_PROVIDER_IS_STORE_AND_TRANSPORT`), and the
backend that is its candidate cannot see the account. The place that can is the
collection backend in `evolution-source-registry`: it is handed the account and
can walk `e_collection_backend_list_mail_sources()` for the children parented to
it. That is M6-side work and the next increment; writing a host this backend does
not know would have been the alternative and is not one. It is written down in
`backend.rs` under "The gap this leaves, said plainly" rather than left in this
log alone.

**What this is not.** Still no module, no `e_module_load`, no
`module-jmap-configuration.so`, no widget, and no account created through
Evolution's UI — M7's actual acceptance, which this VM cannot do. The vfunc body
is not tested either, for the same reason as `check_complete`'s: driving it needs
a live `EMailConfigServiceBackend`, and the detached instance would reach
`e_mail_config_service_backend_get_collection`'s `E_IS_...` assertion. What is
tested is `commit`, over real `ESource`s: a finished account's server lands on a
blank mail source and reads back through the registry's own `server_of` as
`https://jmap.example.com:8443` — the same string the collection itself reads
back as — with the user beside it; the account `new_collection` offers writes
nothing at all, groups included, so the source still answers `MissingHost` rather
than naming an empty server; the mock server's plaintext account arrives as
`http://localhost:8443`; and neither a NULL collection nor a NULL source writes
anything. M7 carries no completion tag and this needs human verification in real
Evolution.

Not verified locally, as in every session: `reuse lint` and `cargo deny` (neither
binary is on this VM); no new files this time, so no new SPDX headers either.
`cargo fmt --check`, `cargo test --locked` (491 tests on the default members,
unchanged) and `cargo clippy --all-targets --locked -- -D warnings` are clean, as
are `clippy`/`test` over the EDS crates — 831 tests, `jmap-config`'s 75 (was 70)
included. `RUSTDOCFLAGS=-D warnings cargo doc` clean for the crate.
Pre-existing and untouched: `example-module` does not build on this VM, which is
why the workspace-wide runs exclude it.

No milestone tag.

Next: the transport source's server, in `jmap-backend-collection`. `populate`
has the account source and can reach the mail children the setup parented to it,
which is the one place both halves are in scope; `prepare_mail`'s module comment
already argues why the *vfunc* cannot do it, so the note belongs beside it.

## 2026-08-10 (hundred-and-twelfth session)

**The transport's server, and the mail sources' security method.** A new module
`rust/crates/jmap-backend-collection/src/mail_child.rs` with thirteen tests, red
first (they did not compile: there was no `mail_child`), in
`tests/mail_child.rs`; three lines in `child_added.rs` to reach it, and one test
in `jmap-config`'s `tests/mail.rs` holding the two crates' spelling of the
security method against each other, as that file already does for
`MAIL_BACKEND_NAME`.

**The gap the last session wrote down, closed where it said it would be.** The
transport that Evolution's assistant mints carries the service name `jmap` and no
`[Authentication]` group at all — the *Sending Email* page is hidden for a
store-and-transport provider, so the setup backend that is its candidate is never
shown the account. The collection backend is the one place holding both halves:
it is handed the account source, and `child_added` fires for every source
parented to the collection, the three mail sources included. So the group is
created there, on a source this account owns, and the account's host, port and
user are *bound* into it rather than copied — the same reasoning `child_added`
already gives for every other child, plus one that is specific to mail: EDS
decides whether a mail source shares its collection's password by comparing the
two `[Authentication] Host` strings, so a stale host is a second password prompt
as well as a wrong server.

**A bug found on the way, and it is the reason this is a module rather than a
flag.** `child_added` binds `[Security] secure` onto every child, and
`e_source_security_set_secure()` writes EDS's own word `"tls"`. On a *mail*
source that same key is additionally bound by `ESourceCamel` to
`CamelNetworkSettings:security-method` through
`e_binding_transform_enum_nick_to_value`, which reads it as a
`CamelNetworkSecurityMethod` **enum nick** — and `"tls"` is not one. The
transform returns FALSE, the binding sets nothing, and the settings object keeps
the property's default, `STARTTLS_ON_STANDARD_PORT`. So the existing binding was
overwriting the `ssl-on-alternate-port` that `jmap_config::mail::apply_server`
had just committed, with a string that silently means "whatever Camel defaults
to". The mail children now bind `secure` to *`method`* through a transform of
this module's, which writes Camel's nick; `tests/mail_child.rs` pins EDS's
`"tls"` spelling as the thing being worked around, so the workaround disappears
loudly if EDS ever stops needing it.

**What the tests assert, and where they assert it.** At the far end, which is
what makes them worth having: two of them read the transport back through
`jmap_mail::server::ServerConfig::from_settings` on the very `CamelSettings`
object `e_source_camel_configure_service` would hand a `CamelJmapTransport` —
`https://jmap.example.com:8443` for the account, and `http://localhost:8080` plus
`CAMEL_NETWORK_SECURITY_METHOD_NONE` for a plaintext one. That needed
`jmap-mail` as a dev-dependency of `jmap-backend-collection`, for the same reason
and with the same note as `jmap-config` already has it. Checked by mutation:
spelling the constant `"tls"` fails
`the_transport_sends_through_the_server_the_provider_would_connect_to`, so the
assertion bites rather than comparing a constant with itself — the string
assertions were rewritten as literals for the same reason. The rest: the
identity is left without a server, an `smtp` transport parented to this account
is left alone, an account with no `[Security]` reaches its mail sources as TLS
(what `collection_source` reads absence as) without a group appearing in the
user's own file, an account naming no server writes nothing, and the account's
`[Authentication] Method` — a credentials provider there, a SASL mechanism here —
is bound nowhere.

**What this is not.** Not verified in a running `evolution-source-registry`: that
`child_added` is dispatched for mail sources at all is EDS's own behaviour, which
`child_added.rs` documents from `collection_backend_child_added` and which this
VM cannot exercise — there is no registry on this bus. What is tested is the
function EDS would call, over real `ESource`s. M6 and M7 both still need human
verification in real Evolution and neither carries a completion tag.

Not verified locally, as in every session: `reuse lint` and `cargo deny` (neither
binary is on this VM); the two new files carry SPDX `GPL-3.0-or-later` headers.
`cargo fmt --check`, `cargo test --locked` (491 tests on the default members,
unchanged) and `cargo clippy --all-targets --locked -- -D warnings` are clean, as
are `clippy`/`test` over the EDS crates — 845 tests, was 831. The one ignored
test is the pre-existing `ignore` doctest in `jmap-backend-core`'s
`instance::Slot`. `RUSTDOCFLAGS=-D warnings cargo doc` clean for both touched
crates. Pre-existing and untouched: `example-module` does not build on this VM.

No milestone tag.

Next: `docs/manual-test-collection-backend.md` is written for an account with
`MailEnabled=false` and so says nothing about either mail source; now that a
`.source` file gains a server it was not written with, the recipe is worth
extending with a mail account whose transport can be read back out of the
registry's own directory after a populate — that is the one place this session's
binding becomes observable without a GUI.

## 2026-08-10 (hundred-and-thirteenth session)

**The mail run in the collection recipe.** Four new keyfiles under
`docs/examples/` — `jmap-mock-mail-collection.source` and the three sources a
mail account is made of, parented to it — a second run in
`docs/manual-test-collection-backend.md` that quotes them, and six tests in
`rust/crates/jmap-backend-collection/tests/recipe.rs`, red first (the files did
not exist; `RegistrySource::load` happily returns an empty source for a path with
nothing at it, so the failures were about content rather than about opening a
file).

**Why the recipe was the next thing rather than more code.** Last session gave
an account's mail sources a server, and nothing a reader can run showed it.
`docs/manual-test-collection-backend.md` was written for `MailEnabled=false` and
says, in the bullet explaining that line, that mail is the setup UI's to create.
That is still true — this backend creates no mail children — but it is not the
same as "there is nothing to test": the binding `child_added` puts on a mail
source is exactly what the assistant *cannot* supply, and writing the three
sources by hand is the only way to see it before M7 exists. So the run is an
account with `MailEnabled=true`, the three files, a restart, and then reading the
transport's own `.source` back to find two groups in it that the reader did not
write.

**What the tests assert, and the one that is worth the file.** The parents (a
`Parent=` typo is three sources belonging to nothing, shown nowhere and logged
nowhere); the two uids the sources point at each other with, read through
`ESourceMailAccount:identity-uid` and `ESourceMailSubmission:transport-uid`
rather than out of the text; which of the three `mail_service_of` claims, with
the identity claimed by nothing; that the account switches on all three parts,
because `collection_backend_bind_child_enabled()` binds each mail source's
`enabled` to the account's `mail-enabled` and mail off is three sources that
arrive disabled with nothing said about it. And the far end: `follow_collection`
over the documented account and the documented transport, then
`ServerConfig::from_settings` on the `CamelSettings` object
`e_source_camel_configure_service` would hand a `CamelJmapTransport` — the same
assertion `tests/mail_child.rs` makes, over the files a reader copies rather than
over sources a test built. It comes back `http://127.0.0.1:8080` with no user and
`CAMEL_NETWORK_SECURITY_METHOD_NONE`.

Checked by mutation, one file at a time, restoring in between: a wrong `Parent`,
`BackendName=smtp` on the transport, a misspelt `TransportUid`, `Method=tls` on
the account, `MailEnabled=false`, and a moved `Host` each fail exactly the tests
that name them and nothing else. `the_recipe_quotes_the_keyfile_verbatim` became
`the_recipe_quotes_every_keyfile_verbatim` over an ordered list of five files —
which makes an ```` ```ini ```` block in that document a whole keyfile and
nothing else, so the fragment showing what a mail source *grows* is fenced
without a language, and the test comment says so.

**What this is not.** Not run. That a live `evolution-source-registry`
dispatches `child-added` for hand-written mail sources, that EDS writes the
changed child back out to the file the reader will inspect, and that Evolution
then shows the mock's mailboxes are the three things the recipe exists to have a
human check; this VM has no registry on its bus and no display. The document says
which parts the test suite covers and which are left to the reader, in the
closing paragraph of the new section. M6 and M7 both still need human
verification in real Evolution and neither carries a completion tag.

Not verified locally, as in every session: `reuse lint` and `cargo deny` (neither
binary is on this VM); the four new files are `.source` keyfiles under `docs/`,
which `REUSE.toml` annotates as a directory rather than per file, so there are no
new headers to write. `cargo fmt --check`, `cargo test --locked` (491 tests on
the default members, unchanged) and `cargo clippy --all-targets --locked -- -D
warnings` are clean, as are `clippy`/`test` over the EDS crates — 850 tests, was
845, `jmap-backend-collection`'s recipe suite now 9 (was 4). The one ignored test
is the pre-existing `ignore` doctest in `jmap-backend-core`'s `instance::Slot`.
`RUSTDOCFLAGS=-D warnings cargo doc` clean for the crate. Pre-existing and
untouched: `example-module` does not build on this VM.

No milestone tag.

Next: `docs/` now has recipes for the book, cal and collection backends and none
for M5's Camel provider, which is the one surface a reader reaches through
Evolution's mail view; the mail run added here is written as if a
`docs/manual-test-mail-provider.md` existed to point at for the folder-listing
half. Writing it — a hand-written mail account against `jmap-mockd`, with the
`.urls` file and the `camel-provider` install component — would give this section
somewhere to hand off to, and `tests/` an obvious place for the same
quoted-verbatim check.

## 2026-08-10 (hundred-and-fourteenth session)

**The mail provider's own recipe.** `docs/manual-test-mail-provider.md`, the
three keyfiles it tells the reader to copy — `jmap-mock-standalone-mail.source`,
`-identity` and `-transport` under `docs/examples/` — and five tests in
`rust/crates/jmap-mail/tests/recipe.rs`, red first (four of the five; the fifth,
the no-`Parent=` check, passed vacuously because `RegistrySource::load` returns
an empty source for a path with nothing at it, so it only became a real
assertion once the files existed).

**Why this rather than more code.** M3, M4 and M6 each have a documented manual
recipe and a `tests/recipe.rs` holding it to the source tree; M5 had neither,
and it is the one surface a reader reaches through Evolution's mail view. Last
session's mail run in the collection recipe was written as if this document
existed — it now points at it, with the reason: running the standalone account
first is how you tell a broken provider from a broken binding.

**What the account is, and the one line that differs from the collection run.**
No `Parent=`, because until M7's assistant exists there is no account to hang
these off — the same shape as the book and calendar recipes. The consequence is
that `[Authentication]`/`[Security]` appear *twice*, once on the store and once
on the transport: two `CamelService`s configured from two sources, with no
collection above them to copy a server from one to the other. A transport that
lost its copy is the quietest failure in the document — the account receives
mail perfectly and fails only at Send — so
`the_documented_services_both_reach_the_mock_server` runs `ServerConfig::from_settings`
over *both* files, off the `CamelSettings` an `e_source_camel_configure_service`
would hand the service, and both come back `http://127.0.0.1:8080` with no user
and `CAMEL_NETWORK_SECURITY_METHOD_NONE`. The other four: both services name the
protocol `camel_provider_module_init` registers and the identity names none; the
two uids the sources point at each other with, read through
`ESourceMailAccount:identity-uid` and `ESourceMailSubmission:transport-uid`;
none of the three has a parent; and every ```` ```ini ```` block in the document
is one of the three files verbatim.

**One claim checked rather than assumed.** The scratch-tree instructions name
`EDS_CAMEL_PROVIDER_DIR`, and the document says it *replaces* Camel's provider
directory rather than adding to it — which matters more here than for the
registry, since every other mail provider lives in the directory being replaced.
Verified on this VM with a throwaway C program against the installed
`libcamel-1.2`: `camel_provider_get("rss")` finds the stock provider by default
and answers *No provider available for protocol 'rss'* with the variable set to
an empty directory. That error string is quoted in the document as the symptom
of a `BackendName` typo, for the same reason.

Checked by mutation, one file at a time and restoring in between: a transport
with its `[Authentication]` group removed, `BackendName=jmapp` on the account, a
misspelt `TransportUid`, a `Parent=` added to the account, `Method=tls` on the
transport, and an `Address=` changed in the document each fail exactly the test
that names them (plus the verbatim check, whenever a file moved and the document
did not) and nothing else.

**What this is not.** Not run. That Camel dlopens the installed module by its
`.urls` file, that Evolution offers the account, and that the mock then serves
`Mailbox/get`, `Email/query`+`Email/get`, a blob download, `Email/set` and
`EmailSubmission/set` are what the recipe exists to have a human check; this VM
has no Evolution and no display. The document's closing paragraph says which
half the test suite covers and which is the reader's. M5 gets no completion tag
from a document.

Not verified locally, as in every session: `reuse lint` and `cargo deny`
(neither binary is on this VM); the one new source file carries an SPDX
`GPL-3.0-or-later` header, and the three new `.source` keyfiles are under
`docs/`, which `REUSE.toml` annotates as a directory. `cargo fmt --check`,
`cargo test --locked` (491 tests on the default members, unchanged) and
`cargo clippy --all-targets --locked -- -D warnings` are clean, as are
`clippy`/`test` over the EDS crates — 855 tests, was 850, `jmap-mail`'s suite now
410. The one ignored test is the pre-existing `ignore` doctest in
`jmap-backend-core`'s `instance::Slot`. Pre-existing and untouched:
`example-module` does not build on this VM.

**A finding for a later session.** `RUSTDOCFLAGS=-D warnings cargo doc -p
jmap-mail` fails with 25 errors, all `rustdoc::private_intra_doc_links` in
`src/cache.rs` and its neighbours — module-level prose linking `[`UNUSED_FOR`]`,
`[`valid_key`]`, `[`claimed`]` and friends, which are private. Confirmed
pre-existing by stashing this session's work and rerunning: the same 25 on clean
master. Nothing here touches those files; it is a self-contained cleanup worth
its own commit, and worth a check in CI afterwards so it cannot come back.

No milestone tag.

Next: the README's "Trying the address book backend in Evolution" section still
points only at `docs/manual-test-book-backend.md`, and there are now four
recipes. A short list there, or a `docs/README.md` index the four hang off, is
the cheap way to make the other three findable — the calendar and collection
ones have been unlinked from anywhere a newcomer reads since they landed.

## 2026-08-10 (hundred-and-fifteenth session)

**The module M7's class was missing.** `jmap-config` grew `src/module.rs` —
`e_module_load` and `e_module_unload`, the two symbols Evolution's shell
resolves out of `module-jmap-configuration.so` — became a `cdylib` as well as an
rlib, and got an install rule in `cmake/Backends.cmake` beside the four that
were already there. Five tests in `tests/module.rs`, red first (they would not
compile: there was no `jmap_config::module`).

**Why this and not more decisions.** Everything in the crate so far is a
function over an `ESource`, tested and reached by nothing: the class
`tests/backend.rs` pins down was registered only by the test that pinned it.
The crate's own header said so — "there is a subclass now, but nothing
registers it" — and until a module registered it, no part of M7 was a thing
Evolution could do. The two vfuncs still missing (`insert_widgets`,
`setup_defaults`) need GTK bound in `evo-sys`, which is a bigger piece than one
increment; the module is the small one that was blocking nothing but its own
absence.

**The one way it differs from the three EDS modules already in tree, and the
test that came out of it.** The book, calendar and collection modules each
register a *factory* as well, because their hosts look a backend up by name in
a table the factory puts it in. Evolution's account editor has no such table:
`EMailConfigServicePage` is an `EExtensible`, and `e_extensible_load_extensions`
walks the children of `EExtension` that exist at that moment and instantiates
every one whose class `extensible_type` is the page's own type. So registering
the type *is* the registration — there is nowhere to add it and nobody to tell
— and the thing that can silently go wrong is not a missing factory but a
clobbered `extensible_type`, which registers a type nothing ever instantiates
with no error anywhere. Hence
`the_registered_type_is_an_extension_of_the_page_that_will_load_it`, which reads
the field through `EExtensionClass` and asserts `g_type_name` of it is
`EMailConfigServicePage`. The other four: the type is registered and is an
`EMailConfigServiceBackend`; it belongs to the `GTypeModule` rather than being
static (a static type in a dlopened module outlives the code its vfunc pointers
point into); a use/unuse/use cycle hands the types back, because a second
`e_module_load` that treated "already registered" as "nothing to do" would leave
the account type gone for the rest of the session; and the class the *module*
registered still carries `backend_name` and the three installed vfunc slots,
since the dynamic path initialises the class later and again after every reload.

Checked by mutation, restoring in between: `register_static` in place of
`register_dynamic` fails only `the_registered_type_belongs_to_the_module`, and
zeroing `extensible_type` in `backend`'s `class_init` fails only the extensible
test.

**The install rule.** `add_cargo_cdylib(jmap_config OUTPUT_NAME
module-jmap-configuration.so DESTINATION ${EVOLUTION_MODULE_DIR} COMPONENT
config-module SYMBOLS e_module_load e_module_unload VERIFY_DESTINATION_FROM
evolution-shell-3.0 moduledir)` — the fifth `install-*` CTest, and the first
into Evolution's own module directory (`/usr/lib/evolution/modules` here) rather
than one of the data server's. Run on this VM: `ctest -R install-config-module`
passes, staging
`usr/lib/evolution/modules/module-jmap-configuration.so`, and `nm -D` over the
built object shows exactly two defined symbols, `e_module_load` and
`e_module_unload`. CI needed no change — it runs `ctest` over the whole tree and
uploads `build/cargo-target/release/*.so` by glob.

`gobject-sys` moved from dev-dependencies to dependencies, for the `GTypeModule`
in the entry point's signature and nothing else.

**What this is not.** Not run. That Evolution dlopens the installed module, that
JMAP then appears in the account type list, and that the page it opens behaves
are what needs a running Evolution and a display this VM has neither of. M7
**still needs human verification in real Evolution** and carries no completion
tag — and it would not deserve one yet regardless: `insert_widgets` and
`setup_defaults` are missing, so the assistant would show a JMAP account with no
entry to type an address into and *Next* correctly greyed out. `src/lib.rs` and
`src/backend.rs` were updated to say that rather than the older "no module loads
this class".

Not verified locally, as in every session: `reuse lint` and `cargo deny`
(neither binary is on this VM); the two new source files carry SPDX
`GPL-3.0-or-later` headers. `cargo fmt --check`, `cargo test --locked` (491
tests on the default members, unchanged) and `cargo clippy --all-targets
--locked -- -D warnings` are clean, as are `clippy`/`test` over the EDS crates —
860 tests, was 855. The one ignored test is the pre-existing `ignore` doctest in
`jmap-backend-core`'s `instance::Slot`. `RUSTDOCFLAGS=-D warnings cargo doc -p
jmap-config` is clean. Still open from last session and untouched here: the same
command over `jmap-mail` fails with 25 pre-existing
`rustdoc::private_intra_doc_links`. Pre-existing and untouched: `example-module`
does not build on this VM.

No milestone tag.

Next: the two vfuncs, which means GTK in `evo-sys` — `insert_widgets` takes a
`GtkBox`, currently an opaque pointer there on purpose (gtk-rs's `gtk-sys` is
frozen on `glib-sys` 0.18 and this workspace is on 0.22, so the widget calls
have to be generated from Evolution's own headers rather than borrowed). That is
a self-contained `evo-sys` increment — allowlist the handful of GTK entry points
an entry-and-label page needs, with a `layout.rs` check on each — before any of
it can be a vfunc. The standing directive on translatable strings starts biting
there too: the first labels this project originates are the ones
`insert_widgets` adds, and `bindtextdomain` has to be wired into this module's
init at the same time.

## 2026-08-10 (hundred-and-sixteenth session)

**The widget calls M7's page needs, in `evo-sys`.** Last session's "next" was
this: `insert_widgets` is handed a `GtkBox` and has to put widgets in it, and
`evo-sys` spoke no GTK at all — `GtkBox` was there as `c_void`, a placeholder for
the container of a page nothing could build. `build.rs` now generates eleven GTK
entry points beside the Evolution ones (`ALLOWED_GTK_FUNCTIONS`): a grid packed
into the box, and per setting a mnemonic label right-aligned beside an
`hexpand`ing entry, plus the six `*_get_type` getters the new tests ask the type
system through. Four tests in `tests/gtk.rs`, red first (25 compile errors: no
`gtk_*` anything).

**Named one function at a time, not by prefix.** `gtk_(grid|label|entry)_.*`
would take several hundred functions, nearly all of them about a realized widget
hierarchy, into a surface whose whole claim is that what is in it was looked at.
The list is meant to grow a line at a time with the code that calls it, and the
entry-point test names each one back, so adding a call to the module without
adding it here is a compile error rather than a `dlopen` failure in Evolution.
The property *bindings* stay out: an entry's `text` is
`g_object_bind_property` onto the `CamelSettings`, which is `gobject-sys` and
already available.

**Distinct opaque handles instead of one `c_void`.** GTK's types are still not
generated — no `gtk-sys` to re-export from (it is frozen on `glib-sys` 0.18
against this workspace's 0.22, and two `GObject`s in one process is the failure
the blocklist exists to prevent), and a generated GTK class layout would be the
one thing in this crate nothing cross-checks against `g_type_query`, since no GTK
class is subclassed or allocated here. So `GTK_HANDLES` emits a zero-sized
`#[repr(C)]` struct with a private field per class. Separate types rather than
five aliases for `c_void`, which is the change of mind from the older comment:
GTK's C API takes one object as a `GtkGrid *` here and a `GtkWidget *` there, and
with distinct types each crossing has to be written as a `.cast()` — the Rust
spelling of `GTK_GRID()`. Aliased to `c_void` a `GtkBox` could be handed to
`gtk_grid_attach`, which compiles and is undefined behaviour.

**What the tests can and cannot check, since none of them builds a widget.** GTK
3 will not construct one without a display: `gtk_grid_new()` reaches
`GtkWidget`'s instance init, which wants a `GtkStyleContext`, which aborts with
"Can't create a GtkStyleContext without a display connection" — verified here
before writing the file, and there is no Xvfb on this VM either. So the page
itself remains M9's Xvfb tier. What is checked is the part that fails silently:
the entry points *link* (a missing name fails the test binary's link, which is
the `undefined symbol` Evolution would hit on dlopen, moved to a red test); the
six classes are registered under the names this crate calls them; they stand in
the inheritance relations the casts assume (`g_type_is_a` against `GtkWidget` and
`GtkContainer` — and note `GtkLabel`'s parent really is the deprecated `GtkMisc`
on 3.24, which is why the assertion is `is_a` and not `g_type_parent`); and the
handles stayed zero-sized.

**A GTK thread-safety finding, caught by the first green run.** Two of the tests
touched the type system in parallel — the harness runs `#[test]`s on separate
threads — and the run printed `cannot register existing type 'GtkContainer'`,
then a `<invalid>` type whose getter returns 0 forever, failing one test.
`gtk_container_get_type()` in GTK 3 is hand-written rather than a `G_DEFINE_TYPE`
macro, and its guard is a plain `static GType container_type = 0;` with no
`g_once_init_enter`: two threads both see zero and both register. That is GTK
behaving as documented (GTK 3 is a one-thread library), so the tests are what
changed — all six types are resolved once behind an `OnceLock` and every test
reads them from there. Worth remembering for any future test that touches GTK:
it must not do so from two threads. The module itself is fine; Evolution calls
these vfuncs on the main thread.

Checked by mutation, restoring in between: dropping `gtk_label_set_xalign` from
the allowlist fails the entry-point test (as a compile error, which is the
intent), and `_opaque: [u8; 1]` in the handle template fails only
`the_widget_handles_carry_no_layout`.

**Two doc corrections that came with it.** `jmap-config`'s `backend` said
reaching `insert_widgets`/`setup_defaults` "means binding more of Evolution's
headers, GTK among them"; half of that is now done, and what is actually left is
one accessor — `e_mail_config_service_page_get_email_address`, from a header
`evo-sys` does not read yet — which is `setup_defaults`'s one input. It now says
so, and says plainly that the reason neither vfunc is written is not the bindings
but that a widget body would be code no test on this machine runs, which is the
opposite of the order the rest of the crate was built in.

**What this is not.** Not a working dialog, and not a step that can be verified
in one: no widget was created, let alone shown. M7 still **needs human
verification in real Evolution** and carries no completion tag.

Not verified locally, as in every session: `reuse lint` and `cargo deny`
(neither binary is on this VM); the one new source file carries an SPDX
`GPL-3.0-or-later` header. `cargo fmt --check`, `cargo test --locked` (491 tests
on the default members, unchanged) and `cargo clippy --all-targets --locked -- -D
warnings` are clean, as are `clippy`/`test` over the EDS crates — 864 tests, was
860, `evo-sys`'s suite now 7. The one ignored test is the pre-existing `ignore`
doctest in `jmap-backend-core`'s `instance::Slot`. `RUSTDOCFLAGS=-D warnings
cargo doc` is clean for `evo-sys` and `jmap-config`; still open and untouched
from two sessions ago is the same command over `jmap-mail`, 25 pre-existing
`rustdoc::private_intra_doc_links`. Pre-existing and untouched: `example-module`
does not build on this VM.

No milestone tag.

Next: `setup_defaults` is now within reach of being *written* if not run — it
needs `e_mail_config_service_page_get_email_address` bound (one more allowlist
line and a header in `wrapper.h`) and its body is
`defaults::from_identity` followed by `mail::apply_server`, both already tested.
It would be the first vfunc here whose body cannot be exercised on this machine,
so it wants a deliberate decision rather than a drive-by: either write it
conservatively and mark it unverified, or hold M7 until there is a session with a
real Evolution. The other still-open items are unchanged: the four manual-test
recipes are unlinked from the README, and `jmap-mail`'s rustdoc is dirty.

## 2026-08-10 (hundred-and-seventeenth session)

**`setup_defaults`, the third vfunc whose answer is an `ESource` — plus the one
binding it was missing.** Last session's "next" named it and said it wanted a
deliberate decision rather than a drive-by, because its body would be the first
here that no test on this machine runs. It turned out not to be: the vfunc splits
the same way `check_complete` and `commit_changes` do, into a trampoline that
fetches two pointers and a `pub unsafe fn setup(collection, address)` that is
ordinary Rust over an `ESource`. Seven new tests in `jmap-config`, two in
`evo-sys`; red first in both.

**`evo-sys`: `e_mail_config_service_page_get_email_address`.** The one input
`setup_defaults` has, and the one thing this module asks of Evolution rather than
of a source. `wrapper.h` now reads `mail/e-mail-config-service-page.h`, and the
accessor is allowlisted by name — a `e_mail_config_service_page_.*` would have
taken on the page's scratch sources, its backend lookup and its
auto-configuration, none of which this module may touch. The page *type* joins
the opaque handles (`EVO_HANDLES`, beside `GTK_HANDLES`): it is a
`GtkScrolledWindow` two classes down, so generating it means generating the GTK
class structs the blocklist exists to keep out — bindgen said so itself, with a
`parent: GtkScrolledWindow` naming a type nothing defines. One wrinkle worth
recording: a blocklisted Evolution class comes through in the generated
signatures under its *struct tag* (`_EMailConfigServicePage`), unlike the GTK
ones which come through under their typedef, so the handle is emitted under the
tag with the typedef aliased to it — which is also how bindgen writes the
Evolution classes it does generate. `tests/page.rs` checks the two things that
fail silently otherwise: that the accessor links (the `undefined symbol`
Evolution would hit on dlopen, moved to a red test), and — as a pair of typed
function pointers, so the compiler is the assertion — that the page a backend
hands back is the same type the address is read from.

**What the defaults are, and why they are not simply `from_identity` applied.**
`from_identity` describes a whole account: address, the server its domain
implies, the login name it offers, *and* the three parts and the TLS switch. Only
the first three are things the address says; the other two were written by
`new_collection` before the user saw the page, and by the time this runs they may
be answers the user gave. So `setup` takes the derived fields from the offer and
leaves the rest of the account as it stands — a *Calendars* box the user unticked
stays unticked. On a collection fresh from `new_collection` the result is exactly
`from_identity(address)`, which is asserted rather than assumed, read back
through the registry's own reader.

And an address that has not changed writes nothing at all. A JMAP server may
perfectly well not live at the domain of the address — RFC 8620 §2.2 only says
that is where to *ask* — so a user who corrected the server by hand must not have
the correction reverted merely because they looked at the previous page again. A
*changed* address is the opposite case and is re-derived: the server they typed
was for the address they have just stopped naming.

**What could not be checked, and was therefore not claimed.** How often Evolution
calls this vfunc. The assistant's page-preparation order is in a source this
machine does not have (no `deb-src`) and a dialog it cannot run, so the first
draft's "runs every time the assistant reaches the page" came out of the comments
and what is written instead is that the implementation is correct for one call or
for many — which is the property the merge rule above actually gives it.

**One thing that *was* checked rather than assumed.** The first draft of the slot
test asserted Evolution installs no `setup_defaults` of its own; it does. Rather
than guess what it does, the pointer was resolved to a file offset through
`/proc/self/maps` and disassembled: `endbr64; ret`, an empty stub, as is
`insert_widgets` next to it. So the test reads like the other three —
`setup_defaults_displaces_the_inherited_one` — and the class comment says
"an empty function (read off the installed library, not assumed)".

Checked by mutation, restored after: applying the whole `from_identity` instead
of merging fails exactly the two tests that describe the merge, and dropping the
allowlist line fails `tests/page.rs` as a compile error. (Note to future
sessions: restore a mutation with the editor, not `git checkout` — that reverted
the whole file and the build.rs work had to be typed again.)

**What this is not.** Not a dialog anyone has seen. No widget is created, the
page is still opaque on this side of the ABI, and `insert_widgets` remains
unwritten — which now means an account arrives on the server settings page
filled in and *cannot be corrected there*, since there are no entries. M7 still
**needs human verification in real Evolution** and carries no completion tag.

Not verified locally, as in every session: `reuse lint` and `cargo deny` (neither
binary is on this VM); both new source files carry an SPDX `GPL-3.0-or-later`
header. `cargo fmt --check`, `cargo test --locked` (491 on the default members,
unchanged) and `cargo clippy --all-targets --locked -- -D warnings` are clean, as
are `clippy`/`test` over the EDS crates — 873 tests, was 864; `evo-sys`'s suite
is now 9 and `jmap-config`'s `tests/backend.rs` 24. The one ignored test is the
pre-existing `ignore` doctest in `jmap-backend-core`'s `instance::Slot`.
`RUSTDOCFLAGS=-D warnings cargo doc` is clean for `evo-sys` and `jmap-config`;
still open and untouched is the same command over `jmap-mail`, 25 pre-existing
`rustdoc::private_intra_doc_links`. Pre-existing and untouched: `example-module`
does not build on this VM.

No milestone tag.

Next: `insert_widgets` is now the only unwritten slot, and it is still the one
whose body no test here can run — the entries would bind onto the same collection
source every other vfunc reads, which is the decision already made and recorded
in `check_complete`'s comment. It is work for a session with a real Evolution or
for M9's Xvfb tier, and the standing directive on translatable strings starts
biting with the first label it adds (`bindtextdomain` in the module's init, and a
`po/POTFILES.in` that has this crate in it). The other still-open items are
unchanged: the four manual-test recipes are unlinked from the README, and
`jmap-mail`'s rustdoc is dirty.

## 2026-08-10 (hundred-and-eighteenth session)

**The standing directive on translatable strings, from the bottom up: a gettext
domain that is bound before anything looks a string up in it.** The directive
has been open since 2026-08-09 and says a string shipped unmarked is a bug to be
filed; nothing was marked yet because there was nowhere for a marked string to
be looked *up*. This session put that in.

**Two commits, because the first was already written.** The working tree at the
start of this session held an uncommitted, finished `i18n` module in
`jmap-backend-core` — `build.rs`, `src/i18n.rs`, `tests/i18n.rs`,
`tests/catalogue.rs`, and the `Cargo.toml`/`lib.rs` edits — evidently the last
session's work, cut off before it could commit. It was verified here rather than
taken on trust: `cargo fmt --check`, `clippy`, and the suite are clean, and
`tests/catalogue.rs` reported `catalogue lookup exercised under locale
en_US.UTF-8`, i.e. it took the branch that actually compiles a `.mo`, files it
at `<dir>/<lang>/LC_MESSAGES/evolution-jmap.mo` and reads a German string back
through the binding, not the degenerate branch it falls back to on a machine
with no non-C locale. Committed as it stood.

**The increment: `camel_provider_module_init` binds the domain.** The provider
carries a `translation_domain` and Camel calls `dgettext` with it when it
displays the provider's name and description — the JMAP entry in the account
assistant's list of account types. That lookup happens in Evolution's or
`evolution-source-registry`'s process, neither of which has heard of this
project, and it is Camel's call with no hook in it. So the binding has to be
made by the one piece of our code guaranteed to have run by then, which is the
entry point Camel dlopened the module for. `provider.rs` now names the domain
through `i18n::DOMAIN` instead of repeating the literal: the binding is only
worth making for the strings that are looked up in it, and nothing but the new
test connects the two — Camel reads the provider field, glibc holds the binding.

**`i18n::binding`, and why the tests start from a wrong directory.** `bind` and
`bind_to` answer with the binding they just made, which is no use to a test that
wants to know whether *someone else* bound it — asking by binding would make the
answer yes either way. `bindtextdomain` with a NULL directory is the read-only
form. Both new tests (`jmap-backend-core/tests/binding.rs`,
`jmap-mail/tests/textdomain.rs`) first bind the domain to
`/nonexistent/jmap-decoy-locale`. Without the decoy neither would be worth
running: on an uninstalled build `LOCALE_DIR` *is* gettext's compiled-in
`/usr/share/locale`, so a process that had never bound anything reports it too
and the test would pass against a module that did nothing. Each is alone in its
file because the binding, and `bind`'s `OnceLock`, are process-global — a
sibling test reaching the entry point first would spend the `OnceLock` and leave
the decoy in place. Red first in both cases; the jmap-mail one failed with
`left: "/nonexistent/jmap-decoy-locale"`, which is the failure the decoy exists
to produce.

**CMake now passes the directory `build.rs` was written to receive.** Until this
session nothing set `EVOLUTION_JMAP_LOCALEDIR`, so the whole "the directory is a
build-time input" argument in `build.rs` was aspirational. `CARGO_ENV` in
`cmake/Rust.cmake` now carries `EVOLUTION_JMAP_LOCALEDIR=${LANGUAGE_SUPPORT_DIRECTORY}`
(`${CMAKE_INSTALL_PREFIX}/share/locale`, already defined at the top level).
Checked, not assumed: configuring with `-DCMAKE_INSTALL_PREFIX=/opt/staged`
puts `EVOLUTION_JMAP_LOCALEDIR=/opt/staged/share/locale` into
`CMakeFiles/rust-build.dir/build.make`, and building the crate with that
variable set puts the string in the rlib (`strings | grep -c` → 2) while
building without it puts it there zero times — so `rerun-if-env-changed` tracks
it in both directions. The test invocations deliberately do *not* get it: a test
binary is installed nowhere, and the fallback is the same `/usr/share/locale`
gettext would have used anyway.

**Blocker found, not caused, not fixed: `ctest`'s `rust-test-eds` cannot link.**
`cmake/Rust.cmake` runs that test with `CARGO_INCREMENTAL=0`, and under that
setting `jmap-config`'s test binaries fail to link:

    rust-lld: error: duplicate symbol: e_module_load
      >>> crates/jmap-config/src/module.rs:60          in libjmap_config.rlib
      >>> crates/jmap-backend-collection/src/module.rs:45 in libjmap_backend_collection.rlib

`jmap-config` dev-depends on `jmap-backend-collection`, both build `rlib` beside
`cdylib` so the tests can call the entry points, and both must export the C
symbol `e_module_load` because that is the name `EModule` resolves. With
incremental codegen on (the default, and what a plain `cargo test` uses) the
symbol lands in a codegen unit nothing pulls out of the archive and the link
succeeds; with `-Ccodegen-units=16` it shares an object with something the test
does reference, and the collision is real. Reproduce with:

    CARGO_INCREMENTAL=0 cargo test --locked -p jmap-config

Verified pre-existing rather than introduced here: the same command fails
identically in a worktree at `4184350`, this session's base, ten `duplicate
symbol` lines. It dates from `91372bc`, which gave `jmap-config` its
`e_module_load` while the crate already dev-depended on the collection backend.
Not fixed this session — it is a second item, and the roadmap says not to start
one. It is also not obviously a one-liner: the candidate fixes (drop the
dev-dependency and get whatever the tests want from it another way; put the C
entry points behind `#[cfg(not(test))]` and lose the "rlib and cdylib cannot
drift" property the two crates document; `--allow-multiple-definition`) trade
against each other and want a decision rather than a reflex.

**Housekeeping: 34 GB of stale `target/` sat on a 58 GB disk** and the first full
run of this session died on `No space left on device` mid-link, which is worth
recording because the resulting `rustc-LLVM ERROR`/`Bus error` output looks
nothing like a full disk. `target/debug` and `target/doc` were removed and
rebuilt from scratch; 31 GB free afterwards. It was during that cold rebuild
that the duplicate-symbol failure surfaced, which is presumably why no earlier
session saw it — a warm incremental tree hides it.

Not verified locally, as in every session: `reuse lint` and `cargo deny`
(neither binary is on this VM); both new test files carry an SPDX
`GPL-3.0-or-later` header, and `cmake/Rust.cmake` already had one.
`cargo fmt --check`, `cargo test --locked` (491 on the default members,
unchanged) and `cargo clippy --all-targets --locked -- -D warnings` are clean,
as are `clippy`/`test` over the EDS crates — 880 tests, was 873: four from
`tests/i18n.rs`, one from `tests/catalogue.rs`, one from the new
`jmap-backend-core/tests/binding.rs`, one from the new
`jmap-mail/tests/textdomain.rs`. The one ignored test is the pre-existing
`ignore` doctest in `jmap-backend-core`'s `instance::Slot`. Pre-existing and
untouched: `example-module` does not build on this VM, and `jmap-mail`'s rustdoc
carries 25 `rustdoc::private_intra_doc_links`.

No milestone tag. The translatable-strings directive is started, not carried
out: one domain is bound from one of five module entry points, and no string
anywhere is marked yet.

Next, in the order they would be taken: (1) the `e_module_load` collision above,
because it is the only thing here that makes a CI check red rather than merely
incomplete. (2) The same `bind()` call in the other four entry points —
`jmap-backend-book`, `-cal`, `-collection`, `jmap-config` — each of which needs
a `GTypeModule` stand-in in its test, the pattern `jmap-config/tests/module.rs`
and `jmap-backend-cal/tests/factory.rs` already have. (3) `po/` with
`POTFILES.in` and `LINGUAS`, plus the lint the directive asks for, and an install
rule putting a compiled `.mo` under `LANGUAGE_SUPPORT_DIRECTORY` — nothing
installs a catalogue yet, so every lookup still falls back to English by design.
Note while there: the top-level `GETTEXT_PACKAGE` is still the skeleton's
`example-module`, while our domain is `evolution-jmap`; the C example module and
the Rust modules do not share a catalogue and probably should not, but it should
be a decision rather than an oversight. Unchanged from previous sessions: M7
still **needs human verification in real Evolution** (`insert_widgets` remains
unwritten, so an account arrives on the server settings page filled in and
cannot be corrected there), the four manual-test recipes are unlinked from the
README, and `jmap-mail`'s rustdoc is dirty.

## 2026-08-10 (hundred-and-nineteenth session)

**The `e_module_load` collision, which the previous session found and ranked
first because it is the only thing here that makes a CI check red rather than
merely incomplete.** Fixed, with a red test that turned out to describe a worse
bug than the link error did.

**The link error was the mild symptom.** `CARGO_INCREMENTAL=0 cargo test -p
jmap-config` — CMake's setting for the `rust-test-eds` ctest — failed with
`duplicate symbol: e_module_load` between `jmap-config`'s and
`jmap-backend-collection`'s rlibs. A plain `cargo test` linked fine, and *that*
is the bad case: a `#[unsafe(no_mangle)]` function is not a Rust function that
also has a C name, it **is** the C symbol, so the Rust path
`jmap_config::module::e_module_load` compiles to a call to the symbol
`e_module_load` — and with one definition kept and one dropped, both crates'
paths reached the survivor. The new
`jmap-config/tests/entry_points.rs` calls each crate's entry point through a
`GTypeModule` stand-in of its own and asks the type system which types appeared.
Red first, and not with a link error:

    the setup module's entry point did not register
    "EMailConfigServiceBackendJmap" against it
      left: 0x0
     right: 0x75f7e80024a0

— i.e. `jmap_config`'s entry point had registered the *collection* backend's
types. That is the failure `--allow-multiple-definition` would have made
permanent and silent, which is why it was rejected out of hand along with
`#[cfg(not(test))]` (integration tests link the dependency's non-test rlib, so
it does nothing) and a cargo feature (features unify across a workspace build,
so `rust-test-eds`, which passes both `-p` flags, would re-enable it).

**The fix: the C symbol belongs to the shared object, not to the library.** Four
new crates — `jmap-backend-book-module`, `jmap-backend-cal-module`,
`jmap-backend-collection-module`, `jmap-config-module` — each `crate-type =
["cdylib"]` and nothing else, each holding the two `#[unsafe(no_mangle)]`
definitions and delegating to `module::load`/`module::unload` in the rlib beside
it. The four libraries are now `rlib` only and export ordinary mangled names.
Two cdylibs are never linked together because nothing links a cdylib, so the
class of bug is gone rather than the one instance of it.

Applied to all four and not only to the colliding pair: all four define the same
symbol pair, so any future test that links two of them would have hit the same
thing. `jmap-mail`'s `camel_provider_module_init` was deliberately left where it
is — it is the only definition of that name in the workspace, so there is
nothing to collide with and no red test to drive a change.

**The "cdylib and rlib cannot drift" property the old manifests claimed is kept
and strengthened.** It used to rest on both crate types being built from one
source file; it now rests on the cdylib having no behaviour to drift with — two
calls, no `guard` (the bodies are guarded where they are written, and wrapping
twice would mean two places to get the panic boundary wrong). The tests call the
bodies directly, which is what they did before under a different name.

**Verified end to end, not just compiled.** `cmake -S . -B build-verify -G
Ninja && cmake --build build-verify && ctest` — 7/7 pass, including
`rust-test-eds` (the previously red one) and the five `install-*` staged-install
checks, which are what actually prove the change: `add_cargo_cdylib`'s `SYMBOLS`
argument re-checks `e_module_load`/`e_module_unload` on each installed `.so`
after the rename from `libjmap_backend_book.so` to
`libjmap_backend_book_module.so` and friends. `nm -D` on the four debug cdylibs
shows both symbols as `T` in each.

`cargo fmt --check`, `cargo test --locked` (491 on the default members,
unchanged) and `cargo clippy --all-targets --locked -- -D warnings` are clean,
as are clippy and test over the EDS crates — 881 tests, was 880, the one new one
being `entry_points`. `RUSTDOCFLAGS=-D warnings cargo doc` is clean for the four
new crates and for `jmap-config`/`jmap-backend-collection`. Not verified locally,
as in every session: `reuse lint` and `cargo deny` (neither binary is on this
VM); every new file carries an SPDX `GPL-3.0-or-later` header. Pre-existing and
untouched: `example-module` does not build on this VM, and `jmap-mail`,
`jmap-backend-book` and `jmap-backend-cal` carry `rustdoc::private_intra_doc_links`.

No milestone tag.

Next, unchanged from the previous session apart from the item now struck off:
(1) the `bind()` call in the other four entry points — `jmap-backend-book`,
`-cal`, `-collection`, `jmap-config`. Note that the natural home for it has just
moved: these are now `*-module` cdylib crates with no tests of their own, so
either the binding goes in `module::load` (testable where the existing
`tests/factory.rs` stand-ins already are, and the honest place, since `load` is
what runs) or those crates need test harnesses. (2) `po/` with `POTFILES.in` and
`LINGUAS`, the lint the standing directive asks for, and an install rule putting
a compiled `.mo` under `LANGUAGE_SUPPORT_DIRECTORY`; while there, the top-level
`GETTEXT_PACKAGE` is still the skeleton's `example-module` while our domain is
`evolution-jmap`, which should be a decision rather than an oversight.
Unchanged: M7 still **needs human verification in real Evolution**
(`insert_widgets` remains unwritten), the four manual-test recipes are unlinked
from the README, and `jmap-mail`'s rustdoc is dirty.

## 2026-08-10 (hundred-and-twentieth session)

**The `bind()` call in the other four entry points**, which the previous two
sessions ranked next: `jmap-backend-book`, `jmap-backend-cal`,
`jmap-backend-collection` and `jmap-config` now bind this project's gettext
domain from `module::load`, as `jmap-mail`'s `camel_provider_module_init`
already did. Red first, in four new `tests/textdomain.rs` files:

    the entry point did not bind the domain, so this backend's translated
    strings would be looked for wherever the address book factory happened to
    point
      left: "/nonexistent/jmap-book-decoy-locale"
     right: "/usr/share/locale"

The decoy binding is the whole design of the test. On an uninstalled build
`LOCALE_DIR` *is* gettext's compiled-in `/usr/share/locale`, so a process that
had never bound anything reports exactly what a correct module would — asserting
`binding() == LOCALE_DIR` from a clean process passes against a module that does
nothing. Binding to a directory neither of them would be, first, is what makes
the assertion able to fail.

**`module::load`, not the cdylib.** The previous session moved the C symbols out
into four `*-module` cdylib crates and noted the binding's natural home had moved
with them. It went into `load` rather than into those crates: `load` is what
actually runs, it is where the existing `GTypeModule` stand-in tests can reach
it, and the cdylibs were deliberately left with no behaviour of their own to
drift with. Inside the `guard`, and before the registrations — a translated
string can be asked for as soon as a class exists, and there is no later point
at which we are called and could still get in front of the first lookup.

**Four files, not one shared helper, and four separate bindings.** Each test
needs its own process: `bind` is a `OnceLock`, so a sibling test in the same
binary that reached the entry point first would spend it and leave the decoy in
place, failing this one for the wrong reason. Cargo gives each file in `tests/`
its own process, so being the only test in the file *is* the isolation — the
same reason `jmap-mail/tests/textdomain.rs` is its own file. The `GTypeModule`
stand-in is therefore duplicated per crate, as it already is between each
crate's `factory.rs`/`module.rs` tests. Each test drives `g_type_module_use`
rather than calling `load` directly, so the entry point is reached through the
vfunc the way EDS/Evolution reach it. And the binding is made in all four
modules rather than in one, because they are dlopened by *different processes*:
a calendar-only account never loads the book module, and neither can rely on the
other having bound anything.

Nothing looks a string up in the domain yet — no string anywhere in this
repository is marked for translation. That is deliberate and is what the
standing directive asks for in this order: the binding has to be in place before
the first marked string, or it would be marked and silently untranslated.

`cargo fmt --check`, `cargo test --locked` (491 on the default members,
unchanged) and `cargo clippy --all-targets --locked -- -D warnings` are clean, as
are clippy and test over the EDS crates — 885 tests, was 881, the four new ones
being one `textdomain` per crate. The one ignored test is the pre-existing
`ignore` doctest in `jmap-backend-core`'s `instance::Slot`. `cmake -S . -B
build-verify -G Ninja && cmake --build build-verify && ctest` is 7/7, including
`rust-test-eds` and the five staged-install checks.
`RUSTDOCFLAGS=-D warnings cargo doc --no-deps` is clean for `jmap-config` and
`jmap-backend-collection`. Not verified locally, as in every session: `reuse
lint` and `cargo deny` (neither binary is on this VM); every new file carries an
SPDX `GPL-3.0-or-later` header. Pre-existing and untouched: `example-module` does
not build on this VM, and `jmap-mail`, `jmap-backend-book` and `jmap-backend-cal`
carry `rustdoc::private_intra_doc_links`.

No milestone tag. The translatable-strings directive is now half carried out:
all five module entry points bind the domain, and no catalogue exists for them
to find.

Next, in the order they would be taken: (1) `po/` with `POTFILES.in` and
`LINGUAS`, the lint the standing directive asks for, and an install rule putting
a compiled `.mo` under `LANGUAGE_SUPPORT_DIRECTORY` — until that exists every
lookup falls back to English by construction, so the bindings just landed are
untested against a real catalogue end to end (`jmap-backend-core`'s
`tests/catalogue.rs` proves the mechanism against one it writes itself, which is
as far as that can go without an installed `.mo`). While there: the top-level
`GETTEXT_PACKAGE` is still the skeleton's `example-module` while our domain is
`evolution-jmap` — the C example module and the Rust modules do not share a
catalogue and probably should not, but that should be a decision rather than an
oversight. (2) The first actually-marked strings, which by the directive means
the collection backend's child-source display names, since those are the
user-visible strings this repository already originates. Unchanged from previous
sessions: M7 still **needs human verification in real Evolution**
(`insert_widgets` remains unwritten, so an account arrives on the server settings
page filled in and cannot be corrected there), the four manual-test recipes are
unlinked from the README, and `jmap-mail`'s rustdoc is dirty.

## 2026-08-10 (hundred-and-twenty-first session)

**`po/`, the first two marked strings, and the check that keeps the list of
them honest.** The previous session ranked this first, and most of it landed;
the part that did not is blocked on something outside this repository, recorded
below rather than worked around.

**The strings were already there and already user-visible.** The previous
session's "next" note guessed at the collection backend's child-source display
names. Reading them says otherwise: `Child::display_name` is
`resource.name.clone()` — the name the *server* gave the address book or the
calendar. Translating server data would be a bug, not a feature. The strings
this repository actually originates and a user actually reads are the
`CamelProvider`'s `name` and `description`: `"JMAP"` and `"For reading and
storing mail on JMAP servers."`, which is why the catalogue test written two
sessions ago uses the latter as its msgid. They are already routed through our
domain — `translation_domain: DOMAIN` — so Camel has been calling `dgettext` on
them all along, against a catalogue that does not exist.

**Red first, twice, and the second red is the interesting one.**
`jmap-backend-core/tests/potfiles.rs` failed first because `po/POTFILES.in` did
not exist. Adding `po/POTFILES.in` listing `jmap-mail/src/provider.rs` made the
first test pass vacuously and the second one fail:

    po/POTFILES.in lists these, and none of them marks a string any more —
    either the strings moved and their new home is unlisted, or the entry
    should go: ["rust/crates/jmap-mail/src/provider.rs"]

Marking the two strings turned it green. The two directions are separate tests
because they catch opposite mistakes and both are silent in the build: a marked
string in an unlisted file never reaches a translator, and a listed file that
has stopped marking anything goes on working while its strings quietly leave
the catalogue.

**`N_`, in `jmap_backend_core::i18n`.** A `const fn` returning its argument,
`#[allow(non_snake_case)]` for the spelling every gettext-using project and
every extractor's defaults already know. `N_` rather than `translate` because
the lookup is not ours to make: Camel translates those two strings each time it
displays them, and doing it here would freeze them into whatever locale was
current when the module happened to be dlopened. It also makes them constants,
which `translate` cannot be — it returns an owned `String`.

**The lint matches text, on purpose.** `N_(c"` and `translate(c"`, on lines that
do not start with `//`. That is what `xgettext` does, so the check agrees with
the tool by construction — including where the tool is the crude one. The `c"`
is part of the pattern because a marker whose argument is not a literal
(`translate(NAME)`) contributes nothing to extract; the literal is wherever
`NAME` was written. Comments are stripped so that documentation may spell a
marker out — `potfiles.rs`'s own module docs do — without dragging the file
into the list.

**Verified against the real extractor, not just asserted.** gettext is not on
this VM; it was installed locally (`apt-get install gettext`, 0.21) purely to
check the design, and the command in `po/POTFILES.in`'s header produced exactly
the two msgids, with both `TRANSLATORS:` comments and correct line numbers.

It also produced one warning, which is a finding rather than noise:

    rust/crates/jmap-mail/src/provider.rs:125: warning: unterminated character constant

`-L C` has no Rust parser (0.21 has none at all) and lexes the lifetime in
`&'static CamelProvider` as an opening character constant. Harmless where it
stands — the lexer resumes at the next `'` and both strings still came out —
but not something to leave unwatched: a marked string sitting between two
lifetimes could in principle be swallowed silently. Written into
`po/POTFILES.in` beside the command, for whoever wires extraction into the
build.

**What did not land, and why it is a blocker rather than a shortcut.** The
install rule putting a compiled `.mo` under `LANGUAGE_SUPPORT_DIRECTORY` needs
`msgfmt` at build time, and `xgettext` for the `.pot`. Neither is in the CI
image: `Containerfile.ci` installs no gettext package, and the image is
referenced by digest, so adding one means editing `Containerfile.ci` and
rebuilding through `.github/workflows/ci-image.yml` — which this session is
directed not to touch. A `find_program(... msgfmt)` that skips when absent
would have compiled here and done nothing in CI forever, which is the kind of
machinery that looks done and is not. So: `po/` is the source side only.
`po/LINGUAS` is deliberately empty and says so — there are no translations yet,
which is not the same as translation being unsupported.

**The `GETTEXT_PACKAGE` question, decided rather than left open.** The
top-level `CMakeLists.txt` still sets it to the skeleton's `example-module`
while our domain is `evolution-jmap`, and `src/*.c` marks ten strings with
`N_()` against it. Those stay out of `po/POTFILES.in`: they are upstream
demonstration text ("My Maildir Folder Action…") that leaves with the skeleton,
and handing translators strings for a module nobody ships is worse than leaving
them untranslated. Written into the file's header so the exclusion is a
decision on the record. The lint therefore scans `rust/crates/*/src` only.

`cargo fmt --check`, `cargo test --locked` (491 on the default members,
unchanged — the new tests are in an EDS-gated crate) and `cargo clippy
--all-targets --locked -- -D warnings` are clean, as are clippy and test over
the EDS crates and the four `*-module` cdylibs — 887 tests, was 885, the two new
ones being the two directions of the `POTFILES.in` check. The one ignored test
is the pre-existing `ignore` doctest in `jmap-backend-core`'s `instance::Slot`.
`cmake -S . -B build-verify -G Ninja && cmake --build build-verify && ctest` is
7/7. `RUSTDOCFLAGS=-D warnings cargo doc --no-deps` is clean for
`jmap-backend-core`; `jmap-mail` still carries its 25 pre-existing
`rustdoc::private_intra_doc_links` and none of them is in `provider.rs`. Not
verified locally, as in every session: `reuse lint` and `cargo deny` (neither
binary is on this VM); every new file carries an SPDX `GPL-3.0-or-later` header,
`po/POTFILES.in` and `po/LINGUAS` as `#` comments, which is why neither needed a
`REUSE.toml` entry. Pre-existing and untouched: `example-module` does not build
on this VM.

No milestone tag. The translatable-strings directive is now carried out except
for the catalogue's binary half: markers exist, the first strings are marked,
`POTFILES.in` and `LINGUAS` exist, and a check in both directions keeps the list
in step with the sources. Nothing compiles a `.mo` and nothing installs one.

Next, in the order they would be taken: (1) get gettext into the CI image — the
one blocker above — and then the `.pot` target and the `msgfmt` install rule,
with a staged-install ctest asserting the catalogue lands at
`<LANGUAGE_SUPPORT_DIRECTORY>/<lang>/LC_MESSAGES/evolution-jmap.mo`, which is
exactly the path `i18n::LOCALE_DIR` plus gettext's layout, and which would make
`tests/catalogue.rs`'s hand-built proof an end-to-end one. This is a maintainer
decision, not an autonomous one: it needs `Containerfile.ci` and a CI image
rebuild. (2) While there: decide whether the top-level `GETTEXT_PACKAGE` should
stop saying `example-module`, which today is only read by the C skeleton.
(3) M7's account-setup labels, which are the next strings a user reads and the
first that will use `translate` rather than `N_` — they must be marked as they
are written, which is what the directive asks and what the lint now enforces.
Unchanged from previous sessions: M7 still **needs human verification in real
Evolution** (`insert_widgets` remains unwritten, so an account arrives on the
server settings page filled in and cannot be corrected there), the four manual-
test recipes are unlinked from the README, and `jmap-mail`'s rustdoc is dirty.

## 2026-08-10 (hundred-and-twenty-second session)

**M8: the `.deb`, and the two things building one turned up.** The previous
session's ranked "next" list began with getting gettext into the CI image,
which needs `Containerfile.ci` and a CI image rebuild and is explicitly a
maintainer decision, not an autonomous one; the second item is the same kind of
decision about `GETTEXT_PACKAGE`. So this session took the next milestone
instead. M8 is unusually well suited to this machine: `cpack` and `dpkg-deb`
are both here, so a package is not something to be asserted about, it is
something to be built and then taken apart.

**The test is an equality over the file list, not a subset.** `ctest -R
package-deb` runs `cpack -G DEB` into a scratch directory, then requires that
the set of regular files in the `.deb` is *exactly* the six the install rules
install — the five modules and Camel's `.urls`. A subset check ("are the
modules in there?") would have passed the first package this repository could
have produced, and that package was wrong: `src/` installs the upstream C
example module into Evolution's module directory with no install `COMPONENT`
of its own, so the obvious monolithic `include(CPack)` ships a demonstration
module to every machine that installs JMAP support. It installs, it works, and
it carries something nobody asked for — the failure mode a subset check cannot
see. Mutation-checked rather than argued: flipping `CPACK_DEB_COMPONENT_INSTALL`
back to `OFF` fails the test naming
`/usr/lib/evolution/modules/libexample-module.so`.

The package is therefore one `.deb` built from five named components
(`ALL_COMPONENTS_IN_ONE`), which is what makes the exclusion a list someone
maintains rather than a directory someone hopes stays clean.

**`dpkg-shlibdeps` did not merely miss a dependency; it refused to run.**
`CPACK_DEBIAN_PACKAGE_SHLIBDEPS ON` is the whole point of packaging from the
install tree — these modules are *dlopened*, so a library missing at runtime is
not a link error anyone sees but a backend that never appears in the account
type list, and the true list is knowable only by reading our own ELF files.
The first attempt died: `cannot find library libevolution-mail.so.0 needed by
module-jmap-configuration.so`. Evolution keeps its own libraries in a private
directory (`privlibdir`, /usr/lib/evolution) off the loader's default path;
they resolve at runtime because the shell process has already loaded them, and
`dpkg-shlibdeps` is not that process. Fixed with
`CPACK_DEBIAN_PACKAGE_SHLIBDEPS_PRIVATE_DIRS` set from
`pkg_check_variable(... evolution-shell-3.0 privlibdir)` — asked of pkg-config
like every other directory in this build rather than written down. Worth noting
that this was a hard error, not a warning: with no private dir, no package is
produced at all.

What comes out is the interesting part, and it is the M10 ABI contract stated
by the package manager rather than by a document:

    Depends: libc6 (>= 2.34), libcamel-1.2-64t64 (>= 3.45.2),
      libebackend-1.2-11t64 (>= 3.38.0), libebook-contacts-1.2-4t64 (>= 3.16.2),
      libecal-2.0-3 (>= 3.17), libedata-book-1.2-27t64 (>= 3.25.90),
      libedata-cal-2.0-2t64 (>= 3.25.90), libedataserver-1.2-27t64 (>= 3.17),
      libevolution (>= 3.52.3), libevolution (<< 3.53), libgcc-s1 (>= 4.2),
      libglib2.0-0t64 (>= 2.36.0), libical3t64 (>= 3.0.0)

`libevolution (>= 3.52.3), libevolution (<< 3.53)` is apt refusing to install
this package next to an Evolution it was not built against — which is exactly
what M10 says the deployment contract is, and it arrives for free from having
derived the dependencies instead of writing them. Turning `SHLIBDEPS` off
fails the test (`package declares no Depends`), so it cannot be quietly lost.

**The `Description` check, which found a real defect in its own first draft.**
Debian policy §5.6.13 gives the field a shape: a synopsis line, then extended
lines each beginning with *exactly* one space, `.` alone for a paragraph break.
An extra space means "preformatted", and `apt show` then declines to wrap.
Writing `CPACK_DEBIAN_PACKAGE_DESCRIPTION` the way it should appear — synopsis
included, lines indented — produced both defects at once, because CPack adds
the synopsis and the indentation itself:

    Description: JMAP support for GNOME Evolution
     JMAP support for GNOME Evolution
      Backends that let Evolution and evolution-data-server speak JMAP
      …
     must be built against the same versions they are installed alongside.

Duplicated synopsis, double-indented body, and one line that lost its indent
and would have run into the previous paragraph. The test asserts all three
properties (opens with the summary, never repeats it, every continuation line
is one space then text); it went red on exactly that output, and the variable
now holds the extended description alone, unindented. This is a cosmetic bug in
the sense that nothing crashes and a user-facing one in the sense that the
Description is the one field a person reads before installing.

**Naming.** `CPACK_DEBIAN_FILE_NAME "DEB-DEFAULT"` gives
`evolution-jmap_0.0.1_amd64.deb` rather than CPack's own CMake-flavoured
spelling. The roadmap writes the goal as `apt install ./evolution-jmap.deb`;
the versioned, arch-qualified name is what every tool that reads a directory of
packages expects, and dropping the version off an artifact that pins an ABI
would be the wrong economy. Also new: `PACKAGE_NAME` in the top-level
`CMakeLists.txt` — this project installs as `evolution-jmap` while the CMake
project is still the skeleton's `example-modules`, and that difference now has
a name instead of being spelled out at each use.

`ctest` is 8/8 (was 7/7; `package-deb` is the new one), `cargo fmt --check`,
`cargo test --locked` (491, unchanged) and `cargo clippy --all-targets --locked
-- -D warnings` are clean, as are clippy and test over the EDS crates — no Rust
changed this session, the whole increment is CMake. Not verified locally, as in
every session: `reuse lint` and `cargo deny` (neither binary is on this VM); the
two new files carry SPDX `GPL-3.0-or-later` headers as `#` comments.
Pre-existing and untouched: `example-module` does not build on this VM, and
`jmap-mail`, `jmap-backend-book` and `jmap-backend-cal` carry
`rustdoc::private_intra_doc_links`.

No milestone tag. M8 has two halves and this is the first: the package exists,
is checked, and can be built by hand from a clean tree. The second half —
"wire into release.yml with attestation like the other artifacts" — is
untouched, and deliberately not attempted in the same session as the thing it
would publish: a release workflow cannot be run here, so it is the kind of
change that compiles and is not known to work, and it should land on its own
where its diff is the only thing under suspicion.

Next, in the order they would be taken: (1) release.yml — add the `.deb` to the
artifacts the release job builds, hashes into `SHA256SUMS` and attests, and
mention it in `docs/verifying-artifacts.md`; that completes M8 and is the point
at which the milestone could be tagged. Note the release runner must have
`dpkg-dev` for `dpkg-shlibdeps` — the CI image is not this VM, and if it lacks
it the packaging step fails loudly rather than silently emitting a
dependency-less package, which is the right failure but wants checking before
it is a release. (2) Reproducibility: the repo builds everything twice and
compares checksums, and nothing yet says a `.deb` built twice is byte-identical.
CPack's DEB generator does not obviously honour `SOURCE_DATE_EPOCH` for its ar
member timestamps; worth a test before the package is something people verify
against Rekor. (3) Still blocked on a maintainer decision, unchanged: gettext in
the CI image, and whether the top-level `GETTEXT_PACKAGE` should stop saying
`example-module`. Unchanged from previous sessions: M7 still **needs human
verification in real Evolution** (`insert_widgets` remains unwritten, so an
account arrives on the server settings page filled in and cannot be corrected
there), the four manual-test recipes are unlinked from the README, and
`jmap-mail`'s rustdoc is dirty.

## 2026-08-10 (hundred-and-twenty-third session)

**The `.deb` this repository produces was not reproducible, and now it is.**
The previous session ranked its own follow-ups: (1) wire the package into
`release.yml`, (2) find out whether a `.deb` built twice is byte-identical.
Taken in the other order deliberately. Item (1) cannot be run on this VM — a
release workflow that compiles is not a release workflow that works — whereas
item (2) is exactly the kind of question this machine can answer, and it is a
question that has to be answered *before* the package is published, not after:
the whole point of publishing a digest is that someone else can rebuild and get
the same bytes. Shipping an irreproducible artifact next to `SHA256SUMS` and
in-toto attestation would have been a promise the artifact could not keep.

It could not keep it. Packaging the same build tree twice, two seconds apart,
gave two different files:

    08f9771b3e375157bc4273188aa3ad0c40c9792ff864476250f57c07fd150266
    efe5c3ca4ae1f16e39475cdf2bf24f37c9951d773085032c2a2a0032965ae827

Taken apart with `ar x`, the difference was byte 5 of both `control.tar.gz` and
`data.tar.gz` — the gzip header's MTIME — and, underneath, byte 146 of the tar
streams, which is inside a tar header's `mtime` field. The entry it belonged to
was `./usr/`. `cpack` re-runs the install into a fresh staging tree, so every
directory in the archive is created at packaging time; the ar member headers
carry the same clock. The file entries had a second problem of their own: they
keep the mtimes of the modules as linked, so an unclamped package is dated by
`ld` rather than by anything anyone chose. Five distinct timestamps in one
package, none of them a property of the sources.

**The fix is `CPACK_PROJECT_CONFIG_FILE`, which is the only hook in the right
place.** CMake 3.28's DEB generator honours `SOURCE_DATE_EPOCH` and — worth
knowing, because it is not what the name suggests — does not *clamp* to it but
*sets* every entry to it: the Camel `.urls` file, whose mtime is older than the
last commit, comes out dated with the epoch and not with its own. So one
environment variable collapses all five timestamps to one. But it must be an
environment variable in the `cpack` process, and `CPackConfig.cmake` carries
only CPack variables. `CPACK_PROJECT_CONFIG_FILE` names a script CPack includes
inside that process, once per generator, before it stages anything; a generated
`build/cpack-project-config.cmake` doing `set(ENV{SOURCE_DATE_EPOCH} ...)` is
therefore enough, and it has to be set before `include(CPack)`, which is what
writes the path into the config.

The value is the one the build already resolved for the Rust side (exported
`SOURCE_DATE_EPOCH`, else `git log -1 --format=%ct`), so the package is dated by
the same instant as the binaries inside it. Deliberately *not* deferring to
whatever happens to be exported when `cpack` runs: that would let whoever
packages redate a build they did not make.

**The test is three runs, not two, and that is the part worth defending.**
`ctest -R package-deb-reproducible` packages the tree three times — once with
`SOURCE_DATE_EPOCH=1600000000` exported, once with `1700000000`, once with the
variable removed from the environment via `cmake -E env --unset=` — and
requires all three to be byte-identical. A plain run-it-twice test would have
gone green on any machine that exports nothing and then failed on the release
runner, which exports the commit timestamp; the decoy epochs are what turn "the
caller's environment does not reach the package" into an assertion. The red
output named both defects at once — three different digests, and the unset run
listing all five timestamps.

Byte-equality across three runs still would not prove the *file* entries are
pinned, since one build tree gives all three runs the same module mtimes. So
the second assertion is that every entry in the package — control tar and data
tar, via `dpkg-deb --ctrl-tarfile`/`--fsys-tarfile` piped to `tar --utc
--full-time -tv` — carries one and the same timestamp. Nothing about a real
tree makes that true by accident: the modules here are linked two seconds
apart, the `.urls` is two days older, the directories are made at packaging
time. Checked out of band that this is the property that matters: `touch` on
every module `.so` and on `libcameljmap.urls`, then repackage, gives
`d923b111…` both before and after. That is the fresh-clone case, where git sets
every mtime to the moment of checkout.

`ctest` is 9/9 (was 8/8), `cargo fmt --check`, `cargo test --locked` (491,
unchanged) and `cargo clippy --all-targets --locked -- -D warnings` clean — no
Rust changed, the increment is one CMake template, one CMake test script, and
two hunks of `cmake/Packaging.cmake`. Not verified locally, as in every
session: `reuse lint` and `cargo deny` (neither binary is on this VM); both new
files carry SPDX `GPL-3.0-or-later` headers as `#` comments, and neither needs
a `REUSE.toml` entry.

No milestone tag; M8's second half is still the one thing left. Next, in order:
(1) `release.yml` — add the `.deb` to what the release job builds, hashes into
`SHA256SUMS` and attests, and describe it in `docs/verifying-artifacts.md`;
that is the point at which M8 could be tagged, and it is now safe to publish
because the artifact is reproducible. The runner needs `dpkg-dev` for
`dpkg-shlibdeps`, which fails loudly rather than silently if absent — worth
confirming before a release rather than during one. (2) Consider whether the
`reproducible` CI job, which today builds twice and compares the `.so`s, should
compare the `.deb` too; `ctest` now proves it within one tree, and that job
would prove it across two. (3) Unchanged maintainer decisions: gettext in the
CI image, and whether `GETTEXT_PACKAGE` should stop saying `example-module`.
Unchanged: M7 still **needs human verification in real Evolution**, the four
manual-test recipes are unlinked from the README, and `jmap-mail`'s rustdoc is
dirty.

## 2026-08-10 (hundred-and-twenty-fourth session)

**The `.deb` is now in the release, and the release workflow is checked as a
document because it cannot be run.** This was the previous session's ranked
item (1), and the reason it was ranked below reproducibility was that it cannot
be executed here: a release workflow runs when a tag is pushed and at no other
time. That is an argument for care, not for deferral — the workflow was already
unrunnable when it was written, and it will be unrunnable again on every future
edit. So the increment is the wiring *plus* a `ctest` case that asserts the
properties of the file which only a pushed tag would otherwise exercise.

**Two jobs, because the artifacts need two environments.** `jmap-mockd` and the
source tarball need Rust and git; the `.deb` needs the Evolution/EDS headers the
modules link against and `dpkg-shlibdeps` to read the result. So `package`
builds in the digest-pinned CI image — the same image and the same
`safe.directory` dance as CI's `build-full`, since without it git will not
report the commit timestamp that seeds `SOURCE_DATE_EPOCH` and the package would
be dated from the packaging clock instead of from the tag. Deliberately not
`apt-get install`-ing the headers on the stock runner: `dpkg-shlibdeps` derives
the package's `Depends` by reading our own modules against the libraries
actually present, so the `.deb` describes the distribution it was built in, and
that description is only worth something if it is the distribution CI tests in.
The confirmation the previous session asked for — that the environment has
`dpkg-dev`, whose absence makes `dpkg-shlibdeps` fail loudly — is that
`Containerfile.ci` installs `build-essential`, which depends on it; the packaging
`ctest`s already pass in that image.

**The one real bug this could have shipped is an unattested artifact.** The old
workflow listed its attestation subjects by hand (`dist/jmap-mockd`,
`dist/evolution-jmap-*.tar.xz`, `dist/SHA256SUMS`) and published `dist/*`. Two
lists that happened to agree. Add a fourth artifact to `dist/` and the release
gains a file with no provenance — which is invisible in the worst way, because
an unattested `.deb` downloads exactly like its signed neighbours and
`gh attestation verify` on it fails with "no attestation found", reading like a
tooling problem rather than a gap. The fix is not to remember: both sides are
now the same glob, `dist/*`, so the attested set and the published set are equal
by construction.

**`cmake/tests/check-release-workflow.cmake` (test 10 of 10) asserts three
things**, each an ordinary edit away from being false:

1. The `subject-path:` entries and the `dist/`-tokens on `gh release create`
   are the same set, compared as written — two globs that happen to cover the
   same files today are exactly the drift being caught.
2. The workflow builds a package (`cpack` or `--target package`) *and* copies a
   `.deb` into `dist/`. A release that quietly stops carrying the package is the
   regression nothing else would notice, `apt install ./evolution-jmap.deb`
   being what M8 exists to deliver.
3. Every CI config that pins the shared image pins the same digest, and
   `release.yml` is one of them. `ci-image.yml` is skipped: it *builds* the
   image, so a digest there is an output, not a pin.

Red first, and red for the right reason: the first run failed on assertion 1
with the two lists printed side by side. Then each assertion was checked by
mutation against a scratch copy of the CI configs — remove `--target package`,
drop the `cp deb/*.deb dist/`, skew the digest, unpin the image, delete the
`subject-path` — and all five mutants fail with their own message. Without that
the test would have been three tautologies over a file I had just written to
satisfy them.

Verified locally: `ctest` 10/10, `cmake --build build --target package` puts
`evolution-jmap_0.0.1_amd64.deb` exactly where the workflow's
`path: build/*.deb` looks for it, the workflow parses as YAML with the two
expected jobs, `cargo fmt --check`, `cargo test --locked` (491, unchanged — no
Rust changed) and `cargo clippy --all-targets --locked -- -D warnings` clean.
`reuse lint` and `cargo deny` not run (neither binary is on this VM); the one
new file carries an SPDX `GPL-3.0-or-later` header as `#` comments and needs no
`REUSE.toml` entry, like the other `cmake/tests/` scripts.

**Not verified, and this is the honest limit: the workflow has never run.**
Compiling is not working and parsing is not running. What a document check
cannot see: whether the container job can pull the private ghcr image with the
job token (`ci.yml`'s `build-full` does exactly this, so the pattern is proven
in this repo, and `packages: read` was added to `release.yml`'s permissions);
whether `actions/upload-artifact` and `download-artifact` behave in that
container; whether the release runner's `cp deb/*.deb dist/` finds one file.
**So no `M8 COMPLETE` tag** — and `docs/MILESTONES.md` still does not exist, so
no tag has ever been written. The human step that would settle it is one test
tag (`git tag v0.0.1-rc1 && git push --tags`), then checking the release has
four files, that `gh attestation verify` passes on each, and that the `.deb`
installs; that is a five-minute check and it is the only thing standing between
here and a defensible M8.

Next, in order: (1) that test tag, by hand — everything else in M8 is done.
(2) The `reproducible` CI job could build the `.deb` in two checkouts and
compare; `ctest` proves it within one tree, that job would prove it across two,
and it is the one reproducibility claim in `docs/verifying-artifacts.md` no
machine checks. (3) M9 Layer 1 — the headless EDS functional tests — is the
next milestone with nothing blocking it, and this VM has the EDS runtime to do
it. (4) Unchanged maintainer decisions: gettext in the CI image, and whether
`GETTEXT_PACKAGE` should stop saying `example-module`. Unchanged: M7 still
**needs human verification in real Evolution**, the four manual-test recipes are
unlinked from the README, and `jmap-mail`'s rustdoc is dirty.

## 2026-08-10 (hundred-and-twenty-fifth session)

**M9 layer 1 starts, and the first thing it did was find a bug that made the
address book read-only.** The previous session ranked this third, behind a
test tag only a human can push and a CI job that compares two `.deb`s; it is
first here because it is the one item on that list this VM can actually
execute, and because the roadmap is explicit that layer 1 is the priority of
M9. The increment is the harness, one test through it, and the defect it
found.

**The defect: `e_book_meta_backend_set_connected_writable` is not the call.**
`connect_sync` ended with it, with a comment saying it is "how
`EBookMetaBackend` decides whether a connected backend accepts writes". It
reads like the right call and it is not. The moment the vfunc returns TRUE,
EDS runs `ebmb_update_connection_values`, whose last line is

    e_book_meta_backend_set_connected_writable (meta_backend,
        e_book_backend_get_writable (E_BOOK_BACKEND (meta_backend)));

— it *overwrites* connected-writable with the backend's writable flag, which
nothing had set, so FALSE. Our setter was undone by the very call that was
about to read it. The vfunc's own documentation says so plainly ("The
descendant should also call `e_book_backend_set_writable()` after successful
connect"); the fix is that one call, and it also sets the flag EDS persists
for opening the book offline. Read against
`evolution-data-server-3.52.3/src/addressbook/libedata-book/e-book-meta-backend.c`
lines 303–346 and 3560–3590, not guessed.

The user-visible shape of this: an address book that connects, syncs, and is
greyed out in Evolution, where every write comes back as "Cannot add contact:
Permission denied". Nothing in 491 unit tests could see it — they call the
vfunc bodies, and the vfunc body was doing something. Only EDS's opinion of
what it did was wrong, and until tonight nothing asked EDS for its opinion.
**`jmap-backend-cal` has the identical bug** (`e-cal-meta-backend.c:358` is
line-for-line the same), left alone deliberately: fixing it without the
calendar half of the harness would be an unverified fix, and that is the next
session's first item, red test included.

**The harness.** `rust/crates/jmap-functional` builds a throwaway EDS
installation per test — scratch XDG directories, a `.source` keyfile carrying
the mock's ephemeral port, and a module directory holding the one backend
under test named by `EDS_ADDRESS_BOOK_MODULES` — then runs a client program
on a private bus from `dbus-run-session`. Private because the alternative is
reaching the developer's own already-running factory, started with the wrong
environment; scratch cache because `EBookMetaBackend` connects during the open
only when it has never connected before, so a reused cache would race the
connect against a background refresh. Nothing is installed, nothing needs
`sudo`, and every daemon dies with the bus.

The client is C (`tests/functional/book-client.c`). That surface — libebook,
the *client* API — is one no crate here binds, `eds-sys` being what the
backends implement; binding a second FFI surface only to call it from a test
would put a layer of our own between EDS and the thing under test. It prints
`key=value` and holds no opinion; every judgement is in
`tests/address-book.rs`, which checks both ends: what EDS handed the client,
and what the mock was asked for.

**Gated behind `-DENABLE_FUNCTIONAL_TESTS=ON`, and loud when on.** The CI
image has the EDS headers and neither daemon. A test registered
unconditionally would fail every CI run — or, the tempting fix and the worse
one, be written to skip itself when the runtime is missing and report green
on a machine where it never ran. With the option off the tests do not exist;
with it on, a missing `evolution-source-registry` or `dbus-run-session` is a
configure error. There is no arrangement in which they quietly pass. The cost
is stated in `docs/functional-tests.md`: **CI does not run these today**, so
a regression in this layer stays green until someone runs them here. Closing
that needs `evolution-data-server` and `dbus-daemon` in `Containerfile.ci`
and a `workflow_dispatch` job — a maintainer decision, since it grows the
image every job pulls. (This VM did not have the runtime either; `apt install
evolution-data-server` was the only environment change made.)

**Red first, and three mutations to prove the green means something.**
The first run failed on `readonly=1` with the client's whole output in the
message. Then: revert the fix → red on `readonly`, 0.22 s; stage the calendar
module instead of the book's → red on "the client failed before it opened the
book", with EDS's "Backend factory ... cannot be found" in the report; seed
the mock's address book as non-default so the backend cannot find the
account's default → red again. Three mutations, three distinct messages, so
the assertion is not a tautology over a fixture the test also wrote.

**An open question, deliberately not chased tonight.** `ESource:connection-
status` never reaches `CONNECTED` on the client side — it stops at
`CONNECTING` — so `e_book_client_connect_sync` with a wait burns the entire
timeout (30 s, measured) before returning a client that then works fine. EDS
sets the status itself in `ebmb_ensure_connected_sync` right after our vfunc
returns, so the backend is not obviously at fault, and I could not explain the
propagation failure inside the time this increment had. It is not
hypothetical: Evolution opens books with a wait, so if this is ours it is a
30-second stall on opening a JMAP address book. The test sidesteps it with
EDS's documented "do not wait" value and one explicit
`e_client_retrieve_properties_sync`, which is better than waiting anyway —
reading `e_client_is_readonly` off the client's cache in a program with no
main loop was a race that could have hidden the very bug this was written for.
**Next session should chase this before adding the calendar test.**

Verified locally: `ctest` 11/11 with the option on and 10/10 with it off (CI's
path — the default is unchanged and the new test does not exist there),
`cargo fmt --check`, `cargo test --locked` (491, unchanged — `jmap-functional`
is out of `default-members`, like the header-needing crates but for a
different reason: it needs the EDS *runtime* and paths only CTest knows),
`cargo clippy --all-targets --locked -- -D warnings` clean, and the same for
`-p jmap-functional -p jmap-backend-book`. `reuse lint` and `cargo deny` not
run (neither binary is on this VM); all five new files carry SPDX
`GPL-3.0-or-later` headers and `docs/functional-tests.md` is covered by
`REUSE.toml`'s `docs/**` annotation.

No milestone tag. M9 layer 1 is one backend of three and has no CI job; M8 is
still one human test tag away.

Next, in order: (1) the `connection-status` question above. (2) The calendar's
identical writable bug, with the calendar half of the harness as its red test.
(3) The mail provider through Camel, which is layer 1's third leg and a
different host process again. (4) The M8 test tag, by hand. (5) Unchanged
maintainer decisions: `evolution-data-server` + `dbus-daemon` in the CI image
(now with a concrete reason), gettext in the CI image, and whether
`GETTEXT_PACKAGE` should stop saying `example-module`. Unchanged: M7 still
**needs human verification in real Evolution**, the manual-test recipes are
unlinked from the README, and `jmap-mail`'s rustdoc is dirty.

## 2026-08-10

The calendar half of the functional harness, and the bug it was written to
find. `jmap-backend-cal` had the address book's read-only bug, line for line,
exactly where last session said it would be — and now there is a test that
says so before a user does.

**The fix is one call, and it is not the obvious one.** `connect_sync` was
calling `e_cal_meta_backend_set_connected_writable`, which reads like the
setter for "this connected backend accepts writes" and is not: the moment the
vfunc returns TRUE, `ecmb_update_connection_values` overwrites
connected-writable with `e_cal_backend_get_writable()`, which nothing had set
— so FALSE, and our setter is undone by the very call that was about to read
it. `e_cal_backend_set_writable` sets both, and it is what the vfunc's own
documentation asks for ("The descendant should also call
e_cal_backend_set_writable() after successful connect"). Read against
`evolution-data-server-3.52.3/src/calendar/libedata-cal/e-cal-meta-backend.c`
lines 358 and 1372–1374 and 4934, not guessed — the tarball is not on this VM
by default, `download.gnome.org` has it.

**Red first, and two more mutations.** The first run failed on `readonly=1`
with `create: Cannot create calendar object: Permission denied` in the
client's stderr — the user-visible shape of this bug, a calendar Evolution
greys out. Then: stage the *book* module as the calendar backend → red with
EDS's "Backend factory for source ... and extension ?Calendar? cannot be
found"; seed the mock's calendar as non-default → red on `readonly` again.

That third mutation is worth writing down rather than glossing: it does **not**
produce a distinct message, because `e_cal_client_connect_sync` succeeds even
when the backend's `connect_sync` failed — `ECalMetaBackend` opens the
calendar and schedules the connect — so a calendar the backend could not open
reaches the client looking exactly like one it opened and forgot to claim
writable. The `readonly` assertion is therefore a broad net over both, which
is fine (both are bugs) but means a future failure there needs the stderr in
the report to tell which. The test comment says so.

**The harness grew a second surface, not a second harness.** `Session` now has
`stage_calendar_backend` beside `stage_address_book_backend`, both over one
private `stage_backend`; the two module directories differ only in the
variable EDS reads them from (`EDS_CALENDAR_MODULES`, from
`e-data-cal-factory.h`) and the name the factory expects. The client is C
again — `tests/functional/cal-client.c`, a plain libecal consumer — and builds
its VEVENT from text through `i_cal_component_new_from_string` rather than a
chain of libical-glib setters: the component sent is then exactly the one
written in the file. `DTSTART` is a UTC instant so nothing depends on a
timezone database being reachable from the scratch session.

CTest registers the two as separate tests (`cargo test --test address-book`
and `--test calendar`) rather than one run of the crate, so a failure names
the surface and each gets only the paths it needs. They share a cargo target
directory, so cargo's own lock serialises them however CTest schedules them.

**An intermittent hang, seen once, not chased.** The first full `ctest` run of
the night wedged for ten minutes in `jmap-mail`'s `tests/transport.rs`: four
threads, all in `futex_do_wait`, zero CPU. Re-run standalone it passed in
0.03 s, and the whole suite passed on the next full run. It is unrelated to
tonight's change (nothing here touches `jmap-mail`) and smells like GType
registration racing across concurrently-run tests in that binary. Recorded so
the next person who sees it knows it is not new; if it recurs, `RUST_TEST_THREADS=1`
on that binary is the first thing to try, and the real fix is finding which
two tests register into the type system at once.

Verified locally: `ctest` 12/12 with `-DENABLE_FUNCTIONAL_TESTS=ON`, a
configure with it off registering zero functional tests (CI's path, unchanged),
`cargo fmt --check`, `cargo test --locked` (491, unchanged), `cargo clippy
--all-targets --locked -- -D warnings` clean and the same for `-p
jmap-backend-cal -p jmap-functional`. `reuse lint` and `cargo deny` not run
(neither binary is on this VM); both new files carry SPDX `GPL-3.0-or-later`
headers.

No milestone tag. M9 layer 1 is now two backends of three — the mail provider
through Camel is the third and a different host process again — and still has
no CI job.

**Not done, and deliberately.** Last session's first item was the
`ESource:connection-status` question (it never reaches `CONNECTED`, so a
client that waits burns the full 30 s). It is untouched: it is open-ended
reading with no obvious end, and the calendar bug was a known, user-visible
defect with a known fix. Correctness over progress cuts that way — but the
question is still first in the queue, and now the EDS source to answer it from
is one `curl` away.

Next, in order: (1) the `connection-status` question. (2) The mail provider
through Camel, layer 1's third leg. (3) The M8 test tag, by hand. (4)
Unchanged maintainer decisions: `evolution-data-server` + `dbus-daemon` in the
CI image, gettext in the CI image, and whether `GETTEXT_PACKAGE` should stop
saying `example-module`. Unchanged: M7 still **needs human verification in
real Evolution**, the manual-test recipes are unlinked from the README, and
`jmap-mail`'s rustdoc is dirty.

## 2026-08-10 (hundred-and-twenty-seventh session)

The `connection-status` question, answered, and the answer turned into an
assertion. It had been first in the queue for two sessions and skipped twice as
"open-ended reading"; it took about an hour of reading
`evolution-data-server-3.52.3` and it is not open-ended at all.

**The finding: the 30-second stall is the test program's fault, not the
backend's.** `ESource` does not apply a connection-status change where it
learns of it — `source_notify_dbus_connection_status_cb` queues an *idle* on
the source's `GMainContext` (`e-source.c:899`), and that context is whatever
was thread-default when `ESourceRegistry` was constructed
(`e-source-registry.c:1726`, and `:683` where each `ESource` is handed it) — in
a synchronous program with no main loop, the default context on the main
thread. `e_client_wait_for_connected_sync` then blocks *that thread* on an
`EFlag` until `notify::connection-status` fires (`e-client.c:1732`). The signal
comes from the idle; the idle needs the context iterated; the only thread that
would iterate it is the one blocked on the flag. The wait therefore always
expires, whatever the backend did. Evolution never meets this because it has a
main loop and does the wait on a worker thread.

So there was no backend bug to fix, and — worth stating, because it was the
obvious guess — no missing `e_backend_ensure_source_status_connected` either:
`e_book_meta_backend_ensure_connected_sync` sets the status itself
(`e-book-meta-backend.c:3576-3579`), `connecting` before the vfunc and
`connected`/`disconnected` after it. That call is for backends which connect
outside the meta-backend machinery (LDAP and weather use it; nothing else in
3.52 does).

**What landed instead of a fix: the observation the question was really
about.** Both functional clients now report `connection-status=<nick>`, waited
for with an actual main loop, from a new shared `tests/functional/
connection-status.c` compiled into both — one file because the question is the
same for a book and a calendar, and because the reasoning above needed one
place to live. The two Rust tests assert `connected`.

This is worth more than a tidied comment. It is EDS's own verdict on our
connect, and for the *calendar* it is the sharp signal the last session said
was missing: `e_cal_client_connect_sync` succeeds even when the backend's
`connect_sync` failed, so `readonly` could not tell "could not open the
calendar" from "opened it and forgot to claim it writable". `connection-status`
can, so it is asserted before `readonly` in both tests — cause before symptom.

**Red first, then two mutations.** Red: both tests failed on `left: None`, the
key not being printed yet. Green in 0.24 s per test — the status was already
`connected` before the first iteration, which is itself the point: the value
was always there, only nobody was iterating the context that delivers it. Then
(1) replace the `g_main_loop_run` with a busy poll of the same condition →
both tests fail after the full 10 s with `connection-status=disconnected`,
which is the deadlock above reproduced on demand, and proof the new assertion
has teeth. (2) Seed the mock's calendar as non-default so the backend's
connect fails → `connection-status=disconnected` while `readonly=1`, the two
distinguished exactly as claimed.

Verified locally: `ctest` 12/12 with `-DENABLE_FUNCTIONAL_TESTS=ON`, a
configure with it off registering 10 tests and no functional ones (CI's path,
unchanged), `cargo fmt --check`, `cargo test --locked` (491, unchanged),
`cargo clippy --all-targets --locked -- -D warnings` clean, and
`connection-status.c` compiled standalone under `-Wall -Wextra` clean since
the project does not pass those. `reuse lint` and `cargo deny` not run (neither
binary is on this VM); both new files carry SPDX `GPL-3.0-or-later` headers and
`docs/functional-tests.md` is covered by `REUSE.toml`'s `docs/**` annotation.

No milestone tag. M9 layer 1 is still two backends of three and still has no CI
job; nothing about tonight changes either.

Next, in order: (1) the mail provider through Camel, layer 1's third leg — now
unambiguously first, with nothing ahead of it. (2) The M8 test tag, by hand.
(3) Unchanged maintainer decisions: `evolution-data-server` + `dbus-daemon` in
the CI image, gettext in the CI image, and whether `GETTEXT_PACKAGE` should
stop saying `example-module`. Unchanged: M7 still **needs human verification in
real Evolution**, the manual-test recipes are unlinked from the README,
`jmap-mail`'s rustdoc is dirty, and the once-seen `jmap-mail`
`tests/transport.rs` hang from the last session is still unexplained.

## 2026-08-10 (hundred-and-twenty-eighth session)

M9 layer 1's third leg: the Camel mail provider, driven the way Camel actually
finds one. `tests/functional/mail-client.c` and
`rust/crates/jmap-functional/tests/mail.rs`, registered as `functional-mail`.

**Why it is not a third mirror of the other two.** The book and calendar legs
are a client talking to a factory daemon that hosts the module; the daemon
finds the module by it being a file in a directory it scans. A Camel provider
has no daemon. It is dlopened into the *mail client's own process*, and only
when something asks for a protocol that a `.urls` file in Camel's provider
directory claims. So this client program is not a consumer of the host — it is
the host. Nothing links the provider in (`jmap-mail`'s own tests do, which is
exactly why they cannot check this): the harness stages `libjmap_mail.so` as
`libcameljmap.so` into a scratch `EDS_CAMEL_PROVIDER_DIR` with the *installed*
`.urls` file beside it, and `camel_session_add_service` is where the dlopen
either happens or does not.

Reaching `store-connected=1` therefore already proves the three spellings
agree that live in three files — `BackendName=jmap` in the keyfile, the one
line in `libcameljmap.urls`, and the string `camel_provider_module_init`
registers. The protocol is read off the `ESource` rather than spelled in the
client, so a client that hardcoded it would only be agreeing with itself.

Beyond that: the folder tree is the mock's three mailboxes from one
`Mailbox/get`; the inbox is the mailbox with the JMAP `inbox` role, asked for
by role through `camel_store_get_inbox_folder_sync`; the summaries are the two
seeded messages; and *every* body downloads, which is a different request
again — a blob download is a plain HTTP GET, not a method call.

**Red first, then the mutation that matters.** Red: `JMAP_FUNCTIONAL_MAIL_*`
unset, then the test's first real run failed on `message-subject` — it fetched
`uids->pdata[0]` and got the second seeded message, because a folder's uid
order is the provider's business and not the test's. That is a finding about
the test, not the provider, and the fix is the shape the whole client now has:
every list it prints is sorted, and it fetches *all* the messages rather than
one by position. A test that compared an order nobody promised would have been
a flake with a plausible-looking failure message.

The mutation with teeth: stage the module without its `.urls` file. The client
fails at `add-service` with *No provider available for protocol 'jmap'* — the
exact failure `docs/manual-test-mail-provider.md` warns about, arriving at
exactly the call the document says it arrives at, and proof the assertion is
about Camel's loading and not about a provider that happened to be linked in.

**A CamelSession is instantiated directly rather than subclassed**, and the
base class's `authenticate_sync` warns on stderr that it "is not intended for
production use". That warning is expected output: the provider asks its session
to authenticate from `connect_sync`, as every Camel provider does, and with no
`User=` in the keyfile there is nothing to resolve. Evolution's subclass
(EMailSession) exists to answer the vfuncs that need a user, and a subclass
here would be a second implementation of an interface nothing in this test
calls. Documented in the client rather than silenced.

Sending is deliberately not covered. A transport is a second `CamelService`
configured from a second source, and the thing worth testing about it — that it
kept a server of its own, the failure the manual recipe calls the quietest one
here — is its own leg.

Verified locally: `ctest` 13/13 with `-DENABLE_FUNCTIONAL_TESTS=ON`,
`cargo fmt --check`, `cargo test --locked` (491, unchanged — `jmap-functional`
is out of `default-members`), `cargo clippy --all-targets --locked -- -D
warnings` clean and the same for `-p jmap-functional -p jmap-mail`, and
`mail-client.c` compiled standalone under `-Wall -Wextra` clean. `reuse lint`
and `cargo deny` not run (neither binary is on this VM); the new files carry
SPDX `GPL-3.0-or-later` headers.

No milestone tag. M9 layer 1 is now all three surfaces, which is the last of
what the roadmap asks of it *except* the gated CI job — and that is still a
maintainer decision (`evolution-data-server` + `dbus-daemon` in the CI image),
not something this session could land. Tier 2, the GUI smoke test, is untouched
and needs a display this VM does not have.

Next, in order: (1) the mail *transport* through Camel — the send half, and the
one place a source that lost its `[Authentication]` group shows up. (2) The M8
test tag, by hand. (3) Unchanged maintainer decisions: `evolution-data-server`
+ `dbus-daemon` in the CI image, gettext in the CI image, and whether
`GETTEXT_PACKAGE` should stop saying `example-module`. Unchanged: M7 still
**needs human verification in real Evolution**, the manual-test recipes are
unlinked from the README, `jmap-mail`'s rustdoc is dirty, and the once-seen
`jmap-mail` `tests/transport.rs` hang is still unexplained.

## 2026-08-10 (hundred-and-twenty-ninth session)

M9 layer 1's send half: the Camel *transport*, reached from the account the way
Evolution reaches it. `tests/functional/transport-client.c` and
`rust/crates/jmap-functional/tests/transport.rs`, registered as
`functional-transport`.

**Why it is a leg and not three more assertions on the mail one.** Camel knows
nothing about `ESource`, so nothing in Camel joins a transport to the account it
sends for. What joins them is two hops of uid indirection through a *third*
source: `[Mail Account] IdentityUid` names an identity, and that identity's
`[Mail Submission] TransportUid` names the transport. Evolution walks that chain
out of `libedataserver` accessors; the client program is handed only the account
uid and walks the same one, so `transport-uid=jmap-functional-transport` is the
assertion the whole leg exists for. Every link is a string in a file that no
compiler and no unit test can hold to the file it names, and a broken link is
the quietest failure this provider has — the recipe already says so: the account
receives mail perfectly and fails only when the user presses Send.

The transport also comes out of a different entry of the same registered
provider struct than the store does, `object_types[CAMEL_PROVIDER_TRANSPORT]`. A
provider that left it `G_TYPE_INVALID` loads, receives mail, and fails only
here.

Beyond the chain: one submission, through the identity the mock seeded for the
address the *identity source* named (resolved over the wire by `Identity/get`);
the envelope built from the two `CamelAddress` lists rather than from the
headers; the sent copy in Sent and no longer `$draft`, which is the server's own
`onSuccessUpdateEmail` and therefore evidence the submission was *accepted* and
not merely posted; `out_sent_message_saved` TRUE, which is one copy in Sent
rather than two; the uploaded bytes carrying the subject and the body Camel's own
emitter wrote; and `EmailSubmission/set` last of the method calls, the blob
upload before the import being a plain HTTP PUT and so absent from that list.

**The subject and body are arguments, not constants.** The client builds the
message from `argv`, so what the test asserts and what goes on the wire are one
string. mail.rs can afford constants on both sides because the mock is what
holds the message there; here the client is where it originates, and a constant
in each file would be two that can drift.

**A second test makes the recipe's mistake on purpose**: the same three files
with the transport's `[Authentication]` group deleted. The chain still resolves
— this is a source that was found and that names no server — and the send fails
at the *connect* with `the account does not name a JMAP server`, having made not
one request, so nothing is imported and no draft is left behind for a send that
never happened. That is a permanent test rather than a note, because the thing
it catches is a line missing from a keyfile and the only other thing that would
catch it is a reader.

**Red first, then three mutations.** Red: both tests on
`JMAP_FUNCTIONAL_TRANSPORT_CLIENT` unset. Then, each reverted after: (1) a typo
in `TransportUid` → both tests fail on the uid, with the client's own *no
transport source with UID* beside it, so the walk is really the keyfile's and
not the client agreeing with itself; (2) seed the mock's identity for a
different address → `send: this account cannot send mail as alice@example.com`,
so `Identity/get` is really consulted and the refusal really happens before the
upload; (3) seed the Sent mailbox with no role → `sent-copy-saved=0`, which is
`OutgoingMailboxes` reached through the whole stack and the out-parameter having
teeth.

The two client programs are now built by a `foreach` over their names — same
libraries, same kind of process, and the send half opens no store so it shares
no code with the receiving half beyond that.

Verified locally: `ctest` 14/14 with `-DENABLE_FUNCTIONAL_TESTS=ON`,
`cargo fmt --check`, `cargo test --locked` (491, unchanged — `jmap-functional`
is out of `default-members`), `cargo clippy --all-targets --locked -- -D
warnings` clean and the same for `-p jmap-functional`, and
`transport-client.c` compiled standalone under `-Wall -Wextra` clean since the
project does not pass those. `reuse lint` and `cargo deny` not run (neither
binary is on this VM); both new files carry SPDX `GPL-3.0-or-later` headers,
`docs/functional-tests.md` is covered by `REUSE.toml`'s `docs/**` annotation,
and nothing was added to `po/POTFILES.in` because the policy there excludes
tests and neither new file holds a user-visible string.

No milestone tag. M9 layer 1 now covers both halves of mail, but it still has
**no CI job** — that needs `evolution-data-server` + `dbus-daemon` in the CI
image, a maintainer decision — and tier 2, the GUI smoke test, needs a display
this VM does not have. Both are the same blockers as the last three sessions.

Next, in order: (1) the M8 test tag, by hand. (2) Unchanged maintainer
decisions: `evolution-data-server` + `dbus-daemon` in the CI image, gettext in
the CI image, and whether `GETTEXT_PACKAGE` should stop saying `example-module`.
Unchanged: M7 still **needs human verification in real Evolution**, the
manual-test recipes are unlinked from the README, `jmap-mail`'s rustdoc is
dirty, and the once-seen `jmap-mail` `tests/transport.rs` hang is still
unexplained.

## 2026-08-10 (hundred-and-thirtieth session)

The calcard directive, reading half: `jmap-vcard`'s syntax module now *parses*
with [`calcard`](https://github.com/stalwartlabs/calcard) 0.3.9 and still emits
by hand. Gone from this repository: `unfold`, `parse_line`, `unescape`,
`split_unescaped`, `split_unquoted`, `find_unquoted` and `unquote_param` — the
whole receiving side of RFC 2425/2426, which is also the side hostile input
arrives on. `Property` keeps its shape and `contact.rs` is untouched; what
changed underneath is that a property now holds its values *decoded*
(`Vec<String>`) instead of in escaped on-the-wire form, so `text()` joins the
components on `;` and `components()` hands the vector out.

**Red first, and the red test is the reason to do it at all.**
`ENCODING=QUOTED-PRINTABLE` is vCard 2.1, but exporters still write it and the
.vcf files users import carry it; the hand-rolled lexer handed `V=C3=A9ra`
through as a value, which would put that text in the address book and send it
back to the server on the next save. calcard decodes it, honouring `CHARSET`.
That is one line of test and a capability we were never going to hand-roll.

**The emitter stays ours, deliberately, and here is what it costs to give it
up.** calcard's vCard writer targets 4.0 output, and three of its choices are
wrong for the only reader we have:

1. It folds one octet late. The `:` between the parameters and the value is
   written without being counted, so the first line of every folded property is
   76 octets against RFC 2426 §2.6's 75. Reproduced standalone, outside this
   repository, with a single `NOTE` of 200 `x`: `76 NOTE:xxx…`. Worth an
   upstream issue (not filed — that is an outward-facing action for the
   maintainer to take), and it is exactly what
   `folds_long_lines_without_splitting_characters` was written to catch.
2. It escapes a CR in a text value as `\r`, which vCard 3.0 does not define
   (RFC 2426 §5 has only `\n`). `EVCard` resolves an unknown escape to the
   character itself, so a `\r` would arrive in the address book as a literal
   `r` — the value corrupted rather than re-encoded.
3. It escapes a `"` inside a quoted parameter value as `\"`, which is RFC 6868
   territory and correct for a 4.0 peer. A 3.0 reader has no escape inside the
   quotes at all: it reads the `"` as the end of the value, and a server-chosen
   `emails` map key of `x";FN="Mallory` gets to open a parameter of its own.
   That is finding F1's class of bug walking back in, so the strip in
   `quote_param`/`fold_into` stays where the audit put it.

So the split is: calcard owns the *syntax* we read, this crate owns the
*policy* a vCard 3.0 consumer needs when we write. Both halves are stated in
`syntax.rs`'s own doc comments rather than only here.

**Strict mode, and how that was checked against the real thing.** The parser
runs `Parser::new(..).strict()`, so a truncated card is an error rather than
half a contact the next save would write back over the whole one. That makes
"anything EVCard emits, this reads" a property worth a permanent test, so
`reads_a_card_evolution_wrote` parses a card shaped like
`e_vcard_to_string (EVC_FORMAT_VCARD_30)` output — a folded base64 `PHOTO`, an
`X-EVOLUTION-FILE-AS`, an empty `NOTE`, a grouped `item1.TEL` beside an
`item1.X-ABLabel` — and asserts the mapped properties still come out. The
authentic check is `ctest -R functional-book`, which drives a contact through
`e-book-client` and real EDS into the mock and passed against the rebuilt
module.

Dependency notes: `calcard` with `default-features = false`, which leaves out
its JSCalendar/JSContact halves — the JMAP side of this mapping is ours and is
already tested against the mock — and with it `jmap-tools`, `uuid`, `serde` and
`serde_json`. The 40 crates that entered `Cargo.lock` were checked by hand
against `rust/deny.toml`'s allow list (`cargo-deny` is not on this VM): all
MIT/Apache-2.0 dual licences bar `phf`/`slab` (MIT), `zerocopy`
(BSD-2-Clause OR …), `r-efi` (… OR LGPL-2.1-or-later) and the
`wasip2`/`wit-bindgen` Apache-with-LLVM-exception, every one of them already
allowed.

Verified locally: `cargo test` 493 (was 491: +1 QUOTED-PRINTABLE and +1
EVCard-shaped card, so `syntax.rs` went 11→13),
the EDS-header crates green via the `rust-test-eds` set
(`-p eds-sys -p evo-sys -p jmap-backend-core -p jmap-backend-book
-p jmap-backend-cal -p jmap-mail -p jmap-backend-collection -p jmap-config`),
`ctest -L functional` 4/4, `cargo fmt --check`, and `cargo clippy
--all-targets --locked -- -D warnings` clean. `reuse lint` not run (not on this
VM); no files were added, so no new SPDX headers were needed. The
`example-module` link failure under `cargo test --workspace` is the one an
earlier session already recorded — that crate has no dependency but
`pkg-config` and is untouched here.

No milestone tag: the directive is half carried out. What is left, in order:
(1) `jmap-ical`'s lexer — the same exercise on the calendar side, where
calcard's iCalendar *writer* may well be usable since libical is a more
forgiving reader than EVCard; (2) the two emitters, once the fold off-by-one is
fixed upstream (or once the maintainer decides 76-octet lines are acceptable —
RFC 2426 says SHOULD, and every real reader unfolds regardless). Unchanged
blockers: M9 has no CI job (needs `evolution-data-server` + `dbus-daemon` in the
CI image, a maintainer decision) and no GUI tier (needs a display this VM lacks);
M7 still **needs human verification in real Evolution**; the manual-test recipes
are unlinked from the README; `jmap-mail`'s rustdoc is dirty; the once-seen
`jmap-mail` `tests/transport.rs` hang is still unexplained.

## 2026-08-10 (hundred-and-thirty-first session)

The calcard directive, calendar side: `jmap-ical`'s syntax module now reads
content lines with [`calcard`](https://github.com/stalwartlabs/calcard) 0.3.9
and still emits by hand, mirroring what the previous session did to
`jmap-vcard`. Gone from this repository: `parse_line`, `unescape`,
`split_unescaped`, `split_unquoted`, `find_unquoted` and `unquote_param` — the
receiving side of RFC 5545 §3.1/§3.2, escapes and quoted parameters included.
What arrives instead is *typed*: `DTSTART` comes back as a date-time, `DURATION`
as a duration and `RRULE` as a rule, so `Property` holds its values decoded
(`Vec<String>`, one per `,`-separated part) with the typed ones rendered in
their iCalendar spelling, and `event.rs` reads them exactly as before.

**Red first, two of them, and both are about not losing a whole calendar.**
A UTF-8 byte order mark in front of `BEGIN:VCALENDAR` — what Windows exporters
write, and what an imported `.ics` keeps — made the hand-rolled parser answer
`NotACalendar`, so every event in the file was gone over three invisible bytes.
And a single unreadable content line failed the *whole* parse, which contradicts
the policy `event.rs` states in its own header: a property that cannot be read
is treated as absent, because an event that loses a field beats a calendar that
refuses to open. Both are one line of test each and now hold one layer down.
`ICalError::Malformed` is therefore retired — nothing can construct it.

**Three things calcard cannot do here, and what each one cost.**

1. `Parser::strict()`, which the vCard side relies on, is **unusable for
   iCalendar in 0.3.9**: `icalendar()` returns `Entry::InvalidLine("BEGIN")`
   after handling a nested `BEGIN:` when strict, so it rejects every calendar
   that has a `VEVENT` in it — i.e. all of them. Reproduced against the
   fixtures; `src/icalendar/parser.rs` lines 104-108 are the unconditional
   `return`. Upstream issue material (not filed — an outward-facing action for
   the maintainer). Consequence: lenient mode is the only mode, and it reads a
   *truncated* document as a whole one, and lets an `END:VTODO` close a
   `VEVENT`. Handing the mapping half an event means the next save writing the
   fragment back over the whole one, so `check_structure` — `BEGIN`/`END`
   pairing over the unfolded lines, and nothing else — stays ours and runs
   before calcard is asked for the content. That is the one piece of the old
   lexer deliberately kept, and `unfold` with it.
2. The depth cap (audit finding F4) had to move rather than go: calcard's tree
   is *flat*, a `Vec` addressed by index, so its parse survives 100 000 levels
   where ours would have overflowed the stack — but the `Component` tree we
   build from it still recurses, in `from_component` and in the drop glue. So
   `check_depth` measures the flat graph iteratively before anything recurses
   over it, and the three F4 regression tests pass unchanged.
3. RFC 6868 (`^n`, `^^`, `^'` in parameter values) is decoded by neither
   implementation. No gain, no loss; written down so nobody re-checks.

**One behaviour difference, examined rather than absorbed.** calcard completes
a DATE-TIME that is missing its seconds: `20260115T1300` reads as 13:00:00,
where the old lexer refused anything that was not 8 or 15/16 characters. The
missing field can only be zero, so the event does not move, and that is the
better answer — `a_dtstart_missing_its_seconds_is_completed_rather_than_dropped`
now asserts it. But the boundary is laxer than libical's and is written into
that test: `2026011` reads as 2026-01-01, so a *date* that lost a digit moves
the event two weeks. Nothing here can produce that — the iCalendar this mapping
reads comes from EDS, whose libical would have refused the value first, and the
server sends JSON — but it is a real divergence and is on the record rather than
left to be discovered. Also noted while measuring, and *pre-existing*: neither
implementation range-checks the components, so a `DTSTART:20261315T250000`
reaches the server as `2026-13-15T25:00:00` for it to reject. Unchanged by this
work; a candidate for a later increment.

**The emitter stays ours** for the reasons the vCard session measured — the fold
off-by-one, the `\r` escape, the quote inside a quoted parameter — plus one that
is iCalendar's alone: `Property::raw` exists precisely so that `DURATION`, an
`RRULE`'s `FREQ` and a `TZID` parameter are *not* escaped, and `fold_into`'s CR
and LF strip is what keeps those unescaped, server-chosen strings from ending a
content line early (findings F2 and F4). All eight `hostile.rs` tests pass
untouched.

Verified locally: `cargo test --locked` 496 (was 493: +2 red-first, +1 the
date-time boundary), the EDS-header crates green via the `rust-test-eds` set
(`-p eds-sys -p evo-sys -p jmap-backend-core -p jmap-backend-book
-p jmap-backend-cal -p jmap-mail -p jmap-backend-collection -p jmap-config`),
`ctest -L functional` 4/4 — `functional-cal` drives an event through
`e-cal-client` and real EDS into the mock against the rebuilt module, which is
the authentic check for this change — `cargo fmt --check`, and `cargo clippy
--all-targets --locked -- -D warnings` clean for both crate sets. `reuse lint`
and `cargo deny` not run (neither is on this VM); no files were added, so no new
SPDX headers were needed, and no crate entered `Cargo.lock` — calcard and its
dependencies were already there for `jmap-vcard`, hand-checked against
`deny.toml` last session.

Housekeeping: `/` had filled to 100% mid-session and stopped a build with "No
space left on device". `rust/target/debug/incremental` (3.5G), `target/tmp` and
`target/doc` were deleted, leaving ~3G free. Worth watching — 55G of 58G is in
use and the debug target alone is 30G.

No milestone tag yet: the directive's two emitters are still ours by choice, and
that is the remaining question — the fold off-by-one wants an upstream fix or a
maintainer decision that 76-octet lines are acceptable (RFC 2426/5545 say
SHOULD, and every real reader unfolds regardless). With both text layers now
reading through calcard, that judgement is all that stands between here and
`CALCARD COMPLETE`. Unchanged blockers: M9 has no CI job (needs
`evolution-data-server` + `dbus-daemon` in the CI image, a maintainer decision)
and no GUI tier (needs a display this VM lacks); M7 still **needs human
verification in real Evolution**; `docs/MILESTONES.md` does not exist yet, so
the M8 tag the last three sessions asked for is still unwritten; the manual-test
recipes are unlinked from the README; `jmap-mail`'s rustdoc is dirty; the
once-seen `jmap-mail` `tests/transport.rs` hang is still unexplained.

## 2026-08-10 (hundred-and-thirty-second session)

The item the previous session left on the record as "a candidate for a later
increment": neither the old lexer nor calcard range-checks a date-time's
*fields*, so `DTSTART:20261315T250000` travelled intact in both directions.
`jmap-ical` now asks whether the instant exists before converting it, and treats
one that does not the way it treats a value it cannot read at all — as absent.

**Red first, and both directions are a real loss.** Outbound (server → EDS) the
impossible `DTSTART` reaches libical, which refuses the component and takes
every other field of the event with it — the summary, the description, the
recurrence, all of it, over one bad digit. Inbound (EDS → server) it becomes
`"start": "2026-13-15T25:00:00"`, which is not a JSCalendar LocalDateTime, so
the whole `CalendarEvent/set` fails and the user's edit to the *title* is lost
alongside the start they never typed. Dropping the property costs one field and
nothing else: `jmap-cal-sync`'s patch builder already refuses to send
`"start": null`, so an unreadable start means the server's start simply stands.

The check is month 1-12, day against the month's real length in the proleptic
Gregorian calendar, hour ≤ 23, minute ≤ 59, second ≤ 60. The two dates that look
wrong and are not have their own test: 29 February of a leap year (2024 and 2000
yes, 1900, 2100 and 2026 no), and the leap second RFC 5545 §3.3.12 and RFC 3339
§5.6 both spell `:60` — calcard round-trips it, so a server that stores one gets
it back unchanged.

**A second finding fell out of the same code path, and it is the worse one.**
`UNTIL` goes through the same conversion, and an `UNTIL` that could not be
written was simply left off the `RRULE` — turning a recurrence that *ends* into
one that never does. A weekly meeting that finished in March would have been
drawn into every week of the user's calendar for ever. Such a rule is now
refused whole: showing no recurrence under-states the event, showing an
unbounded one fabricates it. `maps_recurrence_rule` was widened to agree — it
now answers "does this rule survive the trip" for all three ways it can fail
(unmodeled parts in `extra`, no frequency, an unwritable `until`), so the save
path still never patches `recurrenceRules` over a recurrence the user was not
shown. Deliberately unchanged: a rule with `byDay` in `extra` is still *drawn*,
narrowed to what an `RRULE` holds, because a weekly event on the wrong days
beats no event; `extra` is a narrowing, the other two are losses.

Verified locally: `cargo test --locked` 499 (was 496: +3 red-first), the
EDS-header crates green via the `rust-test-eds` set, `ctest -L functional` 4/4 —
`functional-cal` drives an event through `e-cal-client` and real EDS into the
mock against the rebuilt module — `cargo fmt --check`, and `cargo clippy
--all-targets --locked -- -D warnings` clean for both crate sets. `reuse lint`
and `cargo deny` not run (neither is on this VM); no files were added, so no new
SPDX headers were needed, and no dependency changed.

Housekeeping, and it is now a pattern rather than an incident: `/` hit 100% again
mid-session and failed a link. `rust/target/debug` had reached 33G, 32G of it in
`deps` — cargo never collects the test binaries of past sessions, and this repo
has had a great many. Deleting it freed 33G and cost one full rebuild. Worth a
`cargo clean` between sessions, or a `CARGO_TARGET_DIR` on the larger disk.

No milestone tag. Unchanged blockers: the calcard directive's two emitters are
still ours by choice, waiting on the fold off-by-one being fixed upstream or a
maintainer decision that 76-octet lines are acceptable; M9 has no CI job (needs
`evolution-data-server` + `dbus-daemon` in the CI image, a maintainer decision)
and no GUI tier (needs a display this VM lacks); M7 still **needs human
verification in real Evolution**; `docs/MILESTONES.md` does not exist yet, so the
M8 tag the last four sessions asked for is still unwritten; the manual-test
recipes are unlinked from the README; `jmap-mail`'s rustdoc is dirty; the
once-seen `jmap-mail` `tests/transport.rs` hang is still unexplained.

## 2026-08-10 (hundred-and-thirty-third session)

The calendar mapping read `DURATION` and nothing else, and `DURATION` is not
what Evolution writes. The appointment editor calls
`e_cal_component_set_dtend`, and RFC 5545 §3.6.1 makes `DTEND` and `DURATION`
mutually exclusive, so an event a user created said how long it was only
through its end — and that end was dropped on the floor. What reached the
server was an event with no duration, which is `P0D` by RFC 8984 §4.2.2: every
appointment made in Evolution was shared with the rest of the world as a
zero-length blip at its start time. `jmap-ical` now measures the difference
when there is no `DURATION` to read.

**Red first, and red on the path that matters.** Two unit tests failed for the
right reason (`an_events_length_may_arrive_as_a_dtend_instead_of_a_duration`,
`a_length_read_from_a_dtend_is_written_back_as_a_duration`), and the save-path
test through `jmap-mockd`
(`a_new_event_that_states_its_end_rather_than_its_length_still_has_one`) was
checked red by disabling the new branch and re-running rather than by
assertion. Then the end that no unit test can reach: `tests/functional/
cal-client.c` now writes `DTEND` where it wrote `DURATION`, the shape Evolution
actually produces, and `functional-cal` asserts the mock holds `PT1H30M` — real
`evolution-calendar-factory`, real `libecalbackendjmap.so`, real
`e_cal_client_create_object_sync`. Mutating the expectation to `PT2H` fails it
with the stored event printed, so the assertion is live and not a tautology.

**The arithmetic, and what it deliberately does not know.** The difference is
taken on the wall clock: each end is turned into seconds by Hinnant's
`days_from_civil` and subtracted. That is also how JSCalendar reads the answer
back — its `P1D` is a nominal day, the same time on the next day, not 24 exact
hours — so whole days are emitted as days (`P1D`, `P2DT1H`) rather than as
hours, and the value survives a daylight saving change the way the user's
calendar does. The two agree for as long as both ends are in one zone, which is
the only shape Evolution writes; a `DTEND` in a *different* zone than the
`DTSTART` comes out short or long by the offset between them, and that is
written into the doc comment rather than hidden. The alternative — dropping the
length of an event that plainly states it — is the bug this fixes.

Three refusals, each tested: a `DTEND` before the start, one equal to it (the
`P0D` default anyway), and one that cannot be read or names no instant that
exists — the range check the last session added guards this path too, so
`DTEND:20260230T130000` yields no duration rather than a negative one. And when
a malformed component carries both `DURATION` and `DTEND`, `DURATION` wins: it
maps to the JSCalendar property with no arithmetic at all.

`DTEND` is now the one property this crate reads and never writes; a length
always goes back out as `DURATION`, which round-trips exactly and which libical
reads. One consequence worth knowing before it is noticed as a bug: a server
that spells a duration `PT90M` and an Evolution that re-saves the event as
`DTEND` will disagree on spelling, so the save patches `duration` to the
equivalent `PT1H30M`. Same length, one needless write; normalising it would
mean parsing ISO 8601 durations, which is a bigger increment than this one.

Next, and now more visible than before: `showWithoutTime` is still unmodeled.
An all-day event from Evolution (`DTSTART;VALUE=DATE` + `DTEND;VALUE=DATE`)
now at least gets its `P1D`, but its start still reads as midnight and the
server is never told it is an all-day event, so it comes back to every other
client as a midnight appointment. That is the obvious next increment in this
area: `showWithoutTime` on `CalendarEvent`, `VALUE=DATE` on the way out when
the start is midnight, the duration is whole days and there is no zone to lose,
and the flag in the patch diff so switching an event to timed reaches the
server.

Verified locally: `cargo test --locked` 504 (was 499: +5 red-first), the
EDS-header crates green via the `rust-test-eds` set (`-p eds-sys -p evo-sys
-p jmap-backend-core -p jmap-backend-book -p jmap-backend-cal -p jmap-mail
-p jmap-backend-collection -p jmap-config`), `ctest` 14/14 including the four
functional tests, `cargo fmt --check`, and `cargo clippy --all-targets --locked
-- -D warnings` clean for both crate sets. `reuse lint` and `cargo deny` not run
(neither is on this VM); no files were added, so no new SPDX headers were
needed, and no dependency changed.

No milestone tag. Unchanged blockers: the calcard directive's two emitters are
still ours by choice, waiting on the fold off-by-one being fixed upstream or a
maintainer decision that 76-octet lines are acceptable; M9 has no CI job (needs
`evolution-data-server` + `dbus-daemon` in the CI image, a maintainer decision)
and no GUI tier (needs a display this VM lacks); M7 still **needs human
verification in real Evolution**; `docs/MILESTONES.md` does not exist yet, so
the M8 tag the last five sessions asked for is still unwritten; the manual-test
recipes are unlinked from the README; `jmap-mail`'s rustdoc is dirty; the
once-seen `jmap-mail` `tests/transport.rs` hang is still unexplained.

## 2026-08-10 (hundred-and-thirty-fourth session)

All-day events were a lie in both directions. Evolution writes one as
`DTSTART;VALUE=DATE` — iCalendar has no other way to say "a day, not a time of
day" — and the mapping read that as midnight and said nothing else, so what
reached the server was a midnight appointment. Every other client reading the
account then drew it as one. Coming back, an event the server marked
`showWithoutTime` was rendered with a time on it, so Evolution showed the same
midnight appointment. `showWithoutTime` is now modeled: on `CalendarEvent`, in
`jmap-ical` both ways, and in the save path's patch.

**Red first, at three levels.** Six unit tests in `jmap-ical` (the DATE read,
the DATE write and its round trip, the four shapes that refuse the DATE form,
the length-less case, and the two `UNTIL` ones) and three save-path tests
through `jmap-mockd` — four of the nine failed on assertions immediately, and
the five that passed vacuously were each checked live afterwards by mutating
the code they guard: dropping the midnight/zone guard, the whole-days guard,
the `UNTIL`-midnight guard, and diffing against `current` instead of
`baseline` each fail exactly one named test. Then the end no unit test reaches:
`tests/functional/cal-client.c` now writes a second event, `VALUE=DATE` on
both ends, and `functional-cal` asserts the mock holds `showWithoutTime: true`,
`2026-02-01T00:00:00`, `P1D` and no zone — real `evolution-calendar-factory`,
real `libecalbackendjmap.so`, real `e_cal_client_create_object_sync`. Making
the reader answer `None` for a date-only start fails it with the stored event
printed, so that assertion is live too.

**The flag lives in a value type, which is why writing it has conditions.**
RFC 8984 §4.1.5 asks that an event shown without a time start at midnight and
last whole days, but a server may send otherwise, and RFC 5545 then has nothing
to write: a DATE value has no time to hold 09:00, takes no `TZID` (§3.2.19),
stands only beside a duration of whole days (§3.6.1), and — the one that is
easy to miss — obliges an `RRULE`'s `UNTIL` to be a DATE as well (§3.3.10). So
`shows_without_time` checks all four, and an event failing any of them is
written as the timed event it half is: wrong about its day-ness, right about
when it happens. That is deliberately the safer loss, and it costs nothing on
the way back, because `patch::diff` compares against **the server's own event
put through the same rendering**. A flag the component could never show is a
flag both sides lose, so the two agree and nothing is patched — the mechanism
that already protected `timeZone` and `recurrenceRules` covers this for free,
and there is a test that fails if the diff is taken against the server's event
instead.

Decisions worth naming. The read answers `Some(true)` or `None`, never
`Some(false)`: the RFC 8984 default is false anyway, and since an edit is read
off a difference from the baseline, answering `false` where the server said
nothing would invent one. Clearing it patches `null` rather than `false`, which
is how a PatchObject says "back to the default". A `TZID` on a date-only
`DTSTART` is ignored rather than kept, per §3.2.19, which also keeps the reader
symmetric with the only shape the writer emits. And an all-day event with no
length is still written as a DATE: RFC 5545 §3.6.1 makes that one day where RFC
8984 would call it zero, and a day is what the user meant — the reverse, a
midnight appointment of no duration, is not something a calendar can draw. The
length does not come back from that rendering, so the day RFC 5545 implies is
never read back as a length the user typed.

Verified locally: `cargo test --locked` 513 (was 504: +9 red-first), the
EDS-header crates green via the `rust-test-eds` set (`-p eds-sys -p evo-sys
-p jmap-backend-core -p jmap-backend-book -p jmap-backend-cal -p jmap-mail
-p jmap-backend-collection -p jmap-config`), `ctest` 14/14 including the four
functional tests, `cargo fmt --check`, and `cargo clippy --all-targets --locked
-- -D warnings` clean for both crate sets. `reuse lint` and `cargo deny` not run
(neither is on this VM); no files were added, so no new SPDX headers were
needed, and no dependency changed.

Next in this area, in the order they matter: `DTEND` is still the only way
Evolution states a length and `DURATION` the only way we write one, so an
all-day event re-saved from Evolution patches `duration` to the equivalent
spelling — same length, one needless write, and normalising it needs an ISO
8601 duration parser. `showWithoutTime` is now mapped but `VALUE=DATE` on
`UNTIL` is the only place the date-ness reaches a *second* property; RDATE and
EXDATE are unmapped, so a recurrence with exceptions still loses them. And
nothing yet tests an all-day *recurring* event end to end through EDS, only in
unit tests.

No milestone tag. Unchanged blockers: the calcard directive's two emitters are
still ours by choice, waiting on the fold off-by-one being fixed upstream or a
maintainer decision that 76-octet lines are acceptable; M9 has no CI job (needs
`evolution-data-server` + `dbus-daemon` in the CI image, a maintainer decision)
and no GUI tier (needs a display this VM lacks); M7 still **needs human
verification in real Evolution**; `docs/MILESTONES.md` does not exist yet, so
the M8 tag the last six sessions asked for is still unwritten; the manual-test
recipes are unlinked from the README; `jmap-mail`'s rustdoc is dirty; the
once-seen `jmap-mail` `tests/transport.rs` hang is still unexplained.

## 2026-08-10 (hundred-and-thirty-fifth session)

Deleting one occurrence of a recurring event did nothing. Evolution's "Delete
this occurrence" leaves the master component in place and adds an `EXDATE` to
it — RFC 5545 §3.8.5.1 is the only thing iCalendar has for "not that one" — and
the mapping read neither `EXDATE` nor `RDATE`, so the property was dropped on
the floor and the save patched everything *except* it. The cancelled standup
stayed on the server and in every other client reading the account, and the
user's own Evolution showed it gone: the two ends disagreed silently, which is
worse than either answer. `recurrenceOverrides` (RFC 8984 §4.3.4) is now
modeled — on `CalendarEvent`, in `jmap-ical` both ways, and in the save path's
patch.

**Red first, at three levels.** Ten unit tests in `jmap-ical` (the `EXDATE` and
`RDATE` round trips, several instances on one line, the UTC `Z` form, the
all-day DATE form and the shape that refuses it, an instance edited on its own,
an unwritable id, the both-excluded-and-added contradiction, and the event that
names none) and four save-path tests through `jmap-mockd`. Eleven of the
fourteen failed on assertions immediately. The other three pass vacuously
against the old code, so each was checked live afterwards by mutating what it
guards: dropping the midnight condition on override ids, swapping the
`EXDATE`/`RDATE` read order, removing the id check from
`maps_recurrence_override`, dropping the save guard, and diffing against
`current` instead of `baseline` each fail exactly one named test. The last of
those mutations survived the first four tests, which is how the fourth save test
— an override the server spells `{"excluded": false}`, which the component can
only render as the shorter `RDATE` — came to be written at all. Then the end
no unit test reaches: `tests/functional/cal-client.c` writes a third event —
`RRULE:FREQ=WEEKLY;COUNT=6` with `EXDATE:20260129T130000Z` — and
`functional-cal` asserts the mock holds `{"2026-01-29T13:00:00": {"excluded":
true}}` beside a weekly rule counting six. Real `evolution-calendar-factory`,
real `libecalbackendjmap.so`, real `e_cal_client_create_object_sync`; making
the reader answer `None` fails it with the stored event printed.

**One property in JSCalendar, two in iCalendar, and a third thing neither
`EXDATE` nor `RDATE` can say.** An override is a PatchObject, so it says three
things: this instance is off (`excluded: true` → `EXDATE`), this instance
happens (`{}` → `RDATE`), and — the one with no spelling in a single `VEVENT` —
this instance happens *differently*, which iCalendar writes as another `VEVENT`
carrying a `RECURRENCE-ID`. The third is handled exactly as a rule with `byDay`
in it already was: still **drawn**, narrowed to a bare `RDATE` so the occurrence
is at least visible at the parent's title, and flagged by a new
`maps_recurrence_override` so the save path never patches
`recurrenceOverrides` over it. `patch.rs` now has three properties that need
more than a difference from the baseline rather than two.

The id is checked as well as the patch, which the rule side taught: an override
keyed on a LocalDateTime no `EXDATE` can spell — `2026-13-29T13:00:00`,
`2026-02-30T…` — would vanish from a property replaced whole, so it fails the
guard too. Two smaller decisions, each tested. `excluded` is read strictly:
anything that is not literally `true`, including the `false` RFC 8984 defaults
to and a value of the wrong type, counts as an instance that happens, because
that reading cannot make an appointment disappear. And a component naming one
instant in *both* properties is read as excluded, since `EXDATE` wins over the
recurrence set anyway and the other reading invents an appointment.

**The three date-time properties now agree by construction.** RFC 5545
§3.8.5.1/§3.8.5.2 oblige an `EXDATE`/`RDATE` to carry `DTSTART`'s value type and
zone — otherwise the exclusion resolves against a different clock and misses the
occurrence it was meant to remove — so the four-way choice that used to be
inline in `DTSTART` (DATE, `Z`, `TZID`, floating) moved into one `dated()`
helper all three go through. `shows_without_time` grew the matching condition:
an all-day event whose override is named at 09:00 is written as the timed event
it half is, rather than truncating an exclusion onto the wrong day. That is the
same trade already made for an `UNTIL` at 09:00.

Also corrected: `MAPPED_PROPERTIES` claimed seven properties and omitted
`recurrenceRules`, which the save path has named since M4. It is now nine, with
a note that the last two are covered conditionally.

Verified locally: `cargo test --locked` 527 (was 513: +14 red-first), the
EDS-header crates green via the `rust-test-eds` set (`-p eds-sys -p evo-sys
-p jmap-backend-core -p jmap-backend-book -p jmap-backend-cal -p jmap-mail
-p jmap-backend-collection -p jmap-config`), `ctest` 14/14 including the four
functional tests, `cargo fmt --check`, and `cargo clippy --all-targets --locked
-- -D warnings` clean for both crate sets. `reuse lint` and `cargo deny` not run
(neither is on this VM); no files were added, so no new SPDX headers were
needed. One dependency line changed: `serde_json` is now a real dependency of
`jmap-ical` (it was a dev-dependency) and a dev-dependency of `jmap-functional`,
both already in the lock file as workspace members' deps, so `Cargo.lock` gained
one line and no new crate.

Next in this area, in the order they matter: a `RECURRENCE-ID` `VEVENT` is the
only way a *modified* instance can be shown properly, and it is a bigger
increment than this one — it means rendering several components into one
`VCALENDAR` and reading them back into a map of patches, which is also what
`ECalMetaBackend` expects for a recurring event with detached instances. An
`RDATE` of `VALUE=PERIOD` (legal, not something Evolution writes) is read as its
start and would be written back as a plain date-time. And `DTEND` is still the
only way Evolution states a length while `DURATION` is the only way we write
one, so a re-saved event patches `duration` to the equivalent spelling.

No milestone tag. Unchanged blockers: the calcard directive's two emitters are
still ours by choice, waiting on the fold off-by-one being fixed upstream or a
maintainer decision that 76-octet lines are acceptable; M9 has no CI job (needs
`evolution-data-server` + `dbus-daemon` in the CI image, a maintainer decision)
and no GUI tier (needs a display this VM lacks); M7 still **needs human
verification in real Evolution**; `docs/MILESTONES.md` does not exist yet, so
the M8 tag the last seven sessions asked for is still unwritten; the manual-test
recipes are unlinked from the README; `jmap-mail`'s rustdoc is dirty; the
once-seen `jmap-mail` `tests/transport.rs` hang is still unexplained.

## 2026-08-10 (hundred-and-thirty-sixth session)

Editing one occurrence of a recurring event was the half of "not that one" the
last session left. Deleting an occurrence now reaches the server; *changing*
one did not — Evolution writes it as a second `VEVENT` with the same `UID` and
a `RECURRENCE-ID` naming the day it replaces (RFC 5545 §3.8.4.4), and both ends
of this project threw that component away. `marshal::icalendar_from_instances`
put only the master in the envelope it handed the mapping, and `jmap-ical` read
only the first `VEVENT` and wrote only one. So the user's "Sprint review on the
29th" never left the machine, and the server's own edited instance came back
from `load_component_sync` as an ordinary occurrence at the series' title.

**Both directions, because either alone is worse than neither.** Rendering an
override as a component without reading one back would make the *next* save
delete it: the save path diffs the edited component against a re-rendering of
what the server holds, so an override the writer draws and the reader cannot
see is a difference, and `recurrenceOverrides` would be patched down to what
the reader found. The same for marshal: once the mapping draws an edited
instance, a save that dropped the detached components before parsing them says
"that day is like every other" and means it. The three changes are one change.

**Red first, and the vacuous ones mutated.** Fifteen new tests in `jmap-ical`
(the component and its round trip, an instance moved to another time, one that
drops a property with a `null`, one both edited and excluded, five patches the
drawing cannot take, a half-known patch, the all-day forms both ways, the
series found whatever its position, a detached instance that restates the
series, one with no series at all, `RANGE=THISANDFUTURE`, and a detached
instance beside an `RDATE` for the same instant), three in `jmap-cal-sync`
against `jmap-mockd`, and the rewritten marshal test. Ten failed on assertions
immediately. Four passed against the old code, so each was checked by mutating
what it guards — letting an `excluded` override say more, accepting any patch
key, ignoring `RANGE`, and not skipping the series in the detached loop each
fail a named test. `ctest` still runs the four functional tests through real
EDS daemons; no functional test covers an *edited* occurrence yet, which is the
gap in this increment (it needs `e_cal_client_modify_object_sync` with
`E_CAL_OBJ_MOD_THIS` in `tests/functional/cal-client.c`).

**What an override may say, and what it still may not.** `OVERRIDE_PROPERTIES`
is the new machine-readable list: `title`, `description`, `start`, `duration`,
`status`. A patch naming anything else — a location, a participant, an alert —
is still *drawn* with a bare `RDATE` at the series' title and still flagged by
`maps_recurrence_override`, exactly as an `RRULE` that had to drop its `byDay`
is. `timeZone` is deliberately out: every date-time in the document is written
in the series' zone, so an instance in another one has no spelling here.
`excluded` is now exclusive — an override that is off may say nothing else,
because the `EXDATE` carrying it has nowhere to put an edited title, so
`{"excluded": true, "title": …}` is flagged rather than silently truncated.

Three readings worth writing down. A detached instance is a *whole component*,
not a patch, so a property the series has and the instance does not comes back
as a `null` — which is how a PatchObject removes one, and the only reading
that lets a user clear an instance's description. An override's key *is* its
instance's start (RFC 8984 §4.3.4), so `DTSTART` is compared against the
`RECURRENCE-ID` rather than against the series, and says something only when
the occurrence moved. And `RANGE=THISANDFUTURE` (RFC 5545 §3.2.13) is skipped
rather than read: it stands for every instance from that one on, which
`recurrenceOverrides` has no single entry for, so reading it as one would move
one day and drop the change to all the others. Evolution splits the series
instead of writing it, so this should not arise — but misreading it would move
appointments, and skipping it only loses an edit.

`shows_without_time` grew the matching conditions: an edited instance's own
start has to be midnight and its length whole days, or the DATE form cannot
hold it and the whole document goes out timed — the same trade already made for
an `UNTIL` or an `EXDATE` at 09:00.

Verified locally: `cargo test --locked` 541 (was 527: +14 net, 15 added and one
rewritten), the EDS-header crates green via the `rust-test-eds` set, `ctest`
14/14, `cargo fmt --check`, and `cargo clippy --all-targets --locked -D
warnings` clean for both crate sets. `reuse lint` and `cargo deny` not run
(neither is on this VM); no files were added, so no new SPDX headers were
needed, and no dependency changed.

Next in this area: a functional test for an edited occurrence, which is the
level this increment is missing. Then an `RDATE` of `VALUE=PERIOD` (legal, not
something Evolution writes) is read as its start and written back as a plain
date-time; a per-instance `timeZone` has no spelling; and `DTEND` is still the
only way Evolution states a length while `DURATION` is the only way we write
one, so a re-saved event patches `duration` to the equivalent spelling — which
now applies to an instance's duration too, where calcard also normalises
`PT60M` to `PT1H`, so an unrelated edit rewrites the spelling of a length that
did not change.

No milestone tag. Unchanged blockers: the calcard directive's two emitters are
still ours by choice, waiting on the fold off-by-one being fixed upstream or a
maintainer decision that 76-octet lines are acceptable; M9 has no CI job (needs
`evolution-data-server` + `dbus-daemon` in the CI image, a maintainer decision)
and no GUI tier (needs a display this VM lacks); M7 still **needs human
verification in real Evolution**; `docs/MILESTONES.md` does not exist yet, so
the M8 tag the last eight sessions asked for is still unwritten; the
manual-test recipes are unlinked from the README; `jmap-mail`'s rustdoc is
dirty; the once-seen `jmap-mail` `tests/transport.rs` hang is still unexplained.

## 2026-08-10 (hundred-and-thirty-seventh session)

The level the last session was missing: an edited occurrence driven through
real EDS. The mapping had tests for a detached instance at the component level
and `jmap-cal-sync` had them against `jmap-mockd`, but nothing said EDS would
ever *hand* the backend such a component — `ECalMetaBackend` decides what a
`E_CAL_OBJ_MOD_THIS` modify turns into, and that decision is exactly what no
test in this tree had ever exercised. So `tests/functional/cal-client.c` now
renames the second occurrence of the weekly series it already creates, with
`e_cal_client_modify_object_sync (…, E_CAL_OBJ_MOD_THIS, …)`, and
`calendar.rs` holds both ends to it: EDS kept the instance, and the server was
told about it.

**Red first, and this one went red three ways.** The `argc` check first (the
client refused the new argument), then the read-back, then the server. Two
assertions were added: `edited-occurrence-summary` off the client, and the
`recurrenceOverrides` map grown from one entry to two — asserted as one map
rather than two lookups, because an override written for the edited instance
that dropped the excluded one is a deletion undone and two separate
assertions would not see it.

**Neither assertion is vacuous, checked by mutation.** Making
`marshal::icalendar_from_instances` drop the non-master instances again fails
the run; so does making `jmap-ical`'s `modified_instances` render nothing. And
because the client-side assertions fire before the server-side ones, the
server assertion was checked on its own by temporarily relaxing them: with
marshal mutated, the mock holds `{"2026-01-29T13:00:00": {"excluded": true}}`
and the assertion names it. Both layers are live.

**Two things EDS does that the test now records.** `e_cal_client_get_object_sync`
with a UID and no RECURRENCE-ID answers with the *master alone* — not a
`VCALENDAR` holding the series and its exceptions, which is what the first
draft of this test assumed and why it failed against a cache that in fact held
the instance. The pair (UID, RECURRENCE-ID) is how `ECalCache` keys a detached
instance, so asking for that pair is the only question that distinguishes an
instance EDS kept from one it dropped. Second, and for the same reason,
`get_object_list "#t"` now returns **four** objects for three events: the
detached instance is a row of its own beside the series. The count assertion
moved from 3 to 4 with that written down — three would mean the edit never
landed, five would mean an occurrence became a second event.

The read-back also checks the component it got back actually carries a
`RECURRENCE-ID`, because an instance EDS expanded out of the `RRULE` would
carry the series' own summary and, if the backend had dropped the override,
could make this pass on a component that replaces nothing.

Verified locally: `cargo test --locked` 541 (unchanged — this increment adds no
Rust unit test; its coverage is a functional one), the EDS-header crates green
via the `rust-test-eds` set, `ctest` 14/14 including all four functional legs,
`cargo fmt --check`, and `cargo clippy --all-targets --locked -- -D warnings`
clean for the default set and for the EDS set plus `jmap-functional`. `reuse
lint` and `cargo deny` not run (neither is on this VM); no files were added, so
no new SPDX headers were needed, and no dependency changed.

Next in this area: no functional test yet deletes an occurrence through
`E_CAL_OBJ_MOD_THIS` on `remove_object` — the `EXDATE` case is created
directly rather than reached the way a user reaches it, so EDS's own
translation of "Delete this occurrence" is still untested here. Then an `RDATE`
of `VALUE=PERIOD` is read as its start and written back as a plain date-time; a
per-instance `timeZone` has no spelling; and `DTEND` is still the only way
Evolution states a length while `DURATION` is the only way we write one.

No milestone tag. Unchanged blockers: the calcard directive's two emitters are
still ours by choice, waiting on the fold off-by-one being fixed upstream or a
maintainer decision that 76-octet lines are acceptable; M9 has no CI job (needs
`evolution-data-server` + `dbus-daemon` in the CI image, a maintainer decision)
and no GUI tier (needs a display this VM lacks); M7 still **needs human
verification in real Evolution**; `docs/MILESTONES.md` does not exist yet, so
the M8 tag the last nine sessions asked for is still unwritten; the manual-test
recipes are unlinked from the README; `jmap-mail`'s rustdoc is dirty; the
once-seen `jmap-mail` `tests/transport.rs` hang is still unexplained.

## 2026-08-10 (hundred-and-thirty-eighth session)

The gap the last session named: nothing in this tree ever asked EDS to delete
an occurrence. The `EXDATE` the calendar functional test already covers is
written into the component *before* it is created, so it holds the mapping to
account and says nothing about Evolution — "Delete this occurrence" is
`e_cal_client_remove_object_sync` with a `RECURRENCE-ID` and
`E_CAL_OBJ_MOD_THIS`, and what `ECalMetaBackend` makes of that is the step
under test. So `tests/functional/cal-client.c` now removes the fourth
occurrence of the weekly series that way, and `calendar.rs` holds both ends to
it.

**What EDS does with it, now written down.** The removal never reaches the
backend's removal vfunc at all: `ECalMetaBackend` answers a `MOD_THIS` removal
by adding an `EXDATE` to the master and *saving* it, so from the backend's side
"delete this occurrence" and "edit this occurrence" are the same call with
different components. That is why the created-with-an-`EXDATE` case could not
stand in for this one — it exercises the same mapping but not EDS's decision to
route a removal through the save path, and a backend that got the routing wrong
would fail only here.

**Green on the first run, so both assertions were checked by mutation.** No
production code changed; this increment is coverage, and coverage that passes
immediately has to earn its place. Disabling the removal call in the client
fails `recurring-exdates` with `["20260129T130000Z"]` against the expected
pair; with that assertion then relaxed as well, the server-side one fails on
its own, naming the two overrides the mock holds against the three it should.
Both layers are live and neither is standing in for the other.

**Two assertions rather than one, for the reason the edited occurrence has
two.** The client-side one reads the master back out of EDS's cache and reports
every `EXDATE` on it, because `ECalMetaBackend` diffs the *next* save against
what it cached: an exclusion that reached the server but not the cache would be
undone by the following write, whatever the mock holds at this instant. The
server-side one grew from two overrides to three, asserted as one map for the
reason already recorded — an override written for one exception that dropped
another is a cancellation undone, and three separate lookups would not see it.
The removal is done *after* the edit deliberately, so the series already
carries an exception of each kind when it happens; a save that rebuilt
`recurrenceOverrides` from scratch would flatten exactly that state.

`exdate_values()` asks each property for its value as text rather than as a
time, which folds the two shapes libical may hold a list in — one property
carrying `a,b`, or two properties carrying one each — into the same string, so
what the client reports depends on which instants are excluded and not on how
they were spelled. The Rust side sorts before comparing: the order libical
hands two exclusions back is not what this test is about.

Verified locally: `cargo test --locked` 541 (unchanged — this increment's
coverage is a functional test, not a Rust unit one), the EDS-header crates
green via the `rust-test-eds` set, `ctest` 14/14 including all four functional
legs, `cargo fmt --check`, and `cargo clippy --all-targets --locked -- -D
warnings` clean for the default set and for the EDS set plus `jmap-functional`.
`reuse lint` and `cargo deny` not run (neither is on this VM); no files were
added, so no new SPDX headers were needed, and no dependency changed.

Next in this area: `E_CAL_OBJ_MOD_THISANDFUTURE` is the third thing that menu
offers and is still untouched at every level — the mapping skips a
`RANGE=THISANDFUTURE` `RECURRENCE-ID` on read (deliberately, logged three
sessions ago) and nothing says what EDS hands the backend when a user picks it,
which is worth finding out before deciding whether skipping is still the right
answer. Then an `RDATE` of `VALUE=PERIOD` is read as its start and written back
as a plain date-time; a per-instance `timeZone` has no spelling; and `DTEND` is
still the only way Evolution states a length while `DURATION` is the only way we
write one.

No milestone tag. Unchanged blockers: the calcard directive's two emitters are
still ours by choice, waiting on the fold off-by-one being fixed upstream or a
maintainer decision that 76-octet lines are acceptable; M9 has no CI job (needs
`evolution-data-server` + `dbus-daemon` in the CI image, a maintainer decision)
and no GUI tier (needs a display this VM lacks); M7 still **needs human
verification in real Evolution**; `docs/MILESTONES.md` does not exist yet, so
the M8 tag the last ten sessions asked for is still unwritten; the manual-test
recipes are unlinked from the README; `jmap-mail`'s rustdoc is dirty; the
once-seen `jmap-mail` `tests/transport.rs` hang is still unexplained.

## 2026-08-10 (hundred-and-thirty-ninth session)

The third thing that menu offers, and the one the last three sessions kept
naming: "Edit this and future occurrences", `E_CAL_OBJ_MOD_THIS_AND_FUTURE`.
`tests/functional/cal-client.c` now asks EDS for it on the fifth occurrence of
the weekly series it already builds, and `calendar.rs` holds both ends to what
came of it.

**What EDS does with it, read out of `e-cal-meta-backend.c` first and then
confirmed by the test.** It is not an exception to the series at all: EDS
*splits the series in two*. `e_cal_util_split_at_instance_ex` clones the
component, `e_cal_util_remove_instances_ex` truncates the master's rule to stop
before the named instance, the clone gets a UID from `e_util_generate_uid` and
is handed to the backend as an ordinary **create**. So this is the only one of
the three menu items that reaches the backend as two writes, and the only one
whose result is a second event rather than an entry in `recurrenceOverrides`.
`COUNT=6` came back as `COUNT=4` on the old series and `COUNT=2` on the new one
— EDS counts the occurrences before the split rather than converting to an
`UNTIL`, when the rule was stated as a count.

**Which settles the open question about `RANGE=THISANDFUTURE`.** `jmap-ical`'s
`read_overrides` skips a `RECURRENCE-ID` carrying that parameter, logged three
sessions ago as deliberate-but-unverified: `recurrenceOverrides` has no single
entry for "every instance from here on", so reading it as one would move one day
and silently drop the change to the rest. This test says the parameter never
arrives from EDS in the first place — Evolution's request is resolved into plain
components before the backend sees anything — so the skip costs nothing on the
path a user actually takes. It is still the right answer for a *document* that
carries one (an imported `.ics`, a `PUT` from elsewhere), and that is now the
only case it covers.

**Green on the first run, so every new assertion was checked by mutation.** No
production code changed; this increment is coverage, and coverage that passes
immediately has to earn its place.
- Disabling the modify call in the client: `series-rrule` comes back
  `FREQ=WEEKLY;COUNT=6` against the expected `COUNT=4`, and no series titled
  the split's summary is in the calendar at all.
- With the client-side assertions then relaxed as well, the server-side ones
  fail on their own: the mock holds three events against the four it should.
- And a production mutation — `jmap-ical` dropping `COUNT` on read — fails the
  *client-side* rule assertion with `FREQ=WEEKLY;UNTIL=20260212T125959Z`. That
  is worth writing down for its own sake: EDS's cached master is the component
  our mapping handed back, so the rule EDS truncates is the rule that survived
  the JSCalendar round trip. With `COUNT` lost, the series on the server recurs
  forever, and EDS then cuts it with an `UNTIL` instead. The client-side
  assertion is therefore not merely a check on EDS; it is a check on the round
  trip, and it fires before the server-side one for that reason.
- With that mutation and the client-side assertions relaxed, the server-side
  `count` assertion fails on its own too.

**Four assertions on the client side rather than one.** The truncated rule, the
new series' `DTSTART`, its rule, and that it carries no `EXDATE`. Either half of
a split passes for a split that went wrong in the other: a truncated master with
no new event is the fortnight the user renamed and lost, and a new event beside
an untruncated master is every one of those days twice, under two titles. The
absent `EXDATE` is its own assertion because both cancellations are before the
split, so an exclusion here is one EDS or the mapping moved onto days where the
user never cancelled anything. `events-after` went 4 → 5: `ECalCache` keys on
(uid, rid), so the four events plus the one detached instance are five rows.

`docs/functional-tests.md` listed only the first event the calendar test writes;
its "what the calendar test asserts" section now names the all-day case and all
three menu items, which four sessions of recurrence work had left out.

Verified locally: `cargo test --locked` 541 (unchanged — this increment's
coverage is a functional test, not a Rust unit one), the EDS-header crates green
via the `rust-test-eds` set, `ctest` 14/14 including all four functional legs,
`cargo fmt --check`, and `cargo clippy --all-targets --locked -- -D warnings`
clean for the default set and for the EDS set plus `jmap-functional`. `reuse
lint` and `cargo deny` not run (neither is on this VM); no files were added, so
no new SPDX headers were needed, and no dependency changed.

Next in this area: the three menu items are now all covered, so the remaining
recurrence gaps are the mapping's own — an `RDATE` of `VALUE=PERIOD` is read as
its start and written back as a plain date-time; a per-instance `timeZone` has
no spelling; and `DTEND` is still the only way Evolution states a length while
`DURATION` is the only way we write one. Worth considering before those: nothing
asserts what the *second* write of a split looks like on the wire — whether the
truncated master and the new event arrive as one `CalendarEvent/set` or two, and
whether a failure of the second leaves the first committed.

No milestone tag. Unchanged blockers: the calcard directive's two emitters are
still ours by choice, waiting on the fold off-by-one being fixed upstream or a
maintainer decision that 76-octet lines are acceptable; M9 has no CI job (needs
`evolution-data-server` + `dbus-daemon` in the CI image, a maintainer decision)
and no GUI tier (needs a display this VM lacks); M7 still **needs human
verification in real Evolution**; `docs/MILESTONES.md` does not exist yet, so
the M8 tag the last eleven sessions asked for is still unwritten; the
manual-test recipes are unlinked from the README; `jmap-mail`'s rustdoc is
dirty; the once-seen `jmap-mail` `tests/transport.rs` hang is still unexplained.

## 2026-08-10 (hundred-and-fortieth session)

The first of the three mapping gaps the last session left named: an `RDATE`
that states a **period** rather than an instant. RFC 5545 §3.8.5.2 allows
`RDATE;VALUE=PERIOD:20260205T130000Z/PT2H`, which is how iCalendar says "this
extra occurrence runs longer than the rest of the series". `read_overrides`
read only the part before the `/`, so the occurrence appeared — at the series'
length. A two-hour slot the document plainly described was shown, and saved
back, as the series' hour.

**What it maps onto.** `recurrenceOverrides` already has the vocabulary: the
entry's patch carries a `duration`. So a period becomes a `duration` patch, and
the rule for *when* is the one `instance_patch` already applies to a detached
`VEVENT` — only a length that differs from the series' is an override, so a
period restating the series' own length is the empty patch a bare `RDATE`
produces. That symmetry matters beyond tidiness: the write side puts an
override that says something into a `VEVENT` of its own, so the length leaves
as that component's `DURATION` and comes back through `instance_patch`. The
round trip is closed by two different code paths agreeing on the same rule.

**Both spellings of a period, answered the way `DTEND` and `DURATION` already
are.** `period_length` passes a stated duration through — the two formats spell
an ISO 8601 duration identically, which is exactly why `read_duration` passes
`DURATION` through — and measures a stated end on the wall clock via the
existing `instant`/`to_duration`. A period that ends at or before it starts
yields no length, and so does a negative duration, which falls out for free:
`-PT1H` is not a `P` and is not a date-time either, so it takes the measuring
branch and fails there. "No length" patches a `null` rather than nothing at
all — the instance keeps its length *removed* instead of quietly inheriting one
the document never gave it, which is the answer a detached `VEVENT` carrying
neither `DURATION` nor `DTEND` already gets.

**One divergence, tested rather than hidden.** A duration written as zero
(`.../PT0S`) is passed through as written, where a zero-length *range*
(`.../20260205T130000Z`) becomes the `null`. Catching it would mean parsing the
value instead of passing it through — `PT0S`, `P0D` and `PT0H0M0S` all spell
zero — and RFC 8984 §4.2.2 reads both answers as the same zero length, so the
two differ on paper and not in the calendar. There is an assertion pinning the
`PT0S` case so the divergence is on the record and cannot drift silently.

Four tests, three of them red first: the length arriving from either spelling,
the unreadable ones, and the write-back. The fourth — a period as long as the
series patching nothing — passed on the day it was written and guards the
comparison against the series' duration; without that filter it fails. The
`EXDATE` half of the loop was split out rather than folded into the new shape:
RFC 5545 §3.8.5.1 admits no period there, and an instance that does not happen
has no length to state. Its position after the `RDATE` loop is unchanged, which
is what keeps a document naming one instant both ways reading as excluded.

Verified locally: `cargo test --locked` 545 (up 4), the EDS-header crates green
via `ctest -R rust-test-eds`, the full `ctest` 14/14 including all four
functional legs against real EDS, `cargo fmt --check`, and `cargo clippy
--all-targets --locked -- -D warnings` clean. `reuse lint` and `cargo deny` not
run (neither is on this VM); no files were added, so no new SPDX headers were
needed, and no dependency changed.

Next in this area: a per-instance `timeZone` still has no spelling, and
`DTEND` is still the only way Evolution states a length while `DURATION` is the
only way we write one. Also still open from last session: nothing asserts
whether a split series' two writes arrive as one `CalendarEvent/set` or two,
and what a failure of the second leaves behind. Noticed while here and *not*
fixed, because it is a separate function on a separate path: `read_duration`
passes a negative `DURATION` straight through to the server, where RFC 8984
§1.4.6 has no negative duration — the new code refuses one, the old one does
not.

No milestone tag. Unchanged blockers: the calcard directive's two emitters are
still ours by choice, waiting on the fold off-by-one being fixed upstream or a
maintainer decision that 76-octet lines are acceptable; M9 has no CI job (needs
`evolution-data-server` + `dbus-daemon` in the CI image, a maintainer decision)
and no GUI tier (needs a display this VM lacks); M7 still **needs human
verification in real Evolution**; `docs/MILESTONES.md` does not exist yet, so
the M8 tag the last twelve sessions asked for is still unwritten; the
manual-test recipes are unlinked from the README; `jmap-mail`'s rustdoc is
dirty; the once-seen `jmap-mail` `tests/transport.rs` hang is still unexplained.

## 2026-08-10 (hundred-and-forty-first session)

The thing the last session noticed while working next door and deliberately did
not fix, because it lived on a different path: `read_duration` handed a
`DURATION` value straight to the server. RFC 5545 §3.3.6 spells a length with a
sign, RFC 8984 §1.4.6 has none, so a component saying `DURATION:-PT1H` became a
JSCalendar `duration` of minus an hour. That is not a value a server accepts,
and the `CalendarEvent/set` it rides in fails whole — so the save that carried
it takes the user's *real* edits down with it. The same passthrough let
anything through: `next tuesday`, `3600`, a bare `P`.

**Checked rather than parsed.** `stated_duration` decides whether a value is a
length and, if it is, hands it over exactly as written — the two formats spell
an ISO 8601 duration identically, and re-rendering would mean owning a
normalisation nobody asked for. A leading `+` is the one thing dropped, since
it is RFC 5545's way of saying the same length and RFC 8984 has nowhere to put
it. A value that fails is treated as absent like every other unreadable one,
which is not a new rule but the module's oldest: the caller falls through to
the `DTEND` branch, so a malformed component that states its length twice is
read from the half that works, and one that states it only badly ends up
without a length rather than with an unusable one.

**Looser than RFC 5545 on purpose, and the loosening is tested.** The RFC nests
its units — an hour may be followed only by minutes, a week stands alone — so
`PT1H15S` and `P1W2D` are strictly invalid, while every reader adds them up the
same way and some emitters write them. Refusing a length an event plainly
states is the failure this check exists to *avoid*; it is here to refuse values
that are not lengths. So the accepted grammar is `W D H M S`, each at most
once, in that order, at least one of them, with `T` before the first time unit.

**What the check actually sees is calcard's answer, not the octets.** Probing
first rather than assuming was worth it: three values that look malformed never
reach the check as written — `P1DT` arrives trimmed to `P1D`, `PTH` as `PT0S`,
and `PT30M1H` with its units put back in order. They are in the *accepted*
test's table with that noted, so the boundary is on the record and a future
calcard that stops repairing them shows up as a red test rather than as a
silently different reading.

`period_length`'s duration half went through the same door: it used to return
anything beginning with the designator, so an `RDATE;VALUE=PERIOD` ending in
`/PT` gave the occurrence a `duration` of `PT`. The negative case there still
falls out of the measuring branch for free, unchanged. The documented `PT0S`
divergence is untouched — zero is a length, and it passes.

Three tests red first (the refused values, the `DTEND` fallthrough, the two new
period halves) plus one that passed on the day it was written, pinning the
values that must keep crossing. That last one is the one that matters: the
change can only regress by refusing too much, and nothing else in the suite
would notice.

Deliberately *not* done: the mirror on the write side. A `duration` the server
sends is still put into `DURATION` verbatim, so a server sending a value this
mapping would now refuse to read writes iCalendar libical may reject. It is the
same rule and one line — but the save path diffs against a re-rendering of what
the server holds, so dropping the property there would make the next save patch
`duration: null` over the server's value without the user touching anything.
That needs deciding, not just coding.

Verified locally: `cargo test --locked` 548 (up 3), `ctest` 14/14 including
`rust-test-eds` and all four functional legs against real EDS, `cargo fmt
--check`, and `cargo clippy --all-targets --locked -- -D warnings` clean for the
default set and for the EDS crates this touches (`jmap-ical`, `jmap-cal-sync`,
`jmap-backend-cal`, `jmap-functional`). `reuse lint` and `cargo deny` not run
(neither is on this VM); no files were added, so no new SPDX headers were
needed, and no dependency changed.

Next in this area: the write-side mirror above, and the two mapping gaps still
named from before — a per-instance `timeZone` has no spelling, and nothing
asserts whether a split series' two writes arrive as one `CalendarEvent/set` or
two, nor what a failure of the second leaves behind.

No milestone tag. Unchanged blockers: the calcard directive's two emitters are
still ours by choice, waiting on the fold off-by-one being fixed upstream or a
maintainer decision that 76-octet lines are acceptable; M9 has no CI job (needs
`evolution-data-server` + `dbus-daemon` in the CI image, a maintainer decision)
and no GUI tier (needs a display this VM lacks); M7 still **needs human
verification in real Evolution**; `docs/MILESTONES.md` does not exist yet, so
the M8 tag the last thirteen sessions asked for is still unwritten; the
manual-test recipes are unlinked from the README; `jmap-mail`'s rustdoc is
dirty; the once-seen `jmap-mail` `tests/transport.rs` hang is still unexplained.
