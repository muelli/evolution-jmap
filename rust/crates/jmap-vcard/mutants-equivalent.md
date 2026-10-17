<!--
SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
SPDX-License-Identifier: GPL-3.0-or-later
-->

# Equivalent mutants in `jmap-vcard`

- `crates/jmap-vcard/src/contact.rs:1296:23: replace > with >= in fold_overlong_lines`: equivalent because `str::is_char_boundary(0)` is always true, so `cut > 0` and `cut >= 0` terminate the loop at the same point.
- `crates/jmap-vcard/src/contact.rs:1491:21: delete field extra from struct Nickname expression in vcard_to_card`: equivalent because `Nickname::default()` already initializes `extra` to an empty map, so removing the explicit `extra: BTreeMap::new()` does not change the resulting value.
