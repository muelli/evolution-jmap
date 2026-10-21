// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later
//
// Tests the type of the vacation extension: its parent, and the extensible
// type that it extends (the account editor's notebook).

mod common;

use jmap_ui::vacation::extension::JmapVacationExtension;

#[test]
fn the_vacation_extension_is_an_extension_of_the_notebook() {
    common::assert_extends::<JmapVacationExtension>(
        // SAFETY: no arguments, and the type initialises itself.
        || unsafe { evo_sys::e_mail_config_notebook_get_type() },
        "EMailConfigNotebook",
    );
}
