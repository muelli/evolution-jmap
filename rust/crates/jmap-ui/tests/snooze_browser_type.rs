// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later
//
// Tests the type of the snooze extension of the message window: its parent,
// and the browser that it extends.

mod common;

use jmap_ui::snooze::browser_ext::JmapSnoozeBrowserExtension;

#[test]
fn the_snooze_extension_is_an_extension_of_the_message_window() {
    common::assert_extends::<JmapSnoozeBrowserExtension>(
        // SAFETY: no arguments, and the type initialises itself.
        || unsafe { evo_sys::e_mail_browser_get_type() },
        "EMailBrowser",
    );
}
