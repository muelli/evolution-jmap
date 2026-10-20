// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Panic guards for `extern "C"` entry points.
//!
//! C calls every vfunc that this crate overrides. If a Rust panic unwinds
//! across that boundary, the result is undefined behaviour. In practice it is
//! an abort that stops the full `evolution-addressbook-factory` process, and
//! with it the user's other accounts. Rust 1.81+ changes the unwind into an
//! abort, not UB. This is safer, but equally fatal.
//!
//! Thus every entry point wraps its body in one of these guards. The guard
//! logs the panic as a GLib critical (an Evolution bug report looks there).
//! It then returns the failure value of that vfunc signature: FALSE plus a
//! `GError`, NULL, or a fallback from the caller.

use std::any::Any;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::ptr;

use glib_sys::{G_LOG_LEVEL_CRITICAL, GError, GFALSE, g_error_new_literal, g_log, gboolean};

use crate::error::{JMAP_BACKEND_ERROR_INTERNAL, cstring_lossy, jmap_backend_error_quark};

/// The log domain of the criticals. `G_MESSAGES_DEBUG` and Evolution bug
/// reports can use it to find them.
const LOG_DOMAIN: &std::ffi::CStr = c"evolution-jmap";

/// Runs `f`, and returns `fallback` if it panics. `context` names the entry
/// point and goes into the log.
pub fn guard<T>(context: &str, fallback: T, f: impl FnOnce() -> T) -> T {
    match catch(context, f) {
        Ok(value) => value,
        Err(_) => fallback,
    }
}

/// Guard for the `gboolean (*) (..., GError **error)` vfuncs (most of
/// `EBookMetaBackendClass` and `ECalMetaBackendClass`). A panic becomes FALSE
/// *with* the error set. EDS logs its own warning for a backend that fails
/// without a reason.
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
/// and similar), where NULL is the failure value.
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

/// Guard for the vfuncs that return a value of their own *and* have a
/// `GError` out-parameter. An example is `EBackendClass::authenticate_sync`,
/// which returns an `ESourceAuthenticationResult`. This function cannot pick
/// the failure value. Four of that enum's five values are failures, and each
/// means a different thing to the user (prompt again, stop, distrust the
/// password). Thus the caller names the one it wants. But this function sets
/// the error. EDS changes every non-accepting result into an
/// `ESourceCredentialsReason` that it shows to a person. The `GError` is the
/// only part of that a person can read.
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

/// Runs `f`, and changes a panic into a description that is already logged.
///
/// The closure is wrapped in [`AssertUnwindSafe`]. A vfunc body always
/// changes `&mut` state through raw pointers, so the compiler cannot prove
/// unwind safety. The only alternative is no guard. The necessary guarantee
/// is weaker: the C caller must not see a half-finished operation as a
/// success. The returned failure value gives this guarantee.
fn catch<T>(context: &str, f: impl FnOnce() -> T) -> Result<T, String> {
    catch_unwind(AssertUnwindSafe(f)).map_err(|payload| {
        let message = format!("{context}: panicked: {}", panic_message(&payload));
        log_critical(&message);
        message
    })
}

/// `panic!("literal")` gives a `&str` payload and `panic!("{x}")` gives a
/// `String`. All other payloads come from `panic_any` and cannot be printed.
fn panic_message(payload: &Box<dyn Any + Send>) -> &str {
    if let Some(s) = payload.downcast_ref::<&'static str>() {
        s
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s
    } else {
        "unprintable panic payload"
    }
}

/// Reports a bug in this code to the reader of the process's GLib log
/// (`evolution-data-server`'s journal, or a developer's terminal with
/// `G_DEBUG=fatal-criticals`). It also sends a `tracing::error!` event. If a
/// module has called [`crate::logging::init`], that event also goes to
/// journald with the other events of this project.
///
/// Use it only when a vfunc cannot report in a different way: it has no
/// `GError` out-parameter, or the caller is GObject and cannot receive an
/// error. Put all problems that a *user* can correct in a `GError`. A
/// critical is for "this cannot happen", not for an incorrect account
/// configuration.
pub fn log_critical(message: &str) {
    tracing::error!("{message}");
    g_log_critical(message);
}

