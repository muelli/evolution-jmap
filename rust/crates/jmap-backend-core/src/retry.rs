// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! The one retry-on-stale-credentials control flow, written once.
//!
//! `docs/ROADMAP.md` item 23: a long-lived connection (the calendar, address
//! book and mail backends each keep one open across many operations) can
//! outlive the OAuth 2.0 access token it was built with. Today a 401 on such
//! a connection is escalated straight to a fresh consent window, even though
//! the stored refresh token is still good — the fix is "on 401, fetch a new
//! access token, install it, and retry the operation once; only a 401 on the
//! freshly refreshed token may escalate."
//!
//! That sentence is the same for `jmap-backend-cal`, `jmap-backend-book` and
//! `jmap-mail`'s connection-retry wiring, so it is written here once rather
//! than copied three times into three different `unsafe fn`s where a copy
//! could quietly drift from the other two. What is deliberately *not* here is
//! any of the FFI: fetching a fresh token
//! ([`crate::oauth2::access_token`]), installing it
//! (`jmap_client::Client::set_credentials`), and deciding "was that failure a
//! 401" (each backend's own `SyncError`/`StoreError::is_unauthorized`) all
//! stay the caller's job, passed in as plain closures — which is what makes
//! this generic function testable with no `ESource`, no `GError`, and no
//! GObject vfunc in sight.
//!
//! [`retry_on_authentication_failure`] is the same sentence one layer down,
//! for the callers whose "attempt" is an EDS vfunc body — `gboolean` plus a
//! `GError **` rather than a `Result`. It is here rather than in either
//! backend because the delicate part is not the control flow but the `GError`
//! bookkeeping a second attempt implies, and written here it is testable with
//! plain closures: no `ESource`, no registry, no GObject instance.

use std::ptr;

use glib_sys::{GError, GFALSE, g_error_free, gboolean};

use crate::error::{is_authentication_failed, set_raw_gerror};

/// Runs `attempt`. If it fails with an error `is_retryable` recognises, runs
/// `refresh`; when `refresh` reports success, runs `attempt` exactly one more
/// time and returns *that* outcome instead — even if it fails again. If
/// `refresh` reports failure, or the first failure was not retryable, the
/// original failure is returned untouched and `refresh` is not called.
///
/// `refresh` reports success as `bool` rather than a `Result` because none of
/// its own failure detail changes what a caller does with it — the original
/// error is what gets reported either way (a caller wanting the refresh
/// failure's own detail can still read it, e.g. via `tracing`, before
/// returning `false`).
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
/// Runs `attempt`. If it answers `FALSE` having reported an
/// [authentication failure][is_authentication_failed], runs `refresh`; when
/// that reports success, runs `attempt` exactly once more and answers *that*.
/// Whatever error the final attempt left is moved into `error`.
///
/// ## Why the attempts do not write to `error` directly
///
/// Two reasons, both of which have teeth:
///
/// - [`set_raw_gerror`] deliberately keeps the **first** `GError` written to a
///   slot and frees any later one, mirroring `g_set_error`'s own
///   `g_return_if_fail (err == NULL || *err == NULL)`. A retry writing into a
///   slot that still held the first attempt's 401 would therefore report the
///   spent 401 and silently discard whatever actually went wrong the second
///   time — which is precisely the consent window this function exists to
///   stop, now raised for an unrelated failure.
/// - `error` is allowed to be NULL ("the caller does not want the error"),
///   and a NULL slot carries no domain or code to read back, so the refresh
///   would never happen for such a caller. A private slot is asked the
///   question either way, and `set_raw_gerror` frees rather than leaks when
///   it turns out nobody wanted the answer.
///
/// So the first attempt's `GError` is freed and the slot re-nulled before the
/// retry, and exactly one error — the last attempt's — reaches `error`.
///
/// `attempt` is `FnMut` rather than `FnOnce` because it may run twice; every
/// EDS vfunc body in this tree captures only `Copy` raw pointers and satisfies
/// that as written. It must be safe to run twice, which is a real constraint
/// on the caller: an attempt that half-filled its out-parameters before
/// failing would have them filled again. The vfuncs here write theirs only in
/// their success tail.
///
/// # Safety
///
/// `error` must be NULL or a valid, currently-NULL `GError **`, and `attempt`
/// must treat the pointer it is handed the same way — that is, exactly the
/// contract an EDS vfunc is already called under.
pub unsafe fn retry_on_authentication_failure(
    error: *mut *mut GError,
    mut attempt: impl FnMut(*mut *mut GError) -> gboolean,
    refresh: impl FnOnce() -> bool,
) -> gboolean {
    let mut slot: *mut GError = ptr::null_mut();
    let mut outcome = attempt(&mut slot);

    // SAFETY: `slot` is this frame's own, currently-NULL `GError **`, and
    // after the attempt it is NULL or a `GError` this frame owns.
    if outcome == GFALSE && unsafe { is_authentication_failed(slot) } && refresh() {
        // SAFETY: non-NULL, since `is_authentication_failed` said so, and
        // owned here. Re-nulling is what makes the slot reusable.
        unsafe { g_error_free(slot) };
        slot = ptr::null_mut();
        outcome = attempt(&mut slot);
    }

    if !slot.is_null() {
        // SAFETY: `error` satisfies the contract by this function's own, and
        // `slot` is a `GError` whose ownership passes to the call — which
        // frees it if `error` is NULL.
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

    /// What an `ops::*` call does on failure: fill the `GError **` it was
    /// handed and answer FALSE.
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
        // SAFETY: the caller's contract: a GError ownership of which has
        // passed to this test.
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
        // The point of the whole exercise: the caller — EDS — never sees the
        // 401, so nothing prepares a consent window.
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
        // SAFETY: ownership of the reported GError is the caller's, i.e. ours.
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
        // The bookkeeping this whole function exists for. `set_raw_gerror`
        // deliberately keeps the *first* `GError` written to a slot and frees
        // the second, so a retry into a slot still holding the 401 would
        // report the stale 401 and silently discard what actually went wrong
        // the second time — i.e. re-consent for a failure that had nothing to
        // do with credentials.
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
        // Item 23's own "only a 401 on the freshly refreshed token may
        // escalate": the second 401 is the one EDS is allowed to turn into a
        // consent window, so it must arrive, and exactly once.
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
        // does in places. The refresh must not depend on the caller having
        // asked for the failure detail — which is exactly why the attempts
        // run against a private slot rather than the caller's pointer.
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
        // The second `attempt` must never run: a failed refresh means there is
        // no fresher credential to retry with, and retrying anyway would just
        // reproduce the same 401.
        assert_eq!(attempts.get(), 1);
    }

    #[test]
    fn a_401_on_the_freshly_refreshed_token_is_returned_as_is() {
        // The retried attempt's own failure is what item 23's own text says
        // "may escalate" — this pins that it is returned unchanged, not
        // retried again or swallowed.
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
