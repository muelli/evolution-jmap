// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! A strong reference that keeps a widget alive across a nested main loop.
//!
//! `gtk_dialog_run` runs a nested main loop. In that loop, the shell can
//! destroy the window that holds the widget, and the last reference to the
//! widget can go. Then its qdata is freed, and a caller that still uses the
//! qdata after the dialog uses freed memory. [`Hold`] keeps the object and
//! its qdata until the caller ends. [`Hold::disposed`] tells the caller if a
//! destroy ran in the meantime: then the children of the widget are gone.

use gobject_sys::{GObject, g_object_ref, g_object_unref};
use jmap_backend_core::weak::WeakBackend;

/// A strong reference, plus a weak one that dispose clears.
pub struct Hold {
    object: *mut GObject,
    weak: Option<WeakBackend>,
}

impl Hold {
    /// Takes a strong and a weak reference to `object`.
    ///
    /// # Safety
    ///
    /// `object` must be a live GObject, on the main thread.
    pub unsafe fn new(object: *mut GObject) -> Self {
        // SAFETY: per the contract. `Drop` releases the reference.
        unsafe {
            g_object_ref(object.cast());
            Self {
                object,
                weak: Some(WeakBackend::new(object)),
            }
        }
    }

    /// True once dispose ran on the object, for example from
    /// `gtk_widget_destroy`. `g_object_run_dispose` clears weak references.
    pub fn disposed(&self) -> bool {
        self.weak
            .as_ref()
            .and_then(|weak| weak.with_strong(|_| ()))
            .is_none()
    }
}

impl Drop for Hold {
    fn drop(&mut self) {
        // Clear the weak reference before the object can finalize.
        drop(self.weak.take());
        // SAFETY: `new` took this reference.
        unsafe { g_object_unref(self.object.cast()) };
    }
}

#[cfg(test)]
mod tests {
    use std::ptr;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use glib_sys::gpointer;
    use gobject_sys::{
        G_TYPE_OBJECT, g_object_new_with_properties, g_object_run_dispose, g_object_set_data_full,
    };

    use super::*;

    unsafe extern "C" fn count_finalize(data: gpointer) {
        // SAFETY: the test passes a pointer to its live counter.
        unsafe { &*data.cast::<AtomicUsize>() }.fetch_add(1, Ordering::SeqCst);
    }

    /// A destroy during the hold shows as disposed, and the object finalizes
    /// only when the hold ends, not when its owner lets go.
    #[test]
    fn a_hold_outlives_a_dispose_and_the_last_other_reference() {
        let finalized = AtomicUsize::new(0);
        // SAFETY: a new plain GObject, and this test owns its reference.
        unsafe {
            let object =
                g_object_new_with_properties(G_TYPE_OBJECT, 0, ptr::null_mut(), ptr::null());
            // GObject frees qdata in finalize, not in dispose.
            g_object_set_data_full(
                object,
                c"probe".as_ptr(),
                ptr::from_ref(&finalized) as gpointer,
                Some(count_finalize),
            );
            let hold = Hold::new(object);
            assert!(!hold.disposed());
            g_object_run_dispose(object);
            g_object_unref(object);
            assert!(hold.disposed());
            assert_eq!(finalized.load(Ordering::SeqCst), 0);
            drop(hold);
        }
        assert_eq!(finalized.load(Ordering::SeqCst), 1);
    }
}
