// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! [`jmap_client::Error`] → `GError`.
//!
//! Evolution does not only show the `GError` that a backend returns. It
//! branches on the domain and code. Three of those branches decide whether
//! the product behaves correctly:
//!
//! - `G_IO_ERROR_CANCELLED`: the user pushed Stop. EDS ignores it and shows
//!   no alert.
//! - `E_CLIENT_ERROR_AUTHENTICATION_FAILED`: starts the credentials prompt.
//!   With a different error for a 401, the user sees that the account is
//!   broken, and has no way to repair it.
//! - `E_CLIENT_ERROR_REPOSITORY_OFFLINE`: tells a meta backend that the
//!   server is unreachable, so that it serves its cache. With a generic
//!   failure, an unreliable network causes an empty address book.
//!
//! All other errors are `E_CLIENT_ERROR_OTHER_ERROR` with the `Display` text
//! of the client error. That text already contains the JMAP error type and
//! description.

use std::ffi::{CString, c_int};
use std::ptr;

use eds_sys::{
    E_CLIENT_ERROR_AUTHENTICATION_FAILED, E_CLIENT_ERROR_INVALID_ARG, E_CLIENT_ERROR_OTHER_ERROR,
    E_CLIENT_ERROR_OUT_OF_SYNC, E_CLIENT_ERROR_PERMISSION_DENIED,
    E_CLIENT_ERROR_REPOSITORY_OFFLINE, EClientError, e_client_error_create,
};
use glib_sys::{GError, GFALSE, GQuark, g_error_new_literal, g_quark_from_static_string, gboolean};
use jmap_client::Error;

/// Error domain for failures of this code, not of the server or the
/// protocol. Now, only a caught panic uses it. It is separate from
/// `E_CLIENT_ERROR`, so that a log can show the difference between a bug in
/// this code and an incorrect JMAP server.
pub fn jmap_backend_error_quark() -> GQuark {
    // SAFETY: the string is 'static and NUL-terminated, as
    // g_quark_from_static_string requires.
    unsafe { g_quark_from_static_string(c"evolution-jmap-backend".as_ptr()) }
}

/// A vfunc panicked. No useful recovery is possible. The code exists so that
/// the message in the log names the crate that produced it.
pub const JMAP_BACKEND_ERROR_INTERNAL: c_int = 1;

/// Allocates a new `GError` that describes `err`. Ownership goes to the
/// caller. The caller must `g_error_free` it or give it to a C caller that
/// frees it.
pub fn to_gerror(err: &Error) -> *mut GError {
    let message = cstring_lossy(&err.to_string());

    if matches!(err, Error::Cancelled) {
        // SAFETY: the quark and the message pointer are valid. The function
        // copies the message.
        return unsafe {
            g_error_new_literal(
                gio_sys::g_io_error_quark(),
                gio_sys::G_IO_ERROR_CANCELLED,
                message.as_ptr(),
            )
        };
    }

    // SAFETY: e_client_error_create copies the message and returns a new
    // GError. The code is a value of the enum.
    unsafe { e_client_error_create(client_error_code(err), message.as_ptr()) }
}

/// Writes `err` into a GLib-style `GError **` out-parameter.
///
/// # Safety
///
/// `dest` must be NULL (the GLib convention for "the caller does not want
/// the error"), or a valid pointer to a `*mut GError` that is now NULL. Every
/// EDS vfunc receives one of these.
pub unsafe fn set_gerror(dest: *mut *mut GError, err: &Error) {
    unsafe { set_raw_gerror(dest, to_gerror(err)) }
}

/// The same, for a `GError` that already exists. Takes ownership of `error`:
/// if the caller did not ask for an error, this function frees it and does
/// not leak it.
///
/// If `*dest` is already set, the caller did not obey the contract above.
/// This function does not overwrite it (that would leak the first `GError`).
/// It keeps the first error and frees `error`. The `g_set_error()` family of
/// GLib refuses in the same way (`g_return_if_fail (err == NULL || *err ==
/// NULL)`). In GLib, this is a real runtime check, not only a debug check.
///
/// # Safety
///
/// As [`set_gerror`]. Also, `error` must be a `GError` that this call can
/// consume.
pub unsafe fn set_raw_gerror(dest: *mut *mut GError, error: *mut GError) {
    unsafe {
        if dest.is_null() {
            glib_sys::g_error_free(error);
        } else if (*dest).is_null() {
            *dest = error;
        } else {
            crate::trampoline::log_critical(
                "set_raw_gerror: *dest was already set; keeping the first GError",
            );
            glib_sys::g_error_free(error);
        }
    }
}

