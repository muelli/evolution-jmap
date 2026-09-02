// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! `CamelJmapFolder`: one mailbox, as the object Camel opens.
//!
//! Everything the store has produced so far describes folders from the
//! outside — a `CamelFolderInfo` forest, plain structs Camel reads once and
//! frees. This is the folder itself: what `camel_store_get_folder_sync` hands
//! back, what Evolution keeps alive while the user has it open, and what every
//! later message operation is a method on.
//!
//! Being an object rather than a description is what makes it the first place
//! the provider keeps per-folder state, and the piece of state that has to be
//! there from the start is the JMAP mailbox id. Camel has no field for it and
//! nothing can recover it later: the path Camel keys the folder by is an
//! identifier this crate invented out of the mailbox's *name* (see
//! `jmap-mail-sync`'s `path` module), and the encoding is not reversible by
//! anything that only holds the result — while `Email/query`, which is where
//! the folder's contents come from, filters on `inMailbox`, an id. A folder
//! that knew only its path could describe itself and fetch nothing.
//!
//! ## Two flags words, one letter apart
//!
//! A folder has a flags word and so does a `CamelFolderInfo`, and they are not
//! the same word. `CamelFolderInfoFlags` — the one [`crate::folder_info`] fills
//! in — says what kind of folder this is: its type field, whether it is
//! subscribed, whether it has children. `CamelFolderFlags`, the one set here,
//! says how Camel *treats* the folder: whether incoming mail in it runs through
//! the user's filters, whether it is the account's trash. Only the second is
//! meaningful on an object, and only two of its bits can honestly be set by
//! this increment — see `flags`.
//!
//! The vfuncs a folder answers to — its summary, its messages — are not here
//! yet; this is the type, and what one is constructed from.

use std::ffi::CStr;
use std::ptr;

use eds_sys::{
    CAMEL_FOLDER_FILTER_JUNK, CAMEL_FOLDER_FILTER_RECENT, CAMEL_FOLDER_HAS_SUMMARY_CAPABILITY,
    CamelFolder, CamelFolderClass, CamelFolderFlags, CamelOfflineFolder, CamelOfflineFolderClass,
    CamelService, CamelStore, camel_folder_get_parent_store, camel_folder_set_flags,
    camel_offline_folder_get_type, camel_service_get_user_cache_dir,
};
use glib_sys::{GType, gchar};
use gobject_sys::g_object_new;
use jmap_backend_core::instance::Slot;
use jmap_backend_core::marshal::{checked_borrow, dispatched_borrow};
#[cfg(camel_folder_search_object)]
use jmap_backend_core::owned::Owned;
use jmap_backend_core::subclass::{ObjectSubclass, register_static};
use jmap_mail_sync::{FolderInfo, FolderRole};
use jmap_proto::Id;

use crate::cache::MessageCache;
use crate::folder_info::c_string;
use crate::store::{JmapStore, store_type};
use crate::summary::attach_summary;

/// The instance struct. `#[repr(C)]` leading with the parent's instance struct
/// is what makes a `*mut JmapFolder` usable as the `CamelFolder *` every Camel
/// function takes.
#[repr(C)]
pub struct JmapFolder {
    parent: CamelOfflineFolder,
    /// The mailbox this folder is a view of.
    ///
    /// Written once, by [`new_folder`], before anything else can reach the
    /// object, and never again: a mailbox that is renamed or moved is still
    /// the same mailbox, and one that is deleted leaves a folder Camel drops
    /// rather than re-points. A [`Slot`] rather than a plain field because the
    /// instance struct arrives zeroed and is freed without a destructor
    /// running over it — the same reason the store keeps its connection in one.
    mailbox: Slot<Id>,
    /// The messages of this account already downloaded, on disk.
    ///
    /// Filled by [`new_folder`] like the mailbox, and for a reason of the same
    /// kind: what a cache needs is the account's cache directory, and the
    /// moment that is both known and settled is when the folder is built on a
    /// store Camel has finished constructing. Empty on a folder whose cache
    /// directory could not be made, which is a folder that fetches every
    /// message it opens — see [`crate::cache`].
    ///
    /// It is not per-folder state despite living here: the entries are keyed by
    /// JMAP email id under the *account's* directory, so a message filed in
    /// several mailboxes is one file that every one of those folders reads.
    cache: Slot<MessageCache>,
}

impl JmapFolder {
    /// The JMAP mailbox id every request about this folder filters on.
    ///
    /// `None` on an instance whose construction did not finish, which is the
    /// state a vfunc reached on a half-built folder would find. Callers report
    /// that rather than assuming an id.
    pub fn mailbox(&self) -> Option<&Id> {
        self.mailbox.get()
    }

