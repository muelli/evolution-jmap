// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! `CamelJmapFolder`: one mailbox, as the object that Camel opens.
//!
//! A `CamelFolderInfo` tree only describes folders: Camel reads it once and
//! frees it. This type is the folder itself. `camel_store_get_folder_sync`
//! returns it, Evolution keeps it while the user has it open, and all message
//! operations are methods on it.
//!
//! Thus the folder is the first place where the provider keeps per-folder
//! state. The JMAP mailbox id must be there from the start. Camel has no field
//! for it, and nothing can recover it later. The Camel path of the folder comes
//! from the mailbox *name* (see the `path` module of `jmap-mail-sync`), and
//! the encoding is not reversible. But `Email/query` filters on `inMailbox`,
//! which is an id. A folder with only its path cannot fetch messages.
//!
//! ## Two flags words, one letter apart
//!
//! A folder and a `CamelFolderInfo` each have a flags word, and they are
//! different. `CamelFolderInfoFlags` (which [`crate::folder_info`] fills in)
//! gives the kind of folder: its type field, its subscription, its children.
//! `CamelFolderFlags` (which this module sets) tells Camel how to *treat* the
//! folder: for example, whether the user's filters apply to incoming mail, or
//! whether it is the trash. Only the second applies to an object. See `flags`
//! for the bits that this module sets.

use std::cmp::Ordering;
use std::ffi::CStr;
use std::ptr;

#[cfg(camel_folder_search_object)]
use eds_sys::camel_pstring_strdup;
use eds_sys::{
    CAMEL_FOLDER_FILTER_JUNK, CAMEL_FOLDER_FILTER_RECENT, CAMEL_FOLDER_HAS_SUMMARY_CAPABILITY,
    CAMEL_MESSAGE_ANSWERED, CAMEL_MESSAGE_DELETED, CAMEL_MESSAGE_DRAFT, CAMEL_MESSAGE_FLAGGED,
    CAMEL_MESSAGE_FORWARDED, CAMEL_MESSAGE_JUNK, CAMEL_MESSAGE_NOTJUNK, CAMEL_MESSAGE_SEEN,
    CAMEL_MESSAGE_USER, CamelFolder, CamelFolderClass, CamelFolderFlags, CamelOfflineFolder,
    CamelOfflineFolderClass, CamelService, CamelStore, camel_folder_get_parent_store,
    camel_folder_set_flags, camel_offline_folder_get_type, camel_service_get_user_cache_dir,
};
use glib_sys::{GType, gchar, gint};
#[cfg(camel_folder_search_object)]
use glib_sys::{g_ptr_array_new, g_ptr_array_set_size};
use gobject_sys::g_object_new;
#[cfg(camel_folder_search_object)]
use jmap_backend_core::cancel::observe;
#[cfg(camel_folder_search_object)]
use jmap_backend_core::error::fail;
use jmap_backend_core::instance::Slot;
#[cfg(camel_folder_search_object)]
use jmap_backend_core::marshal::read_string;
use jmap_backend_core::marshal::{checked_borrow, dispatched_borrow};
#[cfg(camel_folder_search_object)]
use jmap_backend_core::owned::Owned;
use jmap_backend_core::subclass::{ObjectSubclass, register_static};
#[cfg(camel_folder_search_object)]
use jmap_backend_core::trampoline::guard_ptr;
use jmap_mail_sync::{FolderInfo, FolderRole};
use jmap_proto::Id;

use crate::cache::MessageCache;
#[cfg(camel_folder_search_object)]
use crate::connect::StoreError;
use crate::folder_info::c_string;
#[cfg(camel_folder_search_object)]
use crate::search_sexp;
use crate::store::{JmapStore, store_type};
use crate::summary::attach_summary;

