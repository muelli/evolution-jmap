// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later
//
// Tests the type of the server filters extension: its parent, and the
// extensible type that it extends (the account editor's notebook).

mod common;

use jmap_ui::filters::extension::JmapFiltersExtension;

#[test]
fn the_filters_extension_is_an_extension_of_the_notebook() {
    common::assert_extends::<JmapFiltersExtension>(
        // SAFETY: no arguments, and the type initialises itself.
        || unsafe { evo_sys::e_mail_config_notebook_get_type() },
        "EMailConfigNotebook",
    );
}