    /// The account's message cache, or `None` if this folder has none — which
    /// every caller treats as "fetch it", never as an error.
    pub fn cache(&self) -> Option<&MessageCache> {
        self.cache.get()
    }

    /// The Rust view of a `CamelFolder *` Camel handed over.
    ///
    /// # Safety
    ///
    /// `folder` must be NULL or point at an instance of this type. Camel only
    /// dispatches a class's vfuncs on instances of that class, so a vfunc's
    /// argument satisfies this; anything else has to check with
    /// `G_TYPE_CHECK_INSTANCE_TYPE` first.
    pub unsafe fn borrow<'a>(folder: *mut CamelFolder) -> Option<&'a Self> {
        // SAFETY: the doc comment above states the same contract.
        unsafe { dispatched_borrow(folder) }
    }
}

/// The store the folder hangs off, as our own.
///
/// Every request a folder makes goes out over the store's connection — the
/// folder holds a mailbox id and nothing else — so this is the first line of
/// [`crate::refresh`] and of [`crate::message`] alike, and it lives here rather
/// than in either of them because it is a fact about the folder object.
///
/// Type-checked rather than assumed, unlike the store vfuncs' first argument:
/// those are dispatched by GObject on an instance of the class, while
/// `parent-store` is an ordinary construct property that anything holding a
/// `CamelStore` could have been given. A folder of ours on someone else's store
/// is not a case that arises, but reading a `JmapStore` out of one would be
/// undefined behaviour rather than a wrong answer.
///
/// # Safety
///
/// `folder` must point at a live `CamelFolder`.
pub(crate) unsafe fn parent_store<'a>(folder: *mut CamelFolder) -> Option<&'a JmapStore> {
    // SAFETY: the accessor borrows the store the folder holds a reference to;
    // the contract above is what makes the type check itself legal, and the
    // check is what makes the cast inside sound.
    unsafe {
        let store = camel_folder_get_parent_store(folder);
        checked_borrow(store, store_type())
    }
}

/// The class struct, same rule one level up. It carries nothing of its own —
/// what it carries is the parent's vfunc slots with our functions in them,
/// which is still not the same as *being* `CamelOfflineFolderClass`: the type
/// needs a class of its own for those overrides to have somewhere to go.
#[repr(C)]
pub struct JmapFolderClass {
    parent_class: CamelOfflineFolderClass,
}

// SAFETY: both structs are #[repr(C)] and lead with the CamelOfflineFolder
// instance and class structs, whose layouts eds-sys's tests/layout.rs checks
// against `g_type_query`; CamelOfflineFolder derives from CamelFolder, from
// CamelObject, from GObject.
unsafe impl ObjectSubclass for JmapFolder {
    /// `CamelJmapFolder`, matching `CamelJmapStore`: Camel's own folders are
    /// all `Camel<Protocol>Folder`, and the type name is what a user sees in a
    /// GObject warning about the wrong folder type.
    const NAME: &'static CStr = c"CamelJmapFolder";
    type Instance = JmapFolder;
    type Class = JmapFolderClass;

    fn parent_type() -> GType {
        // SAFETY: no arguments, and the type initialises itself.
        unsafe { camel_offline_folder_get_type() }
    }

