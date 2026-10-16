<!--
SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
SPDX-License-Identifier: GPL-3.0-or-later
-->

# Equivalent mutants in `jmap-vcard`

- `crates/jmap-vcard/src/contact.rs:1296:23: replace > with >= in fold_overlong_lines`: equivalent because `str::is_char_boundary(0)` is always true, so `cut > 0` and `cut >= 0` terminate the loop at the same point.
