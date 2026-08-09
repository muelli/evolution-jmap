// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! `create_folder_sync`, `delete_folder_sync` and `rename_folder_sync`: the
//! three `CamelStore` vfuncs behind the folder the user adds to an account, the
//! one they take away, and the one they move or rename.
//!
//! One module, because Evolution offers all three behind one flag and they
//! share every decision in them. [`crate::folders`] answers questions about the
//! account's folders; this is where the account gains, loses and reshapes one.
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
//! The rename's `new_name` is both at once, and telling which part is which is
//! the decision this module turns on. It is a whole path, so everything up to
//! the last separator is the parent the folder is to hang under — that is how
//! Camel spells a move, there being no separate vfunc for one. What the last
//! component is depends on whether it *changed*:
//!
//! * **Unchanged, and the folder keeps its name.** Evolution builds the path
//!   for a drag and drop out of the folder's existing one, so the component
//!   that arrives is this crate's *encoding* of the name rather than the name.
//!   Reading it as a new name would rename `Bills/2026` to `Bills%2F2026` for
//!   the crime of being dragged, and there is no decoder to read it back with —
//!   the folder's name is in the listing, which is where this takes it from.
//! * **Changed, and it is the name the user typed**, verbatim, exactly as a
//!   create reads `folder_name`. Evolution's rename dialog is prefilled with
//!   the display name and refuses a `/`, so what arrives is a name and not a
//!   path component.
//!
//! The limit that comes with the second half, stated rather than hidden: a
//! typed name this crate has to encode — one containing a `%`, or a lone `.` —
//! puts the folder at a path that is *not* the one Camel asked for, because the
//! path is the encoding of the name and the caller wrote the name unencoded.
//! The name is the one the user asked for and the answer carries the folder's
//! real path, so what Evolution draws is right; anything above that remembered
//! the path it *requested* is out of step until the account is listed again.
//! The alternative — refusing a rename this crate would have to encode — is
//! refusing a legal folder name, which is worse.
//!
//! ## Who says the folder tree changed
//!
//! Two of the three vfuncs end in an emission and the third deliberately does
//! not, and the difference is Camel's rather than ours.
//!
//! `camel_store_create_folder_sync` and `camel_store_delete_folder_sync` call
//! the vfunc and nothing else, so a create and a delete that said nothing would
//! leave every view of the account except the one window that made the call
//! showing a folder tree that never moves. The two `camel_store_folder_*` lines
//! here are what tells it, as they are in every other provider.
//!
//! `camel_store_rename_folder_sync` is not like them: it renames the folders in
//! the store's object bag and then emits `folder-renamed` *itself*, building the
//! info by asking the store for the new path. So a rename that emitted as well
//! would announce one rename twice — which is what `tests/emissions.rs` observed
//! and what took the line out again. The cost is stated rather than hidden: the
//! info Camel builds is asked for with `CAMEL_STORE_FOLDER_INFO_SUBSCRIBED`,
//! because this store is subscribable, so a subtree with nothing subscribed
//! anywhere in it is renamed with no announcement at all. That is Camel's rule
//! applied to every provider alike, not a gap of ours, and the folder tree the
//! rename is invoked from is not showing such a subtree in the first place;
//! `tests/emissions.rs` pins both halves.
//!
//! ## What puts all three in front of the user
//!
//! Evolution offers "New Folder", "Rename Folder" and "Delete Folder" for a
//! store whose flags carry `CAMEL_STORE_CAN_EDIT_FOLDERS` — and Camel's own
//! `camel_store_init` sets that bit, along with `VTRASH` and `VJUNK`, on every
//! store there is. `tests/manage.rs` pins the whole word this store ends up
//! with: this bit as Camel left it, and those two cleared by
//! [`crate::store`]'s `instance_init` for a reason that has nothing to do with
//! folder management.
//!
//! So the flag was never the thing standing between the user and these three
//! vfuncs: a store *opts out* of folder management by clearing the bit, and
//! this one never did. What that means for the two vfuncs written before this
//! one is that the menu items have been on offer all along, and the third —
//! Rename — reached a NULL slot, which Camel answers by refusing to call it and
//! the user sees as nothing happening. Filling the slot is what fixes that; the
//! flag needs no line of ours, and a line that OR-ed in a bit already set would
//! be one nothing could ever observe.

