// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later
//
// The recipe tests of four crates load the documented `.source` files with
// `RegistrySource`. A wrong uid, a leak or an early release breaks them all.

use std::ffi::CStr;
use std::path::{Path, PathBuf};
use std::ptr;

use eds_sys::e_source_get_uid;
use glib_sys::gpointer;
use gobject_sys::{GObject, g_object_add_weak_pointer};
use jmap_backend_core::source::RegistrySource;

fn example(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../../docs/examples")
        .join(name)
}

#[test]
fn the_uid_is_the_file_name_without_the_extension() {
    let source = RegistrySource::load(&example("jmap-mock.source"));

    // SAFETY: a live source. The uid belongs to the source.
    let uid = unsafe { CStr::from_ptr(e_source_get_uid(source.as_ptr())) };
    assert_eq!(uid.to_str(), Ok("jmap-mock"));
}

#[test]
fn drop_releases_the_only_reference() {
    let source = RegistrySource::load(&example("jmap-mock.source"));
    let mut weak: *mut GObject = source.as_ptr().cast();

    // SAFETY: a live object, and `weak` lives longer than the object.
    unsafe { g_object_add_weak_pointer(weak, ptr::addr_of_mut!(weak).cast::<gpointer>()) };
    drop(source);

    assert!(weak.is_null(), "the source is still alive after drop");
}

#[test]
#[should_panic(expected = "the registry could not read")]
fn a_file_that_is_no_keyfile_panics_with_the_reason() {
    let _ = RegistrySource::load(&Path::new(env!("CARGO_MANIFEST_DIR")).join("src/lib.rs"));
}
