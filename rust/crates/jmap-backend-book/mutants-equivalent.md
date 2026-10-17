<!--
SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
SPDX-License-Identifier: GPL-3.0-or-later
-->

# Genuinely Equivalent Mutants in `jmap-backend-book`

## `src/connect.rs`

- Zero missed mutants (1 unviable mutant: replacing `open_book` with `Ok(Default::default())`, as `BookSync` does not implement `Default`).

## `src/factory.rs`

- Zero missed mutants (5 of 5 viable mutants caught by test suites).
- 3 previously missed mutants killed by `tests/mutation_factory_marshal_ops.rs`:
  - `remember_backend_type` replaced with `()`
  - `class_init_types` returning `vec![]`
  - `class_init_types` returning `vec![Default::default()]`

## `src/marshal.rs`

- Zero missed mutants (6 of 6 viable mutants caught by test suites).

## `src/ops.rs`

- Zero missed mutants (1 of 1 viable mutants caught by test suites).

## `src/backend.rs`

- Zero missed mutants (18 of 18 viable mutants caught by test suites, 30 unviable).
- 1 previously missed mutant killed by `tests/mutation_backend.rs`:
  - `replace JmapBookBackend::inspect_connection -> Option<R> with None`
- 30 unviable mutants identified:
  - absence of `Default` implementations on `JmapBookBackend` (in `detached`), generic `R` (in `inspect_connection`), and `EBookMetaBackendClass` (in `parent_class`).
  - let-chain syntax errors with `||` in `refresh_push_headers`.
  - missing constructors and `Default` implementations on guard types `RwLockReadGuard`, `RwLockWriteGuard`, and `MutexGuard` (in `read`, `write`, `lock`).
  - unviable static allocations for `session` return values.