/// The instance struct. It is `#[repr(C)]` and starts with the parent instance
/// struct, so a `*mut JmapFolder` is a valid `CamelFolder *`.
#[repr(C)]
pub struct JmapFolder {
    parent: CamelOfflineFolder,
    /// The mailbox that this folder shows.
    ///
    /// [`new_folder`] writes it once, before other code can reach the object.
    /// A renamed or moved mailbox is the same mailbox, and Camel drops the
    /// folder of a deleted mailbox. It is a [`Slot`] because the instance
    /// struct starts zeroed and no destructor runs on it (the store keeps its
    /// connection in a slot for the same reason).
    mailbox: Slot<Id>,
    /// The downloaded messages of this account, on disk.
    ///
    /// [`new_folder`] fills it, because the cache needs the account cache
    /// directory, and that directory is known only when Camel has constructed
    /// the store. It is empty if the cache directory could not be made. Then
    /// the folder fetches each message that it opens (see [`crate::cache`]).
    ///
    /// This is not per-folder state. The entries are keyed by JMAP email id in
    /// the *account* directory, so all folders of a message read one file.
    cache: Slot<MessageCache>,
}

impl JmapFolder {
    /// The JMAP mailbox id that all requests about this folder filter on.
    ///
    /// `None` if the construction did not complete. Callers report this
    /// condition and do not assume an id.
    pub fn mailbox(&self) -> Option<&Id> {
        self.mailbox.get()
    }

    /// The message cache of the account, or `None` if this folder has none.
    /// All callers then fetch the message: `None` is not an error.
    pub fn cache(&self) -> Option<&MessageCache> {
        self.cache.get()
    }

    /// The Rust view of a `CamelFolder *` from Camel.
    ///
    /// # Safety
    ///
    /// `folder` must be NULL or point at an instance of this type. Camel
    /// dispatches the vfuncs of a class only on instances of that class, so a
    /// vfunc argument obeys this rule. Check other pointers with
    /// `G_TYPE_CHECK_INSTANCE_TYPE` first.
    pub unsafe fn borrow<'a>(folder: *mut CamelFolder) -> Option<&'a Self> {
        // SAFETY: the doc comment above states the same contract.
        unsafe { dispatched_borrow(folder) }
    }
}

/// The parent store of the folder, as a [`JmapStore`].
///
/// All requests of a folder use the connection of the store, because the
/// folder holds only a mailbox id. [`crate::refresh`] and [`crate::message`]
/// both start with this function.
///
/// This function checks the type. A store vfunc can assume the type of its
/// first argument, because GObject dispatches it on an instance of the class.
/// But `parent-store` is a usual construct property that can hold any
/// `CamelStore`. That case does not occur, but a `JmapStore` read from a
/// different store is undefined behaviour.
///
/// # Safety
///
/// `folder` must point at a live `CamelFolder`.
pub(crate) unsafe fn parent_store<'a>(folder: *mut CamelFolder) -> Option<&'a JmapStore> {
    // SAFETY: the folder holds only a weak pointer to its store, which can be
    // NULL. `checked_borrow` returns `None` for NULL. The caller's own store
    // reference keeps the store alive, so do not keep the result past the vfunc.
    // The contract above makes the type check legal, and the type check makes
    // the cast inside sound.
    unsafe {
        let store = camel_folder_get_parent_store(folder);
        checked_borrow(store, store_type())
    }
}

/// The class struct, with the same layout rule. It has no fields of its own,
/// but the type needs its own class to hold the overridden vfunc slots of the
/// parent `CamelOfflineFolderClass`.
#[repr(C)]
pub struct JmapFolderClass {
    parent_class: CamelOfflineFolderClass,
}

// SAFETY: both structs are #[repr(C)] and start with the CamelOfflineFolder
// instance and class structs. The tests/layout.rs of eds-sys checks those
// layouts against `g_type_query`. CamelOfflineFolder derives from CamelFolder,
// which derives from CamelObject, which derives from GObject.
unsafe impl ObjectSubclass for JmapFolder {
    /// `CamelJmapFolder`, to match `CamelJmapStore`. All Camel folders are
    /// `Camel<Protocol>Folder`, and a GObject warning about a wrong folder type
    /// shows this name to the user.
    const NAME: &'static CStr = c"CamelJmapFolder";
    type Instance = JmapFolder;
    type Class = JmapFolderClass;

