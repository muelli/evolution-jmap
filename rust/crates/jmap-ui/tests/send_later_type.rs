// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later
//
// Tests the type of the send-later extension: its parent, and the extensible
// type that it extends (the message composer).

mod common;

use jmap_ui::send_later::extension::JmapSendLaterExtension;

#[test]
fn the_send_later_extension_is_an_extension_of_the_composer() {
    common::assert_extends::<JmapSendLaterExtension>(
        // SAFETY: no arguments, and the type initialises itself.
        || unsafe { evo_sys::e_msg_composer_get_type() },
        "EMsgComposer",
    );
}