    unsafe fn class_init(class: *mut Self::Class) {
        // What the folder is asked about its contents. `CamelFolder` leaves
        // `refresh_info_sync` NULL and its wrapper answers TRUE without doing
        // anything for a class that has not filled it in — so without this line
        // a JMAP folder is one that reports every refresh as a success and
        // stays permanently empty.
        //
        // SAFETY: the class leads with CamelOfflineFolderClass, which leads
        // with CamelFolderClass — the contract above.
        unsafe { crate::refresh::install_vfuncs(class.cast::<CamelFolderClass>()) };

        // And what one of those rows becomes when the user opens it.
        // `camel_folder_get_message_sync` refuses to call a class that has not
        // filled this in, so without it a folder is one whose message list draws
        // and whose messages cannot be read.
        //
        // SAFETY: as above.
        unsafe { crate::message::install_vfuncs(class.cast::<CamelFolderClass>()) };

        // And the one vfunc that goes the other way: what the user changed
        // about a row, put on the server. `CamelFolder` leaves
        // `synchronize_sync` NULL and its wrapper answers TRUE for a class that
        // has not filled it in — so without this line a JMAP folder is one that
        // reports every flag change as saved and sends none of them.
        //
        // SAFETY: as above.
        unsafe { crate::synchronize::install_vfuncs(class.cast::<CamelFolderClass>()) };

        // And the other thing the user does to a message, which is not a
        // property of it at all: where it is filed. `CamelFolder` fills this
        // slot in with a generic implementation that downloads every message
        // and appends it to the destination — correct between two accounts and
        // absurd within one, where the server can file the message itself
        // without the mail ever leaving it.
        //
        // SAFETY: as above.
        unsafe { crate::transfer::install_vfuncs(class.cast::<CamelFolderClass>()) };

        // And the arrival the one above cannot serve: a message that is not in
        // the account yet. `CamelFolder` leaves `append_message_sync` NULL and
        // `camel_folder_append_message_sync` refuses to call a class that has
        // not filled it in — so without this line a message dragged in from
        // another account, or a draft the composer saves, fails inside Camel.
        //
        // SAFETY: as above.
        unsafe { crate::append::install_vfuncs(class.cast::<CamelFolderClass>()) };

        // And the departure none of those is: a message the user is finished
        // with. `CamelFolder` leaves `expunge_sync` NULL and its wrapper
        // answers TRUE for a class that has not filled it in — so without this
        // line "Expunge" and "Empty Trash" are menu items that report success
        // and leave every message where it was.
        //
        // SAFETY: as above.
        unsafe { crate::expunge::install_vfuncs(class.cast::<CamelFolderClass>()) };

        // And what the account's mailbox quota looks like. Unlike the vfuncs
        // above, `CamelFolder`'s own base class already fills this one in —
        // with an implementation that always answers
        // `G_IO_ERROR_NOT_SUPPORTED` — so without this line a JMAP account is
        // not broken, only one Evolution's folder-properties dialog shows no
        // quota row for. This line is what makes it show the account's real
        // usage where the server reports one.
        //
        // SAFETY: as above.
        unsafe { crate::quota::install_vfuncs(class.cast::<CamelFolderClass>()) };

        // And what the folder answers when it is asked which of its messages
        // match an expression — which is not only the search bar: every
        // message-list view is one ("Unread Messages", "Hide Deleted
        // Messages"), so a folder that cannot answer is a folder whose list
        // does not draw.
        //
        // Only up to EDS 3.52, and this is the one vfunc in the class that a
        // newer EDS takes *away* from a provider rather than renaming. There,
        // `CamelFolderClass` leaves `search_by_expression`/`search_by_uids`
        // NULL and asserts on a class that has not filled them in, so the two
        // functions below fill them with a `CamelFolderSearch` over the local
        // summary. From 3.58 both slots are gone: `search_sync` replaces them
        // and the base class installs an implementation of it built on
        // `CamelStoreSearch`, over the same rows and with the same result. So
        // the right thing to install on a newer EDS is nothing at all —
        // overriding would replace a working implementation with a
        // reimplementation of it. `tests/search.rs` is what holds that claim
        // up: it drives whichever entry point the EDS in front of it has and
        // asserts the same answers on both.
        #[cfg(camel_folder_search_object)]
        {
            let folder_class = unsafe { &mut *class.cast::<CamelFolderClass>() };
            folder_class.search_by_expression = Some(search_by_expression);
            folder_class.search_by_uids = Some(search_by_uids);
        }
    }

    // No `instance_init`, unlike the store's: there is nothing to fill in yet.
    // The mailbox is not known until `new_folder` has the `FolderInfo`, and a
    // zeroed `Slot` — which is what GObject hands over — is already an empty
    // one. `finalize` is not optional the same way, because the slot may by
    // then have something in it.

    unsafe fn finalize(instance: *mut Self::Instance) {
        // SAFETY: the instance is being finalized, so nothing can still reach
        // it and no borrow handed out by `get` is alive. Without this the id
        // leaks — once per folder the user ever opened — and the cache's
        // reference on its `CamelDataCache` with it.
        unsafe {
            (*instance).mailbox.clear();
            (*instance).cache.clear();
        };
    }
}