    fn parent_type() -> GType {
        // SAFETY: no arguments, and the type initialises itself.
        unsafe { camel_offline_folder_get_type() }
    }

    unsafe fn class_init(class: *mut Self::Class) {
        // Refresh: `CamelFolder` leaves `refresh_info_sync` NULL, and its
        // wrapper then returns TRUE and does nothing. Without this line, each
        // refresh reports success and the folder stays empty.
        //
        // SAFETY: the class starts with CamelOfflineFolderClass, which starts
        // with CamelFolderClass (the contract above).
        unsafe { crate::refresh::install_vfuncs(class.cast::<CamelFolderClass>()) };

        // Message read: `camel_folder_get_message_sync` refuses a class that
        // does not fill in this slot. Without this line, the message list
        // shows, but the user cannot read the messages.
        //
        // SAFETY: as above.
        unsafe { crate::message::install_vfuncs(class.cast::<CamelFolderClass>()) };

        // Flag changes to the server: `CamelFolder` leaves `synchronize_sync`
        // NULL, and its wrapper then returns TRUE. Without this line, each flag
        // change reports success and the server gets none of them.
        //
        // SAFETY: as above.
        unsafe { crate::synchronize::install_vfuncs(class.cast::<CamelFolderClass>()) };

        // Move and copy: the generic `CamelFolder` implementation downloads
        // each message and appends it to the destination. That is correct
        // between two accounts. In one account, the server can file the message
        // itself, and the mail does not leave the server.
        //
        // SAFETY: as above.
        unsafe { crate::transfer::install_vfuncs(class.cast::<CamelFolderClass>()) };

        // Append (a message that is not in the account yet): `CamelFolder`
        // leaves `append_message_sync` NULL, and
        // `camel_folder_append_message_sync` refuses a class that does not
        // fill it in. Without this line, a message from a different account or
        // a saved draft fails in Camel.
        //
        // SAFETY: as above.
        unsafe { crate::append::install_vfuncs(class.cast::<CamelFolderClass>()) };

        // Expunge: `CamelFolder` leaves `expunge_sync` NULL, and its wrapper
        // then returns TRUE. Without this line, "Expunge" and "Empty Trash"
        // report success and remove no message.
        //
        // SAFETY: as above.
        unsafe { crate::expunge::install_vfuncs(class.cast::<CamelFolderClass>()) };

        // Quota: the `CamelFolder` base class fills in this slot, but its
        // implementation always returns `G_IO_ERROR_NOT_SUPPORTED`. Without
        // this line, the folder-properties dialog of Evolution shows no quota
        // row. With it, the dialog shows the usage that the server reports.
        //
        // SAFETY: as above.
        unsafe { crate::quota::install_vfuncs(class.cast::<CamelFolderClass>()) };

        // Raw message bytes on disk, for "View Source" and "Save Message As":
        // the `CamelFolder` base class leaves this slot NULL and asserts on a
        // class that does not fill it in. Without this line, Evolution
        // serializes the message again from memory (this works), and logs a
        // `camel-CRITICAL` at each call. This line answers from the on-disk
        // cache of the folder, as `camel-imapx-folder.c` does (see
        // `crate::cache`).
        //
        // SAFETY: as above.
        unsafe { crate::cache::install_vfuncs(class.cast::<CamelFolderClass>()) };

        // Uid order: the `CamelFolder` compare parses each uid as a decimal
        // number, as for IMAP uids. A JMAP id that does not start with a digit
        // parses as 0, so all pairs are equal and `sort_uids` does not change
        // the order. A JMAP id is an opaque string (RFC 8620 section 1.2), so
        // its only order is the byte order.
        //
        // SAFETY: as above.
        unsafe { (*class.cast::<CamelFolderClass>()).cmp_uids = Some(cmp_uids) };

        // Permanent flags: the `CamelFolder` implementation returns 0. This
        // tells a caller that no flag of a message is kept, not even its read
        // state.
        //
        // SAFETY: as above.
        unsafe {
            (*class.cast::<CamelFolderClass>()).get_permanent_flags = Some(get_permanent_flags)
        };

        // Search by expression: this is not only the search bar. Each
        // message-list view ("Unread Messages", "Hide Deleted Messages") is an
        // expression, so if the folder cannot answer, the list does not show.
        //
        // Only EDS up to 3.52 has these slots. There, `CamelFolderClass` leaves `search_by_expression` and `search_by_uids`
        // NULL and asserts on a class that does not fill them in. The two
        // functions below fill them with a `CamelFolderSearch` over the local
        // summary. From 3.58, both slots are removed: `search_sync` replaces
        // them, and the base class implements it with `CamelStoreSearch` over
        // the same rows, with the same result. Thus on a newer EDS, install
        // nothing: an override only does again what already works.
        // `tests/search.rs` proves this: it uses the entry point of the
        // available EDS and asserts the same answers on both.
        #[cfg(camel_folder_search_object)]
        {
            // SAFETY: as above.
            let folder_class = unsafe { &mut *class.cast::<CamelFolderClass>() };
            folder_class.search_by_expression = Some(search_by_expression);
            folder_class.search_by_uids = Some(search_by_uids);
        }

        // Body search: a newer EDS *adds* this slot. Its own `CamelStoreSearch`
        // implementation cannot match a body that this provider did not
        // download. See `crate::search_body` for the cost if this slot stays
        // NULL.
        //
        // SAFETY: as above.
        #[cfg(camel_folder_search_body_sync)]
        unsafe {
            crate::search_body::install_vfuncs(class.cast::<CamelFolderClass>())
        };
    }

