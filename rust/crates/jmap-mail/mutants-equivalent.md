<!--
SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
SPDX-License-Identifier: GPL-3.0-or-later
-->

# Genuinely Equivalent Mutants in `jmap-mail`

## `src/folder_info.rs`

- `crates/jmap-mail/src/folder_info.rs:183:60`, `184:62`, `185:58`, `186:60`,
  `187:58`, `188:64`: replace `|` with `^` in `flags`, one per
  `CAMEL_FOLDER_TYPE_* | CAMEL_FOLDER_SYSTEM` match arm. `CAMEL_FOLDER_SYSTEM`
  is `1 << 6` and every `CAMEL_FOLDER_TYPE_*` value is `n << CAMEL_FOLDER_TYPE_BIT`
  with `CAMEL_FOLDER_TYPE_BIT == 10` (`camel-enums.h`), so the two operands
  never share a set bit. For disjoint operands `|` and `^` are the same
  operation, and `flags` starts the match fresh each call (not OR-ed onto a
  pre-existing value), so the two forms compute identical results for every
  role.
