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