/// Like [`log_critical`], but also attaches `message_uid` as a structured
/// `tracing` field. Thus `journalctl F_MESSAGE_UID=<uid>` (or a different
/// structured consumer) can find a hostile or damaged message, not only a
/// grep of the free-text message. (`tracing_journald` adds its default `F_`
/// prefix.) The `GError`/`g_log` side does not change. `message` also
/// contains the uid in prose, for readers that use this log as before.
pub fn log_critical_for_message(message_uid: &str, message: &str) {
    tracing::error!(message_uid, "{message}");
    g_log_critical(message);
}

/// Like [`log_critical`], but also attaches `resource_id` as a structured
/// `tracing` field. A failure can occur in one child of a collection account
/// (a mailbox, address book or calendar that EDS refused to create, abandon,
/// or remove). `journalctl F_RESOURCE_ID=<resource_id>` can find that
/// failure, not only a grep of the free-text message. The `GError`/`g_log`
/// side does not change.
pub fn log_critical_for_resource(resource_id: &str, message: &str) {
    tracing::error!(resource_id, "{message}");
    g_log_critical(message);
}

/// Like [`log_critical`], but also attaches `account_id` as a structured
/// `tracing` field. Some failures have no single resource as their cause
/// (a credentials lookup, or a populate/fan-out summary for all children of
/// an account). `journalctl F_ACCOUNT_ID=<account_id>` can find them. The
/// `GError`/`g_log` side does not change.
pub fn log_critical_for_account(account_id: &str, message: &str) {
    tracing::error!(account_id, "{message}");
    g_log_critical(message);
}

