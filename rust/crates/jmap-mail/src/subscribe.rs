// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! `CamelSubscribable`: the tick beside a folder in Evolution's subscription
//! editor, and the three methods behind it.
//!
//! An interface rather than more `CamelStoreClass` vfuncs, because not every
//! store has the question: a Maildir has whatever folders are on the disk. So
//! this is the one part of the provider whose implementations are reached
//! through a vtable GObject hands the type rather than through our own class
//! struct — [`Subscribable`] is that vtable's filling, declared from
//! [`JmapStore`]'s `interfaces`.
//!
//! ## What the store already does, and what is left here
//!
//! [`JmapStore::set_subscribed`] is the write: one `Mailbox/set` and the edit
//! to the folder listing the store holds, which is what makes the answer to the
//! next question agree with it. What is left for this module is the shape of
//! the interface around that:
//!
//! * **A path, not a mailbox.** Camel names a folder by its path, so resolving
//!   it against the folder tree is this layer's job — through
//!   `tree_holding`, which looks again before
//!   giving up, so that a mailbox another client created since the last listing
//!   is subscribable without a restart.
//! * **A non-blocking read.** `folder_is_subscribed` is declared by Camel as
//!   one of the methods that may not go to the server; Evolution asks it once
//!   per folder while drawing the tree. So it reads
//!   [`JmapStore::held_folders`] and nothing else — an answer or nothing, never
//!   a request.
//! * **The signal.** Camel's wrapper does *not* emit `folder-subscribed` for
//!   the implementation; IMAPX emits it from inside its own vfunc and so does
//!   this one. Without it Evolution's folder tree keeps showing what it last
//!   drew until something else refreshes it.
//!
//! ## Where the two halves are tested
//!
//! Everything above the emission is `tests/subscriptions.rs`, which drives
//! `folder_is_subscribed` through the vtable and [`set_subscribed`] against the
//! mock server with [`JmapStore::detached`] stores.
//!
//! The emission itself is `tests/emissions.rs`, and it needs more: a store
//! instantiated through a `CamelSession`, because `camel_subscribable_folder_*`
//! queues the signal on the session's main context rather than emitting it. A
//! detached store is not even a GObject, so the two lines went untested until
//! that file's harness existed.

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

/// Whether the user asked to see this folder, answered out of the listing the
/// store is holding and out of nothing else.
///
/// `false` for a store with no listing and for a path no folder answers to.
/// That is the honest reading of a question about the *user's* wishes: nothing
/// in hand says they asked for this folder. It is also the conservative one —
/// `true` would put a tick on a folder this store knows nothing about.
pub fn is_subscribed(store: &JmapStore, path: &str) -> bool {
    store
        .held_folders()
        .and_then(|tree| tree.find(path).map(|folder| folder.subscribed))
        .unwrap_or(false)
}

/// Puts the user's tick on the server for the folder Camel named by path.
///
/// What comes back is that folder as it now is — the answer the
/// `folder-subscribed` signal is built from, which is why it is returned rather
/// than dropped. It is a copy: the listing the store holds is edited by
/// [`JmapStore::set_subscribed`] under its own lock, and handing out a borrow
/// of it would mean holding that lock across the emission.
///
/// A path no folder answers to is [`StoreError::NoFolder`] — the store's own
/// domain rather than the service's, because nothing is wrong with the account
/// when another client has deleted a folder this one still lists.
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

/// The filling of `JmapStore`'s copy of `CamelSubscribableInterface`.
///
/// A type beside the store rather than the store itself, which is what
/// [`InterfaceImpl`] asks for: a class may fill several interfaces, and a trait
/// implemented on the class could only describe one of them.
pub struct Subscribable;

// SAFETY: `CamelSubscribableInterface` is bindgen's `#[repr(C)]` binding of the
// interface struct `camel_subscribable_get_type` names, and it leads with
// `GTypeInterface` — eds-sys's tests/camel.rs pins the interface's shape.
unsafe impl InterfaceImpl for Subscribable {
    type Vtable = CamelSubscribableInterface;

    fn gtype() -> GType {
        // SAFETY: no arguments, and the type initialises itself.
        unsafe { camel_subscribable_get_type() }
    }

    unsafe fn interface_init(vtable: *mut Self::Vtable) {
        // All three, because Camel installs no default behind any of them: a
        // slot left NULL is a call through NULL from inside the wrapper.
        //
        // SAFETY: the contract of `InterfaceImpl::interface_init` — this is
        // our own copy of the vtable, and nothing else can reach it yet.
        let vtable = unsafe { &mut *vtable };
        vtable.folder_is_subscribed = Some(folder_is_subscribed);
        vtable.subscribe_folder_sync = Some(subscribe_folder_sync);
        vtable.unsubscribe_folder_sync = Some(unsubscribe_folder_sync);
    }
}