    // No `instance_init` (the store has one): there is nothing to fill in.
    // The mailbox id is known only when `new_folder` has the `FolderInfo`, and
    // a zeroed `Slot` (which GObject supplies) is empty. `finalize` is
    // necessary, because the slots can then hold values.

    unsafe fn finalize(instance: *mut Self::Instance) {
        // SAFETY: the instance is in finalization, so nothing can still reach
        // it and no borrow from `get` is alive. Without this, the id leaks
        // (once for each folder that the user opened), and the reference of
        // the cache on its `CamelDataCache` also leaks.
        unsafe {
            (*instance).mailbox.clear();
            (*instance).cache.clear();
        };
    }
}

/// `CamelFolderClass.cmp_uids`: orders two uids by their bytes, as `strcmp`
/// does. A NULL uid is equal to all uids, as in the default after its
/// `g_return_val_if_fail`.
unsafe extern "C" fn cmp_uids(
    _folder: *mut CamelFolder,
    uid1: *const gchar,
    uid2: *const gchar,
) -> gint {
    if uid1.is_null() || uid2.is_null() {
        return 0;
    }
    // SAFETY: Camel supplies two NUL-terminated uids and keeps them alive for
    // the duration of the call.
    let (uid1, uid2) = unsafe { (CStr::from_ptr(uid1), CStr::from_ptr(uid2)) };
    match uid1.cmp(uid2) {
        Ordering::Less => -1,
        Ordering::Equal => 0,
        Ordering::Greater => 1,
    }
}

/// `CamelFolderClass.get_permanent_flags`: the flags that the server keeps as
/// keywords (RFC 8621 section 4.1.1), and `USER` for the labels, which are also
/// keywords. Also `DELETED`: JMAP has no keyword for it, but the Camel summary
/// keeps it until the expunge. IMAPX and evolution-ews also return it.
extern "C" fn get_permanent_flags(_folder: *mut CamelFolder) -> u32 {
    CAMEL_MESSAGE_ANSWERED
        | CAMEL_MESSAGE_DELETED
        | CAMEL_MESSAGE_DRAFT
        | CAMEL_MESSAGE_FLAGGED
        | CAMEL_MESSAGE_SEEN
        | CAMEL_MESSAGE_FORWARDED
        | CAMEL_MESSAGE_JUNK
        | CAMEL_MESSAGE_NOTJUNK
        | CAMEL_MESSAGE_USER
}

