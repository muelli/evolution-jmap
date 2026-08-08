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
