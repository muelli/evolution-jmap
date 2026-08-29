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

#[cfg(test)]
mod tests {
    use std::cell::Cell;

    use super::retry_once_after;

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
