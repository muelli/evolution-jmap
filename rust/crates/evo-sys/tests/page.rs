// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later
//
// The one thing M7's `setup_defaults` asks the page it is an extension of: the
// address the user typed on the assistant's identity page, which is the whole
// input to the account a JMAP setup starts from.
//
// As in `tests/gtk.rs`, nothing here constructs anything — an
// `EMailConfigServicePage` is a `GtkWidget` and GTK 3 will not build one
// without a display. What is checked is what fails silently otherwise: that the
// accessor exists in the libraries this crate links (a missing one is an
// `undefined symbol` the moment Evolution dlopens the module), that the page it
// takes is the same Rust type the backend accessor hands back — two distinct
// opaque types here would compile on one side and be a wrong pointer on the
// other — and that the handle stayed a handle.

use evo_sys::*;
use std::mem::size_of;

/// The pair that has to agree, stated as a pair of function pointers so the
/// compiler checks it: `setup_defaults` gets its page from the backend and
/// reads the address off *that* page, so a binding in which those are two
/// different types is one where the call cannot be written at all — or, worse,
/// is written with a cast that says the two are the same when nothing checked.
#[test]
fn the_page_a_backend_hands_back_is_the_page_an_address_is_read_from() {
    let handed_back: unsafe extern "C" fn(
        *mut EMailConfigServiceBackend,
    ) -> *mut EMailConfigServicePage = e_mail_config_service_backend_get_page;
    let read_from: unsafe extern "C" fn(*mut EMailConfigServicePage) -> *const gchar =
        e_mail_config_service_page_get_email_address;

    // The assertions are a formality — the types above are the test. They stop
    // the addresses being optimised away, and they are the same link check the
    // GTK entry points get, written the same way: a name that is not in the
    // Evolution this was built against fails the test binary's link.
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

/// The page is a widget this crate never allocates, subclasses or reads a field
/// of, so it carries no layout — the same statement `tests/gtk.rs` makes about
/// the GTK classes, and for the same reason: a generated layout here would be a
/// claim about a struct nothing cross-checks against `g_type_query`, unlike the
/// `EMailConfigServiceBackend` `tests/layout.rs` does check, which this module
/// really does subclass.
#[test]
fn the_page_handle_carries_no_layout() {
    assert_eq!(
        size_of::<EMailConfigServicePage>(),
        0,
        "EMailConfigServicePage is no longer opaque"
    );
}

/// `insert_widgets` reaches `e_mail_config_page_changed` through the page
/// `e_mail_config_service_backend_get_page` hands back, cast straight to the
/// interface handle rather than through any accessor — the same
/// pointer-is-the-same-object cast `E_MAIL_CONFIG_PAGE()` is in C, and sound
/// for the same reason `build.rs` writes down: `e-mail-config-service-page.c`
/// (tag 3.52.3) implements `EMailConfigPage` on exactly this type. Nothing
/// here can check that implements-relationship at runtime without a display —
/// `e_mail_config_service_page_get_type` is not part of this crate's allowed
/// surface — so what this test can hold down is the half that fails silently
/// otherwise: the symbol resolves, and the handle it takes carries no layout.
#[test]
fn the_page_changed_entry_point_resolves_and_its_handle_carries_no_layout() {
    let changed: unsafe extern "C" fn(*mut EMailConfigPage) = e_mail_config_page_changed;
    // A variable of the erased pointer type, as in the entry-point check
    // above: clippy's `useless_ptr_null_checks` (rightly) refuses a direct
    // `fn as *const ()` cast, since a function pointer is never null, but the
    // check this is standing in for is a *link* check — does the symbol
    // resolve in the library this crate links — for which going through a
    // variable is what the rest of this file already does.
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
