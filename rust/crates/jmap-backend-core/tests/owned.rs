// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! What `Owned<T>` promises about a GObject reference, measured on a real one.
//!
//! Each of these promises could be a comment instead. The ~15 sites in the
//! calendar backend's marshalling that unref by hand used comments. You cannot
//! check a comment that says "this reference is dropped on every path". You
//! can check a `ref_count` read before and after. Thus these tests do not test
//! a mapping or a vfunc. They monitor the refcount of a plain `G_TYPE_OBJECT`
//! instance. That is the only purpose of the wrapper, and no calendar
//! round-trip test can see it. An error here causes a double free or a leak in
//! another process. Neither shows as a failing assertion in a different test.

use std::ptr;

use gobject_sys::{
    G_TYPE_OBJECT, GObject, g_object_new_with_properties, g_object_ref, g_object_unref,
};
use jmap_backend_core::owned::Owned;

/// A fresh `GObject` with one reference, which the caller owns.
fn plain_object() -> *mut GObject {
    // SAFETY: `G_TYPE_OBJECT` is instantiable and has no construct
    // properties, so zero properties with NULL arrays is a complete call.
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
    // SAFETY: the caller guarantees a live instance. `ref_count` is a public
    // field of the `GObject` struct at the start of every instance.
    unsafe { (*object).ref_count }
}

#[test]
fn a_null_pointer_is_no_reference_at_all() {
    // SAFETY: NULL is explicitly allowed, and this test checks only NULL.
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

    // SAFETY: this caller owns the reference taken above and can give it.
    let owned = unsafe { Owned::from_raw(object) }.expect("a non-NULL pointer is a reference");
    // SAFETY: `object` is still live. The wrapper holds a reference, and this
    // scope holds a reference.
    assert_eq!(
        unsafe { strong_count(object) },
        2,
        "taking ownership must not add or drop a reference"
    );

    drop(owned);
    // SAFETY: this scope still holds the reference that it took at the top,
    // so the instance is live for the read.
    assert_eq!(
        unsafe { strong_count(object) },
        1,
        "drop must release the reference it was given, and only that one"
    );

    // SAFETY: this releases the last reference, which this scope owns.
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

    // SAFETY: this scope now owns both references and can release them.
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
        // SAFETY: `object` stays live for the full loop. This scope never
        // gives its own reference to the wrapper.
        let owned = unsafe {
            g_object_ref(object);
            Owned::from_raw(object)
        }
        .expect("a non-NULL pointer is a reference");
        drop(owned);
    }
    // An imbalance of one per cycle gives 1001 or 0 here. A count of 0 frees
    // the instance before this read. The loop is necessary because one wrong
    // cycle can still give a plausible number.
    // SAFETY: live instance, if the wrapper kept its promise.
    assert_eq!(unsafe { strong_count(object) }, 1);

    // SAFETY: this releases the last reference, which this scope owns.
    unsafe { g_object_unref(object) };
}
