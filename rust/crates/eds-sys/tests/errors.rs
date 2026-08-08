// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later
//
// The error domains a backend reports failures in. There are three of them and
// they are not interchangeable: EDS decides that an object is gone rather than
// that the sync failed by matching on the *pair* of domain and code, so a
// missing event reported in the generic E_CLIENT_ERROR domain — even with a
// numerically equal code — is a cache entry that never goes away.

use eds_sys::*;

/// The three domains are distinct quarks. `E_CLIENT_ERROR` and its two
/// per-object-type siblings share a `NOT_FOUND`-ish code between them, so the
/// domain is the only thing that tells them apart.
#[test]
fn the_client_error_domains_are_three_different_quarks() {
    // SAFETY: each is a plain quark accessor with no arguments.
    let (client, book, cal) = unsafe {
        (
            e_client_error_quark(),
            e_book_client_error_quark(),
            e_cal_client_error_quark(),
        )
    };
    assert_ne!(client, 0);
    assert_ne!(client, book);
    assert_ne!(client, cal);
    assert_ne!(book, cal);
}

/// `e_cal_client_error_create` copies the message, so the `CString` may die
/// with the call — the same contract the address book's half already relies on.
#[test]
fn a_calendar_client_error_carries_the_domain_and_code_it_was_given() {
    let message = c"no such event";
    // SAFETY: the code is one of the enum's own values and the message is a
    // 'static NUL-terminated literal the call copies.
    unsafe {
        let error =
            e_cal_client_error_create(E_CAL_CLIENT_ERROR_OBJECT_NOT_FOUND, message.as_ptr());
        assert!(!error.is_null());
        assert_eq!((*error).domain, e_cal_client_error_quark());
        assert_eq!((*error).code, E_CAL_CLIENT_ERROR_OBJECT_NOT_FOUND as i32);
        g_error_free(error);
    }
}
