// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! [`jmap_client::Error`] → `GError`.
//!
//! Evolution does not merely display the `GError` a backend returns; it
//! branches on the domain and code. Three of those branches decide whether
//! the product behaves sensibly:
//!
//! - `G_IO_ERROR_CANCELLED` — the user pressed Stop. EDS swallows it instead
//!   of raising an alert.
//! - `E_CLIENT_ERROR_AUTHENTICATION_FAILED` — drives the credentials prompt.
//!   Report anything else for a 401 and the user is told the account is
//!   broken with no way to fix it.
//! - `E_CLIENT_ERROR_REPOSITORY_OFFLINE` — tells a meta backend the server is
//!   unreachable, so it serves its cache. Reporting a generic failure instead
//!   turns a flaky network into an empty address book.
//!
//! Everything else is `E_CLIENT_ERROR_OTHER_ERROR` with the client error's
//! `Display` text, which already carries the JMAP error type and description.

use std::ffi::{CString, c_int};
use std::ptr;

use eds_sys::{
    E_CLIENT_ERROR_AUTHENTICATION_FAILED, E_CLIENT_ERROR_INVALID_ARG, E_CLIENT_ERROR_OTHER_ERROR,
    E_CLIENT_ERROR_PERMISSION_DENIED, E_CLIENT_ERROR_REPOSITORY_OFFLINE, EClientError,
    e_client_error_create,
};
use glib_sys::{GError, GFALSE, GQuark, g_error_new_literal, g_quark_from_static_string, gboolean};
use jmap_client::Error;

/// Error domain for failures that are ours, not the server's or the
/// protocol's — currently only a caught panic. Kept separate from
/// `E_CLIENT_ERROR` so a bug in this code is distinguishable in a log from a
/// misbehaving JMAP server.
pub fn jmap_backend_error_quark() -> GQuark {
    // SAFETY: the string is 'static and NUL-terminated, which is exactly what
    // g_quark_from_static_string requires.
    unsafe { g_quark_from_static_string(c"evolution-jmap-backend".as_ptr()) }
}

/// A vfunc panicked. There is no useful recovery; the code exists so the
/// message in the log names the crate that produced it.
pub const JMAP_BACKEND_ERROR_INTERNAL: c_int = 1;

/// Allocates a new `GError` describing `err`. Ownership passes to the caller,
/// who must `g_error_free` it or hand it to a C caller that will.
pub fn to_gerror(err: &Error) -> *mut GError {
    let message = cstring_lossy(&err.to_string());

    if matches!(err, Error::Cancelled) {
        // SAFETY: both the quark and the message pointer are valid; the
        // function copies the message.
        return unsafe {
            g_error_new_literal(
                gio_sys::g_io_error_quark(),
                gio_sys::G_IO_ERROR_CANCELLED,
                message.as_ptr(),
            )
        };
    }

    // SAFETY: e_client_error_create copies the message and returns a fresh
    // GError; the code is one of the enum's own values.
    unsafe { e_client_error_create(client_error_code(err), message.as_ptr()) }
}

/// Writes `err` into a GLib-style `GError **` out-parameter.
///
/// # Safety
///
/// `dest` must be either NULL — the GLib convention for "the caller does not
/// want the error" — or a valid pointer to a `*mut GError` that is currently
/// NULL, as every EDS vfunc receives.
pub unsafe fn set_gerror(dest: *mut *mut GError, err: &Error) {
    unsafe { set_raw_gerror(dest, to_gerror(err)) }
}

/// Same, for an already-built `GError`. Takes ownership of `error`: if the
/// caller did not ask for an error, it is freed rather than leaked.
///
/// # Safety
///
/// As [`set_gerror`], and `error` must be a `GError` this call may consume.
pub unsafe fn set_raw_gerror(dest: *mut *mut GError, error: *mut GError) {
    unsafe {
        if dest.is_null() {
            glib_sys::g_error_free(error);
        } else {
            debug_assert!((*dest).is_null(), "overwriting an already-set GError");
            *dest = error;
        }
    }
}

/// A `GError` for an argument EDS itself passed us that was invalid — a
/// vCard/iCalendar Evolution asked us to save that the mapping cannot read,
/// or an operation with nothing to act on. Shared by call sites across the
/// backend crates that each independently built this exact `GError`.
pub fn invalid_arg_gerror(message: &str) -> *mut GError {
    let message = cstring_lossy(message);
    // SAFETY: the code is one of the enum's own values and the message is
    // copied by the call.
    unsafe { e_client_error_create(E_CLIENT_ERROR_INVALID_ARG, message.as_ptr()) }
}

/// Reports `failure` through `error` (mapped to a `GError` by `to_gerror`,
/// since every crate's mapping is its own — a `StoreError`, a `SyncError`,
/// whatever the caller's vfunc fails with) and returns the null pointer that
/// means failure to a vfunc whose success return is a fresh, owned pointer.
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

/// The same, for the arguments EDS itself got wrong: reports `message` as an
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
        // status: the server answered, so the account is fine and only this
        // operation failed. The message carries the detail.
        _ => E_CLIENT_ERROR_OTHER_ERROR,
    }
}

/// `CString::new` refuses interior NULs, which a hostile or broken server
/// could put in a description. Truncating there loses less than panicking.
pub fn cstring_lossy(s: &str) -> CString {
    match CString::new(s) {
        Ok(c) => c,
        Err(e) => {
            let nul = e.nul_position();
            let mut bytes = e.into_vec();
            bytes.truncate(nul);
            // SAFETY: bytes now stops before the first interior NUL.
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
    fn the_error_domain_is_stable_across_calls() {
        assert_eq!(jmap_backend_error_quark(), jmap_backend_error_quark());
        assert_ne!(jmap_backend_error_quark(), 0);
    }

    #[test]
    fn invalid_arg_gerror_carries_the_invalid_arg_code_and_the_message() {
        let error = invalid_arg_gerror("not a vcard");
        assert!(!error.is_null());
        // SAFETY: a freshly allocated GError this test owns.
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
