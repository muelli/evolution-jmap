// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! `create_folder_sync`, `delete_folder_sync` and `rename_folder_sync`: the
//! three `CamelStore` vfuncs that add, delete, and move or rename a folder of
//! an account.
//!
//! One module, because Evolution offers all three behind one flag, and they
//! share each decision. [`crate::folders`] answers questions about the folders
//! of the account. This module adds, removes and changes them.
//!
//! ## What the store adds to the write
//!
//! `jmap-mail-sync` already sends both writes and builds the folder that a
//! create returns. Only the store can keep its *listing* in step, and this is
//! necessary.
//!
//! Camel gives the `CamelFolderInfo` of a create directly to the folder tree of
//! Evolution. Then it opens that folder by path through `get_folder_sync`,
//! which answers from the listing.
//! If the store made the folder on the server but not in its listing, the user
//! sees a folder that the store refuses to open. A delete has the opposite
//! problem: Camel opens again a folder that is gone from the account but still
//! in the listing.
//!
//! ## How Camel names folders
//!
//! `parent_name` and the `folder_name` of the delete are Camel *paths*. They
//! resolve against the tree through `tree_holding`, as in each other folder
//! vfunc. `tree_holding` looks again before it fails. Thus a mailbox that another
//! client created after the last listing can still be a parent.
//!
//! The `folder_name` of the create is not a path. It is the name of the new
//! folder, and this provider uses it verbatim as the *mailbox* name.
//! JMAP puts a mailbox under an explicit `parentId`. Thus, unlike in an IMAP
//! store, the name has no hierarchy and no separator to split on.
//! A `/` that the user types is a character of the name. `jmap-mail-sync`
//! encodes it when it builds the path.
//!
//! The `new_name` of the rename contains both, and this module depends on
//! which part is which.
//! It is a full path, so the text before the last separator is the new parent
//! of the folder. Camel has no vfunc for a move; it requests a move this way.
//! The meaning of the last component depends on whether it *changed*:
//!
//! * **Unchanged: the folder keeps its name.** For a drag and drop, Evolution
//!   builds the path from the existing path of the folder. Thus the component
//!   is the *encoding* of the name by this crate, not the name.
//!   As a new name, it renames `Bills/2026` to `Bills%2F2026` only because of
//!   a drag, and no decoder exists. This module takes the name from the listing.
//!   This reading applies only when the parent also changed. Only a drag sends
//!   the old component back unchanged.
//!   The parent condition prevents a misread of a *typed* name. The next bullet
//!   tells why a match alone is not sufficient.
//! * **Changed: it is the name that the user typed**, verbatim, as a create
//!   reads `folder_name`. The rename dialog of Evolution starts with the display
//!   name and refuses a `/`, so the value is a name and not a path component.
//!   The dialog never changes the parent, so each same-parent call is this case.
//!   This includes a typed name equal to the old encoded component (`Bills/2026`
//!   typed again as `Bills%2F2026`).
//!
//! The second case has a limit. A typed name that this crate must encode (a
//! name with a `%`, or a lone `.`) gets a path that is *not* the requested path.
//! The reason: the path is the encoding of the name, and the caller wrote the
//! name without encoding.
//! The name is correct and the answer has the real path of the folder, so
//! Evolution shows the correct tree. A higher layer that kept the *requested*
//! path is out of step until the next listing of the account.
//!
//! The alternative is to refuse a rename that this crate must encode. That
//! refuses a legal folder name, which is worse.
//!
//! ## Who announces a change to the folder tree
//!
//! Two of the three vfuncs end in an emission, and the third intentionally does
//! not. Camel causes this difference, not this provider.
//!
//! `camel_store_create_folder_sync` and `camel_store_delete_folder_sync` only
//! call the vfunc. Without an emission, each view of the account except the
//! calling window shows a folder tree that never changes.
//! The two `camel_store_folder_*` calls in this module announce the change, as
//! in each other provider.
//!
//! `camel_store_rename_folder_sync` is different. It renames the folders in the
//! object bag of the store, then emits `folder-renamed` *itself*. It gets the
//! info from the store for the new path.
//! Thus an emission here also announces one rename twice. `tests/emissions.rs`
//! showed this, and the emission was removed.
//!
//! This has a cost. Camel requests the info with
//! `CAMEL_STORE_FOLDER_INFO_SUBSCRIBED`, because this store is subscribable.
//! Thus a rename of a subtree with no subscribed folder has no announcement.
//! That is a Camel rule for all providers, not a gap in this provider. Also,
//! the folder tree where the user starts the rename does not show such a
//! subtree. `tests/emissions.rs` pins both halves.
//!
//! ## Why the user sees all three
//!
//! Evolution offers "New Folder", "Rename Folder" and "Delete Folder" for a
//! store whose flags contain `CAMEL_STORE_CAN_EDIT_FOLDERS`. The Camel function
//! `camel_store_init` sets that bit, and `VTRASH` and `VJUNK`, on each store.
//!
//! `tests/manage.rs` pins the full flags word of this store. It contains this
//! bit as Camel set it and `SUPPORTS_INITIAL_SETUP`, but not `VTRASH` and `VJUNK`.
//! The `instance_init` of [`crate::store`] clears `VTRASH` and `VJUNK` and sets
//! `SUPPORTS_INITIAL_SETUP`, for reasons not related to folder management.
//!
//! Thus the flag never blocked these three vfuncs. A store *opts out* of folder
//! management when it clears the bit, and this store never did.
//! Consequently, the menu items were always available for the first two vfuncs.
//! The third, Rename, reached a NULL slot. Camel then refuses to call it, and
//! the user sees no result.
//! The filled slot fixes that. The flag needs no line here, and a line that sets
//! a bit that is already set has no observable effect.

