// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later
//
// The GtkSourceView surface item 171a's text mode needs, checked the same
// way tests/gtk.rs checks the plain GTK one: the entry points resolve, and
// the handles stay opaque. `gtk_source_language_manager_get_language` also
// gets a behavioural check, since a language manager that cannot find a
// language it ships itself would mean the binding's string handling, not
// just its link step, is wrong. "sieve" is not that check: no `sieve.lang`
// exists yet (item 171a step 3), so this file asks for a language
// GtkSourceView ships built in instead.

use evo_sys::*;
use std::ffi::CString;
use std::mem::size_of;

/// The handles stay opaque handles, the same claim tests/gtk.rs makes for
/// plain GTK classes. Nothing here subclasses a GtkSourceView class or
/// reads a field of one.
#[test]
fn the_gtksourceview_handles_carry_no_layout() {
    assert_eq!(
        size_of::<GtkSourceView>(),
        0,
        "GtkSourceView is no longer opaque"
    );
    assert_eq!(
        size_of::<GtkSourceBuffer>(),
        0,
        "GtkSourceBuffer is no longer opaque"
    );
    assert_eq!(
        size_of::<GtkSourceLanguageManager>(),
        0,
        "GtkSourceLanguageManager is no longer opaque"
    );
    assert_eq!(
        size_of::<GtkSourceLanguage>(),
        0,
        "GtkSourceLanguage is no longer opaque"
    );
}

/// Every call this crate offers resolves. The important part is that this
/// links: a name missing from the GtkSourceView version this crate was
/// built against fails the test binary's link, the same way a missing
/// Evolution entry point would fail to dlopen. The assertion is a
/// formality that stops the compiler from optimizing the addresses away.
#[test]
fn every_gtksourceview_entry_point_this_crate_offers_resolves() {
    let entry_points: &[(&str, *const ())] = &[
        ("gtk_source_view_new", gtk_source_view_new as *const ()),
        (
            "gtk_source_language_manager_get_default",
            gtk_source_language_manager_get_default as *const (),
        ),
        (
            "gtk_source_language_manager_get_language",
            gtk_source_language_manager_get_language as *const (),
        ),
        (
            "gtk_source_buffer_set_language",
            gtk_source_buffer_set_language as *const (),
        ),
    ];
    for (name, address) in entry_points {
        assert!(!address.is_null(), "{name} resolved to NULL");
    }
}

/// `get_language` finds a language GtkSourceView ships in its own
/// `language-specs` data, proving the id string crosses the FFI boundary
/// correctly and the returned pointer is usable. This needs no display
/// connection: unlike `gtk_grid_new` (see tests/gtk.rs), nothing here
/// constructs a widget.
#[test]
fn language_manager_finds_a_language_it_ships_itself() {
    unsafe {
        let manager = gtk_source_language_manager_get_default();
        assert!(!manager.is_null(), "get_default returned NULL");
        let id = CString::new("sh").expect("no interior NUL");
        let language = gtk_source_language_manager_get_language(manager, id.as_ptr());
        assert!(
            !language.is_null(),
            "the language manager could not find its own built-in \"sh\" language"
        );
    }
}
