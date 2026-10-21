<!--
SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
SPDX-License-Identifier: GPL-3.0-or-later
-->

# Equivalent or out-of-scope mutants in `jmap-ui`

## `src/undo_delete/shell_ext.rs`

`<impl Drop for UndoData>::drop` survives replaced with `()`. The body releases
the references to the source and the shell content that `show` took. Under the
mutant, each closed info bar leaks these two objects. Only `show` makes an
`UndoData`, and `show` needs a live `EShellView` with its shell content. No test
in this crate makes a widget (see `evo-sys/tests/gtk.rs`), and `jmap-functional`
runs EDS without Evolution. Thus no test here can see the leak. Out of scope.
