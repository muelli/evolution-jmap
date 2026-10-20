// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! The one retry-on-stale-credentials control flow, written once.
//!
//! A long-lived connection (the calendar, address book and mail backends each
//! keep one open across many operations) can live longer than the OAuth 2.0
//! access token it uses. Without this module, a 401 on such a connection goes
//! directly to a new consent window, although the stored refresh token is
//! still good. The fix: "on 401, fetch a new access token, install it, and
//! retry the operation once; only a 401 on the freshly refreshed token may
//! escalate."
//!
//! `jmap-backend-cal`, `jmap-backend-book` and the connection-retry code of
//! `jmap-mail` all use this rule. It is here once, so that three copies in
//! three different `unsafe fn`s cannot drift apart. This module deliberately
//! contains no FFI. The caller supplies these steps as plain closures:
//!
//! - fetch a new token ([`crate::oauth2::access_token`]),
//! - install it (`jmap_client::Client::set_credentials`),
//! - decide if the failure was a 401 (each backend's own
//!   `SyncError`/`StoreError::is_unauthorized`).
//!
//! Thus the tests of this generic function need no `ESource`, no `GError` and
//! no GObject vfunc.
//!
//! [`retry_on_authentication_failure`] is the same rule one layer down, for
//! callers whose "attempt" is an EDS vfunc body: `gboolean` plus a
//! `GError **` rather than a `Result`. It is here rather than in a backend
//! because the difficult part is the `GError` bookkeeping for a second
//! attempt, not the control flow. Here, plain closures can test it: no
//! `ESource`, no registry, no GObject instance.

use std::ptr;

use glib_sys::{GError, GFALSE, g_error_free, gboolean};

use crate::error::{is_authentication_failed, set_raw_gerror};

/// Runs `attempt`. If it fails with an error that `is_retryable` recognises,
/// runs `refresh`. If `refresh` reports success, runs `attempt` exactly one
/// more time and returns *that* outcome, also if it fails again.
/// If the first failure is not retryable, returns it unchanged and does not
/// call `refresh`. If `refresh` reports failure, returns the original failure
/// unchanged.
///
/// `refresh` reports success as `bool`, not as a `Result`: its failure detail
/// does not change what the caller does. The function returns the original
/// error in both cases. A caller that wants the detail of the refresh failure
/// can read it (for example, with `tracing`) before it returns `false`.
pub fn retry_once_after<T, E>(
    mut attempt: impl FnMut() -> Result<T, E>,
    is_retryable: impl FnOnce(&E) -> bool,
    refresh: impl FnOnce() -> bool,
) -> Result<T, E> {
    match attempt() {
        Err(error) if is_retryable(&error) => {
            if refresh() {
                attempt()
            } else {
                Err(error)
            }
        }
        other => other,
    }
}

