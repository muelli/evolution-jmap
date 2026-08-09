// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! `create_folder_sync` and `delete_folder_sync`: the two `CamelStore` vfuncs
//! behind the folder the user adds to an account and the one they take away.
//!
//! One module, because they are one operation seen from either end and they
//! share every decision in it. [`crate::folders`] answers questions about the
//! account's folders; this is where the account gains and loses one.
//!
//! ## What the store adds to the write
//!
//! `jmap-mail-sync` already puts both writes on the wire and already builds the
//! folder a create answers with. What only a store can do is keep the *listing*
//! it holds in step, and that is not housekeeping: Camel hands the
//! `CamelFolderInfo` a create returns straight to Evolution's folder tree and
//! then opens that folder by path, through `get_folder_sync`, which is answered
//! out of the listing. A store that made the folder on the server and left its
//! own listing without it would offer the user a folder it then refuses to
//! open. A delete has the mirror image: a folder gone from the account and
//! still in the listing is one Camel will open again.
//!
//! ## What Camel names things by
//!
//! `parent_name` and the delete's `folder_name` are Camel *paths*, resolved
//! against the tree the way every other folder vfunc resolves one — through
//! [`tree_holding`], which looks again before giving up so that a mailbox
//! another client created since the last listing can still be a parent.
//!
//! The create's `folder_name` is not a path. It is the name of the folder being
//! made, and this provider reads it as the *mailbox* name, verbatim: JMAP puts
//! a mailbox under an explicit `parentId`, so unlike an IMAP store there is no
//! hierarchy to read out of the name and no separator to split it on. A `/` the
//! user typed is a character of the name they chose, and the path
//! `jmap-mail-sync` builds is where it gets encoded.
//!
//! ## What is not covered by a test
//!
//! The emission at the end of each vfunc, for the reason [`crate::subscribe`]
//! gives: `camel_store_folder_created` begins by taking the service's session
//! and queueing the signal on it, so a store without a `CamelSession` behind it
//! cannot emit at all, and the stores these tests use are
//! [`JmapStore::detached`] instances. Everything the vfuncs decide is
//! [`create_folder`] and [`delete_folder`], which `tests/manage.rs` drives
//! against the mock.
//!
//! Camel does not emit either signal for us. Its own
//! `camel_store_create_folder_sync` and `camel_store_delete_folder_sync` call
//! the vfunc and nothing else — the emitters are called nowhere in libcamel
//! outside `CamelVeeStore` — which is why the two lines are here, as they are
//! in every other provider.
//!
//! ## What still keeps this out of the user's reach
//!
//! Evolution offers "New Folder" and "Delete Folder" for a store whose flags
//! carry `CAMEL_STORE_CAN_EDIT_FOLDERS`, and this store does not set it. That
//! is deliberate and it is the next increment's job: the same flag also offers
//! "Rename Folder", and `rename_folder_sync` is still NULL on this class, so
//! setting the flag today would put a menu item in front of the user that
//! reaches a slot Camel refuses to call.

use std::ptr;
use std::slice;

use eds_sys::{
    CamelFolderInfo, CamelStore, CamelStoreClass, camel_store_folder_created,
    camel_store_folder_deleted,
};
use gio_sys::GCancellable;
use glib_sys::{GError, GFALSE, GTRUE, gboolean, gchar};
use jmap_backend_core::error::set_raw_gerror;
use jmap_backend_core::marshal::read_string;
use jmap_backend_core::trampoline::{guard_bool, guard_ptr};
use jmap_mail_sync::FolderInfo;

use crate::connect::StoreError;
use crate::folder_info::FolderInfoChain;
use crate::folders::tree_holding;
use crate::store::JmapStore;

/// Makes a folder on the server and puts it in the store's listing.
///
/// `parent` is the Camel path of the folder the new one hangs under; NULL and
/// the empty string both mean the account itself, which is the reading
/// `get_folder_info_sync`'s `top` already has and the one Camel's own wrappers
/// make. A path that names no folder is [`StoreError::NoFolder`] — the store's
/// own domain, because nothing is wrong with the account when the folder
/// someone asked to nest under has gone.
///
/// The parent is resolved to a whole [`FolderInfo`] rather than to an id
/// because both halves are needed and only the tree has both: the request is
/// built from the id, and the path of the answer from the parent's path.
pub fn create_folder(
    store: &JmapStore,
    parent: Option<&str>,
    name: &str,
) -> Result<FolderInfo, StoreError> {
    let Some(path) = parent.filter(|parent| !parent.is_empty()) else {
        return store.create_folder(None, name);
    };

    let tree = tree_holding(store, |tree| tree.find(path).is_some())?;
    let Some(parent) = tree.find(path) else {
        return Err(StoreError::NoFolder(path.to_owned()));
    };
    store.create_folder(Some(parent), name)
}

/// Removes the folder Camel named by path, and takes it out of the store's
/// listing.
///
/// What comes back is the folder that went — the answer the `folder-deleted`
/// signal is built from, which has to be taken before the removal because
/// afterwards there is nothing left to look it up in. It is a copy, for
/// [`crate::subscribe::set_subscribed`]'s reason: the listing is edited under
/// the store's own lock, and handing out a borrow of it would mean holding that
/// lock across the emission.
pub fn delete_folder(store: &JmapStore, path: &str) -> Result<FolderInfo, StoreError> {
    let tree = tree_holding(store, |tree| tree.find(path).is_some())?;
    let Some(folder) = tree.find(path) else {
        return Err(StoreError::NoFolder(path.to_owned()));
    };

    store.delete_folder(&folder.id)?;
    Ok(folder.clone())
}

