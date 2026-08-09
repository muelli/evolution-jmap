// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Panic guards for `extern "C"` entry points.
//!
//! Every vfunc we override is called from C, and letting a Rust panic unwind
//! across that boundary is undefined behaviour — in practice an abort that
//! takes the whole `evolution-addressbook-factory` process, and the user's
//! other accounts, down with it. Rust 1.81+ turns the unwind into an abort
//! rather than into UB, which is safer but no less fatal.
//!
//! So every entry point wraps its body in one of these guards: the panic is
//! logged as a GLib critical (which is where an Evolution bug report will
//! look) and turned into the failure value that particular vfunc signature
//! uses — FALSE plus a `GError`, or NULL, or a caller-supplied fallback.

use std::any::Any;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::ptr;

use glib_sys::{G_LOG_LEVEL_CRITICAL, GError, GFALSE, g_error_new_literal, g_log, gboolean};

use crate::error::{JMAP_BACKEND_ERROR_INTERNAL, cstring_lossy, jmap_backend_error_quark};

/// Domain the criticals are logged under, so `G_MESSAGES_DEBUG` and Evolution
/// bug reports can single them out.
const LOG_DOMAIN: &std::ffi::CStr = c"evolution-jmap";

/// Runs `f`, returning `fallback` if it panics. `context` names the entry
/// point and ends up in the log.
pub fn guard<T>(context: &str, fallback: T, f: impl FnOnce() -> T) -> T {
    match catch(context, f) {
        Ok(value) => value,
        Err(_) => fallback,
    }
}

/// Guard for the `gboolean (*) (..., GError **error)` vfuncs — most of
/// `EBookMetaBackendClass` and `ECalMetaBackendClass`. A panic becomes FALSE
/// *with* the error set: EDS logs a warning of its own for a backend that
/// fails without saying why.
///
/// # Safety
///
/// `error` must be NULL or a valid, currently-NULL `GError **`.
pub unsafe fn guard_bool(
    context: &str,
    error: *mut *mut GError,
    f: impl FnOnce() -> gboolean,
) -> gboolean {
    match catch(context, f) {
        Ok(value) => value,
        Err(message) => {
            unsafe { crate::error::set_raw_gerror(error, internal_error(&message)) };
            GFALSE
        }
    }
}

/// Guard for the vfuncs that return an object pointer (`load_contact_sync`
/// and friends), where NULL is the failure value.
///
/// # Safety
///
/// As [`guard_bool`].
pub unsafe fn guard_ptr<T>(
    context: &str,
    error: *mut *mut GError,
    f: impl FnOnce() -> *mut T,
) -> *mut T {
    match catch(context, f) {
        Ok(value) => value,
        Err(message) => {
            unsafe { crate::error::set_raw_gerror(error, internal_error(&message)) };
            ptr::null_mut()
        }
    }
}

/// Guard for the vfuncs that answer with a value of their own *and* have a
/// `GError` out-parameter — `EBackendClass::authenticate_sync`, whose answer is
/// an `ESourceAuthenticationResult`. The failure value cannot be picked here:
/// four of that enum's five values are failures and they mean different things
/// to the user (prompt again, give up, distrust the password), so the caller
/// names the one it wants. What is not left to the caller is the error: EDS
/// turns every non-accepting result into an `ESourceCredentialsReason` it shows
/// someone, and the `GError` is the only part of that a person can read.
///
/// # Safety
///
/// As [`guard_bool`].
pub unsafe fn guard_value<T>(
    context: &str,
    error: *mut *mut GError,
    fallback: T,
    f: impl FnOnce() -> T,
) -> T {
    match catch(context, f) {
        Ok(value) => value,
        Err(message) => {
            unsafe { crate::error::set_raw_gerror(error, internal_error(&message)) };
            fallback
        }
    }
}

/// Runs `f`, converting a panic into an already-logged description of it.
///
/// The closure is wrapped in [`AssertUnwindSafe`]: a vfunc body inevitably
/// touches `&mut` state reached through raw pointers, so the compiler cannot
/// prove unwind safety and the alternative is not running the guard at all.
/// The guarantee we actually need is weaker — the C caller must not see a
/// half-finished operation reported as success — and that is what returning
/// the failure value provides.
fn catch<T>(context: &str, f: impl FnOnce() -> T) -> Result<T, String> {
    catch_unwind(AssertUnwindSafe(f)).map_err(|payload| {
        let message = format!("{context}: panicked: {}", panic_message(&payload));
        log_critical(&message);
        message
    })
}

/// `panic!("literal")` yields a `&str` payload and `panic!("{x}")` a `String`;
/// anything else came from `panic_any` and has no printable form.
fn panic_message(payload: &Box<dyn Any + Send>) -> &str {
    if let Some(s) = payload.downcast_ref::<&'static str>() {
        s
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s
    } else {
        "unprintable panic payload"
    }
}

/// Reports a bug in this code to whatever is reading the process's GLib log —
/// `evolution-data-server`'s journal, or a developer's terminal with
/// `G_DEBUG=fatal-criticals`.
///
/// For the cases a vfunc cannot report any other way: it has no `GError`
/// out-parameter, or the caller is GObject itself and there is nothing to hand
/// an error to. Anything a *user* could act on belongs in a `GError` instead —
/// a critical is for "this cannot happen", not for a misconfigured account.
pub fn log_critical(message: &str) {
    let message = cstring_lossy(message);
    // SAFETY: g_log is variadic and takes a printf format; passing the text
    // as an argument to "%s" rather than as the format itself keeps a stray
    // '%' in a server-supplied string from being read as a directive.
    unsafe {
        g_log(
            LOG_DOMAIN.as_ptr(),
            G_LOG_LEVEL_CRITICAL,
            c"%s".as_ptr(),
            message.as_ptr(),
        );
    }
}

fn internal_error(message: &str) -> *mut GError {
    let message = cstring_lossy(message);
    // SAFETY: the quark is valid and g_error_new_literal copies the message.
    unsafe {
        g_error_new_literal(
            jmap_backend_error_quark(),
            JMAP_BACKEND_ERROR_INTERNAL,
            message.as_ptr(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn both_ordinary_panic_payload_shapes_are_printable() {
        let str_payload: Box<dyn Any + Send> = Box::new("literal");
        assert_eq!(panic_message(&str_payload), "literal");

        let string_payload: Box<dyn Any + Send> = Box::new(String::from("formatted"));
        assert_eq!(panic_message(&string_payload), "formatted");

        let odd_payload: Box<dyn Any + Send> = Box::new(42u8);
        assert_eq!(panic_message(&odd_payload), "unprintable panic payload");
    }
}