/// `CamelFolderClass.search_by_expression`, up to EDS 3.52: the messages of
/// this folder that match `expression`.
///
/// It tries the server first. [`search_sexp::translate`] changes `expression`
/// into a JMAP filter if the contains-only conditions of RFC 8621 can express
/// it. `Email/query` then searches the full mailbox, not only the cached
/// summary. A local search cannot search a body that it did not download.
///
/// It uses the local `CamelFolderSearch` over the folder summary if there is
/// no translation, or if the folder or store cannot ask the server. It does
/// not do this on a server error: it reports the error and does not return a
/// local match set that can be wrong.
///
/// Removed from 3.58, where the base class does this: see `class_init`.
#[cfg(camel_folder_search_object)]
unsafe extern "C" fn search_by_expression(
    folder: *mut CamelFolder,
    expression: *const gchar,
    cancellable: *mut gio_sys::GCancellable,
    error: *mut *mut glib_sys::GError,
) -> *mut glib_sys::GPtrArray {
    unsafe {
        guard_ptr("search_by_expression", error, || {
            let _cancel = observe(cancellable);
            match server_matches(folder, expression, None) {
                Some(Ok(uids)) => pstring_uid_array(&uids),
                Some(Err(failure)) => fail(error, &failure, StoreError::to_gerror),
                None => local_search_by_expression(folder, expression, cancellable, error),
            }
        })
    }
}

/// `CamelFolderClass.search_by_uids`: the same search, limited to a list of
/// uids. `Email/query` cannot filter on a list of ids, so this function limits
/// the server match set to `uids` after the query. The local path gets this
/// step from an argument of `camel_folder_search_search`.
///
/// Removed from 3.58, together with `search_by_expression`.
#[cfg(camel_folder_search_object)]
unsafe extern "C" fn search_by_uids(
    folder: *mut CamelFolder,
    expression: *const gchar,
    uids: *mut glib_sys::GPtrArray,
    cancellable: *mut gio_sys::GCancellable,
    error: *mut *mut glib_sys::GError,
) -> *mut glib_sys::GPtrArray {
    unsafe {
        guard_ptr("search_by_uids", error, || {
            let _cancel = observe(cancellable);
            // SAFETY: `uids` is NULL or a live `GPtrArray` of NUL-terminated
            // strings, by the contract of the vfunc that this function backs.
            let restrict = (!uids.is_null()).then(|| string_array(uids));
            match server_matches(folder, expression, restrict.as_deref()) {
                Some(Ok(matched)) => pstring_uid_array(&matched),
                Some(Err(failure)) => fail(error, &failure, StoreError::to_gerror),
                None => local_search_by_uids(folder, expression, uids, cancellable, error),
            }
        })
    }
}

/// The local `CamelFolderSearch` fallback of the two search vfuncs: a search
/// over the folder summary. An expression on flags or headers needs nothing
/// from the server, because the rows are local.
///
/// Each call makes a new search object and drops it. The object holds its
/// folder, and a kept object is a second owner of state that Camel expects
/// the search to read fresh.
///
/// # Safety
///
/// `folder` must be live, `expression` NUL-terminated and live for the call,
/// and `error` NULL or a valid, currently-NULL `GError **`, the same
/// contract as the vfunc this backs.
#[cfg(camel_folder_search_object)]
unsafe fn local_search_by_expression(
    folder: *mut CamelFolder,
    expression: *const gchar,
    cancellable: *mut gio_sys::GCancellable,
    error: *mut *mut glib_sys::GError,
) -> *mut glib_sys::GPtrArray {
    unsafe {
        let Some(search) = Owned::from_raw(eds_sys::camel_folder_search_new()) else {
            return ptr::null_mut();
        };
        eds_sys::camel_folder_search_set_folder(search.as_ptr(), folder);
        eds_sys::camel_folder_search_search(
            search.as_ptr(),
            expression,
            ptr::null_mut(),
            cancellable,
            error,
        )
    }
}