use std::slice;

use eds_sys::{
    CamelFolderInfo, CamelStore, CamelStoreClass, camel_store_folder_created,
    camel_store_folder_deleted,
};
use gio_sys::GCancellable;
use glib_sys::{GError, GTRUE, gboolean, gchar};
use jmap_backend_core::cancel::observe;
use jmap_backend_core::error::{fail, fail_bool};
use jmap_backend_core::marshal::read_string;
use jmap_backend_core::trampoline::{guard_bool, guard_ptr};
use jmap_mail_sync::{FolderInfo, path};

use crate::connect::StoreError;
use crate::folder_info::FolderInfoChain;
use crate::folders::tree_holding;
use crate::store::JmapStore;

/// Makes a folder on the server and adds it to the listing of the store.
///
/// `parent` is the Camel path of the parent folder. NULL and the empty string
/// both mean the account itself, as for `top` in `get_folder_info_sync` and in
/// the Camel wrappers.
///
/// A path that names no folder gives [`StoreError::NoFolder`]. This error is in
/// the store domain, because the account has no fault when the parent is gone.
///
/// The parent resolves to a full [`FolderInfo`], not to an id. The request uses
/// the id and the answer path uses the parent path, and only the tree has both.
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

/// Removes the folder at the Camel path from the server and from the listing
/// of the store.
///
/// The result is the removed folder, for the `folder-deleted` signal. This
/// function gets it before the removal, because after the removal it is gone.
///
/// It is a copy, for the reason in [`crate::subscribe::set_subscribed`]: the
/// listing changes under the lock of the store. A borrow keeps that lock
/// during the emission.
pub fn delete_folder(store: &JmapStore, path: &str) -> Result<FolderInfo, StoreError> {
    let tree = tree_holding(store, |tree| tree.find(path).is_some())?;
    let Some(folder) = tree.find(path) else {
        return Err(StoreError::NoFolder(path.to_owned()));
    };

    store.delete_folder(&folder.id)?;
    Ok(folder.clone())
}

