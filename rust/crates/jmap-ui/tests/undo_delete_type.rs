// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later
//
// Tests the type of the info bar extension: its parent, and the shell view that
// it extends.

mod common;

use jmap_ui::undo_delete::shell_ext::JmapUndoDeleteShellExtension;

#[test]
fn the_undo_extension_is_an_extension_of_the_shell_view() {
    common::assert_extends::<JmapUndoDeleteShellExtension>(
        // SAFETY: no arguments, and the type initialises itself.
        || unsafe { evo_sys::e_shell_view_get_type() },
        "EShellView",
    );
}
