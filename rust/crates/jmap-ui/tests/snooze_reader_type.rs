// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later
//
// Tests the type of the snooze extension: its parent, and the mail reader
// interface that it extends. The extensible is an interface, not a class, so
// one extension covers the mail window and the detached message window.

mod common;

use jmap_ui::snooze::reader_ext::JmapSnoozeReaderExtension;

#[test]
fn the_snooze_extension_is_an_extension_of_the_mail_reader() {
    common::assert_extends::<JmapSnoozeReaderExtension>(
        // SAFETY: no arguments, and the type initialises itself.
        || unsafe { evo_sys::e_mail_reader_get_type() },
        "EMailReader",
    );
}
