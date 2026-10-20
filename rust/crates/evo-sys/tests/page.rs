// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later
//
// The one thing M7's `setup_defaults` asks of the page it extends: the
// address the user typed on the assistant's identity page. That address
// is the whole input a JMAP setup starts an account from.
//
// As in `tests/gtk.rs`, no test here constructs anything. An
// `EMailConfigServicePage` is a `GtkWidget`, and GTK 3 will not build one
// without a display. This file checks what fails silently otherwise: the
// accessor exists in the libraries this crate links (a missing one
// becomes an `undefined symbol` the moment Evolution dlopens the
// module); the page the accessor takes is the same Rust type the
// backend accessor hands back (two distinct opaque types here would
// compile on one side, and be a wrong pointer on the other); and the
// handle stayed a handle.

use evo_sys::*;
use std::mem::size_of;

/// The pair that must agree, stated as two function pointers so the
/// compiler checks it. `setup_defaults` gets its page from the backend
/// and reads the address from that same page. A binding where those are
/// two different types either cannot write the call at all, or writes it
/// with an unchecked cast that claims the two types are the same.
#[test]
fn the_page_a_backend_hands_back_is_the_page_an_address_is_read_from() {
    let handed_back: unsafe extern "C" fn(
        *mut EMailConfigServiceBackend,
    ) -> *mut EMailConfigServicePage = e_mail_config_service_backend_get_page;
    let read_from: unsafe extern "C" fn(*mut EMailConfigServicePage) -> *const gchar =
        e_mail_config_service_page_get_email_address;

    // The assertions are a formality: the types above are the real test.
    // The assertions only stop the compiler from optimizing the addresses
    // away. This is the same link check the GTK entry points get, written
    // the same way. A name missing from the Evolution version this crate
    // was built against fails the test binary's link.
    let entry_points: &[(&str, *const ())] = &[
        (
            "e_mail_config_service_backend_get_page",
            handed_back as *const (),
        ),
        (
            "e_mail_config_service_page_get_email_address",
            read_from as *const (),
        ),
    ];
    for (name, address) in entry_points {
        assert!(!address.is_null(), "{name} resolved to NULL");
    }
}

/// The page is a widget this crate never allocates, subclasses, or reads
/// a field of. So it carries no layout. `tests/gtk.rs` makes the same
/// claim about the GTK classes, for the same reason: a generated layout
/// here would describe a struct that nothing cross-checks against
/// `g_type_query`. By contrast, `tests/layout.rs` does check
/// `EMailConfigServiceBackend`, which this module really does subclass.
#[test]
fn the_page_handle_carries_no_layout() {
    assert_eq!(
        size_of::<EMailConfigServicePage>(),
        0,
        "EMailConfigServicePage is no longer opaque"
    );
}

/// `insert_widgets` reaches `e_mail_config_page_changed` through the page
/// that `e_mail_config_service_backend_get_page` hands back. It casts
/// that page straight to the interface handle, rather than through any
/// accessor. This is the same pointer-is-the-same-object cast that
/// `E_MAIL_CONFIG_PAGE()` performs in C, and it is sound for the same
/// reason `build.rs` documents: `e-mail-config-service-page.c` (tag
/// 3.52.3) implements `EMailConfigPage` on exactly this type.
///
/// Nothing here can check that implements-relationship at runtime
/// without a display: `e_mail_config_service_page_get_type` is not part
/// of this crate's allowed surface. So this test holds down the half
/// that fails silently otherwise: the symbol resolves, and the handle it
/// takes carries no layout.
#[test]
fn the_page_changed_entry_point_resolves_and_its_handle_carries_no_layout() {
    let changed: unsafe extern "C" fn(*mut EMailConfigPage) = e_mail_config_page_changed;
    // This stores the erased pointer type in a variable, as in the
    // entry-point check above. Clippy's `useless_ptr_null_checks`
    // correctly refuses a direct `fn as *const ()` cast, since a function
    // pointer is never null. The real check this stands in for is a link
    // check: does the symbol resolve in the library this crate links.
    // Going through a variable is how the rest of this file performs that
    // same check.
    let address: *const () = changed as *const ();
    assert!(
        !address.is_null(),
        "e_mail_config_page_changed resolved to NULL"
    );
    assert_eq!(
        size_of::<EMailConfigPage>(),
        0,
        "EMailConfigPage is no longer opaque"
    );
}
