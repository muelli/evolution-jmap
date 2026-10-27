<!--
SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
SPDX-License-Identifier: GPL-3.0-or-later
-->

# Equivalent or out-of-scope mutants in `jmap-backend-collection`

## `src/backend.rs`

The `Live` struct (`struct Live(*mut ECollectionBackend)`) and its `Populating`
and `Collection` trait impls are, by the module's own doc comment, "the part
of both that cannot be tested on a machine with no `evolution-source-registry`":
every method is a single call into an EDS C function on a real
`ECollectionBackend`, which needs a running registry on the session bus that
neither this VM nor CI has. Per item 106's own rule, this FFI glue is in scope
only where `jmap-functional` can kill the mutant; `jmap-functional`'s own
tests need the `cmake`/`ctest` harness (`JMAP_FUNCTIONAL_COLLECTION_CLIENT`),
not a plain `cargo mutants` run, so these are out of scope for this crate's
mutation-testing session:

- `Live::export` replaced with `()` (line 1079).
- `Populating::freeze` replaced with `true`/`false`, and its `!=` to `==`
  (line 1135).
- `Populating::thaw` replaced with `()` (line 1140).
- `Populating::chain_up` replaced with `()` (line 1144).
- `Populating::claim_all_resources` replaced with `vec![]` or
  `vec![Default::default()]` (line 1157).
- `Populating::publish` replaced with `()` (line 1161).
- `Populating::request_credentials` replaced with `()` (line 1170).
- `Populating::authenticate_anonymously` replaced with `()` (line 1186).
- `Populating::offer_creation` replaced with `()` (line 1206).
- `Populating::allow_rename` replaced with `()` (line 1224).
- `Collection::new_child` replaced with `Default::default()` (line 1248).
- `Collection::is_new_child` replaced with `true`/`false`, and its `!=` to
  `==` (line 1259).
- `Collection::publish` replaced with `()` (line 1263).
- `Collection::existing_children` replaced with `vec![]` or
  `vec![Default::default()]` (line 1273).
- `Collection::set_remote_deletable` replaced with `()` (line 1285).

`debug_print_eds` replaced with `()` (line 1327): the shared helper that
hands a message to `e_source_registry_debug_print`. Every caller
(`debug_print`, `debug_print_for_resource`, `debug_print_for_account`) also
emits a `tracing` event of its own, already checked by
`debug_print_tests`, so a unit test sees no difference when only the EDS
debug-channel write is skipped. Observing the skip needs
`SOURCE_REGISTRY_DEBUG` and a live EDS debug log, the same live-EDS-only
territory as `Live`.

`LiveChild` (the handle a [`deferred_delete`](deferred_delete.rs) `watch`
thread holds for a child pending delete) and its `Drop` and `PendingChild`
impls are the same live-EDS-only territory, found on a 2026-10-27 re-sweep
(line numbers shifted the rest of this section above; these methods are new
since the file's 2026-10-18 sweep, added together with the deferred-delete
feature):

- `Drop::drop` replaced with `()` (line 893): the two `g_object_unref` calls
  it makes have no observable effect without a live GObject to check the
  refcount of.
- `PendingChild::uid` replaced with `String::new()` or `"xyzzy".into()`
  (line 905): a single call to `e_source_get_uid` on a live `ESource`.
- `PendingChild::present` replaced with `true`/`false`, and its `!=` to `==`
  (line 910): a single call to `e_server_side_source_get_exported`.
- `PendingChild::enabled` replaced with `true`/`false`, and its `!=` to `==`
  (line 915): a single call to `e_source_get_enabled`.
- `PendingChild::due` replaced with `None` (line 920): a single call to
  `jmap_config::pending_delete::read` on a live source.
- `PendingChild::clear_due` replaced with `()` (line 925): a single call to
  `jmap_config::pending_delete::write`.
- `PendingChild::enable` replaced with `()` (line 930): a single call to
  `e_source_set_enabled`.
- `PendingChild::destroy_on_server` replaced with `Ok(())` (line 935): needs
  a live `self.child` and `self.account` to resolve `doomed_of` and
  `login_for` before any server call.
- `PendingChild::remove` replaced with `Ok(())` (line 955): a single call to
  `remove_source` on a live child source.

## `src/child_added.rs`

`cargo mutants` makes no mutants for this file, because each function is an
`unsafe fn`. The mutants below were applied by hand. Each one has no effect
that a reader of the child can see, or is not compiled on this EDS:

- In `follow_collection`, the calls of `e_source_authentication_get_type` and
  `e_source_security_get_type` removed (lines 128 and 129). EDS's
  `e_source_class_init` calls `g_type_ensure` on both types. Thus each live
  `ESource` that this function gets proves that both types are registered.
