// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! The `CamelStore` folder vfuncs.
//!
//! - `get_folder_info_sync` gives the listing of the account's folders.
//! - `get_folder_sync` opens one folder by path.
//! - `get_inbox_folder_sync`, `get_trash_folder_sync` and
//!   `get_junk_folder_sync` open one folder by purpose.
//! - `can_refresh_folder` answers a question about one folder of a listing.
//! - `initial_setup_sync` gives the Sent and Drafts folders to the Evolution
//!   account wizard. It does not open them.
//!
//! The first five vfuncs all read the tree that [`JmapStore::folders`] keeps.
//! Of these five, the last four change a path or a role from that tree back
//! into its mailbox. The listing converts a subtree into C structs that Camel frees.
//! Opening builds the [`crate::folder`] object that Camel keeps. The three
//! purpose vfuncs use the opening, so that all of them get the same object.
//!
//! ## The listing
//!
//! [`JmapStore::folders`] keeps the tree and decides when to fetch it again.
//! [`FolderInfoChain`] converts a tree into the C forest that Camel frees.
//! [`Request`] reads the two remaining arguments: `top`, the root of the
//! answer, and `CAMEL_STORE_FOLDER_INFO_RECURSIVE`, its depth. It is a
//! separate type, so that tests can examine it without a `CamelStore`.
//!
//! ### What Camel means by the arguments
//!
//! `camel_store_get_folder_info_sync`'s own documentation: "This fetches
//! information about the folder structure of @store, starting with @top […] If
//! @flags includes `CAMEL_STORE_FOLDER_INFO_RECURSIVE`, the returned tree will
//! include all levels of hierarchy below @top. If not, it will only include the
//! immediate subfolders of @top." A NULL or empty `top` is the account itself.
//! The wrapper uses the same test (`top == NULL || *top == '\0'`), so the store
//! must read the two forms the same way. The folder that `top` names is in the
//! answer as the head of the chain. IMAPX and `camel_folder_info_build` do the
//! same.
//!
//! This store obeys `RECURSIVE`, although IMAPX, the reference implementation,
//! has a `/* FIXME: obey other flags */` at that location. All real callers
//! (the Evolution folder cache and subscription editor, and
//! `camel_store_delete_folder_sync`) set it. Only the virtual-folder paths of
//! `camel_store_get_folder_info_sync` clear it, and they want only the top
//! level. Thus no caller loses, and a deep account does not convert its full
//! tree into C for a question about one level.
//!
//! This store also reads `SUBSCRIBED` and `SUBSCRIPTION_LIST`. Both are
//! filters on the folders, not different requests. `Mailbox/get` returns all
//! mailboxes of the account with their `isSubscribed`, so this store does not
//! need the second call (`LSUB` next to `LIST`) that an IMAP store makes.
//! [`Request::new`] tells what each flag asks for, and why an unsubscribed
//! folder can stay in the answer.
//!
//! This store does not read two flags. `FAST` is deprecated, and "most
//! backends will behave the same whether it is supplied or not". This is true
//! here, because each JMAP mailbox includes its counts. `NO_VIRTUAL` is not
//! for this vfunc: the wrapper adds and removes vTrash and vJunk.
//!
//! ## The sixth vfunc: a question about one folder of the listing
//!
//! [`refreshable`] answers `can_refresh_folder`, the one non-blocking slot in
//! `CamelStoreClass`: "Returns if this folder (param info) should be checked
//! for new mail or not". Camel asks it once for each folder of a listing. It
//! reads only the flags word, with no store state and no request. Camel calls
//! it during a walk over a forest that it already has. A request to the server
//! here would make one Send / Receive do one round trip for each folder.
//!
//! ## The seventh vfunc: one call, answered from the same tree
//!
//! The Evolution account wizard calls [`initial_setup_sync`] only after the
//! first successful connection of a new account. `CAMEL_STORE_SUPPORTS_INITIAL_SETUP`
//! enables the call. The `instance_init` of [`crate::store`] sets this flag
//! together with the vfunc pointer that this module installs. The answer is
//! not a folder. It is two entries of a `GHashTable` that Evolution saves on
//! the account sources: `CAMEL_STORE_SETUP_SENT_FOLDER` and
//! `CAMEL_STORE_SETUP_DRAFTS_FOLDER`. Each entry is a Camel path, which the
//! wizard changes into a folder URI.
//!
//! RFC 8621 §2 gives a `role` to `sent` and `drafts` mailboxes, as it does to
//! `inbox`, `trash` and `junk`. This vfunc reads the same tree that
//! [`open_by_role`] reads for those three. The difference is the result: a
//! string for Evolution, not a folder for Camel. If the server assigns neither
//! role, nothing is saved. This agrees with the documented contract:
//! "The function should return TRUE even if it didn't populate anything."