/// `CamelFolderClass.search_by_expression`, up to EDS 3.52: which of this
/// folder's messages match `expression`.
///
/// A `CamelFolderSearch` over the folder's own summary, which is what a local
/// search *is* — the rows are already here, and an expression over flags or
/// headers needs nothing from the server. Built and dropped per call rather than
/// kept on the folder: the object holds the folder it was pointed at, and a
/// cached one would be a second owner of state Camel expects the search to read
/// fresh.
///
/// Gone from 3.58, where the base class does this itself — see `class_init`.
#[cfg(camel_folder_search_object)]
unsafe extern "C" fn search_by_expression(
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

/// `CamelFolderClass.search_by_uids`: the same question narrowed to a list of
/// uids, and answered the same way — the uid list is the one extra argument
/// `camel_folder_search_search` takes.
///
/// Gone from 3.58 alongside its sibling.
#[cfg(camel_folder_search_object)]
unsafe extern "C" fn search_by_uids(
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

/// Registers the folder type, or returns it if it is already registered.
///
/// Statically, like the store's and for the same reason: a Camel provider is
/// not a `GTypeModule`, so there is no unload for a dynamic type to be
/// unregistered by.
pub fn folder_type() -> GType {
    register_static::<JmapFolder>()
}

/// Builds the folder for one mailbox of `store`, owned by the caller.
///
/// The three properties are Camel's, and all three are needed at construction:
/// `parent-store` is construct-only, and a folder whose name arrived later
/// would be one that existed, briefly, as a nameless folder in a store keyed by
/// name. The mailbox id is filled in immediately afterwards rather than through
/// a property of our own — a GObject property would have to be a construct
/// parameter to be equally safe, and declaring one would put the id in the
/// public API of a type whose only reader is this crate. Nothing can observe
/// the folder between the two: the reference is still the only one.
///
/// # Safety
///
/// `store` must point at a live `CamelStore`; Camel asserts on anything else
/// and would leave a folder belonging to nothing.
pub unsafe fn new_folder(store: *mut CamelStore, mailbox: &FolderInfo) -> *mut CamelFolder {
    let full_name = c_string(&mailbox.path);
    let display_name = c_string(&mailbox.display_name);

    // SAFETY: a variadic construct call on a registered type. Every property
    // named is one `CamelFolder` declares, the two names are NUL-terminated
    // strings Camel copies, `store` is a live `CamelStore` by this function's
    // contract, and the list is NULL-terminated.
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
        // g_object_new only fails on a bad type or an abstract one, both of
        // which are bugs here rather than conditions; there is nothing to
        // report to and nothing to fill in.
        return folder;
    }

    // SAFETY: `folder` is a fresh instance of this type, and this reference is
    // the only one — nothing else can be reading the slots.
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

/// The message cache of the account `store` is, or `None` if it has none.
///
/// The directory is the one Camel gives the *service* — its session's cache
/// directory with the account's uid under it — rather than one composed here:
/// it is the directory Evolution's own "empty cache" clears and the one Camel
/// removes when the account is deleted, and a provider that cached mail outside
/// it would be a provider whose mail survives the account.
///
/// # Safety
///
/// `store` must point at a live `CamelStore`, which is a `CamelService`.
unsafe fn account_cache(store: *mut CamelStore) -> Option<MessageCache> {
    // SAFETY: the contract above; the string belongs to the service and is only
    // read here.
    let directory = unsafe {
        let directory = camel_service_get_user_cache_dir(store.cast::<CamelService>());
        if directory.is_null() {
            return None;
        }
        CStr::from_ptr(directory)
    };
    // A path Camel built out of a session directory and an ESource uid. Not
    // UTF-8 on principle — a filesystem path is bytes — so a lossy read would
    // point the cache somewhere else; refusing is the version that cannot
    // silently write to the wrong directory.
    MessageCache::open(directory.to_str().ok()?)
}

/// How Camel should treat this folder.
///
/// `HAS_SUMMARY_CAPABILITY` is on every folder, because every folder is given a
/// summary by [`new_folder`] a line after this is read. It is the flag Camel
/// tests before it asks a folder for a message count or a uid list at all, so
/// the two have to be set together: the flag without the summary is a folder
/// that says it can be counted and then cannot, and the summary without the
/// flag is a folder whose contents are never asked for.
///
/// Beyond that only the inbox gets anything, and it gets the two bits that make
/// new mail arriving in it run through the user's rules: `FILTER_RECENT` for
/// the incoming filters, `FILTER_JUNK` for the junk test. Camel's own IMAPX
/// sets exactly this pair, on the folder it identifies by comparing its name
/// against `"INBOX"`; this provider knows which mailbox is the inbox from its
/// JMAP role instead — the same decision taken from the account's data rather
/// than from a convention about a name.
///
/// `IS_TRASH` and `IS_JUNK` are still not set from the role, and they are the
/// one open question this function has left. The guess when it was written was
/// that `camel_store_get_trash_folder_sync` marks the folder it *returns* with
/// them; that is true of the virtual folder Camel builds itself and false of
/// anything a store hands over — `tests/folders.rs` pins the flags word of the
/// folder [`crate::folders`] answers that call with, and it is this one
/// unchanged. So nothing in this provider tells Camel which folder is the trash
/// beyond the answer to the question, and whether that is enough is decided by
/// what reads the bit: it belongs with the increment that makes a delete file
/// the message into the trash, not with one that only says which mailbox that
/// is.
fn flags(mailbox: &FolderInfo) -> CamelFolderFlags {
    let role = match mailbox.role {
        Some(FolderRole::Inbox) => CAMEL_FOLDER_FILTER_RECENT | CAMEL_FOLDER_FILTER_JUNK,
        _ => 0,
    };
    CAMEL_FOLDER_HAS_SUMMARY_CAPABILITY | role
}
