<!--
SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
SPDX-License-Identifier: GPL-3.0-or-later
-->

# Missed mutants in `po-compile`

Batch 13 expects each missed mutant to have either a killing test or a one-line reason.
This lane is comment-only in `rust/crates/po-compile`, so this file records reasons.

- `crates/po-compile/src/lib.rs:37:9: replace <impl fmt::Display for Error>::fmt -> fmt::Result with Ok(Default::default())`: not equivalent, because this drops all user-facing error text; tests currently assert the error variant but not the `Display` message.
- `crates/po-compile/src/lib.rs:43:43: replace match guard declared.is_empty() with true in <impl fmt::Display for Error>::fmt`: not equivalent, because this forces the empty-charset wording for non-empty charset values.
- `crates/po-compile/src/lib.rs:43:43: replace match guard declared.is_empty() with false in <impl fmt::Display for Error>::fmt`: not equivalent, because this forces the non-empty charset wording for empty charset values.
- `crates/po-compile/src/lib.rs:237:55: replace || with && in unquote`: not equivalent, because this can accept malformed quoted fields that should fail with `Error::Unparsed`.
- `crates/po-compile/src/lib.rs:237:26: replace || with && in unquote`: not equivalent, because this can accept malformed quoted fields that should fail with `Error::Unparsed`.
- `crates/po-compile/src/lib.rs:305:18: replace + with * in record`: not equivalent, because this corrupts `.mo` offset accounting and can misreport `Error::TooLarge` boundary handling.
- `crates/po-compile/src/bin/po-compile.rs:9:5: replace main -> ExitCode with Default::default()`: not equivalent, because this forces CLI success exit even when compilation fails.