- In `BOUND`, `secure` replaced with `method` (line 92). `set_method` notifies
  both properties, and `secure` is `method != "none"`. Thus the child gets the
  same `secure` value. Only the spelling of its `method` changes, and no JMAP
  backend reads that spelling.
- The `credential-store-id` value of `CREDENTIAL_PROPERTY` replaced with
  `credential-name` (line 100). This line is compiled only on an EDS with
  `credential-store-id` (3.52 does not have it). There,
  `the_credential_name_follows_too` writes `credential-store-id` and kills the
  mutant. The other arm (line 102) is killed on 3.52 by the same test.

## `src/populate.rs`

`cargo mutants` skips the body of every `unsafe fn` by default (mutating
unsafe code can produce undefined behaviour instead of a clean test
failure), and `populate` itself is `pub unsafe fn populate<P: Populating +
?Sized>(...)`. So the one function that holds this file's whole decision
tree (which `Asked` variant, whether the account is `creatable`, the
unidentified-source count) never offers cargo-mutants a single mutant: a
`--list` run against this file finds only one candidate, inside the
non-unsafe `Drop for Frozen`.

This is a tool limitation, not a test gap: `tests/populate.rs` already
drives every branch of `populate` through a mock `Populating` (oauth2
silent auth, mail-only switched on, no-parts, both creatable directions,
named user versus anonymous, one unidentified source, the freeze-lost and
panic-during-populate paths). The single mutant cargo-mutants did find
(`Frozen::drop` replaced with `()`) was already caught by
`the_freeze_is_given_back_even_when_the_work_panics` and its neighbours;
no new test was needed.

## `src/removal.rs`

`cargo mutants` makes no mutants for this file, because each function is an
`unsafe fn`. The mutants below were applied by hand. Two cannot occur with a
real EDS, and a third is a leak that only LeakSanitizer finds:

- In `remove_source`, the `g_error_free` on the success path removed (line
  162). EDS sets no `GError` when `e_source_remove_sync` returns `TRUE`, so
  this code runs only for a defective callee.
- In `take_message`, the fallback for a `GError` with a NULL message replaced
  with an empty string (line 216). `g_set_error` always sets a message, so no
  EDS refusal has a NULL message.
- In `take_message`, the `g_error_free` removed (line 214). No `cargo test`
  can see the leak. `ci/leak-check.sh` finds it: the `removal` test binary
  exits with code 23 under this mutant and with code 0 without it.

## `src/mail_child.rs`

`cargo mutants` makes no mutants for this file, because each function is an
`unsafe fn`. The mutants below were applied by hand. Both are equivalent,
because the class initialisation of `ESource` registers each built-in source
extension type (checked with EDS 3.52.3). Both functions get a live
`ESource`, so that class initialisation is complete before they run:

- In `mail_service_of`, the calls to `e_source_mail_account_get_type` and
  `e_source_mail_transport_get_type` removed (lines 163 and 164).
- In `follow_server`, the calls to `e_source_authentication_get_type` and
  `e_source_security_get_type` removed (lines 202 and 203).

## `src/deferred_delete.rs`

- `start`'s wait loop, line 235: `if now < end` replaced with `if now <= end`.
  `end` is a monotonic `Instant` fixed at the first wait of a window; the two
  forms differ only at the exact instant `now == end`. There, the real code
  proceeds to destroy and the mutant sleeps for `end - now` (zero), then
  loops back and proceeds on the next check anyway. The only effect is one
  extra loop iteration with a zero-length sleep: no missed destroy, no extra
  delay a user or test could observe. Hitting the exact tie between two
  `Instant::now()` reads is not reproducible without a mock clock, which this
  crate does not have.

Three mutants of this file time out (20s) rather than resolve, because the
`watch` thread they land in starts sleeping against the real wall clock
inside a scenario that normally finishes at once:
- Line 102, `check`'s leading `!` deleted (`if child.present()`): a thread
  watching a child whose source later disappears normally sees `present()`
  false and returns `Settled::Gone` at once; under the mutant it instead
  falls into `decide`'s wait branch and sleeps out the whole undo window.
- Line 208, `start`'s body replaced with `None`: no thread is ever spawned,
  so a test that joins the handle it expects back waits forever.
- Line 226, `!=` replaced with `==` in the restart check: `restarts` starts
  at 0 and so does the value the map holds, so the condition is true on
  every very first loop iteration regardless of the real intent (a restart
  happening). The thread resets `end` to `None` every pass, pins a fresh
  `now + left` on each loop, and so the wait condition never turns due: a
  genuine infinite loop while the test's 20s timeout runs out.
No test is weakened or skipped for these three: a 20s hang under a known
bad mutation is itself evidence the real code's behaviour differs, cargo-
mutants simply has no way to turn that into a pass/fail inside its test
timeout.
