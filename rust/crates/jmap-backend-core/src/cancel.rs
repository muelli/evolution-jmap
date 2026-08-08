// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! `GCancellable` → [`CancelFlag`].
//!
//! EDS hands every vfunc a `GCancellable`; the JMAP client polls a
//! [`CancelFlag`] between requests. [`CancelBridge`] connects the two for the
//! duration of one operation: `g_cancellable_connect` fires the callback
//! immediately if the cancellable has already been cancelled, so a request
//! that was aborted before the backend was even entered never goes out.
//!
//! The bridge deliberately owns the connection for a *scope* rather than for
//! the lifetime of the backend: the same cancellable is not reused across
//! operations, and a handler left connected would keep firing at a flag
//! nobody reads.

use std::ffi::c_ulong;
use std::ptr;

use gio_sys::{GCancellable, g_cancellable_connect, g_cancellable_disconnect};
use glib_sys::gpointer;
use jmap_client::CancelFlag;

/// A `GCancellable` observed through a [`CancelFlag`], disconnected on drop.
pub struct CancelBridge {
    cancellable: *mut GCancellable,
    handler: c_ulong,
    flag: CancelFlag,
}

impl CancelBridge {
    /// Connects to `cancellable`, which may be NULL — GIO's spelling of
    /// "this operation cannot be cancelled", in which case the flag simply
    /// never fires.
    ///
    /// # Safety
    ///
    /// `cancellable` must be NULL or a valid `GCancellable` that outlives the
    /// returned bridge. EDS owns the one it passes to a vfunc for at least
    /// the duration of that call, which is the intended scope.
    pub unsafe fn new(cancellable: *mut GCancellable) -> Self {
        let flag = CancelFlag::new();
        if cancellable.is_null() {
            return Self {
                cancellable,
                handler: 0,
                flag,
            };
        }

        // The callback needs its own handle on the shared state; the box is
        // freed by `destroy_flag` when the handler goes away, including the
        // already-cancelled case where GIO destroys it immediately.
        let data = Box::into_raw(Box::new(flag.clone())).cast::<std::ffi::c_void>();

        // SAFETY: `cancellable` is a valid GCancellable per this function's
        // contract. GCallback is an erased function pointer, which is why the
        // transmute is how GIO's own bindings spell this; `on_cancelled` has
        // the signature the "cancelled" signal actually invokes.
        let handler = unsafe {
            g_cancellable_connect(
                cancellable,
                Some(std::mem::transmute::<
                    unsafe extern "C" fn(*mut GCancellable, gpointer),
                    unsafe extern "C" fn(),
                >(on_cancelled)),
                data,
                Some(destroy_flag),
            )
        };

        Self {
            cancellable,
            handler,
            flag,
        }
    }

    /// The flag to hand to the client. Cloning it is fine and cheap; every
    /// clone observes the same cancellation.
    pub fn flag(&self) -> &CancelFlag {
        &self.flag
    }
}

impl Drop for CancelBridge {
    fn drop(&mut self) {
        // A zero handler means either a NULL cancellable or a cancellable
        // that was already cancelled at connect time — in both cases GIO has
        // nothing left to disconnect and has already freed the boxed flag.
        if self.handler == 0 || self.cancellable.is_null() {
            return;
        }
        // SAFETY: the handler id came from g_cancellable_connect on this same
        // cancellable and has not been disconnected yet.
        unsafe { g_cancellable_disconnect(self.cancellable, self.handler) };
        self.handler = 0;
        self.cancellable = ptr::null_mut();
    }
}

unsafe extern "C" fn on_cancelled(_cancellable: *mut GCancellable, data: gpointer) {
    // SAFETY: `data` is the CancelFlag boxed in `CancelBridge::new`, which is
    // kept alive until `destroy_flag` runs. Setting the flag cannot panic, so
    // no guard is needed here.
    unsafe {
        if let Some(flag) = data.cast::<CancelFlag>().as_ref() {
            flag.cancel();
        }
    }
}

unsafe extern "C" fn destroy_flag(data: gpointer) {
    if data.is_null() {
        return;
    }
    // SAFETY: `data` is the Box::into_raw pointer from `CancelBridge::new`,
    // and GIO calls this exactly once per connection.
    drop(unsafe { Box::from_raw(data.cast::<CancelFlag>()) });
}
