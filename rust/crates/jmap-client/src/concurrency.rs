// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Concurrency bounding for requests and uploads (RFC 8620 §2).
//!
//! Servers declare `maxConcurrentRequests` and `maxConcurrentUpload` in their
//! `urn:ietf:params:jmap:core` session capability.
//!
//! A conforming client must not issue more concurrent requests or uploads
//! than the server limit.

use std::sync::{Condvar, Mutex, PoisonError};
use std::time::Duration;

use crate::Error;
use crate::transport::CancelFlag;

/// Tracks in-flight operations against an advertised concurrency limit.
#[derive(Debug)]
pub(crate) struct ConcurrencyLimiter {
    state: Mutex<LimiterState>,
    cvar: Condvar,
}

#[derive(Debug)]
struct LimiterState {
    limit: usize,
    in_flight: usize,
}

impl ConcurrencyLimiter {
    /// Create a limiter with an optional limit.
    ///
    /// When `limit` is `None`, concurrency is unbounded.
    /// When `limit` is `Some(n)`, at most `max(1, n)` operations run at once.
    pub(crate) fn new(limit: Option<u64>) -> Self {
        let limit = parse_limit(limit);
        Self {
            state: Mutex::new(LimiterState {
                limit,
                in_flight: 0,
            }),
            cvar: Condvar::new(),
        }
    }

    /// Update the concurrency limit, e.g. after a session refresh.
    pub(crate) fn set_limit(&self, limit: Option<u64>) {
        let new_limit = parse_limit(limit);
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        state.limit = new_limit;
        self.cvar.notify_all();
    }

    /// Acquire a permit to execute one operation.
    ///
    /// Blocks until an in-flight slot becomes available or the request is cancelled.
    pub(crate) fn acquire<'a>(
        &'a self,
        cancel: Option<&CancelFlag>,
    ) -> Result<ConcurrencyPermit<'a>, Error> {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        while state.in_flight >= state.limit {
            if cancel.is_some_and(CancelFlag::is_cancelled) {
                return Err(Error::Cancelled);
            }
            let (next_state, _timeout) = self
                .cvar
                .wait_timeout(state, Duration::from_millis(50))
                .unwrap_or_else(PoisonError::into_inner);
            state = next_state;
        }
        if cancel.is_some_and(CancelFlag::is_cancelled) {
            return Err(Error::Cancelled);
        }
        state.in_flight += 1;
        Ok(ConcurrencyPermit { limiter: self })
    }

    fn release(&self) {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        state.in_flight = state.in_flight.saturating_sub(1);
        self.cvar.notify_one();
    }
}

fn parse_limit(limit: Option<u64>) -> usize {
    match limit {
        Some(0) => 1,
        Some(n) => n.try_into().unwrap_or(usize::MAX),
        None => usize::MAX,
    }
}

/// A permit representing an active slot under a [`ConcurrencyLimiter`].
///
/// Dropping the permit decrements the in-flight counter and wakes a waiting thread.
#[derive(Debug)]
pub(crate) struct ConcurrencyPermit<'a> {
    limiter: &'a ConcurrencyLimiter,
}

impl Drop for ConcurrencyPermit<'_> {
    fn drop(&mut self) {
        self.limiter.release();
    }
}