/// The same search, limited to `uids`: [`local_search_by_expression`] with the
/// `uids` argument of `camel_folder_search_search`.
///
/// # Safety
///
/// As [`local_search_by_expression`], plus: `uids` must be NULL or a live
/// `GPtrArray` of NUL-terminated strings.
#[cfg(camel_folder_search_object)]
unsafe fn local_search_by_uids(
    folder: *mut CamelFolder,
    expression: *const gchar,
    uids: *mut glib_sys::GPtrArray,
    cancellable: *mut gio_sys::GCancellable,
    error: *mut *mut glib_sys::GError,
) -> *mut glib_sys::GPtrArray {
    unsafe {
        let Some(search) = Owned::from_raw(eds_sys::camel_folder_search_new()) else {
            return ptr::null_mut();
        };
        eds_sys::camel_folder_search_set_folder(search.as_ptr(), folder);
        eds_sys::camel_folder_search_search(search.as_ptr(), expression, uids, cancellable, error)
    }
}

/// Sends `expression` to the server for the mailbox of `folder`. If `restrict`
/// is given, the result contains only those uids.
///
/// `None` means that the server cannot answer: no translation, no mailbox id,
/// no store, or a disconnected store. The caller then does the local search.
/// A disconnected store gives `None` and not an error by design: the local
/// summary search is for the offline case. All read paths of this provider
/// answer from local data when there is no connection.
///
/// `Some(Err(_))` means that the server *was* asked on a live connection and
/// failed. The caller reports this error and does not use a local answer that
/// can be stale.
///
/// # Safety
///
/// `folder` must be live and `expression` NUL-terminated and live for the
/// call, the same contract as the vfuncs this backs.
#[cfg(camel_folder_search_object)]
unsafe fn server_matches(
    folder: *mut CamelFolder,
    expression: *const gchar,
    restrict: Option<&[String]>,
) -> Option<Result<Vec<String>, StoreError>> {
    // SAFETY: the contract above.
    let expression = unsafe { read_string(expression) }?;
    let filter = search_sexp::translate(&expression)?;
    // SAFETY: as above.
    let mailbox = unsafe { JmapFolder::borrow(folder) }?.mailbox()?.clone();
    // SAFETY: as above.
    let store = unsafe { parent_store(folder) }?;
    let matches = match store.search(&mailbox, filter) {
        Ok(ids) => ids,
        Err(StoreError::Disconnected) => return None,
        Err(failure) => return Some(Err(failure)),
    };
    let matches = matches.into_iter().map(|id| id.as_str().to_owned());
    Some(Ok(match restrict {
        Some(restrict) => narrow(matches, restrict),
        None => matches.collect(),
    }))
}

/// The server match set, limited to the uids that the caller asked about.
///
/// This uses a set and not `[String]::contains`, because a linear scan is
/// `O(matches x restrict)` and both lists can have the size of the folder.
/// `camel-vee-folder.c` gives `search_by_uids` all uids of a subfolder on a
/// rebuild, and a broad expression matches most of them. Measured on
/// 50 000 x 50 000 Fastmail-like ids: the scan took 5.7 s (release) and 21.6 s
/// (debug), and the set took 7.5 ms and 50 ms. At 21.6 s, Evolution seems
/// to hang.
#[cfg(camel_folder_search_object)]
fn narrow(matches: impl Iterator<Item = String>, restrict: &[String]) -> Vec<String> {
    let wanted: std::collections::HashSet<&str> = restrict.iter().map(String::as_str).collect();
    matches
        .filter(|uid| wanted.contains(uid.as_str()))
        .collect()
}

/// Copies the strings out of a `GPtrArray` from Camel: a uid list for the
/// search vfuncs above, a word list for [`crate::search_body`].
///
/// # Safety
///
/// `array` must be a live `GPtrArray` of NUL-terminated strings.
pub(crate) unsafe fn string_array(array: *mut glib_sys::GPtrArray) -> Vec<String> {
    // SAFETY: the contract above. Each element lives at least as long as the
    // array, and the array lives longer than this call.
    unsafe {
        (0..(*array).len)
            .map(|index| {
                let uid: *const gchar = (*array).pdata.add(index as usize).read().cast();
                CStr::from_ptr(uid).to_string_lossy().into_owned()
            })
            .collect()
    }
}

