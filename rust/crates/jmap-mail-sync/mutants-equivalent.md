<!--
SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
SPDX-License-Identifier: GPL-3.0-or-later
-->

# Genuinely Equivalent Mutants in `jmap-mail-sync`

## `src/folder.rs`

- `crates/jmap-mail-sync/src/folder.rs:475:69`: replace `!=` with `==` in
  `FolderTree::walk`, the `children[deep_parent].retain(...)` of the
  `MAX_DEPTH` cut. A mailbox deeper than `MAX_DEPTH` can only have been pushed
  by its parent's visit (roots and cut orphans are pushed at depth 1), so
  `deep_parent` is already visited and its child list has already been pushed.
  `walk` never reads a visited node's `children` again, and nothing after
  `walk` reads `children` at all (`paths` and `assemble` follow `parent`), so
  whatever the retain leaves behind is never observed.
- `crates/jmap-mail-sync/src/folder.rs:493:26`: replace `+` with `*` in
  `FolderTree::walk`, `cut = orphan + 1` becoming `cut = orphan`. `cut` is only
  where the next scan for an unvisited mailbox starts, and `orphan` is visited
  by the time that scan runs, so starting one index earlier finds the same
  mailbox. Only the scan's length changes.
