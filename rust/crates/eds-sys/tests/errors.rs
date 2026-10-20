// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later
//
// The three error domains of a backend are not interchangeable. EDS matches the
// pair of domain and code to decide between "the object is gone" and "the sync
// failed". Thus a missing event in the generic E_CLIENT_ERROR domain, even with
// a numerically equal code, is a cache entry that EDS never removes.

use eds_sys::*;

/// The three domains are different quarks. The book and calendar domains both
/// use code 1 for "not found", and code 1 of `E_CLIENT_ERROR` is `BUSY`.
/// Only the domain tells them apart.
#[test]
fn the_client_error_domains_are_three_different_quarks() {
    // SAFETY: each call is a quark accessor with no arguments.
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

/// `e_cal_client_error_create` copies the message, so the message can go out of
/// scope after the call. The address-book half relies on the same contract.
#[test]
fn a_calendar_client_error_carries_the_domain_and_code_it_was_given() {
    let message = c"no such event";
    // SAFETY: the code is a value of the enum, and the message is a 'static
    // NUL-terminated literal that the call copies.
    unsafe {
        let error =
            e_cal_client_error_create(E_CAL_CLIENT_ERROR_OBJECT_NOT_FOUND, message.as_ptr());
        assert!(!error.is_null());
        assert_eq!((*error).domain, e_cal_client_error_quark());
        assert_eq!((*error).code, E_CAL_CLIENT_ERROR_OBJECT_NOT_FOUND as i32);
        g_error_free(error);
    }
}