use std::borrow::Cow;
use std::ffi::CStr;
use std::ptr;
use std::sync::Arc;

use eds_sys::{
    CAMEL_FOLDER_SUBSCRIBED, CAMEL_FOLDER_TYPE_INBOX, CAMEL_FOLDER_TYPE_MASK,
    CAMEL_STORE_FOLDER_INFO_RECURSIVE, CAMEL_STORE_FOLDER_INFO_REFRESH,
    CAMEL_STORE_FOLDER_INFO_SUBSCRIBED, CAMEL_STORE_FOLDER_INFO_SUBSCRIPTION_LIST,
    CAMEL_STORE_SETUP_DRAFTS_FOLDER, CAMEL_STORE_SETUP_SENT_FOLDER, CamelFolder, CamelFolderInfo,
    CamelFolderInfoFlags, CamelStore, CamelStoreClass, CamelStoreGetFolderFlags,
    CamelStoreGetFolderInfoFlags, GHashTable, camel_folder_get_folder_summary,
    camel_store_get_folder_sync, g_hash_table_insert,
};
use gio_sys::GCancellable;
use glib_sys::{GError, GFALSE, GTRUE, g_strdup, gboolean, gchar};
use jmap_backend_core::cancel::observe;
use jmap_backend_core::error::{fail, fail_bool};
use jmap_backend_core::marshal::read_string;
use jmap_backend_core::owned::Owned;
use jmap_backend_core::trampoline::{guard, guard_bool, guard_ptr};
use jmap_mail_sync::{FolderInfo, FolderRole, FolderTree};

use crate::connect::StoreError;
use crate::folder::new_folder;
use crate::folder_info::{FolderInfoChain, c_string};
use crate::refresh::refresh_info_sync;
use crate::store::JmapStore;
use crate::summary::summary_state;

/// The part of the store's folder tree that one `get_folder_info_sync` call
/// asks for.
pub struct Request<'a> {
    /// The sibling chain at the root of the answer: the one folder that `top`
    /// names, or all top-level folders if `top` names no folder.
    ///
    /// Empty if no folder has the path `top`. This is a valid question with a
    /// valid empty answer (see [`Request::new`]).
    ///
    /// Borrowed from the store's tree if the call wants the folders unchanged.
    /// Owned if the call asks for a filtered view: a subtree with folders
    /// removed is not part of the store's tree, so it must be built.
    pub roots: Cow<'a, [FolderInfo]>,
    /// The number of descendant levels below the roots that the answer
    /// includes; `None` for all levels.
    pub depth: Option<usize>,
}

