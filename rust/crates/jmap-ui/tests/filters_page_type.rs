// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later
//
// Tests the type of the server filters page: its parent and its interface.
// The account editor accepts only a GtkScrolledWindow that is an
// EMailConfigPage.

use gobject_sys::g_type_is_a;
use jmap_backend_core::subclass::register_static;
use jmap_ui::filters::page::FiltersPage;

#[test]
fn the_filters_page_is_a_scrolled_window_and_a_config_page() {
    let gtype = register_static::<FiltersPage>();
    assert_ne!(gtype, 0, "the page type was not registered");
    // SAFETY: the type getters have no arguments. All three types are registered.
    let (scrolled, page) = unsafe {
        (
            g_type_is_a(gtype, evo_sys::gtk_scrolled_window_get_type()),
            g_type_is_a(gtype, evo_sys::e_mail_config_page_get_type()),
        )
    };
    assert_ne!(scrolled, 0, "the page is not a GtkScrolledWindow");
    assert_ne!(page, 0, "the page does not implement EMailConfigPage");
}
