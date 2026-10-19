<!--
SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
SPDX-License-Identifier: GPL-3.0-or-later
-->

# Equivalent or out-of-scope mutants in `jmap-config`

## `src/defaults.rs`

`from_identity`'s body survives replaced with `Default::default()` only as
an `Unviable` mutant: `Account` has no `Default` impl, so it does not
compile. No action needed.