impl<'a> Request<'a> {
    /// Reads the vfunc's `top` and `flags` against the store's tree.
    ///
    /// The depth is different by one between the two `top` cases. "The
    /// immediate subfolders of `top`" is one level below a folder that is in
    /// the answer. But the account's top-level folders *are* the immediate
    /// subfolders of the root. The root is not a folder and is not in the
    /// answer, so no level below them is included.
    ///
    /// If `top` matches no folder, the result has no roots and is not an
    /// error. IMAPX's `get_folder_info_offline` also gives an empty answer:
    /// `camel_folder_info_build` returns NULL for an empty array, and no
    /// GError is set. This case is usual. Another client can delete a folder
    /// before Camel knows about it, and Camel then asks for it again. An error
    /// here would show an account as broken because of a change by a
    /// different client.
    ///
    /// ## The two subscription flags
    ///
    /// `SUBSCRIBED` asks for the folders that the user ticked. The Evolution
    /// folder tree adds it for a `CamelSubscribable` store, so a tick in the
    /// subscription editor changes what the user sees. The filter applies to
    /// the folders that `top` selects, not in place of them. A caller that
    /// asks about one subtree of a filtered account wants the filtered part of
    /// that subtree.
    ///
    /// `SUBSCRIPTION_LIST` is the question of the subscription editor: which
    /// folders can the user tick? The answer is all folders. This store
    /// already has them, because `Mailbox/get` returns all mailboxes of the
    /// account with their `isSubscribed`. If a caller sets both flags,
    /// `SUBSCRIPTION_LIST` wins. An editor that shows only ticked folders
    /// does not let the user tick a new folder.
    pub fn new(
        tree: &'a FolderTree,
        top: Option<&str>,
        flags: CamelStoreGetFolderInfoFlags,
    ) -> Self {
        let (roots, below) = match top.filter(|top| !top.is_empty()) {
            Some(top) => (
                tree.find(top).map(std::slice::from_ref).unwrap_or_default(),
                1,
            ),
            None => (tree.roots(), 0),
        };

        let subscribed_only = flags & CAMEL_STORE_FOLDER_INFO_SUBSCRIBED != 0
            && flags & CAMEL_STORE_FOLDER_INFO_SUBSCRIPTION_LIST == 0;

        Self {
            roots: if subscribed_only {
                Cow::Owned(ticked(roots))
            } else {
                Cow::Borrowed(roots)
            },
            depth: (flags & CAMEL_STORE_FOLDER_INFO_RECURSIVE == 0).then_some(below),
        }
    }

    /// The forest that answers this request. The caller owns it until it gives
    /// it to Camel.
    pub fn answer(&self) -> FolderInfoChain {
        FolderInfoChain::from_forest(&self.roots, self.depth)
    }
}

/// The part of a sibling chain that the user's ticks keep: each subscribed
/// folder, and each ancestor of a subscribed folder.
///
/// The ancestors make this more than a filter. In `CamelFolderInfo`, a child
/// is attached to its parent, so `Work/Invoices` cannot be in the answer
/// without `Work`. If an unticked parent were removed, the ticked folder below
/// it would also go, and the user could not get to that mail. An IMAP server
/// has the same problem and solves it the same way: it returns the
/// unsubscribed parents that the `LSUB` children need.
///
/// Such a folder does *not* get changed flags. It keeps `subscribed: false`,
/// so the subscription editor does not show a tick that the user did not set.
/// It does not get `CAMEL_FOLDER_NOSELECT`. Camel documents this flag as "the
/// folder cannot contain messages", but an unticked JMAP mailbox holds mail
/// like all other mailboxes, and Camel acts on that flag. Thus, if the user
/// unticks a folder that has a ticked folder below it, the folder stays in the
/// tree and the user can open it. The alternative is worse.
///
/// The function does change the children. If the user unticked all children of
/// a folder, the folder has no children *in this view*, and its flags show
/// this. The depth cut in [`FolderInfoChain::from_forest`] is different: it
/// keeps `CAMEL_FOLDER_CHILDREN` on a folder whose children it removes,
/// because those children exist and the expander lets the caller ask for them.
///
/// The function is iterative, for the reason that `from_forest` gives: the
/// depth of the tree comes from a `parentId` chain that the server controls.
fn ticked(siblings: &[FolderInfo]) -> Vec<FolderInfo> {
    // First, a pre-order walk that records the index of each folder's parent.
    // The answer is built in the opposite direction, because the result for a
    // folder depends on the results for all folders below it.
    let mut order: Vec<(&FolderInfo, Option<usize>)> = Vec::new();
    let mut pending: Vec<(&FolderInfo, Option<usize>)> =
        siblings.iter().rev().map(|folder| (folder, None)).collect();
    while let Some((folder, parent)) = pending.pop() {
        let index = order.len();
        order.push((folder, parent));
        pending.extend(
            folder
                .children
                .iter()
                .rev()
                .map(|child| (child, Some(index))),
        );
    }

    // Reverse pre-order gets to each descendant before its ancestor. Thus the
    // kept children of a folder are known before the function decides about
    // the folder.
    let mut kept: Vec<Vec<FolderInfo>> = vec![Vec::new(); order.len()];
    let mut roots: Vec<FolderInfo> = Vec::new();
    for (index, (folder, parent)) in order.into_iter().enumerate().rev() {
        let mut children = std::mem::take(&mut kept[index]);
        if !folder.subscribed && children.is_empty() {
            continue;
        }
        // The reversed walk collected them, so they are in reverse sibling
        // order.
        children.reverse();
        // Each field is copied, not `..folder.clone()`. That clone would copy
        // the full subtree and then discard it.
        let folder = FolderInfo {
            id: folder.id.clone(),
            path: folder.path.clone(),
            display_name: folder.display_name.clone(),
            role: folder.role,
            total: folder.total,
            unread: folder.unread,
            subscribed: folder.subscribed,
            may_add_items: folder.may_add_items,
            children,
        };
        match parent {
            Some(parent) => kept[parent].push(folder),
            None => roots.push(folder),
        }
    }

    roots.reverse();
    roots
}

