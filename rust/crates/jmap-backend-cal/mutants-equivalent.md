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
