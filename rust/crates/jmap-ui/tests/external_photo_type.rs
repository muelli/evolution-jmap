// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later
//
// Tests the type of the external photo extension: its parent, and the web
// view class it extends.
//
// The extensible is `EWebView`, the public parent of the address book
// preview, because Evolution installs no header for `EABContactDisplay`.
// `constructed` narrows to the preview by name. See the module docs.

mod common;

use jmap_ui::external_photo::display_ext::JmapExternalPhotoExtension;

#[test]
fn the_external_photo_extension_is_an_extension_of_the_web_view() {
    common::assert_extends::<JmapExternalPhotoExtension>(
        // SAFETY: no arguments, and the type initialises itself.
        || unsafe { evo_sys::e_web_view_get_type() },
        "EWebView",
    );
}