// ---------------------------------------------------------------------------
// the vfunc slot

/// Installs the store's folder vfuncs on a class whose first member is a
/// `CamelStoreClass`.
///
/// # Safety
///
/// `class` must point to an initialised class struct that starts with a
/// `CamelStoreClass`. This is true for all descendants of `CamelStore`.
pub unsafe fn install_vfuncs(class: *mut CamelStoreClass) {
    // SAFETY: the contract above.
    let vfuncs = unsafe { &mut *class };
    vfuncs.can_refresh_folder = Some(can_refresh_folder);
    vfuncs.get_folder_info_sync = Some(get_folder_info_sync);
    vfuncs.get_folder_sync = Some(get_folder_sync);
    vfuncs.get_inbox_folder_sync = Some(get_inbox_folder_sync);
    vfuncs.get_trash_folder_sync = Some(get_trash_folder_sync);
    vfuncs.get_junk_folder_sync = Some(get_junk_folder_sync);
    vfuncs.initial_setup_sync = Some(initial_setup_sync);
}

/// Tells if Evolution must automatically check this folder for new mail: the
/// inbox, and each folder that the user subscribed to.
///
/// The flags word is the only input, because the question is only about those
/// flags. `CAMEL_FOLDER_TYPE_INBOX` is the default answer of `CamelStore`, and
/// this function keeps it. New mail arrives in the inbox, and an account with
/// an unchecked inbox never reports new mail.
///
/// This store adds `CAMEL_FOLDER_SUBSCRIBED`, because of RFC 8621 §2. It
/// defines `isSubscribed` as "has the user indicated they wish to see this
/// Mailbox". That is the same user and the same intent as "check this folder
/// for new mail". On JMAP, servers usually file mail into folders, so
/// important mail often does not go into the inbox. If the store checked only
/// the inbox, the folder counts would stay old until the user clicked each
/// folder.
///
/// Other providers use a *setting* for this: IMAPX has `check-all` and
/// `check-subscribed`, and evolution-ews has `check-all` and a tick for each
/// folder. This store needs neither, because of the protocol. An IMAP store
/// must ask, because `LIST` and `LSUB` are separate round trips, and IMAP
/// subscriptions often do not match the folders to check. `Mailbox/get`
/// returns each mailbox with its `isSubscribed` in the call that the listing
/// already made, so the answer costs nothing. If a user wants "check all
/// folders" independently of the ticks, add a settings property next to this
/// rule. Do not guess one now.
///
/// One case is not visible here. An unticked folder that is in the listing
/// *only* because a ticked folder is below it (see `ticked`) gets the answer
/// no. The user chose not to see it. Camel asks separately about the ticked
/// folder below it.
///
/// This function does not test `CAMEL_FOLDER_NOSELECT`. This store never sets
/// it (see `ticked`: an unticked ancestor holds mail like all other mailboxes).
/// Also, `mail-send-recv.c` in Evolution removes `NOSELECT` folders itself
/// during the walk that asks this question. A test here would answer again a
/// question that the caller answered, about a flag that this store does not
/// set.
pub fn refreshable(flags: CamelFolderInfoFlags) -> bool {
    let is_inbox =
        flags & CAMEL_FOLDER_TYPE_MASK as CamelFolderInfoFlags == CAMEL_FOLDER_TYPE_INBOX;
    is_inbox || flags & CAMEL_FOLDER_SUBSCRIBED != 0
}

