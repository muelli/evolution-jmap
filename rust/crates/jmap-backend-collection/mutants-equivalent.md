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

- `Live::export` replaced with `()` (line 1007).
- `Populating::freeze` replaced with `true`/`false`, and its `!=` to `==`
  (line 1062).
- `Populating::thaw` replaced with `()` (line 1067).
- `Populating::chain_up` replaced with `()` (line 1071).
- `Populating::claim_all_resources` replaced with `vec![]` or
  `vec![Default::default()]` (line 1084).
- `Populating::publish` replaced with `()` (line 1088).
- `Populating::request_credentials` replaced with `()` (line 1099).
- `Populating::authenticate_anonymously` replaced with `()` (line 1115).
- `Populating::offer_creation` replaced with `()` (line 1136).
- `Populating::allow_rename` replaced with `()` (line 1154).
- `Collection::new_child` replaced with `Default::default()` (line 1178).
- `Collection::is_new_child` replaced with `true`/`false`, and its `!=` to
  `==` (line 1189).
- `Collection::publish` replaced with `()` (line 1193).
- `Collection::existing_children` replaced with `vec![]` or
  `vec![Default::default()]` (line 1203).
- `Collection::set_remote_deletable` replaced with `()` (line 1215).

`debug_print_eds` replaced with `()` (line 1257): the shared helper that
hands a message to `e_source_registry_debug_print`. Every caller
(`debug_print`, `debug_print_for_resource`, `debug_print_for_account`) also
emits a `tracing` event of its own, already checked by
`debug_print_tests`, so a unit test sees no difference when only the EDS
debug-channel write is skipped. Observing the skip needs
`SOURCE_REGISTRY_DEBUG` and a live EDS debug log, the same live-EDS-only
territory as `Live`.

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
