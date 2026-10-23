// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later
//
// The tests of five crates make their `ESource`s with `OwnedSource`. A leak
// or an early release there breaks every one of those tests.

use std::ffi::CStr;
use std::ptr;

use eds_sys::e_source_get_uid;
use glib_sys::gpointer;
use gobject_sys::{GObject, g_object_add_weak_pointer};
use jmap_backend_core::source::OwnedSource;

#[test]
fn a_new_source_has_the_given_uid() {
    let source = OwnedSource::new("jmap-owned-source");

    // SAFETY: a live source. The uid belongs to the source.
    let uid = unsafe { CStr::from_ptr(e_source_get_uid(source.as_ptr())) };
    assert_eq!(uid.to_str(), Ok("jmap-owned-source"));
}

#[test]
fn drop_releases_the_only_reference() {
    let source = OwnedSource::new("jmap-owned-source-drop");
    let mut weak: *mut GObject = source.as_ptr().cast();

    // SAFETY: a live object, and `weak` lives longer than the object.
    unsafe { g_object_add_weak_pointer(weak, ptr::addr_of_mut!(weak).cast::<gpointer>()) };
    drop(source);

    assert!(weak.is_null(), "the source is still alive after drop");
}