/// Makes the `GPtrArray` that a search vfunc returns for a server match set.
/// Each element comes from the Camel string pool, not from `g_malloc`.
///
/// This is necessary: this class installs no `search_free`, so the base
/// implementation frees the result with `camel_pstring_free` on each element.
/// That is the wrong free for a `g_strdup` string.
#[cfg(camel_folder_search_object)]
fn pstring_uid_array(uids: &[String]) -> *mut glib_sys::GPtrArray {
    // SAFETY: a new array, with its size set so that all slots exist before
    // the loop writes one. `Reported` in `transfer.rs` makes its array the
    // same way.
    unsafe {
        let array = g_ptr_array_new();
        g_ptr_array_set_size(array, uids.len() as std::os::raw::c_int);
        for (index, uid) in uids.iter().enumerate() {
            let cuid = c_string(uid);
            let pooled = camel_pstring_strdup(cuid.as_ptr());
            *(*array).pdata.add(index) = pooled.cast_mut().cast();
        }
        array
    }
}

/// Registers the folder type, or returns it if it is already registered.
///
/// The registration is static, as for the store: a Camel provider is not a
/// `GTypeModule`, so a dynamic type is never unregistered.
pub fn folder_type() -> GType {
    register_static::<JmapFolder>()
}

/// Makes the folder for one mailbox of `store`. The caller owns the result.
///
/// The three properties belong to Camel and are necessary at construction.
/// `parent-store` is construct-only. A folder without a name for a short time
/// is a nameless folder in a store keyed by name.
///
/// The mailbox id is set immediately after construction, not through a
/// property. To be equally safe, a GObject property must be a construct
/// parameter, and it puts the id in the public API, but only this crate reads
/// it. Nothing can see the folder between the two steps, because this
/// reference is the only one.
///
/// # Safety
///
/// `store` must point at a live `CamelStore`. Camel asserts on other values,
/// and the result is a folder that belongs to no store.
pub unsafe fn new_folder(store: *mut CamelStore, mailbox: &FolderInfo) -> *mut CamelFolder {
    let full_name = c_string(&mailbox.path);
    let display_name = c_string(&mailbox.display_name);

    // SAFETY: a variadic construct call on a registered type. `CamelFolder`
    // declares each property named here, the two names are NUL-terminated
    // strings that Camel copies, `store` is a live `CamelStore` by the
    // contract of this function, and the list ends with NULL.
    let folder = unsafe {
        g_object_new(
            folder_type(),
            c"full-name".as_ptr(),
            full_name.as_ptr(),
            c"display-name".as_ptr(),
            display_name.as_ptr(),
            c"parent-store".as_ptr(),
            store,
            ptr::null::<gchar>(),
        )
    }
    .cast::<CamelFolder>();

    if folder.is_null() {
        // g_object_new fails only on a bad or abstract type. Both are bugs
        // here, not run-time conditions, so there is nothing to report or fill.
        return folder;
    }

    // SAFETY: `folder` is a new instance of this type, and this reference is
    // the only one, so no other code can read the slots.
    unsafe {
        (*folder.cast::<JmapFolder>())
            .mailbox
            .init(mailbox.id.clone());
        if let Some(cache) = account_cache(store) {
            (*folder.cast::<JmapFolder>()).cache.init(cache);
        }
        camel_folder_set_flags(folder, flags(mailbox));
        attach_summary(folder);
    }

    folder
}

