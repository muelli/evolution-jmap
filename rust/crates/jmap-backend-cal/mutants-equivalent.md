<!--
SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
SPDX-License-Identifier: GPL-3.0-or-later
-->

# Genuinely Equivalent Mutants in `jmap-backend-cal`

## `src/connect.rs`

- Zero missed mutants (1 unviable mutant: replacing `open_calendar` with `Ok(Default::default())`, as `CalSync` does not implement `Default`).

## `src/factory.rs`

- Zero missed mutants (5 of 5 viable mutants caught by test suites).
- 3 previously missed mutants killed by `tests/mutation_factory_marshal_ops.rs`:
  - `remember_backend_type` replaced with `()`
  - `class_init_types` returning `vec![]`
  - `class_init_types` returning `vec![Default::default()]`

## `src/marshal.rs`

- Zero missed mutants (9 of 9 viable mutants caught by test suites).

## `src/ops.rs`

- Zero missed mutants (8 of 8 viable mutants caught by test suites, 1 unviable).
- 1 unviable mutant identified:
  - `replace to_gerror -> *mut GError with Default::default()`, as null pointer fails compilation.

## `src/backend.rs`

- Zero missed mutants (25 of 25 viable mutants caught by test suites, 28 unviable).
- 8 previously missed mutants killed by `tests/mutation_backend.rs`:
  - `replace JmapCalBackend::inspect_connection -> Option<R> with None`
  - `replace JmapCalBackend::last_known_color -> Option<&RwLock<Option<String>>> with None`
  - `replace JmapCalBackend::last_known_color -> Option<&RwLock<Option<String>>> with Some(Box::leak(Box::new(RwLock::new(None))))`
  - `replace JmapCalBackend::last_known_color -> Option<&RwLock<Option<String>>> with Some(Box::leak(Box::new(RwLock::from(None))))`
  - `replace JmapCalBackend::last_known_color -> Option<&RwLock<Option<String>>> with Some(Box::leak(Box::new(RwLock::new(Some(String::new())))))`
  - `replace JmapCalBackend::last_known_color -> Option<&RwLock<Option<String>>> with Some(Box::leak(Box::new(RwLock::from(Some(String::new())))))`
  - `replace JmapCalBackend::last_known_color -> Option<&RwLock<Option<String>>> with Some(Box::leak(Box::new(RwLock::new(Some("xyzzy".into())))))`
  - `replace JmapCalBackend::last_known_color -> Option<&RwLock<Option<String>>> with Some(Box::leak(Box::new(RwLock::from(Some("xyzzy".into())))))`
- 28 unviable mutants identified:
  - absence of `Default` implementations on `JmapCalBackend` (in `detached`), generic `R` (in `inspect_connection`), and `ECalMetaBackendClass` (in `parent_class`).
  - let-chain syntax errors with `||` in `refresh_push_headers`.
  - missing constructors and `Default` implementations on guard types `RwLockReadGuard`, `RwLockWriteGuard`, and `MutexGuard` (in `read`, `write`, `lock`).
  - unviable static allocations for `session` return values (`CalSync` does not implement `Default`).

## `src/scheduling.rs`

- Zero missed mutants (9 of 9 viable mutants caught by test suites).