/// Moves the folder at `from` to `to`, and returns the folder in its new state.
///
/// Both are Camel paths, resolved against the same view of the tree. Thus a
/// move names two folders in one listing: the folder and its new parent.
///
/// If one is absent, the result is [`StoreError::NoFolder`] with the missing
/// path. A move under an absent parent makes the folder unreachable, so
/// nothing is written.
///
/// The last component of `to` is read as the module docs describe. It is the
/// existing name of the folder when it is the existing component *and* the
/// parent changed. Otherwise it is the typed name.
///
/// The parent must also change, because the two calls never overlap: a drag
/// always changes the parent and never the name, and the rename dialog never
/// changes the parent.
/// Without that check, take a folder whose name needs Camel-path encoding (a
/// `/`, a `%`, or a bare `.`/`..`), renamed in the dialog to a *typed* name.
/// If the typed name equals the old encoding, it looks like a drag, and the
/// folder silently keeps its old name.
pub fn rename_folder(store: &JmapStore, from: &str, to: &str) -> Result<FolderInfo, StoreError> {
    let (parent_path, component) = path::split(to);

    let tree = tree_holding(store, |tree| {
        tree.find(from).is_some() && parent_path.is_none_or(|parent| tree.find(parent).is_some())
    })?;

    let Some(folder) = tree.find(from) else {
        return Err(StoreError::NoFolder(from.to_owned()));
    };
    let parent = match parent_path {
        Some(path) => match tree.find(path) {
            Some(parent) => Some(parent),
            None => return Err(StoreError::NoFolder(path.to_owned())),
        },
        None => None,
    };

    let (current_parent_path, held) = path::split(&folder.path);
    let moved = parent_path != current_parent_path;
    let name = if moved && component == held {
        folder.display_name.as_str()
    } else {
        component
    };

    store.rename_folder(folder, parent, name)
}

// ---------------------------------------------------------------------------
// the vfunc slots

/// Installs the three folder-management vfuncs on a class whose first member is
/// a `CamelStoreClass`.
///
/// # Safety
///
/// `class` must point to an initialised class struct that starts with a
/// `CamelStoreClass`, as each descendant of `CamelStore` does.
pub unsafe fn install_vfuncs(class: *mut CamelStoreClass) {
    // SAFETY: the contract above.
    let vfuncs = unsafe { &mut *class };
    vfuncs.create_folder_sync = Some(create_folder_sync);
    vfuncs.delete_folder_sync = Some(delete_folder_sync);
    vfuncs.rename_folder_sync = Some(rename_folder_sync);
}

/// Makes a folder and returns it: the vfunc of `camel_store_create_folder_sync`.
///
/// The answer is a one-folder `CamelFolderInfo` with depth `Some(0)`, and a new
/// folder has no children anyway. The caller owns it and frees it with
/// `camel_folder_info_free`.
///
/// The signal is emitted with the same chain first. This is sound because
/// `camel_store_folder_created` queues the emission on the session and clones
/// the info for it.
/// Thus it only borrows the chain, and the ownership that
/// [`FolderInfoChain::into_raw`] transfers does not change.
///
/// NULL is the failure value, and unlike in `get_folder_info_sync` it has no
/// other meaning: a create either made a folder or did not.
///
/// `cancellable` is [`observe`]d for the duration of the call. The call is one
/// `Mailbox/set` and at most one listing, so a stop almost always occurs before
/// the write. Then the server never gets the request to create the folder.
unsafe extern "C" fn create_folder_sync(
    store: *mut CamelStore,
    parent_name: *const gchar,
    folder_name: *const gchar,
    cancellable: *mut GCancellable,
    error: *mut *mut GError,
) -> *mut CamelFolderInfo {
    // SAFETY: Camel's contract for the vfunc: a valid instance of ours, two
    // NULL-or-NUL-terminated strings, and an out-parameter that is NULL or
    // writable and currently NULL.
    unsafe {
        guard_ptr("create_folder_sync", error, || {
            // SAFETY: Camel keeps its cancellable alive for the duration of the
            // call, so it outlives this observation. The observation makes each
            // request below stop when the user clicks Stop.
            let _cancel = observe(cancellable);

            let Some(instance) = JmapStore::borrow(store) else {
                return fail(error, &StoreError::Disconnected, StoreError::to_gerror);
            };
            // Borrowed from Camel and NUL-terminated; `read_string` copies. A
            // NULL parent is the account itself. A NULL name is no name, which
            // the server refuses, as it refuses an empty name.
            let parent = read_string(parent_name);
            let name = read_string(folder_name).unwrap_or_default();

            let created = match create_folder(instance, parent.as_deref(), &name) {
                Ok(folder) => folder,
                Err(failure) => return fail(error, &failure, StoreError::to_gerror),
            };

            let announcement = FolderInfoChain::from_forest(slice::from_ref(&created), Some(0));
            // SAFETY: `store` is the live instance borrowed above, and the
            // chain stays alive across the call. The emitter clones its
            // argument.
            camel_store_folder_created(store, announcement.as_ptr());
            announcement.into_raw()
        })
    }
}

