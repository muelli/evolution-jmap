// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! The `GPtrArray` of strings that Camel gives a vfunc: uids or search words.

use std::ffi::CString;

use glib_sys::{GPtrArray, GTRUE, g_ptr_array_add, g_ptr_array_free, g_ptr_array_new, gpointer};

/// A `GPtrArray` of NUL-terminated strings, owned for the length of one call.
pub struct StringArray {
    array: *mut GPtrArray,
    strings: Vec<CString>,
}

impl StringArray {
    pub fn of<S: AsRef<str>>(strings: &[S]) -> Self {
        let strings: Vec<CString> = strings
            .iter()
            .map(|string| CString::new(string.as_ref()).expect("a string with no NUL"))
            .collect();
        // SAFETY: a fresh array, filled with pointers into strings that this
        // same value owns. The strings stay alive as long as the array does.
        let array = unsafe {
            let array = g_ptr_array_new();
            for string in &strings {
                g_ptr_array_add(array, string.as_ptr() as gpointer);
            }
            array
        };
        Self { array, strings }
    }

    /// The array, valid while this value lives.
    pub fn as_ptr(&self) -> *mut GPtrArray {
        self.array
    }
}

impl Drop for StringArray {
    fn drop(&mut self) {
        // SAFETY: the one array, allocated above. TRUE frees its pointer
        // block. The array has no element free function, so the strings
        // in `self.strings` stay.
        unsafe { g_ptr_array_free(self.array, GTRUE) };
        self.strings.clear();
    }
}
