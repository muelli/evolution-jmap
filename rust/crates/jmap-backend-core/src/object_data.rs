// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! A typed Rust value that a GObject owns as object data.
//!
//! The UI extensions keep their state in a box on the object that they
//! decorate. The object drops the box when it is finalized. [`ObjectData`]
//! pairs the key with the type, so `get` cannot cast to a different type.

use std::ffi::CStr;
use std::marker::PhantomData;

use glib_sys::gpointer;
use gobject_sys::{GObject, g_object_get_data, g_object_set_data_full};

/// The key of one value of type `T` in the object data of a GObject.
pub struct ObjectData<T: 'static> {
    key: &'static CStr,
    value: PhantomData<fn() -> T>,
}

impl<T: 'static> ObjectData<T> {
    /// Names the key. Use each key with one `ObjectData` only.
    pub const fn new(key: &'static CStr) -> Self {
        Self {
            key,
            value: PhantomData,
        }
    }

    /// Moves `value` into the object data of `object`.
    /// A value that is already set is dropped.
    ///
    /// # Safety
    ///
    /// `object` must be a live GObject. Call on the thread that uses the value.
    pub unsafe fn set(&self, object: *mut GObject, value: T) {
        let data = Box::into_raw(Box::new(value));
        // SAFETY: `object` is live per the contract. The object owns the box
        // from now on, and `drop_box::<T>` frees it with the type it has.
        unsafe {
            g_object_set_data_full(object, self.key.as_ptr(), data.cast(), Some(drop_box::<T>))
        };
    }

    /// The value in the object data of `object`, or `None` if none is set.
    ///
    /// # Safety
    ///
    /// `object` must be a live GObject. Do not keep the reference longer than
    /// the current main-loop callback, and do not call `set` while it is held.
    pub unsafe fn get<'a>(&self, object: *mut GObject) -> Option<&'a T> {
        // SAFETY: `object` is live per the contract. Only `set` writes this
        // key, so the data is NULL or a box of `T`.
        unsafe {
            g_object_get_data(object, self.key.as_ptr())
                .cast::<T>()
                .as_ref()
        }
    }
}

/// Frees a box that [`ObjectData::set`] gave to the object.
unsafe extern "C" fn drop_box<T>(data: gpointer) {
    // SAFETY: GObject calls this one time, with the pointer from `set`.
    drop(unsafe { Box::from_raw(data.cast::<T>()) });
}
