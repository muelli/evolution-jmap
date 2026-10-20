// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later
//
// A Rust panic that unwinds into C is undefined behaviour. C calls every EDS
// vfunc that this crate overrides. These tests make sure that the guards change
// a panic into the failure value that the C caller expects, plus a GError. The
// panic must not cause an abort in the middle of GObject's own stack.

use glib_sys::{GError, g_error_free};
use jmap_backend_core::error::JMAP_BACKEND_ERROR_INTERNAL;
use jmap_backend_core::trampoline::{guard, guard_bool, guard_ptr, guard_value};
use std::ptr;

#[test]
fn guard_passes_the_value_through_when_nothing_panics() {
    assert_eq!(guard("noop", 7, || 3), 3);
}

#[test]
fn guard_returns_the_fallback_when_the_body_panics() {
    assert_eq!(guard("boom", 7, || panic!("deliberate")), 7);
}

/// The `gboolean` vfuncs (`connect_sync`, `save_contact_sync`, ...) must
/// return FALSE *and* set the GError. EDS treats FALSE with a NULL error as a
/// backend bug and logs its own warning.
#[test]
fn guard_bool_reports_a_panic_as_false_plus_an_internal_error() {
    let mut error: *mut GError = ptr::null_mut();
    let ok = unsafe { guard_bool("save_contact_sync", &mut error, || panic!("deliberate")) };

    assert_eq!(ok, glib_sys::GFALSE);
    assert!(!error.is_null(), "panic left the GError unset");
    unsafe {
        assert_eq!((*error).code, JMAP_BACKEND_ERROR_INTERNAL);
        // The message must contain the panic payload. If not, the log does not
        // show which vfunc failed.
        let message = std::ffi::CStr::from_ptr((*error).message).to_string_lossy();
        assert!(message.contains("save_contact_sync"), "{message}");
        assert!(message.contains("deliberate"), "{message}");
        g_error_free(error);
    }
}

#[test]
fn guard_bool_leaves_the_error_alone_on_success() {
    let mut error: *mut GError = ptr::null_mut();
    let ok = unsafe { guard_bool("connect_sync", &mut error, || glib_sys::GTRUE) };
    assert_eq!(ok, glib_sys::GTRUE);
    assert!(error.is_null());
}

/// A caller that passes NULL for the GError out-parameter says "I do not want
/// the error". It does not say "crash for me".
#[test]
fn guard_bool_tolerates_a_null_error_out_parameter() {
    let ok = unsafe { guard_bool("connect_sync", ptr::null_mut(), || panic!("deliberate")) };
    assert_eq!(ok, glib_sys::GFALSE);
}

#[test]
fn guard_ptr_reports_a_panic_as_null_plus_an_error() {
    let mut error: *mut GError = ptr::null_mut();
    let obj: *mut u8 = unsafe { guard_ptr("load_contact_sync", &mut error, || panic!("nope")) };
    assert!(obj.is_null());
    assert!(!error.is_null());
    unsafe { g_error_free(error) };
}

/// `EBackendClass::authenticate_sync` returns an `ESourceAuthenticationResult`,
/// not a boolean or a pointer. Its failure value is one of five, so the guard
/// cannot select it. It still needs the error set, for the same reason as
/// [`guard_bool`]. EDS changes each non-accepting result into an
/// `ESourceCredentialsReason` and shows it to the user. The `GError` is the
/// only part of that which a person can read.
#[test]
fn guard_value_reports_a_panic_as_the_callers_fallback_plus_an_internal_error() {
    let mut error: *mut GError = ptr::null_mut();
    let result = unsafe { guard_value("authenticate_sync", &mut error, 42u32, || panic!("boom")) };

    assert_eq!(result, 42);
    assert!(!error.is_null(), "panic left the GError unset");
    unsafe {
        assert_eq!((*error).code, JMAP_BACKEND_ERROR_INTERNAL);
        let message = std::ffi::CStr::from_ptr((*error).message).to_string_lossy();
        assert!(message.contains("authenticate_sync"), "{message}");
        assert!(message.contains("boom"), "{message}");
        g_error_free(error);
    }
}

#[test]
fn guard_value_passes_the_answer_through_and_leaves_the_error_alone() {
    let mut error: *mut GError = ptr::null_mut();
    let result = unsafe { guard_value("authenticate_sync", &mut error, 42u32, || 7) };

    assert_eq!(result, 7);
    assert!(error.is_null());
}

#[test]
fn guard_value_tolerates_a_null_error_out_parameter() {
    let result =
        unsafe { guard_value("authenticate_sync", ptr::null_mut(), 42u32, || panic!("x")) };
    assert_eq!(result, 42);
}
