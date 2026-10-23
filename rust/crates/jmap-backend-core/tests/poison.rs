// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later
//
// Each test poisons a lock with a panic in a different thread, then reads the
// value through the helper.

use std::sync::{Mutex, RwLock};
use std::thread;

use jmap_backend_core::poison::{lock, read, write};

fn poison_rwlock(value: &RwLock<u32>) {
    thread::scope(|scope| {
        let _ = scope
            .spawn(|| {
                let mut guard = value.write().unwrap();
                *guard = 7;
                panic!("poison the lock");
            })
            .join();
    });
    assert!(value.is_poisoned());
}

#[test]
fn read_returns_the_value_of_a_poisoned_rwlock() {
    let value = RwLock::new(1);
    poison_rwlock(&value);
    assert_eq!(*read(&value), 7);
}

#[test]
fn write_changes_the_value_of_a_poisoned_rwlock() {
    let value = RwLock::new(1);
    poison_rwlock(&value);
    *write(&value) = 9;
    assert_eq!(*read(&value), 9);
}

#[test]
fn lock_returns_the_value_of_a_poisoned_mutex() {
    let value = Mutex::new(Some(1));
    thread::scope(|scope| {
        let _ = scope
            .spawn(|| {
                let mut guard = value.lock().unwrap();
                *guard = Some(5);
                panic!("poison the lock");
            })
            .join();
    });
    assert!(value.is_poisoned());
    assert_eq!(lock(&value).take(), Some(5));
    assert_eq!(*lock(&value), None);
}

#[test]
fn the_helpers_work_on_a_lock_that_is_not_poisoned() {
    let value = RwLock::new(String::from("a"));
    write(&value).push('b');
    assert_eq!(*read(&value), "ab");
    let slot = Mutex::new(3);
    *lock(&slot) += 1;
    assert_eq!(*lock(&slot), 4);
}