/// Removes a folder: the vfunc of `camel_store_delete_folder_sync`.
///
/// The vfunc returns a boolean, so the chain built here is *not* transferred.
/// It is freed when this function returns, one line after the signal that
/// borrows it, as in IMAPX.
///
/// `cancellable` is [`observe`]d, as in [`create_folder_sync`].
unsafe extern "C" fn delete_folder_sync(
    store: *mut CamelStore,
    folder_name: *const gchar,
    cancellable: *mut GCancellable,
    error: *mut *mut GError,
) -> gboolean {
    // SAFETY: Camel's contract for the vfunc: a valid instance of ours, a
    // NULL-or-NUL-terminated string, and an out-parameter that is NULL or
    // writable and currently NULL.
    unsafe {
        guard_bool("delete_folder_sync", error, || {
            // SAFETY: Camel keeps its cancellable alive for the duration of the
            // call, so it outlives this observation. The observation makes each
            // request below stop when the user clicks Stop.
            let _cancel = observe(cancellable);

            let Some(instance) = JmapStore::borrow(store) else {
                return fail_bool(error, &StoreError::Disconnected, StoreError::to_gerror);
            };
            // Borrowed from Camel and NUL-terminated; `read_string` copies, and
            // reads a NULL or empty name as no name. No mailbox has that path,
            // because a path always contains a component.
            let path = read_string(folder_name).unwrap_or_default();

            let removed = match delete_folder(instance, &path) {
                Ok(folder) => folder,
                Err(failure) => return fail_bool(error, &failure, StoreError::to_gerror),
            };

            let announcement = FolderInfoChain::from_forest(slice::from_ref(&removed), Some(0));
            // SAFETY: as in `create_folder_sync`.
            camel_store_folder_deleted(store, announcement.as_ptr());
            GTRUE
        })
    }
}

/// Renames and moves a folder: the vfunc of `camel_store_rename_folder_sync`.
///
/// Of the three vfuncs, only this one announces nothing, for the reason in the
/// module docs.
/// The Camel wrapper emits `folder-renamed` after this returns, with an info
/// that it gets from the store for the new path.
/// That info includes each descendant, because the rename also changed their
/// paths. A second emission here announces the same rename twice.
///
/// `cancellable` is [`observe`]d, as in [`create_folder_sync`].
unsafe extern "C" fn rename_folder_sync(
    store: *mut CamelStore,
    old_name: *const gchar,
    new_name: *const gchar,
    cancellable: *mut GCancellable,
    error: *mut *mut GError,
) -> gboolean {
    // SAFETY: Camel's contract for the vfunc: a valid instance of ours, two
    // NULL-or-NUL-terminated strings, and an out-parameter that is NULL or
    // writable and currently NULL.
    unsafe {
        guard_bool("rename_folder_sync", error, || {
            // SAFETY: Camel keeps its cancellable alive for the duration of the
            // call, so it outlives this observation. The observation makes each
            // request below stop when the user clicks Stop.
            let _cancel = observe(cancellable);

            let Some(instance) = JmapStore::borrow(store) else {
                return fail_bool(error, &StoreError::Disconnected, StoreError::to_gerror);
            };
            // Borrowed from Camel and NUL-terminated; `read_string` copies, and
            // reads a NULL or empty path as no path. No mailbox has that path,
            // and no mailbox can move to it, because a folder needs a component.
            let from = read_string(old_name).unwrap_or_default();
            let to = read_string(new_name).unwrap_or_default();

            if let Err(failure) = rename_folder(instance, &from, &to) {
                return fail_bool(error, &failure, StoreError::to_gerror);
            }
            GTRUE
        })
    }
}