/// The same control flow for a caller whose "attempt" is an EDS vfunc body:
/// `TRUE`/`FALSE` plus a `GError **`, rather than a `Result`.
///
/// Runs `attempt`. If it returns `FALSE` with an
/// [authentication failure][is_authentication_failed], runs `refresh`. If
/// `refresh` reports success, runs `attempt` exactly once more and returns
/// *that* result. The error of the last attempt (if any) moves into `error`.
///
/// ## Why the attempts do not write to `error` directly
///
/// There are two reasons, and both are important:
///
/// - [`set_raw_gerror`] deliberately keeps the **first** `GError` written to a
///   slot and frees all later ones. This mirrors `g_set_error`'s own
///   `g_return_if_fail (err == NULL || *err == NULL)`. If a retry wrote into a
///   slot that still held the 401 of the first attempt, the function would
///   report that old 401 and silently discard the real second failure. The
///   result is the consent window that this function must prevent, now for an
///   unrelated failure.
/// - `error` can be NULL ("the caller does not want the error"). A NULL slot
///   has no domain or code to read back, so the refresh would never occur for
///   such a caller. A private slot always gives the answer, and
///   `set_raw_gerror` frees it (no leak) if the caller does not want it.
///
/// Thus the function frees the `GError` of the first attempt and sets the slot
/// to NULL again before the retry. Exactly one error (from the last attempt)
/// gets to `error`.
///
/// `attempt` is `FnMut` rather than `FnOnce` because it can run two times.
/// Each EDS vfunc body in this tree captures only `Copy` raw pointers, so it
/// satisfies this as written. `attempt` must be safe to run two times. This is
/// a real constraint on the caller: if an attempt partly fills its
/// out-parameters before it fails, the retry fills them again. The vfuncs here
/// write their out-parameters only in their success tail.
///
/// # Safety
///
/// `error` must be NULL or a valid, currently-NULL `GError **`. `attempt` must
/// treat the pointer it receives the same way. This is the same contract under
/// which EDS calls a vfunc.
pub unsafe fn retry_on_authentication_failure(
    error: *mut *mut GError,
    mut attempt: impl FnMut(*mut *mut GError) -> gboolean,
    refresh: impl FnOnce() -> bool,
) -> gboolean {
    let mut slot: *mut GError = ptr::null_mut();
    let mut outcome = attempt(&mut slot);

    // SAFETY: `slot` is this frame's own `GError **`, NULL before the
    // attempt. After the attempt, it is NULL or a `GError` this frame owns.
    if outcome == GFALSE && unsafe { is_authentication_failed(slot) } && refresh() {
        // SAFETY: `slot` is not NULL (`is_authentication_failed` returned
        // true), and this frame owns it. Setting it to NULL again makes the
        // slot reusable.
        unsafe { g_error_free(slot) };
        slot = ptr::null_mut();
        outcome = attempt(&mut slot);
    }

    if !slot.is_null() {
        // SAFETY: `error` satisfies the contract of this function. Ownership
        // of the `GError` in `slot` passes to the call, which frees it if
        // `error` is NULL.
        unsafe { set_raw_gerror(error, slot) };
    }
    outcome
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::ffi::CStr;
    use std::ptr;

    use eds_sys::{E_CLIENT_ERROR_OTHER_ERROR, e_client_error_create, e_client_error_quark};
    use glib_sys::{GError, GFALSE, GTRUE};
    use jmap_client::Error;

    use super::{retry_on_authentication_failure, retry_once_after};
    use crate::error::{cstring_lossy, set_raw_gerror, to_gerror};

    /// What an `ops::*` call does on failure: fill the `GError **` it
    /// receives and return FALSE.
    unsafe fn fail_with(dest: *mut *mut GError, err: &Error) -> glib_sys::gboolean {
        unsafe { set_raw_gerror(dest, to_gerror(err)) };
        GFALSE
    }

    fn unauthorized() -> Error {
        Error::Http {
            status: 401,
            problem: None,
        }
    }

    /// The message of a `GError` an out-parameter received, freeing it.
    unsafe fn take_message(error: *mut GError) -> String {
        assert!(!error.is_null(), "the caller was handed no error at all");
        // SAFETY: the contract of the caller: this test owns the GError.
        unsafe {
            let message = CStr::from_ptr((*error).message)
                .to_string_lossy()
                .into_owned();
            glib_sys::g_error_free(error);
            message
        }
    }

    #[test]
    fn a_successful_attempt_neither_refreshes_nor_reports_an_error() {
        let attempts = Cell::new(0);
        let mut reported: *mut GError = ptr::null_mut();

        // SAFETY: `reported` is a writable, currently-NULL `GError *`.
        let result = unsafe {
            retry_on_authentication_failure(
                &mut reported,
                |_| {
                    attempts.set(attempts.get() + 1);
                    GTRUE
                },
                || panic!("a success must not refresh anything"),
            )
        };

        assert_eq!(result, GTRUE);
        assert_eq!(attempts.get(), 1);
        assert!(reported.is_null());
    }

    #[test]
    fn an_authentication_failure_refreshes_once_and_retries_once() {
        let attempts = Cell::new(0);
        let refreshes = Cell::new(0);
        let mut reported: *mut GError = ptr::null_mut();

        // SAFETY: as above.
        let result = unsafe {
            retry_on_authentication_failure(
                &mut reported,
                |dest| {
                    let n = attempts.get() + 1;
                    attempts.set(n);
                    if n == 1 {
                        fail_with(dest, &unauthorized())
                    } else {
                        GTRUE
                    }
                },
                || {
                    refreshes.set(refreshes.get() + 1);
                    true
                },
            )
        };

        assert_eq!(result, GTRUE);
        assert_eq!(attempts.get(), 2);
        assert_eq!(refreshes.get(), 1);
        // The purpose of the function: the caller (EDS) never sees the 401,
        // so nothing prepares a consent window.
        assert!(reported.is_null(), "the spent 401 leaked to the caller");
    }

    #[test]
    fn a_failure_that_is_not_a_401_is_reported_without_refreshing() {
        let attempts = Cell::new(0);
        let mut reported: *mut GError = ptr::null_mut();

        // SAFETY: as above.
        let result = unsafe {
            retry_on_authentication_failure(
                &mut reported,
                |dest| {
                    attempts.set(attempts.get() + 1);
                    fail_with(dest, &Error::Transport("no route to host".into()))
                },
                || panic!("only an authentication failure may refresh"),
            )
        };

        assert_eq!(result, GFALSE);
        assert_eq!(attempts.get(), 1);
        // SAFETY: the caller (this test) owns the reported GError.
        assert!(unsafe { take_message(reported) }.contains("no route to host"));
    }

    #[test]
    fn a_refresh_that_fails_reports_the_original_401_and_does_not_retry() {
        let attempts = Cell::new(0);
        let mut reported: *mut GError = ptr::null_mut();

        // SAFETY: as above.
        let result = unsafe {
            retry_on_authentication_failure(
                &mut reported,
                |dest| {
                    attempts.set(attempts.get() + 1);
                    fail_with(dest, &unauthorized())
                },
                || false,
            )
        };

        assert_eq!(result, GFALSE);
        assert_eq!(attempts.get(), 1);
        // SAFETY: as above.
        unsafe {
            assert_eq!((*reported).domain, e_client_error_quark());
            assert_eq!(
                (*reported).code,
                eds_sys::E_CLIENT_ERROR_AUTHENTICATION_FAILED as i32
            );
            glib_sys::g_error_free(reported);
        }
    }

    #[test]
    fn the_retrys_own_failure_replaces_the_spent_401_rather_than_being_dropped() {
        // This test checks the bookkeeping that is the purpose of the
        // function. `set_raw_gerror` deliberately keeps the *first* `GError`
        // written to a slot and frees the second. Thus a retry into a slot
        // that still holds the 401 would report the stale 401 and silently
        // discard the real second failure. The result is a new consent for a
        // failure that is not related to credentials.
        let attempts = Cell::new(0);
        let mut reported: *mut GError = ptr::null_mut();

        // SAFETY: as above.
        let result = unsafe {
            retry_on_authentication_failure(
                &mut reported,
                |dest| {
                    let n = attempts.get() + 1;
                    attempts.set(n);
                    let message = cstring_lossy(if n == 1 { "the first" } else { "the second" });
                    set_raw_gerror(
                        dest,
                        if n == 1 {
                            to_gerror(&unauthorized())
                        } else {
                            e_client_error_create(E_CLIENT_ERROR_OTHER_ERROR, message.as_ptr())
                        },
                    );
                    GFALSE
                },
                || true,
            )
        };

        assert_eq!(result, GFALSE);
        assert_eq!(attempts.get(), 2);
        // SAFETY: as above.
        assert_eq!(unsafe { take_message(reported) }, "the second");
    }

    #[test]
    fn a_repeated_401_after_a_successful_refresh_is_reported_rather_than_retried_again() {
        // Only a 401 on the freshly refreshed token may escalate. EDS can
        // change the second 401 into a consent window, so the second 401 must
        // arrive, and exactly once.
        let attempts = Cell::new(0);
        let refreshes = Cell::new(0);
        let mut reported: *mut GError = ptr::null_mut();

        // SAFETY: as above.
        let result = unsafe {
            retry_on_authentication_failure(
                &mut reported,
                |dest| {
                    attempts.set(attempts.get() + 1);
                    fail_with(dest, &unauthorized())
                },
                || {
                    refreshes.set(refreshes.get() + 1);
                    true
                },
            )
        };

        assert_eq!(result, GFALSE);
        assert_eq!(attempts.get(), 2);
        assert_eq!(refreshes.get(), 1);
        // SAFETY: as above.
        unsafe {
            assert_eq!(
                (*reported).code,
                eds_sys::E_CLIENT_ERROR_AUTHENTICATION_FAILED as i32
            );
            glib_sys::g_error_free(reported);
        }
    }

    #[test]
    fn a_caller_that_asked_for_no_error_still_gets_the_retry() {
        // GLib lets a caller pass NULL for "I do not want the error", and EDS
        // does this in some places. The refresh must not need a request for
        // the failure detail from the caller. For this reason, the attempts
        // use a private slot, not the pointer of the caller.
        let attempts = Cell::new(0);
        let refreshes = Cell::new(0);

        // SAFETY: NULL is GLib's own "the caller does not want the error".
        let result = unsafe {
            retry_on_authentication_failure(
                ptr::null_mut(),
                |dest| {
                    let n = attempts.get() + 1;
                    attempts.set(n);
                    if n == 1 {
                        fail_with(dest, &unauthorized())
                    } else {
                        GTRUE
                    }
                },
                || {
                    refreshes.set(refreshes.get() + 1);
                    true
                },
            )
        };

        assert_eq!(result, GTRUE);
        assert_eq!(attempts.get(), 2);
        assert_eq!(refreshes.get(), 1);
    }

    #[test]
    fn a_success_on_the_first_attempt_never_touches_refresh_or_retries() {
        let attempts = Cell::new(0);
        let refreshes = Cell::new(0);

        let result = retry_once_after(
            || {
                attempts.set(attempts.get() + 1);
                Ok::<_, &str>("ok")
            },
            |_: &&str| panic!("is_retryable must not be asked about a success"),
            || {
                refreshes.set(refreshes.get() + 1);
                true
            },
        );

        assert_eq!(result, Ok("ok"));
        assert_eq!(attempts.get(), 1);
        assert_eq!(refreshes.get(), 0);
    }

    #[test]
    fn a_non_retryable_failure_is_returned_without_refreshing() {
        let attempts = Cell::new(0);
        let refreshes = Cell::new(0);

        let result = retry_once_after(
            || {
                attempts.set(attempts.get() + 1);
                Err::<(), _>("not found")
            },
            |_| false,
            || {
                refreshes.set(refreshes.get() + 1);
                true
            },
        );

        assert_eq!(result, Err("not found"));
        assert_eq!(attempts.get(), 1);
        assert_eq!(refreshes.get(), 0);
    }

    #[test]
    fn a_retryable_failure_refreshes_once_and_retries_once() {
        let attempts = Cell::new(0);

        let result = retry_once_after(
            || {
                let n = attempts.get() + 1;
                attempts.set(n);
                if n == 1 { Err("stale token") } else { Ok(n) }
            },
            |_| true,
            || true,
        );

        assert_eq!(result, Ok(2));
        assert_eq!(attempts.get(), 2);
    }

    #[test]
    fn a_failed_refresh_returns_the_original_failure_without_retrying() {
        let attempts = Cell::new(0);

        let result = retry_once_after(
            || {
                attempts.set(attempts.get() + 1);
                Err::<(), _>("stale token")
            },
            |_| true,
            || false,
        );

        assert_eq!(result, Err("stale token"));
        // The second `attempt` must never run: after a failed refresh, there
        // is no newer credential for a retry. A retry would only cause the
        // same 401 again.
        assert_eq!(attempts.get(), 1);
    }

    #[test]
    fn a_401_on_the_freshly_refreshed_token_is_returned_as_is() {
        // The function returns a failure of the retry unchanged. It does not
        // retry again or discard the failure.

        let attempts = Cell::new(0);

        let result = retry_once_after(
            || {
                attempts.set(attempts.get() + 1);
                Err::<(), _>("still 401 after refresh")
            },
            |_| true,
            || true,
        );

        assert_eq!(result, Err("still 401 after refresh"));
        assert_eq!(attempts.get(), 2);
    }
}
