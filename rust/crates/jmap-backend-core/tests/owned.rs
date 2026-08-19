// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! What `Owned<T>` promises about a GObject reference, measured on a real one.
//!
//! Every one of these could be written as a comment instead — and that is what
//! the ~15 hand-unreffed sites `docs/UNSAFE-AUDIT.md` Pattern C found were
//! doing. A comment saying "this reference is dropped on every path" is not
//! checkable; a `ref_count` read before and after is. So these tests do not
//! exercise a mapping or a vfunc: they watch the refcount of a plain
//! `G_TYPE_OBJECT` instance, which is the one thing the wrapper is for and the
//! one thing no calendar round-trip test can see. Getting this wrong is a
//! double free or a leak in someone else's process, neither of which shows up
//! as a failing assertion anywhere else.

use std::ptr;

use gobject_sys::{
    G_TYPE_OBJECT, GObject, g_object_new_with_properties, g_object_ref, g_object_unref,
};
use jmap_backend_core::owned::Owned;

/// A fresh `GObject` with one reference, which the caller owns.
fn plain_object() -> *mut GObject {
    // SAFETY: `G_TYPE_OBJECT` is instantiable and takes no construct
    // properties, so zero of them with NULL arrays is a complete call.
    let object =
        unsafe { g_object_new_with_properties(G_TYPE_OBJECT, 0, ptr::null_mut(), ptr::null()) };
    assert!(
        !object.is_null(),
        "g_object_new_with_properties returned NULL"
    );
    object
}

/// The instance's strong reference count.
///
/// # Safety
///
/// `object` must be a live `GObject` instance.
unsafe fn strong_count(object: *mut GObject) -> u32 {
    // SAFETY: the caller guarantees a live instance, and `ref_count` is a
    // public field of the `GObject` struct every instance starts with.
    unsafe { (*object).ref_count }
}

#[test]
fn a_null_pointer_is_no_reference_at_all() {
    // SAFETY: NULL is explicitly allowed, and is the whole question here.
    let owned = unsafe { Owned::<GObject>::from_raw(ptr::null_mut()) };
    assert!(owned.is_none(), "NULL should not become an owned reference");
}

#[test]
fn dropping_it_releases_exactly_one_reference() {
    let object = plain_object();
    // SAFETY: `object` is live and this is the reference the wrapper takes.
    unsafe { g_object_ref(object) };
    // SAFETY: as above.
    assert_eq!(unsafe { strong_count(object) }, 2);

    // SAFETY: the reference taken above is this caller's to hand over.
    let owned = unsafe { Owned::from_raw(object) }.expect("a non-NULL pointer is a reference");
    // SAFETY: `object` is still live — the wrapper holds a reference and so
    // does this scope.
    assert_eq!(
        unsafe { strong_count(object) },
        2,
        "taking ownership must not add or drop a reference"
    );

    drop(owned);
    // SAFETY: the reference this scope took at the top is still held, so the
    // instance is alive to be read.
    assert_eq!(
        unsafe { strong_count(object) },
        1,
        "drop must release the reference it was given, and only that one"
    );

    // SAFETY: releasing this scope's own reference, the last one.
    unsafe { g_object_unref(object) };
}

#[test]
fn as_ptr_hands_back_the_same_pointer_without_giving_it_away() {
    let object = plain_object();
    // SAFETY: `object` is live; this is the reference the wrapper takes.
    let owned = unsafe {
        g_object_ref(object);
        Owned::from_raw(object)
    }
    .expect("a non-NULL pointer is a reference");

    assert!(ptr::eq(owned.as_ptr(), object));
    // SAFETY: live instance.
    assert_eq!(
        unsafe { strong_count(object) },
        2,
        "reading the pointer must not change the count"
    );

    drop(owned);
    // SAFETY: this scope's reference is still held.
    unsafe {
        assert_eq!(strong_count(object), 1);
        g_object_unref(object);
    }
}

#[test]
fn into_raw_gives_the_reference_away_and_does_not_release_it() {
    let object = plain_object();
    // SAFETY: `object` is live; this is the reference the wrapper takes.
    let owned = unsafe {
        g_object_ref(object);
        Owned::from_raw(object)
    }
    .expect("a non-NULL pointer is a reference");

    let raw = owned.into_raw();
    assert!(ptr::eq(raw, object));
    // SAFETY: two references are outstanding, so the instance is live.
    assert_eq!(
        unsafe { strong_count(object) },
        2,
        "into_raw passes the reference on; releasing it there would be a use-after-free \
         for whoever it was handed to"
    );

    // SAFETY: both references are this scope's to release now.
    unsafe {
        g_object_unref(raw);
        assert_eq!(strong_count(object), 1);
        g_object_unref(object);
    }
}

#[test]
fn a_thousand_take_and_drop_cycles_leave_the_count_where_they_found_it() {
    let object = plain_object();
    for _ in 0..1000 {
        // SAFETY: `object` stays live throughout — this scope's own reference
        // is never handed to the wrapper.
        let owned = unsafe {
            g_object_ref(object);
            Owned::from_raw(object)
        }
        .expect("a non-NULL pointer is a reference");
        drop(owned);
    }
    // An imbalance of one per cycle would read 1001 or 0 here — and a count of
    // 0 would already have freed the instance out from under this read, which
    // is what makes the repetition worth the loop: a single cycle can be wrong
    // and still land on a plausible number.
    // SAFETY: live instance, if the wrapper kept its promise.
    assert_eq!(unsafe { strong_count(object) }, 1);

    // SAFETY: this scope's own reference, the last one.
    unsafe { g_object_unref(object) };
}
