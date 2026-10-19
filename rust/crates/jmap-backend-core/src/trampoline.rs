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
/// `G_DEBUG=fatal-criticals` — and, if a module has called
/// [`crate::logging::init`], as a `tracing::error!` event too, so it also
/// reaches journald down that path with the rest of this project's events.
///
/// For the cases a vfunc cannot report any other way: it has no `GError`
/// out-parameter, or the caller is GObject itself and there is nothing to hand
/// an error to. Anything a *user* could act on belongs in a `GError` instead —
/// a critical is for "this cannot happen", not for a misconfigured account.
pub fn log_critical(message: &str) {
    tracing::error!("{message}");
    g_log_critical(message);
}

/// Like [`log_critical`], but also attaches `message_uid` as a structured
/// `tracing` field, so a hostile or damaged message can be picked out by
/// `journalctl EVOLUTION_JMAP_MESSAGE_UID=<uid>` (or any other structured
/// consumer) rather than only by grepping the free-text message. The `GError`/
/// `g_log` side is unchanged — `message` still carries the uid in prose too,
/// for anything reading this log the way it always has.
pub fn log_critical_for_message(message_uid: &str, message: &str) {
    tracing::error!(message_uid, "{message}");
    g_log_critical(message);
}

/// Like [`log_critical`], but also attaches `resource_id` as a structured
/// `tracing` field, so a failure in one child of a collection account (a
/// mailbox, address book or calendar EDS refused to create, abandon, or
/// remove) can be picked out by `journalctl
/// EVOLUTION_JMAP_RESOURCE_ID=<resource_id>` rather than only by grepping the
/// free-text message. The `GError`/`g_log` side is unchanged.
pub fn log_critical_for_resource(resource_id: &str, message: &str) {
    tracing::error!(resource_id, "{message}");
    g_log_critical(message);
}

/// Like [`log_critical`], but also attaches `account_id` as a structured
/// `tracing` field, so a failure that has no single resource to blame — a
/// credentials lookup or a populate/fan-out summary spanning a whole account's
/// children — can still be picked out by `journalctl
/// EVOLUTION_JMAP_ACCOUNT_ID=<account_id>`. The `GError`/`g_log` side is
/// unchanged.
pub fn log_critical_for_account(account_id: &str, message: &str) {
    tracing::error!(account_id, "{message}");
    g_log_critical(message);
}

/// The `g_log` half [`log_critical`] and [`log_critical_for_message`] share.
fn g_log_critical(message: &str) {
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
    use std::cell::RefCell;
    use std::os::raw::c_char;
    use std::sync::OnceLock;

    use glib_sys::{GLogLevelFlags, gpointer};
    use jmap_tracing_capture::capture;

    use super::*;

    thread_local! {
        static GLOG_SCOPE: RefCell<Option<Vec<String>>> = const { RefCell::new(None) };
    }

    /// Records a `g_log` call into whichever buffer the calling thread's
    /// [`capture_g_log_critical`] has open, the same per-thread routing
    /// `jmap_tracing_capture` uses and for the same reason: several tests in
    /// this binary call a `log_critical*` function at once, on their own
    /// threads, and must not see each other's messages.
    unsafe extern "C" fn record(
        _domain: *const c_char,
        _level: GLogLevelFlags,
        message: *const c_char,
        _user_data: gpointer,
    ) {
        // SAFETY: GLib hands every registered log handler a NUL-terminated
        // string it owns; this function neither frees nor outlives it.
        let text = unsafe { std::ffi::CStr::from_ptr(message) }
            .to_string_lossy()
            .into_owned();
        GLOG_SCOPE.with(|scope| {
            if let Some(buf) = scope.borrow_mut().as_mut() {
                buf.push(text);
            }
        });
    }

    /// Runs `run` and returns the `g_log` criticals under
    /// [`LOG_DOMAIN`] it caused, in place of GLib's own default handler
    /// (which only prints them). Installed once per process: `g_log_set_handler`
    /// has no matching "uninstall the one I just added" call that keeps the
    /// rest of GLib's default handling for other domains intact, so later
    /// callers share the one installation and are routed by their own thread.
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