/// `can_refresh_folder`: [`refreshable`], as the vfunc that Camel calls.
///
/// The function never sets the `error` out-parameter, because no failure can
/// occur: the answer is a test on a word from Camel. The Camel default does
/// the same, and the only caller of `camel_store_can_refresh_folder` gives
/// NULL for it.
///
/// The function also does not read `store`, so it is safe without a
/// connection. Evolution calls it during a walk over a listing. A provider
/// that read the account state here would make a question about a folder into
/// a question about the network.
unsafe extern "C" fn can_refresh_folder(
    _store: *mut CamelStore,
    info: *mut CamelFolderInfo,
    _error: *mut *mut GError,
) -> gboolean {
    guard("can_refresh_folder", GFALSE, || {
        // SAFETY: Camel's contract for the vfunc: an info from a listing, which
        // the Camel wrapper already checked is non-NULL. Camel does not give a
        // NULL here. If it does, the answer no is the harmless answer.
        let Some(info) = (unsafe { info.as_ref() }) else {
            return GFALSE;
        };
        if refreshable(info.flags) {
            GTRUE
        } else {
            GFALSE
        }
    })
}

/// Gives the account's folders, or the part of them that Camel asked for.
///
/// NULL is the failure value and also a valid answer: an account with no
/// folders, or a `top` that names no folder. Thus the error is what tells the
/// two apart. This function never sets an error when the answer simply has no
/// folders.
///
/// The function [`observe`]s `cancellable` during the call. If the user stops
/// the listing, the answer is `G_IO_ERROR_CANCELLED` and no tree. The listing
/// is one or two round trips, not a paged walk, so the stop usually occurs
/// before the request. But it still prevents a hang on a server that does not
/// reply.
unsafe extern "C" fn get_folder_info_sync(
    store: *mut CamelStore,
    top: *const gchar,
    flags: CamelStoreGetFolderInfoFlags,
    cancellable: *mut GCancellable,
    error: *mut *mut GError,
) -> *mut CamelFolderInfo {
    // SAFETY: Camel's contract for the vfunc: a valid instance of ours, a
    // NULL-or-valid string, and an out-parameter that is NULL or writable and
    // currently NULL.
    unsafe {
        guard_ptr("get_folder_info_sync", error, || {
            // SAFETY: Camel keeps its cancellable alive for the duration of the
            // call, so it lives longer than this observation. Thus each request
            // below stops when the user pushes Stop.
            let _cancel = observe(cancellable);

            let Some(store) = JmapStore::borrow(store) else {
                return fail(error, &StoreError::Disconnected, StoreError::to_gerror);
            };
            // Borrowed from Camel and NUL-terminated; `read_string` copies.
            let top = read_string(top);

            let tree = match store.folders(flags) {
                Ok(tree) => tree,
                Err(failure) => return fail(error, &failure, StoreError::to_gerror),
            };

            // The forest borrows the tree only while it is built, and it owns
            // copies of all data that it takes.
            Request::new(&tree, top.as_deref(), flags)
                .answer()
                .into_raw()
        })
    }
}

