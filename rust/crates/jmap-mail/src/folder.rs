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
//! this increment — see [`flags`].
//!
//! The vfuncs a folder answers to — its summary, its messages — are not here
//! yet; this is the type, and what one is constructed from.

use std::ffi::CStr;
use std::ptr;

use eds_sys::{
    CAMEL_FOLDER_FILTER_JUNK, CAMEL_FOLDER_FILTER_RECENT, CAMEL_FOLDER_HAS_SUMMARY_CAPABILITY,
    CamelFolder, CamelFolderClass, CamelFolderFlags, CamelOfflineFolder, CamelOfflineFolderClass,
    CamelStore, camel_folder_set_flags, camel_offline_folder_get_type,
};
use glib_sys::{GType, gchar};
use gobject_sys::g_object_new;
use jmap_backend_core::instance::Slot;
use jmap_backend_core::subclass::{ObjectSubclass, register_static};
use jmap_mail_sync::{FolderInfo, FolderRole};
use jmap_proto::Id;

use crate::folder_info::c_string;
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

    /// The Rust view of a `CamelFolder *` Camel handed over.
    ///
    /// # Safety
    ///
    /// `folder` must be NULL or point at an instance of this type. Camel only
    /// dispatches a class's vfuncs on instances of that class, so a vfunc's
    /// argument satisfies this; anything else has to check with
    /// `G_TYPE_CHECK_INSTANCE_TYPE` first.
    pub unsafe fn borrow<'a>(folder: *mut CamelFolder) -> Option<&'a Self> {
        unsafe { folder.cast::<Self>().as_ref() }
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
    }

    // No `instance_init`, unlike the store's: there is nothing to fill in yet.
    // The mailbox is not known until `new_folder` has the `FolderInfo`, and a
    // zeroed `Slot` — which is what GObject hands over — is already an empty
    // one. `finalize` is not optional the same way, because the slot may by
    // then have something in it.

    unsafe fn finalize(instance: *mut Self::Instance) {
        // SAFETY: the instance is being finalized, so nothing can still reach
        // it and no borrow handed out by `get` is alive. Without this the id
        // leaks — once per folder the user ever opened.
        unsafe { (*instance).mailbox.clear() };
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
    // the only one — nothing else can be reading the slot.
    unsafe {
        (*folder.cast::<JmapFolder>())
            .mailbox
            .init(mailbox.id.clone());
        camel_folder_set_flags(folder, flags(mailbox));
        attach_summary(folder);
    }

    folder
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
/// `IS_TRASH` and `IS_JUNK` are still not set from the role: they are what
/// `camel_store_get_trash_folder_sync` and its junk counterpart mark the folder
/// they *return* with, and marking a folder with them here would tell Camel to
/// treat the mailbox as the account's trash before anything asked for one.
fn flags(mailbox: &FolderInfo) -> CamelFolderFlags {
    let role = match mailbox.role {
        Some(FolderRole::Inbox) => CAMEL_FOLDER_FILTER_RECENT | CAMEL_FOLDER_FILTER_JUNK,
        _ => 0,
    };
    CAMEL_FOLDER_HAS_SUMMARY_CAPABILITY | role
}
