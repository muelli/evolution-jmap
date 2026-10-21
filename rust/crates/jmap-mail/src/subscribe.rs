// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! `CamelSubscribable`: the three methods behind the subscription check box
//! in the Evolution subscription editor.
//!
//! Camel uses an interface here, not `CamelStoreClass` vfuncs, because not all
//! stores have subscriptions. [`Subscribable`] fills the vtable, and the
//! `interfaces` of [`JmapStore`] declare it.
//!
//! [`JmapStore::set_subscribed`] does the write. This module adds these items:
//!
//! * **Path resolution.** Camel names a folder by its path. `tree_holding`
//!   refreshes the listing if the path is not in it, so a mailbox that another
//!   client created is subscribable without a restart.
//! * **A non-blocking read.** Camel declares that `folder_is_subscribed` must
//!   not send requests. Evolution calls it once per folder while it draws the
//!   tree. Thus it reads only [`JmapStore::held_folders`].
//! * **The signal.** The Camel wrapper does not emit `folder-subscribed`, so
//!   this module emits it (IMAPX does the same). Without it, the Evolution
//!   folder tree does not update.
//!
//! ## Tests
//!
//! `tests/subscriptions.rs` tests `folder_is_subscribed` and [`set_subscribed`]
//! against the mock server with [`JmapStore::detached`] stores.
//!
//! `tests/emissions.rs` tests the signal. It needs a store made through a
//! `CamelSession`, because `camel_subscribable_folder_*` queues the signal on
//! the main context of the session. A detached store is not a GObject.

use std::slice;

use eds_sys::{
    CamelStore, CamelSubscribable, CamelSubscribableInterface,
    camel_subscribable_folder_subscribed, camel_subscribable_folder_unsubscribed,
    camel_subscribable_get_type,
};
use gio_sys::GCancellable;
use glib_sys::{GError, GFALSE, GTRUE, GType, gboolean, gchar};
use jmap_backend_core::cancel::observe;
use jmap_backend_core::error::fail_bool;
use jmap_backend_core::marshal::read_string;
use jmap_backend_core::subclass::InterfaceImpl;
use jmap_backend_core::trampoline::{guard, guard_bool};
use jmap_mail_sync::FolderInfo;

use crate::connect::StoreError;
use crate::folder_info::FolderInfoChain;
use crate::folders::tree_holding;
use crate::store::JmapStore;

/// Tells if the user subscribed to the folder at `path`. Only the listing that
/// the store holds supplies the answer.
///
/// The result is `false` if the store has no listing or no folder has that path.
/// No data shows that the user subscribed, and `true` would mark an unknown
/// folder.
pub fn is_subscribed(store: &JmapStore, path: &str) -> bool {
    store
        .held_folders()
        .and_then(|tree| tree.find(path).map(|folder| folder.subscribed))
        .unwrap_or(false)
}

/// Writes the subscription state to the server for the folder at `path`.
///
/// Returns a copy of the updated folder, for the `folder-subscribed` signal. A
/// borrow would hold the lock that [`JmapStore::set_subscribed`] uses across
/// the emission.
///
/// A path with no folder is [`StoreError::NoFolder`]. This error is in the
/// store domain, not the service domain: a folder that another client deleted
/// is not an account problem.
pub fn set_subscribed(
    store: &JmapStore,
    path: &str,
    subscribed: bool,
) -> Result<FolderInfo, StoreError> {
    let tree = tree_holding(store, |tree| tree.find(path).is_some())?;
    let Some(folder) = tree.find(path) else {
        return Err(StoreError::NoFolder(path.to_owned()));
    };

    store.set_subscribed(&folder.id, subscribed)?;

    let mut folder = folder.clone();
    folder.subscribed = subscribed;
    Ok(folder)
}

// ---------------------------------------------------------------------------
// the vtable

/// Fills the `CamelSubscribableInterface` copy of `JmapStore`.
///
/// This is a separate type, as [`InterfaceImpl`] requires. A class can
/// implement many interfaces, but a trait on the class can describe only one.
pub struct Subscribable;

// SAFETY: `CamelSubscribableInterface` is the bindgen `#[repr(C)]` binding of
// the interface struct of `camel_subscribable_get_type`. Its first member is
// `GTypeInterface`. The eds-sys test tests/camel.rs checks the shape.
unsafe impl InterfaceImpl for Subscribable {
    type Vtable = CamelSubscribableInterface;

    fn gtype() -> GType {
        // SAFETY: no arguments, and the type initializes itself.
        unsafe { camel_subscribable_get_type() }
    }

    unsafe fn interface_init(vtable: *mut Self::Vtable) {
        // Set all three slots. Camel has no default for them, so the wrapper
        // calls through NULL if a slot is NULL.
        //
        // SAFETY: the contract of `InterfaceImpl::interface_init`: this is the
        // copy of the vtable for this type, and nothing else can access it yet.
        let vtable = unsafe { &mut *vtable };
        vtable.folder_is_subscribed = Some(folder_is_subscribed);
        vtable.subscribe_folder_sync = Some(subscribe_folder_sync);
        vtable.unsubscribe_folder_sync = Some(unsubscribe_folder_sync);
    }
}