/// Opens one folder of the store: the vfunc of `camel_store_get_folder_sync`.
///
/// This function must *not* keep the folder. `CamelStore` owns a
/// `CamelObjectBag` of its open folders (public as
/// `camel_store_get_folders_bag`, with the keys of the class's
/// `hash_folder_name`/`equal_folder_name`). The wrapper reserves the folder
/// name in that bag before it calls this function, so a second call for the
/// same path does not get here. A cache of our own would duplicate the bag,
/// and could give two `CamelFolder`s (two summaries, two sets of flags) for
/// one mailbox.
///
/// The function does not read the flags. `CREATE` asks to make a missing
/// folder, which for JMAP is a `Mailbox/set` and is the task of
/// `create_folder_sync`. `BODY_INDEX` asks for a body index, which this
/// provider does not build. `PRIVATE` is about vFolder membership, which the
/// wrapper controls. `EXCL` is documented as not honoured.
///
/// The function [`observe`]s `cancellable` during the call, as
/// [`get_folder_info_sync`] does. If the held tree does not have the path,
/// the function lists the account again, and the user can stop that request.
unsafe extern "C" fn get_folder_sync(
    store: *mut CamelStore,
    folder_name: *const gchar,
    _flags: CamelStoreGetFolderFlags,
    cancellable: *mut GCancellable,
    error: *mut *mut GError,
) -> *mut CamelFolder {
    // SAFETY: Camel's contract for the vfunc: a valid instance of ours, a
    // NUL-terminated string, and an out-parameter that is NULL or writable and
    // currently NULL.
    unsafe {
        guard_ptr("get_folder_sync", error, || {
            // SAFETY: Camel keeps its cancellable alive for the duration of the
            // call, so it lives longer than this observation. Thus each request
            // below stops when the user pushes Stop.
            let _cancel = observe(cancellable);

            let Some(instance) = JmapStore::borrow(store) else {
                return fail(error, &StoreError::Disconnected, StoreError::to_gerror);
            };
            // Borrowed from Camel and NUL-terminated; `read_string` copies. A
            // NULL or empty name becomes the empty path. No mailbox has that
            // path, because a path always has a component.
            let path = read_string(folder_name).unwrap_or_default();

            let tree = match tree_holding(instance, |tree| tree.find(&path).is_some()) {
                Ok(tree) => tree,
                Err(failure) => return fail(error, &failure, StoreError::to_gerror),
            };
            let Some(mailbox) = tree.find(&path) else {
                return fail(error, &StoreError::NoFolder(path), StoreError::to_gerror);
            };

            // SAFETY: `store` is the live `CamelStore` borrowed above, which is
            // what `new_folder` asks for. The pointer that it returns is
            // transfer-full, which is what `Owned::from_raw` takes.
            let Some(folder) = Owned::from_raw(new_folder(store, mailbox)) else {
                return ptr::null_mut();
            };

            // A summary with no state (a new account, or a folder not opened
            // before) is empty. Refresh the folder on the first open, so that
            // the first view shows messages.
            let summary = camel_folder_get_folder_summary(folder.as_ptr());
            if summary_state(summary).is_none() {
                let ok = refresh_info_sync(folder.as_ptr(), cancellable, error);
                if ok == GFALSE {
                    // `folder` drops here and releases the reference from
                    // `new_folder`.
                    return ptr::null_mut();
                }
            }

            folder.into_raw()
        })
    }
}

/// Opens the account's inbox: the vfunc of `camel_store_get_inbox_folder_sync`.
///
/// Camel asks for this folder by *purpose*, and only the purpose can find it.
/// Thus this store overrides the vfunc. The `CamelStoreClass` implementation
/// asks `get_folder_sync` for a folder with the name `inbox`. IMAPX does the
/// same with `"INBOX"`. These are IMAP conventions, not facts about all mail
/// stores. RFC 8621 §2 gives a mailbox a `role`, and does not specify the
/// mailbox name or its position in the hierarchy. A JMAP account can keep its
/// inbox below a parent for each address, with a name in the user's language.
/// If such an account *also* has a usual mailbox with the name "inbox", the
/// inherited vfunc gives the incorrect folder. The user's incoming filters
/// then operate on the incorrect folder.
unsafe extern "C" fn get_inbox_folder_sync(
    store: *mut CamelStore,
    cancellable: *mut GCancellable,
    error: *mut *mut GError,
) -> *mut CamelFolder {
    // SAFETY: Camel's contract for the vfunc: a valid instance of ours, and an
    // out-parameter that is NULL or writable and currently NULL. This is also
    // the contract of [`open_by_role`].
    unsafe {
        guard_ptr("get_inbox_folder_sync", error, || {
            open_by_role(store, FolderRole::Inbox, cancellable, error)
        })
    }
}

/// Opens the folder for deleted mail: the vfunc of
/// `camel_store_get_trash_folder_sync`.
///
/// The inherited implementation gives a *virtual* folder: Camel's vTrash, a
/// search of the account for messages with the `CAMEL_MESSAGE_DELETED` flag.
/// For a JMAP account, this answer is incorrect for two reasons. First, JMAP
/// has no deleted keyword (see `FLAGS_FROM_JMAP` in [`crate::message_info`]),
/// so the flag is local to this client. A message deleted here goes into a
/// folder that no other client sees. A message that another client moved to
/// trash is not in it. Second, the account's *own* trash (the mailbox with the
/// `trash` role, where the server and all other clients put deleted mail)
/// would show next to it with its own name.
///
/// Thus the role gives the answer, and the virtual folders are disabled. The
/// `instance_init` of [`crate::store`] clears `CAMEL_STORE_VTRASH` and
/// `CAMEL_STORE_VJUNK`. If a store has those flags,
/// `camel_store_get_folder_info_sync` adds both virtual folders to each
/// listing.
///
/// This vfunc does *not* make the mailbox. If the server assigns no `trash`
/// role, the account has no trash folder. A `Mailbox/set` here would add a
/// folder to the user's account only to answer a question.
unsafe extern "C" fn get_trash_folder_sync(
    store: *mut CamelStore,
    cancellable: *mut GCancellable,
    error: *mut *mut GError,
) -> *mut CamelFolder {
    // SAFETY: as [`get_inbox_folder_sync`].
    unsafe {
        guard_ptr("get_trash_folder_sync", error, || {
            open_by_role(store, FolderRole::Trash, cancellable, error)
        })
    }
}

