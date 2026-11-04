// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later
//
// Tests the type of the mail vCard photo extension: its parent, and the web
// view class it extends.
//
// The extensible is `EWebView`, the same public parent the address book
// placeholder names (see `external_photo_type.rs`), because this extension
// also must attach before Evolution constructs the mail reader's specific
// subclass. `constructed` narrows to that subclass (`EMailDisplay`) itself.
// See the module docs.

mod common;

use jmap_ui::external_photo::mail_display_ext::JmapMailVcardPhotoExtension;

#[test]
fn the_mail_vcard_photo_extension_is_an_extension_of_the_web_view() {
    common::assert_extends::<JmapMailVcardPhotoExtension>(
        // SAFETY: no arguments, and the type initialises itself.
        || unsafe { evo_sys::e_web_view_get_type() },
        "EWebView",
    );
}
