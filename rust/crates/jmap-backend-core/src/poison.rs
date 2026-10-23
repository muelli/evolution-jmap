// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Lock helpers that continue after a poisoned lock.
//!
//! A poisoned lock means that a different operation panicked while it held
//! the lock. The backends lock a session, a folder listing or a push slot.
//! A panic does not damage such a value, so it is better to continue than to
//! stop the backend. These helpers keep that decision in one place.

use std::sync::{Mutex, MutexGuard, PoisonError, RwLock, RwLockReadGuard, RwLockWriteGuard};

/// Gets a read lock, also when the lock is poisoned.
pub fn read<T>(lock: &RwLock<T>) -> RwLockReadGuard<'_, T> {
    lock.read().unwrap_or_else(PoisonError::into_inner)
}

/// Gets a write lock, also when the lock is poisoned.
pub fn write<T>(lock: &RwLock<T>) -> RwLockWriteGuard<'_, T> {
    lock.write().unwrap_or_else(PoisonError::into_inner)
}

/// Locks a mutex, also when the mutex is poisoned.
pub fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}
