<!--
SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
SPDX-License-Identifier: GPL-3.0-or-later
-->

# Genuinely Equivalent Mutants in `jmap-proto`

## `src/id.rs`

- Zero missed mutants (9 of 9 generated mutants caught by test suites).

## `src/state.rs`

- Zero missed mutants (14 of 14 generated mutants caught by test suites).

## `src/blob.rs`

- Zero missed mutants (28 of 28 viable mutants caught by test suites, 21 unviable due to types without Default).

## `src/error.rs`

- Zero missed mutants (5 unviable mutants generated due to types without Default: RequestError, MethodError, and SetError).

## `src/metadata.rs`

- Zero missed mutants (15 of 15 viable mutants caught by test suites, 1 unviable due to MetadataTextFilter without Default).

## `src/quota.rs`

- Zero missed mutants (11 of 11 generated mutants caught by test suites).

## `src/push.rs`

- Zero missed mutants (12 of 12 viable mutants caught by test suites, 6 unviable due to types without Default: PushVerification and StateChange).