/// The message cache of the account that `store` is, or `None` if there is
/// none.
///
/// The directory is the one that Camel gives the *service*: the session cache
/// directory with the account uid below it. Evolution's "empty cache" clears
/// this directory, and Camel removes it when the user deletes the account. Mail
/// cached in a different directory stays after the account is deleted.
///
/// # Safety
///
/// `store` must point at a live `CamelStore`, which is a `CamelService`.
unsafe fn account_cache(store: *mut CamelStore) -> Option<MessageCache> {
    // SAFETY: the contract above. The string belongs to the service, and this
    // code only reads it.
    let directory = unsafe {
        let directory = camel_service_get_user_cache_dir(store.cast::<CamelService>());
        if directory.is_null() {
            return None;
        }
        CStr::from_ptr(directory)
    };
    // A filesystem path is bytes and is not always UTF-8. A lossy read can
    // point the cache at a different directory, so a non-UTF-8 path gives no
    // cache.
    MessageCache::open(directory.to_str().ok()?)
}

/// How Camel must treat this folder.
///
/// `HAS_SUMMARY_CAPABILITY` is on all folders, because [`new_folder`] attaches
/// a summary to each folder immediately after it sets these flags. Camel
/// examines this flag before it asks for a message count or a uid list. Set
/// the flag and the summary together. Without the summary, the count fails.
/// Without the flag, Camel never asks for the contents.
///
/// Only the inbox gets other bits: the two that make the user's rules apply to
/// new mail. `FILTER_RECENT` is for the incoming filters, and `FILTER_JUNK` is
/// for the junk test. IMAPX sets the same two bits on the folder with the name
/// `"INBOX"`. This provider finds the inbox from its JMAP role, which comes
/// from the account data and not from a name convention.
///
/// This function does not set `IS_TRASH` or `IS_JUNK` from the role, and this
/// is an open question. The first guess was that
/// `camel_store_get_trash_folder_sync` sets them on the folder that it returns.
/// That is true for the virtual folder that Camel makes, but false for a
/// folder from a store. `tests/folders.rs` proves that the folder that
/// [`crate::folders`] returns for that call keeps this flags word unchanged.
/// Thus only that answer tells Camel which folder is the trash. Whether that
/// is sufficient depends on the code that reads the bit. Decide this bit when
/// a delete moves the message into the trash.
fn flags(mailbox: &FolderInfo) -> CamelFolderFlags {
    let role = match mailbox.role {
        Some(FolderRole::Inbox) => CAMEL_FOLDER_FILTER_RECENT | CAMEL_FOLDER_FILTER_JUNK,
        _ => 0,
    };
    CAMEL_FOLDER_HAS_SUMMARY_CAPABILITY | role
}

#[cfg(all(test, camel_folder_search_object))]
mod tests {
    use std::time::{Duration, Instant};

    use super::narrow;

    fn ids(range: std::ops::Range<usize>) -> Vec<String> {
        range.map(|i| format!("Mdeadbeef{i:07}")).collect()
    }

    /// `narrow` keeps the server order and removes only the uids that the
    /// caller did not ask about. These are the semantics of `[String]::contains`,
    /// and the set must keep them.
    #[test]
    fn narrowing_keeps_the_asked_for_uids_in_the_servers_order() {
        let matches = ["M3", "M1", "M2", "M9"].map(str::to_owned);
        let restrict = ["M2", "M3"].map(str::to_owned);
        assert_eq!(
            narrow(matches.iter().cloned(), &restrict),
            vec!["M3".to_owned(), "M2".to_owned()]
        );
        assert!(narrow(matches.into_iter(), &[]).is_empty());
    }

    /// A vFolder rebuild over a real mailbox reaches this size. At this size,
    /// the old linear scan took 21.6 s in a debug build. The limit is 100 times
    /// the measured time of the set (50 ms), so this test catches a return of
    /// the quadratic shape. It is not a benchmark.
    #[test]
    fn narrowing_a_folder_sized_match_set_does_not_take_seconds() {
        let matches = ids(0..50_000);
        let restrict = ids(0..50_000);
        let started = Instant::now();
        let narrowed = narrow(matches.into_iter(), &restrict);
        let took = started.elapsed();
        assert_eq!(narrowed.len(), 50_000);
        assert!(
            took < Duration::from_secs(5),
            "narrowing 50 000 x 50 000 took {took:?}; the linear scan is back"
        );
    }
}