/// The non-blocking read. It has no `GError` out-parameter, because it cannot
/// fail: an unknown folder is not subscribed.
unsafe extern "C" fn folder_is_subscribed(
    subscribable: *mut CamelSubscribable,
    folder_name: *const gchar,
) -> gboolean {
    guard("folder_is_subscribed", GFALSE, || {
        // SAFETY: the Camel contract for the vfunc: a valid instance of this
        // type, and a NUL-terminated string that Camel owns. `read_string`
        // copies it.
        let (Some(store), Some(path)) = (unsafe { borrow(subscribable) }, unsafe {
            read_string(folder_name)
        }) else {
            return GFALSE;
        };

        if is_subscribed(store, &path) {
            GTRUE
        } else {
            GFALSE
        }
    })
}

/// Subscribes to a folder and tells Camel.
///
/// The call [`observe`]s `cancellable`. It sends one `Mailbox/set` and at most
/// one listing request.
unsafe extern "C" fn subscribe_folder_sync(
    subscribable: *mut CamelSubscribable,
    folder_name: *const gchar,
    cancellable: *mut GCancellable,
    error: *mut *mut GError,
) -> gboolean {
    // SAFETY: the Camel contract for the vfunc: a valid instance of this type,
    // a NUL-terminated string, and an out-parameter that is NULL or writable and
    // currently NULL.
    unsafe {
        guard_bool("subscribe_folder_sync", error, || {
            // SAFETY: Camel keeps its cancellable alive for the full call, so it
            // outlives this observation. The observation stops every request
            // below when the user clicks Stop.
            let _cancel = observe(cancellable);

            change(subscribable, folder_name, true, error)
        })
    }
}

/// Unsubscribes from a folder. Camel has two slots for the two directions, but
/// JMAP has one `isSubscribed` value.
unsafe extern "C" fn unsubscribe_folder_sync(
    subscribable: *mut CamelSubscribable,
    folder_name: *const gchar,
    cancellable: *mut GCancellable,
    error: *mut *mut GError,
) -> gboolean {
    // SAFETY: the same as for [`subscribe_folder_sync`].
    unsafe {
        guard_bool("unsubscribe_folder_sync", error, || {
            // SAFETY: Camel keeps its cancellable alive for the full call, so it
            // outlives this observation. The observation stops every request
            // below when the user clicks Stop.
            let _cancel = observe(cancellable);

            change(subscribable, folder_name, false, error)
        })
    }
}

/// The shared body of the two sync vfuncs: resolve the path, write, and emit
/// the signal.
///
/// The signal gets a one-folder `CamelFolderInfo` (depth `Some(0)`, thus no
/// descendants). The signal only borrows it, so this function keeps the chain.
/// `camel_folder_info_free` runs when this function returns, as in IMAPX.
///
/// # Safety
///
/// `subscribable` must be a valid instance of [`JmapStore`], `folder_name` a
/// NULL-or-NUL-terminated string, and `error` NULL or a writable, currently
/// NULL `GError **`.
unsafe fn change(
    subscribable: *mut CamelSubscribable,
    folder_name: *const gchar,
    subscribed: bool,
    error: *mut *mut GError,
) -> gboolean {
    // SAFETY: the contract above.
    let Some(store) = (unsafe { borrow(subscribable) }) else {
        // SAFETY: the contract above.
        return unsafe { fail_bool(error, &StoreError::Disconnected, StoreError::to_gerror) };
    };
    // Camel owns the NUL-terminated name, and `read_string` copies it. A NULL or
    // empty name becomes an empty path, and no mailbox has an empty path.
    // SAFETY: the contract above.
    let path = unsafe { read_string(folder_name) }.unwrap_or_default();

    let folder = match set_subscribed(store, &path, subscribed) {
        Ok(folder) => folder,
        // SAFETY: the contract above.
        Err(failure) => return unsafe { fail_bool(error, &failure, StoreError::to_gerror) },
    };

    let announcement = FolderInfoChain::from_forest(slice::from_ref(&folder), Some(0));
    // SAFETY: `subscribable` is the live instance borrowed above. The chain is
    // alive until this function returns, and the signal only borrows it.
    unsafe {
        if subscribed {
            camel_subscribable_folder_subscribed(subscribable, announcement.as_ptr());
        } else {
            camel_subscribable_folder_unsubscribed(subscribable, announcement.as_ptr());
        }
    }
    GTRUE
}

/// Gives the Rust view of the `CamelSubscribable *` from Camel.
///
/// # Safety
///
/// The same as for [`JmapStore::borrow`]: `subscribable` must be NULL or point
/// to an instance of [`JmapStore`]. The cast is sound because the interface
/// has the prerequisite `CamelStore` (the eds-sys test tests/camel.rs checks
/// this). Thus each instance that receives this vfunc is a `CamelStore`.
unsafe fn borrow<'a>(subscribable: *mut CamelSubscribable) -> Option<&'a JmapStore> {
    unsafe { JmapStore::borrow(subscribable.cast::<CamelStore>()) }
}