// ---------------------------------------------------------------------------
// the vfunc slots

/// Installs the two folder-management vfuncs on a class whose first member is a
/// `CamelStoreClass`.
///
/// # Safety
///
/// `class` must point at an initialised class struct that leads with a
/// `CamelStoreClass` — which is every descendant of `CamelStore`.
pub unsafe fn install_vfuncs(class: *mut CamelStoreClass) {
    // SAFETY: the contract above.
    let vfuncs = unsafe { &mut *class };
    vfuncs.create_folder_sync = Some(create_folder_sync);
    vfuncs.delete_folder_sync = Some(delete_folder_sync);
}

/// Makes a folder and answers with it: `camel_store_create_folder_sync`'s
/// vfunc.
///
/// The answer is a one-folder `CamelFolderInfo` — depth `Some(0)`, and a folder
/// that did not exist a moment ago has nothing under it anyway — and it belongs
/// to the caller, who frees it with `camel_folder_info_free`. The same chain is
/// what the signal is emitted with first, which is sound because
/// `camel_store_folder_created` queues the emission on the session and clones
/// the info to do it: what it takes is a borrow, and the ownership handed on by
/// [`FolderInfoChain::into_raw`] is undisturbed.
///
/// NULL is the failure value, and unlike in `get_folder_info_sync` it is only
/// that: a create either made a folder or did not.
///
/// `cancellable` is not observed, the gap the rest of this provider documents:
/// [`Client`] takes its [`CancelFlag`] when it is built and offers no way to
/// re-point it. This call is one `Mailbox/set`, and one listing at worst.
///
/// [`Client`]: jmap_client::Client
/// [`CancelFlag`]: jmap_client::transport::CancelFlag
unsafe extern "C" fn create_folder_sync(
    store: *mut CamelStore,
    parent_name: *const gchar,
    folder_name: *const gchar,
    _cancellable: *mut GCancellable,
    error: *mut *mut GError,
) -> *mut CamelFolderInfo {
    // SAFETY: Camel's contract for the vfunc: a valid instance of ours, two
    // NULL-or-NUL-terminated strings, and an out-parameter that is NULL or
    // writable and currently NULL.
    unsafe {
        guard_ptr("create_folder_sync", error, || {
            let Some(instance) = JmapStore::borrow(store) else {
                return fail(error, &StoreError::Disconnected);
            };
            // Borrowed from Camel and NUL-terminated; `read_string` copies. A
            // NULL parent is the account itself; a NULL name is no name, which
            // the server refuses as it refuses an empty one.
            let parent = read_string(parent_name);
            let name = read_string(folder_name).unwrap_or_default();

            let created = match create_folder(instance, parent.as_deref(), &name) {
                Ok(folder) => folder,
                Err(failure) => return fail(error, &failure),
            };

            let announcement = FolderInfoChain::from_forest(slice::from_ref(&created), Some(0));
            // SAFETY: `store` is the live instance borrowed above, and the
            // chain is alive across the call — the emitter clones what it is
            // given.
            camel_store_folder_created(store, announcement.as_ptr());
            announcement.into_raw()
        })
    }
}

/// Removes a folder: `camel_store_delete_folder_sync`'s vfunc.
///
/// The chain built here is *not* handed over — the vfunc answers with a
/// boolean — so it is freed when this function returns, one line after the
/// signal that borrows it, as it is in IMAPX.
///
/// `cancellable` is not observed, for the reason [`create_folder_sync`] gives.
unsafe extern "C" fn delete_folder_sync(
    store: *mut CamelStore,
    folder_name: *const gchar,
    _cancellable: *mut GCancellable,
    error: *mut *mut GError,
) -> gboolean {
    // SAFETY: Camel's contract for the vfunc: a valid instance of ours, a
    // NULL-or-NUL-terminated string, and an out-parameter that is NULL or
    // writable and currently NULL.
    unsafe {
        guard_bool("delete_folder_sync", error, || {
            let Some(instance) = JmapStore::borrow(store) else {
                return fail_bool(error, &StoreError::Disconnected);
            };
            // Borrowed from Camel and NUL-terminated; `read_string` copies, and
            // reads a NULL or empty name as no name — which no mailbox answers
            // to, because a path always has a component in it.
            let path = read_string(folder_name).unwrap_or_default();

            let removed = match delete_folder(instance, &path) {
                Ok(folder) => folder,
                Err(failure) => return fail_bool(error, &failure),
            };

            let announcement = FolderInfoChain::from_forest(slice::from_ref(&removed), Some(0));
            // SAFETY: as in `create_folder_sync`.
            camel_store_folder_deleted(store, announcement.as_ptr());
            GTRUE
        })
    }
}

/// Reports a failure and answers with nothing.
///
/// # Safety
///
/// As [`set_raw_gerror`].
unsafe fn fail<T>(error: *mut *mut GError, failure: &StoreError) -> *mut T {
    // SAFETY: `to_gerror` hands over an owned GError, and `error` meets
    // `set_raw_gerror`'s contract by this function's.
    unsafe { set_raw_gerror(error, failure.to_gerror()) };
    ptr::null_mut()
}

/// The same, for the vfunc that answers with a boolean.
///
/// # Safety
///
/// As [`set_raw_gerror`].
unsafe fn fail_bool(error: *mut *mut GError, failure: &StoreError) -> gboolean {
    // SAFETY: the contract above.
    unsafe { set_raw_gerror(error, failure.to_gerror()) };
    GFALSE
}