/// Whether `error` is the error that a stale access token produces. This is
/// the readback half of [`crate::retry::retry_on_authentication_failure`].
///
/// An EDS vfunc reports failure as `FALSE` and a `GError`. Thus when a shared
/// retry wrapper sees it, the [`Error`] that caused it is gone. Only the
/// domain and code remain. `client_error_code` gives
/// `E_CLIENT_ERROR_AUTHENTICATION_FAILED` to `Error::Http { status: 401, .. }`
/// and to nothing else. Thus this pair *is* "the server refused these
/// credentials", accurately and without a parse of the message text.
///
/// The function compares the domain and the code. Both are necessary:
/// `E_CLIENT_ERROR` and `G_IO_ERROR` are different enumerations that both
/// start at zero. With only the code, an unrelated GLib I/O failure could
/// look like a rejected token, and the code would fetch a good token again.
///
/// # Safety
///
/// `error` must be NULL or a valid `GError` that outlives the call.
pub unsafe fn is_authentication_failed(error: *const GError) -> bool {
    // SAFETY: the contract of the caller, and the NULL check. The quark
    // function takes no arguments.
    unsafe {
        !error.is_null()
            && (*error).domain == eds_sys::e_client_error_quark()
            && (*error).code == E_CLIENT_ERROR_AUTHENTICATION_FAILED as c_int
    }
}

/// A `GError` for an invalid argument from EDS. Examples: a vCard/iCalendar
/// that Evolution asks the backend to save but the mapping cannot read, or
/// an operation with no object. Call sites in all backend crates share it.
/// Before, each call site built this same `GError` independently.
pub fn invalid_arg_gerror(message: &str) -> *mut GError {
    let message = cstring_lossy(message);
    // SAFETY: the code is a value of the enum, and the call copies the
    // message.
    unsafe { e_client_error_create(E_CLIENT_ERROR_INVALID_ARG, message.as_ptr()) }
}

/// A `GError` for a save that the server copy changed after the base copy, when
/// EDS asked for `E_CONFLICT_RESOLUTION_FAIL`.
pub fn out_of_sync_gerror(message: &str) -> *mut GError {
    let message = cstring_lossy(message);
    // SAFETY: the code is a value of the enum, and the call copies the
    // message.
    unsafe { e_client_error_create(E_CLIENT_ERROR_OUT_OF_SYNC, message.as_ptr()) }
}

/// Reports `failure` through `error`, and returns the null pointer that means
/// failure to a vfunc whose success return is a new, owned pointer.
/// `to_gerror` maps `failure` to a `GError`, because each crate has its own
/// mapping (a `StoreError`, a `SyncError`, or the failure type of the vfunc
/// of the caller).
///
/// # Safety
///
/// As [`set_raw_gerror`].
pub unsafe fn fail<E, T>(
    error: *mut *mut GError,
    failure: &E,
    to_gerror: impl FnOnce(&E) -> *mut GError,
) -> *mut T {
    unsafe { set_raw_gerror(error, to_gerror(failure)) };
    ptr::null_mut()
}

/// The same, for a vfunc whose success return is `gboolean` `TRUE`.
///
/// # Safety
///
/// As [`set_raw_gerror`].
pub unsafe fn fail_bool<E>(
    error: *mut *mut GError,
    failure: &E,
    to_gerror: impl FnOnce(&E) -> *mut GError,
) -> gboolean {
    unsafe { set_raw_gerror(error, to_gerror(failure)) };
    GFALSE
}

/// The same, for incorrect arguments from EDS: reports `message` as an
/// [`invalid_arg_gerror`] and returns `FALSE`.
///
/// # Safety
///
/// As [`set_raw_gerror`].
pub unsafe fn fail_invalid(error: *mut *mut GError, message: &str) -> gboolean {
    unsafe { set_raw_gerror(error, invalid_arg_gerror(message)) };
    GFALSE
}

