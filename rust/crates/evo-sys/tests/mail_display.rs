// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later
//
// The mail reader's own `EWebView` subclass, which jmap-ui's mail vCard
// placeholder extension gates on. Unlike `EABContactDisplay`, Evolution
// installs a header for this one, so the gate calls `e_mail_display_get_type`
// directly instead of a name lookup. The same two facts as `tests/web_view.rs`
// must still hold for that gate to work.

use evo_sys::*;
use std::ffi::CStr;

/// The type accessor answers, and answers under the name the extension's
/// `g_type_check_instance_is_a` call checks against.
#[test]
fn the_mail_display_is_registered_as_e_mail_display() {
    // SAFETY: no arguments, and the type initialises itself.
    let gtype = unsafe { e_mail_display_get_type() };
    assert_ne!(
        gtype, 0,
        "e_mail_display_get_type() answered the invalid type"
    );
    // SAFETY: `gtype` is registered, as the code just asserted. The name
    // belongs to the type system and outlives the call.
    let name = unsafe { CStr::from_ptr(g_type_name(gtype)) };
    assert_eq!(
        name, c"EMailDisplay",
        "the mail display is no longer registered as EMailDisplay"
    );
}

/// The extension attaches to every `EWebView`, then gates on this check. A
/// mail display that stopped being one would make the gate always false.
#[test]
fn the_mail_display_is_a_web_view() {
    // SAFETY: neither call takes arguments, and each type initialises itself.
    unsafe {
        assert_ne!(
            g_type_is_a(e_mail_display_get_type(), e_web_view_get_type()),
            0,
            "EMailDisplay is no longer an EWebView"
        );
    }
}