/// The `g_log` part that all the `log_critical*` functions share.
fn g_log_critical(message: &str) {
    let message = cstring_lossy(message);
    // SAFETY: g_log is variadic and takes a printf format. The text is an
    // argument to "%s", not the format. Thus g_log does not read a '%' in a
    // string from the server as a directive.
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
    use std::cell::RefCell;
    use std::os::raw::c_char;
    use std::sync::OnceLock;

    use glib_sys::{GLogLevelFlags, gpointer};
    use jmap_tracing_capture::capture;

    use super::*;

    thread_local! {
        static GLOG_SCOPE: RefCell<Option<Vec<String>>> = const { RefCell::new(None) };
    }

    /// Records a `g_log` call into the buffer that the calling thread's
    /// [`capture_g_log_critical`] has open. `jmap_tracing_capture` uses the
    /// same per-thread routing for the same reason. Many tests in this binary
    /// call a `log_critical*` function at the same time, on their own
    /// threads. They must not see the messages of other tests.
    unsafe extern "C" fn record(
        _domain: *const c_char,
        _level: GLogLevelFlags,
        message: *const c_char,
        _user_data: gpointer,
    ) {
        // SAFETY: GLib gives every registered log handler a NUL-terminated
        // string that GLib owns. This function does not free it or keep it.
        let text = unsafe { std::ffi::CStr::from_ptr(message) }
            .to_string_lossy()
            .into_owned();
        GLOG_SCOPE.with(|scope| {
            if let Some(buf) = scope.borrow_mut().as_mut() {
                buf.push(text);
            }
        });
    }

    /// Runs `run` and returns the `g_log` criticals under [`LOG_DOMAIN`]
    /// that it caused. It replaces GLib's default handler, which only prints
    /// them. The handler is installed once per process. `g_log_set_handler`
    /// has no matching call that removes only this handler and keeps GLib's
    /// default handling for other domains. Thus later callers share the one
    /// installation, and their own thread routes their messages.
    fn capture_g_log_critical<T>(run: impl FnOnce() -> T) -> (T, Vec<String>) {
        static INSTALLED: OnceLock<()> = OnceLock::new();
        INSTALLED.get_or_init(|| {
            // SAFETY: LOG_DOMAIN is a valid NUL-terminated string; `record`
            // matches the `GLogFunc` signature `g_log_set_handler` expects.
            unsafe {
                glib_sys::g_log_set_handler(
                    LOG_DOMAIN.as_ptr(),
                    G_LOG_LEVEL_CRITICAL,
                    Some(record),
                    std::ptr::null_mut(),
                );
            }
        });
        GLOG_SCOPE.with(|scope| *scope.borrow_mut() = Some(Vec::new()));
        let result = run();
        let captured = GLOG_SCOPE
            .with(|scope| scope.borrow_mut().take())
            .unwrap_or_default();
        (result, captured)
    }

    #[test]
    fn log_critical_emits_a_tracing_error_event() {
        let (_, captured) = capture(|| {
            log_critical("a bug nobody can act on");
        });

        assert!(
            captured
                .iter()
                .any(|(level, _, value)| *level == tracing::Level::ERROR
                    && value.contains("a bug nobody can act on")),
            "expected an ERROR event carrying the message, got {captured:?}"
        );
    }

    #[test]
    fn log_critical_reaches_the_glib_log_as_a_critical() {
        let (_, captured) = capture_g_log_critical(|| {
            log_critical("a bug reported through glib too");
        });

        assert!(
            captured
                .iter()
                .any(|message| message.contains("a bug reported through glib too")),
            "expected a GLib critical carrying the message, got {captured:?}"
        );
    }

    #[test]
    fn both_ordinary_panic_payload_shapes_are_printable() {
        let str_payload: Box<dyn Any + Send> = Box::new("literal");
        assert_eq!(panic_message(&str_payload), "literal");

        let string_payload: Box<dyn Any + Send> = Box::new(String::from("formatted"));
        assert_eq!(panic_message(&string_payload), "formatted");

        let odd_payload: Box<dyn Any + Send> = Box::new(42u8);
        assert_eq!(panic_message(&odd_payload), "unprintable panic payload");
    }

    #[test]
    fn log_critical_for_message_attaches_the_uid_as_a_structured_field() {
        let (_, captured) = capture(|| {
            log_critical_for_message(
                "uid-123",
                "the cached copy was dropped: it has nothing in it",
            );
        });

        assert!(
            captured
                .iter()
                .any(|(_, name, value)| name == "message_uid" && value == "uid-123"),
            "expected a message_uid=uid-123 field, got {captured:?}"
        );
    }

    #[test]
    fn log_critical_does_not_attach_a_message_uid_field() {
        let (_, captured) = capture(|| {
            log_critical("no uid in this one");
        });

        assert!(
            captured.iter().all(|(_, name, _)| name != "message_uid"),
            "did not expect a message_uid field, got {captured:?}"
        );
    }

    #[test]
    fn log_critical_for_resource_attaches_the_resource_id_as_a_structured_field() {
        let (_, captured) = capture(|| {
            log_critical_for_resource(
                "addressbook-1",
                "EDS would not create a child source for it",
            );
        });

        assert!(
            captured
                .iter()
                .any(|(_, name, value)| name == "resource_id" && value == "addressbook-1"),
            "expected a resource_id=addressbook-1 field, got {captured:?}"
        );
    }

    #[test]
    fn log_critical_does_not_attach_a_resource_id_field() {
        let (_, captured) = capture(|| {
            log_critical("no resource id in this one");
        });

        assert!(
            captured.iter().all(|(_, name, _)| name != "resource_id"),
            "did not expect a resource_id field, got {captured:?}"
        );
    }

    #[test]
    fn log_critical_for_account_attaches_the_account_id_as_a_structured_field() {
        let (_, captured) = capture(|| {
            log_critical_for_account(
                "account-1",
                "the account's credentials could not be looked up",
            );
        });

        assert!(
            captured
                .iter()
                .any(|(_, name, value)| name == "account_id" && value == "account-1"),
            "expected an account_id=account-1 field, got {captured:?}"
        );
    }

    #[test]
    fn log_critical_does_not_attach_an_account_id_field() {
        let (_, captured) = capture(|| {
            log_critical("no account id in this one");
        });

        assert!(
            captured.iter().all(|(_, name, _)| name != "account_id"),
            "did not expect an account_id field, got {captured:?}"
        );
    }
}
