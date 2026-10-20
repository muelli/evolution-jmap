// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! `GCancellable` → [`CancelFlag`].
//!
//! EDS gives every vfunc a `GCancellable`. The JMAP client polls a
//! [`CancelFlag`] between requests. [`CancelBridge`] connects the two for the
//! duration of one operation. `g_cancellable_connect` fires the callback
//! immediately if the cancellable is already cancelled. Thus a request that
//! was aborted before the backend started never goes out.
//!
//! Intentionally, the bridge owns the connection for a *scope*, not for the
//! lifetime of the backend. Operations do not share a cancellable, and a
//! handler that stays connected would continue to fire at a flag that nobody
//! reads.
//!
//! A vfunc needs one call, [`observe`], because the flag alone is only half
//! of the path. The [`Client`] of a backend is built one time for each
//! account, at connect time. The cancellable vfunc runs long after that, and
//! has no access to the client. [`observe`] closes this gap: it installs the
//! bridged flag as the cancellation of every request that the *calling
//! thread* makes until the operation returns. [`CancelScope`] tells why the
//! thread is the correct place for it.
//!
//! [`Client`]: jmap_client::Client

use std::ffi::c_ulong;
use std::ptr;

use gio_sys::{GCancellable, g_cancellable_connect, g_cancellable_disconnect};
use glib_sys::gpointer;
use jmap_client::CancelFlag;
use jmap_client::transport::CancelScope;

use crate::callback::gcallback;

/// The cancellable of one operation, bridged and installed while the
/// operation runs.
///
/// A local of its vfunc holds it, and drops it when the vfunc returns. Thus
/// it belongs to the operation, not to the account.
pub struct Cancellation {
    /// Declared first, so that it drops first: the thread stops observing
    /// the flag before the handler that sets it is disconnected.
    ///
    /// `None` for a NULL cancellable (see [`observe`]).
    _scope: Option<CancelScope>,
    _bridge: CancelBridge,
}

/// Makes `cancellable` the cancellation of every request that this thread
/// sends, until the returned value drops.
///
/// A vfunc needs only one call at its start:
/// `let _cancel = unsafe { observe(cancellable) };`. Then all code below it
/// stops when the user pushes Stop. This includes a client built at connect
/// time, and layers that know nothing about GIO.
///
/// **NULL installs nothing.** The NULL cancellable of GIO means "this call
/// cannot be cancelled". Suppose that a flag that can never fire is
/// installed over the scope of the outer operation. It would not only fail
/// to cancel. It would also *hide* an outer cancellation that already
/// arrived. Thus the thread continues to observe its previous flag. That is
/// the honest meaning of "nothing to say".
///
/// # Safety
///
/// `cancellable` must be NULL or a valid `GCancellable` that outlives the
/// returned value. The cancellable that EDS and Camel pass to a sync vfunc
/// obeys this for the duration of that call.
pub unsafe fn observe(cancellable: *mut GCancellable) -> Cancellation {
    // SAFETY: this function has the same contract as the bridge.
    let bridge = unsafe { CancelBridge::new(cancellable) };
    let scope = (!cancellable.is_null()).then(|| CancelScope::install(bridge.flag()));
    Cancellation {
        _scope: scope,
        _bridge: bridge,
    }
}

/// A `GCancellable` observed through a [`CancelFlag`], disconnected on drop.
pub struct CancelBridge {
    cancellable: *mut GCancellable,
    handler: c_ulong,
    flag: CancelFlag,
}

impl CancelBridge {
    /// Connects to `cancellable`. It can be NULL, which in GIO means "this
    /// operation cannot be cancelled". Then the flag never fires.
    ///
    /// # Safety
    ///
    /// `cancellable` must be NULL or a valid `GCancellable` that outlives the
    /// returned bridge. EDS owns the cancellable that it passes to a vfunc
    /// for at least the duration of that call, which is the intended scope.
    pub unsafe fn new(cancellable: *mut GCancellable) -> Self {
        let flag = CancelFlag::new();
        if cancellable.is_null() {
            return Self {
                cancellable,
                handler: 0,
                flag,
            };
        }

        // The callback needs its own handle on the shared state.
        // `destroy_flag` frees the box when the handler is removed. This
        // includes the already-cancelled case, where GIO removes the handler
        // immediately.
        let data = Box::into_raw(Box::new(flag.clone())).cast::<std::ffi::c_void>();

        // SAFETY: `cancellable` is a valid GCancellable by the contract of this
        // function. `on_cancelled` has the signature that the "cancelled"
        // signal invokes.
        let handler = unsafe {
            g_cancellable_connect(
                cancellable,
                gcallback(on_cancelled as unsafe extern "C" fn(*mut GCancellable, gpointer)),
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

    /// The flag for the client. A clone of it is permitted and cheap. Every
    /// clone observes the same cancellation.
    pub fn flag(&self) -> &CancelFlag {
        &self.flag
    }
}

impl Drop for CancelBridge {
    fn drop(&mut self) {
        // A zero handler means a NULL cancellable, or a cancellable that was
        // already cancelled at connect time. In both cases, GIO has nothing
        // to disconnect and already freed the boxed flag.
        if self.handler == 0 || self.cancellable.is_null() {
            return;
        }
        // SAFETY: the handler id came from g_cancellable_connect on this same
        // cancellable, and is not disconnected yet.
        unsafe { g_cancellable_disconnect(self.cancellable, self.handler) };
        self.handler = 0;
        self.cancellable = ptr::null_mut();
    }
}

unsafe extern "C" fn on_cancelled(_cancellable: *mut GCancellable, data: gpointer) {
    // SAFETY: `data` is the CancelFlag boxed in `CancelBridge::new`. It stays
    // alive until `destroy_flag` runs. To set the flag cannot panic, so no
    // guard is necessary here.
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
    // and GIO calls this exactly one time for each connection.
    drop(unsafe { Box::from_raw(data.cast::<CancelFlag>()) });
}