/// The non-blocking read. No `GError` out-parameter, because there is no
/// failure it could report: a store that knows nothing about the folder answers
/// that the user did not ask for it.
unsafe extern "C" fn folder_is_subscribed(
    subscribable: *mut CamelSubscribable,
    folder_name: *const gchar,
) -> gboolean {
    guard("folder_is_subscribed", GFALSE, || {
        // SAFETY: Camel's contract for the vfunc — a valid instance of ours,
        // and a NUL-terminated string it owns, which `read_string` copies.
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

/// Ticks a folder on, and tells Camel it happened.
///
/// `cancellable` is [`observe`]d for the length of the call. This is one
/// `Mailbox/set`, and one listing at worst, rather than a paged walk.
unsafe extern "C" fn subscribe_folder_sync(
    subscribable: *mut CamelSubscribable,
    folder_name: *const gchar,
    cancellable: *mut GCancellable,
    error: *mut *mut GError,
) -> gboolean {
    // SAFETY: Camel's contract for the vfunc: a valid instance of ours, a
    // NUL-terminated string, and an out-parameter that is NULL or writable and
    // currently NULL.
    unsafe {
        guard_bool("subscribe_folder_sync", error, || {
            // SAFETY: Camel keeps its cancellable alive for the length of the
            // call, so it outlives this observation — which is what makes
            // every request below here stop when the user presses Stop.
            let _cancel = observe(cancellable);

            change(subscribable, folder_name, true, error)
        })
    }
}

/// And off again. The same call with the other answer — Camel gives the two
/// directions separate slots, JMAP gives them one `isSubscribed` value.
unsafe extern "C" fn unsubscribe_folder_sync(
    subscribable: *mut CamelSubscribable,
    folder_name: *const gchar,
    cancellable: *mut GCancellable,
    error: *mut *mut GError,
) -> gboolean {
    // SAFETY: as [`subscribe_folder_sync`].
    unsafe {
        guard_bool("unsubscribe_folder_sync", error, || {
            // SAFETY: Camel keeps its cancellable alive for the length of the
            // call, so it outlives this observation — which is what makes
            // every request below here stop when the user presses Stop.
            let _cancel = observe(cancellable);

            change(subscribable, folder_name, false, error)
        })
    }
}

/// The body both sync vfuncs share: resolve, write, announce.
///
/// The announcement is a one-folder `CamelFolderInfo` — depth `Some(0)`, so no
/// descendants — describing the folder as it now is. Camel's signal only
/// borrows it, which is why the chain is kept and dropped here rather than
/// handed over: `camel_folder_info_free` runs when this function returns, as it
/// does one line later in IMAPX.
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
    // Borrowed from Camel and NUL-terminated; `read_string` copies, and reads a
    // NULL or empty name as no name — which no mailbox answers to, because a
    // path always has a component in it.
    // SAFETY: the contract above.
    let path = unsafe { read_string(folder_name) }.unwrap_or_default();

    let folder = match set_subscribed(store, &path, subscribed) {
        Ok(folder) => folder,
        // SAFETY: the contract above.
        Err(failure) => return unsafe { fail_bool(error, &failure, StoreError::to_gerror) },
    };

    let announcement = FolderInfoChain::from_forest(slice::from_ref(&folder), Some(0));
    // SAFETY: `subscribable` is the live instance borrowed above, and the chain
    // is alive until this function returns — the signal only borrows it.
    unsafe {
        if subscribed {
            camel_subscribable_folder_subscribed(subscribable, announcement.as_ptr());
        } else {
            camel_subscribable_folder_unsubscribed(subscribable, announcement.as_ptr());
        }
    }
    GTRUE
}

/// The Rust view of the `CamelSubscribable *` Camel handed over.
///
/// # Safety
///
/// As [`JmapStore::borrow`]: `subscribable` must be NULL or point at an
/// instance of [`JmapStore`]. The cast is sound because the interface's
/// prerequisite is `CamelStore` — checked in eds-sys's tests/camel.rs — so
/// every instance this vfunc is dispatched on is one.
unsafe fn borrow<'a>(subscribable: *mut CamelSubscribable) -> Option<&'a JmapStore> {
    unsafe { JmapStore::borrow(subscribable.cast::<CamelStore>()) }
}