/// Opens the folder for spam: the vfunc of `camel_store_get_junk_folder_sync`.
///
/// This is the same as [`get_trash_folder_sync`], with one difference in the
/// reason. `$junk` *is* a JMAP keyword, so the inherited vJunk folder would not
/// be empty. It would be a second spam folder with different contents. The two
/// folders would disagree when a server files a message into the junk mailbox
/// without the keyword. The user sees the mailbox with the `junk` role in all
/// other clients.
unsafe extern "C" fn get_junk_folder_sync(
    store: *mut CamelStore,
    cancellable: *mut GCancellable,
    error: *mut *mut GError,
) -> *mut CamelFolder {
    // SAFETY: as [`get_inbox_folder_sync`].
    unsafe {
        guard_ptr("get_junk_folder_sync", error, || {
            open_by_role(store, FolderRole::Junk, cancellable, error)
        })
    }
}

/// Gives the account's Sent and Drafts mailboxes to the account wizard: the
/// vfunc of `camel_store_initial_setup_sync`.
///
/// This function opens no folder, unlike the three above. `out_save_setup` is
/// a `GHashTable` that the Camel wrapper created (`g_str_hash`/`g_str_equal`
/// keys, `g_free` as both destroy functions). This function fills it and
/// builds nothing that Camel keeps.
///
/// The function sets `error` if the store has no connection, if the listing
/// fails, or if [`guard_bool`] catches a panic. A role that no mailbox has is
/// not a failure, as it is for [`open_by_role`]. The module documentation
/// gives the contract for this.
unsafe extern "C" fn initial_setup_sync(
    store: *mut CamelStore,
    out_save_setup: *mut GHashTable,
    cancellable: *mut GCancellable,
    error: *mut *mut GError,
) -> gboolean {
    // SAFETY: Camel's contract for the vfunc: a valid instance of ours, and a
    // hash table that it owns and that is safe to insert into for the duration
    // of the call.
    unsafe {
        guard_bool("initial_setup_sync", error, || {
            // SAFETY: Camel keeps its cancellable alive for the duration of the
            // call.
            let _cancel = observe(cancellable);

            let Some(instance) = JmapStore::borrow(store) else {
                return fail_bool(error, &StoreError::Disconnected, StoreError::to_gerror);
            };

            let tree = match tree_holding(instance, |tree| {
                tree.role(FolderRole::Sent).is_some() && tree.role(FolderRole::Drafts).is_some()
            }) {
                Ok(tree) => tree,
                Err(failure) => return fail_bool(error, &failure, StoreError::to_gerror),
            };

            save_role(
                out_save_setup,
                &tree,
                FolderRole::Sent,
                CAMEL_STORE_SETUP_SENT_FOLDER,
            );
            save_role(
                out_save_setup,
                &tree,
                FolderRole::Drafts,
                CAMEL_STORE_SETUP_DRAFTS_FOLDER,
            );

            GTRUE
        })
    }
}

/// Inserts the Camel path of `role` into `out` with the key `key`. Inserts
/// nothing if no mailbox in `tree` has that role.
///
/// # Safety
///
/// `out` must be a live `GHashTable` whose key and value destroy functions are
/// both `g_free`. The table that the `camel_store_initial_setup_sync` wrapper
/// creates agrees with this, and it is the only caller.
unsafe fn save_role(out: *mut GHashTable, tree: &FolderTree, role: FolderRole, key: &CStr) {
    let Some(folder) = tree.role(role) else {
        return;
    };
    let path = c_string(&folder.path);
    // SAFETY: the contract above; both strings are new allocations that the
    // table owns.
    unsafe {
        g_hash_table_insert(
            out,
            g_strdup(key.as_ptr()).cast(),
            g_strdup(path.as_ptr()).cast(),
        );
    }
}