fn client_error_code(err: &Error) -> EClientError {
    match err {
        Error::Transport(_) => E_CLIENT_ERROR_REPOSITORY_OFFLINE,
        Error::Http { status: 401, .. } => E_CLIENT_ERROR_AUTHENTICATION_FAILED,
        Error::Http { status: 403, .. } => E_CLIENT_ERROR_PERMISSION_DENIED,
        // A method error, a /set rejection, malformed JSON or any other HTTP
        // status: the server answered. Thus the account is correct, and only
        // this operation failed. The message contains the details.
        _ => E_CLIENT_ERROR_OTHER_ERROR,
    }
}

/// `CString::new` refuses interior NULs, and a hostile or broken server can
/// put one in a description. A truncation there loses less than a panic.
pub fn cstring_lossy(s: &str) -> CString {
    match CString::new(s) {
        Ok(c) => c,
        Err(e) => {
            let nul = e.nul_position();
            let mut bytes = e.into_vec();
            bytes.truncate(nul);
            // SAFETY: bytes now ends before the first interior NUL.
            unsafe { CString::from_vec_unchecked(bytes) }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use eds_sys::e_client_error_quark;

    #[test]
    fn a_message_with_an_interior_nul_is_truncated_not_panicked_on() {
        assert_eq!(cstring_lossy("before\0after").to_bytes(), b"before");
    }

    #[test]
    fn only_a_401_reads_back_as_an_authentication_failure() {
        // The readback in `crate::retry::retry_on_authentication_failure`
        // needs this. A vfunc gives its caller a `GError`, not a
        // `jmap_client::Error`. Thus "a retry with a new token is permitted"
        // must survive the round trip through `to_gerror`.
        let cases = [
            (
                Error::Http {
                    status: 401,
                    problem: None,
                },
                true,
            ),
            (
                Error::Http {
                    status: 403,
                    problem: None,
                },
                false,
            ),
            (
                Error::Http {
                    status: 500,
                    problem: None,
                },
                false,
            ),
            (Error::Transport("no route to host".into()), false),
            // A fully different domain. A comparison of only the code could
            // get this wrong: G_IO_ERROR_CANCELLED is 19 and
            // E_CLIENT_ERROR_AUTHENTICATION_FAILED is not, but nothing
            // guarantees that across EDS versions.
            (Error::Cancelled, false),
        ];

        for (err, expected) in cases {
            let error = to_gerror(&err);
            assert!(!error.is_null());
            // SAFETY: a newly allocated GError that this test owns.
            unsafe {
                assert_eq!(
                    is_authentication_failed(error),
                    expected,
                    "{err} classified wrongly"
                );
                glib_sys::g_error_free(error);
            }
        }
    }

    #[test]
    fn the_error_domain_is_stable_across_calls() {
        assert_eq!(jmap_backend_error_quark(), jmap_backend_error_quark());
        assert_ne!(jmap_backend_error_quark(), 0);
    }

    #[test]
    fn out_of_sync_gerror_carries_the_out_of_sync_code_and_the_message() {
        let error = out_of_sync_gerror("changed on the server");
        assert!(!error.is_null());
        // SAFETY: a newly allocated GError that this test owns.
        unsafe {
            assert_eq!((*error).domain, e_client_error_quark());
            assert_eq!((*error).code, eds_sys::E_CLIENT_ERROR_OUT_OF_SYNC as i32);
            assert_eq!(
                std::ffi::CStr::from_ptr((*error).message).to_string_lossy(),
                "changed on the server"
            );
            glib_sys::g_error_free(error);
        }
    }

    #[test]
    fn invalid_arg_gerror_carries_the_invalid_arg_code_and_the_message() {
        let error = invalid_arg_gerror("not a vcard");
        assert!(!error.is_null());
        // SAFETY: a newly allocated GError that this test owns.
        unsafe {
            assert_eq!((*error).domain, e_client_error_quark());
            assert_eq!((*error).code, eds_sys::E_CLIENT_ERROR_INVALID_ARG as i32);
            assert_eq!(
                std::ffi::CStr::from_ptr((*error).message).to_string_lossy(),
                "not a vcard"
            );
            glib_sys::g_error_free(error);
        }
    }
}