use std::ptr;
use std::slice;

use eds_sys::{
    CamelFolderInfo, CamelStore, CamelStoreClass, camel_store_folder_created,
    camel_store_folder_deleted,
};
use gio_sys::GCancellable;
use glib_sys::{GError, GFALSE, GTRUE, gboolean, gchar};
use jmap_backend_core::cancel::observe;
use jmap_backend_core::error::set_raw_gerror;
use jmap_backend_core::marshal::read_string;
use jmap_backend_core::trampoline::{guard_bool, guard_ptr};
use jmap_mail_sync::{FolderInfo, path};

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

/// Moves the folder at `from` to `to`, and answers with it as it now is.
///
/// Both are Camel paths and both are resolved against the same look at the
/// tree, so a move names two folders in one listing: the folder itself, and the
/// parent its new path hangs it under. Either being absent is
/// [`StoreError::NoFolder`] with the path that is missing — a move under a
/// parent that is not there would put the folder somewhere nothing can reach
/// it, so nothing is written.
///
/// The last component of `to` is read as the module documents: the folder's own
/// name when it is the component the folder already has, and otherwise the name
/// the user typed.
pub fn rename_folder(store: &JmapStore, from: &str, to: &str) -> Result<FolderInfo, StoreError> {
    let (parent, component) = path::split(to);

    let tree = tree_holding(store, |tree| {
        tree.find(from).is_some() && parent.is_none_or(|parent| tree.find(parent).is_some())
    })?;

    let Some(folder) = tree.find(from) else {
        return Err(StoreError::NoFolder(from.to_owned()));
    };
    let parent = match parent {
        Some(path) => match tree.find(path) {
            Some(parent) => Some(parent),
            None => return Err(StoreError::NoFolder(path.to_owned())),
        },
        None => None,
    };

    let (_, held) = path::split(&folder.path);
    let name = if component == held {
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
/// `class` must point at an initialised class struct that leads with a
/// `CamelStoreClass` — which is every descendant of `CamelStore`.
pub unsafe fn install_vfuncs(class: *mut CamelStoreClass) {
    // SAFETY: the contract above.
    let vfuncs = unsafe { &mut *class };
    vfuncs.create_folder_sync = Some(create_folder_sync);
    vfuncs.delete_folder_sync = Some(delete_folder_sync);
    vfuncs.rename_folder_sync = Some(rename_folder_sync);
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
/// `cancellable` is [`observe`]d for the length of the call. This is one
/// `Mailbox/set`, and one listing at worst, so a stop almost always lands before
/// the write — and a folder the user stopped creating is one the server was
/// never asked to create.
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
            // SAFETY: Camel keeps its cancellable alive for the length of the
            // call, so it outlives this observation — which is what makes
            // every request below here stop when the user presses Stop.
            let _cancel = observe(cancellable);

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
            // SAFETY: Camel keeps its cancellable alive for the length of the
            // call, so it outlives this observation — which is what makes
            // every request below here stop when the user presses Stop.
            let _cancel = observe(cancellable);

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

/// Renames a folder, and moves it: `camel_store_rename_folder_sync`'s vfunc.
///
/// Nothing is announced here, alone among the three, and the module says why:
/// Camel's own wrapper emits `folder-renamed` once this returns, with an info it
/// builds by asking the store for the folder's new path — including everything
/// under it, whose paths the rename changed too. A second emission from here
/// would be the same rename announced twice.
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
            // SAFETY: Camel keeps its cancellable alive for the length of the
            // call, so it outlives this observation — which is what makes
            // every request below here stop when the user presses Stop.
            let _cancel = observe(cancellable);

            let Some(instance) = JmapStore::borrow(store) else {
                return fail_bool(error, &StoreError::Disconnected);
            };
            // Borrowed from Camel and NUL-terminated; `read_string` copies, and
            // reads a NULL or empty path as no path — which no mailbox answers
            // to, and which no mailbox may be moved to either, a folder needing
            // a component of its own.
            let from = read_string(old_name).unwrap_or_default();
            let to = read_string(new_name).unwrap_or_default();

            if let Err(failure) = rename_folder(instance, &from, &to) {
                return fail_bool(error, &failure);
            }
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