/// Opens the mailbox that has `role`, as the folder that Camel keeps for its
/// path.
///
/// The three vfuncs above use this function, because Camel asks the same
/// question three times: which mailbox of the account has this purpose?
/// [`FolderTree::role`] reads the role that this provider *assigned*. If two
/// mailboxes claim a role, only one gets it. Thus this function opens the
/// folder that has that role's `CAMEL_FOLDER_TYPE_*` in the listing, not a
/// second mailbox with the same claim.
///
/// This function does not build the folder. It asks
/// `camel_store_get_folder_sync` for the path, so that the answer goes through
/// the store's folder bag. Evolution gets these folders in two ways: by
/// purpose when it files or empties, and by path when the user clicks one.
/// Two `CamelFolder`s for one mailbox would have two summaries and two sets of
/// flags. If this function built a folder, it would cause that bug.
///
/// If no mailbox has the role, the function reports an error, not only NULL.
/// Camel documents NULL for "no such folder exists" and also for "it failed".
/// The store knows the cause: the account has mailboxes, but none has the
/// role. With this error, the user can see what to correct in the account.
/// Without it, deletes go nowhere and no cause shows. The error also makes the
/// three vfuncs give the same answers.
///
/// This function [`observe`]s `cancellable`, because the role search can list
/// the account again. It also gives `cancellable` to
/// `camel_store_get_folder_sync`, which can call [`get_folder_sync`] and
/// observe it again. The two observations nest: when the inner one ends, the
/// outer one applies again.
///
/// # Safety
///
/// `store` must be NULL or point to a live [`JmapStore`], and `error` must be
/// NULL or a writable location that holds NULL.
unsafe fn open_by_role(
    store: *mut CamelStore,
    role: FolderRole,
    cancellable: *mut GCancellable,
    error: *mut *mut GError,
) -> *mut CamelFolder {
    // SAFETY: the contract above is what `JmapStore::borrow` and `fail` ask
    // for, and `camel_store_get_folder_sync` gets the store borrowed here
    // with a NUL-terminated path that lives longer than the call. No flags on
    // it: `CREATE` would make a mailbox that the tree already has.
    unsafe {
        let Some(instance) = JmapStore::borrow(store) else {
            return fail(error, &StoreError::Disconnected, StoreError::to_gerror);
        };

        // Installed here and not in each of the three vfuncs, because the
        // request occurs here: the role search can list the tree again.
        // `camel_store_get_folder_sync` below installs the same cancellable
        // again, which nests and restores.
        //
        // SAFETY: Camel keeps its cancellable alive for the duration of the call.
        let _cancel = observe(cancellable);

        let tree = match tree_holding(instance, |tree| tree.role(role).is_some()) {
            Ok(tree) => tree,
            Err(failure) => return fail(error, &failure, StoreError::to_gerror),
        };
        let Some(folder) = tree.role(role) else {
            return fail(error, &StoreError::NoRole(role), StoreError::to_gerror);
        };
        let path = c_string(&folder.path);

        camel_store_get_folder_sync(store, path.as_ptr(), 0, cancellable, error)
    }
}

/// The store's folder tree. If the held tree does not have the folder that the
/// caller wants, the function gets the tree again.
///
/// The second fetch lets the user open a mailbox that was created after the
/// last listing. At start, Evolution opens the last selected folder from a URI
/// in its settings, before a refresh of the store. Also, another client can
/// create a folder while this client has the account open. If the store
/// reported an existing folder as missing because its tree is old, only a
/// restart would correct it.
///
/// The cost is one `Mailbox/changes` on a path that would otherwise fail. A
/// hit (each folder that the user clicks) uses the held tree and sends no
/// request.
///
/// `wanted` is a question about the full tree, not a path, because callers
/// ask different questions. Opening a folder wants the path from Camel.
/// Opening the inbox wants the mailbox with the role. [`crate::subscribe`]
/// wants the path of a subscription change.
pub(crate) fn tree_holding(
    store: &JmapStore,
    wanted: impl Fn(&FolderTree) -> bool,
) -> Result<Arc<FolderTree>, StoreError> {
    let held = store.folders(0)?;
    if wanted(&held) {
        return Ok(held);
    }
    store.folders(CAMEL_STORE_FOLDER_INFO_REFRESH)
}
