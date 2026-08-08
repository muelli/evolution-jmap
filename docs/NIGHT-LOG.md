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
