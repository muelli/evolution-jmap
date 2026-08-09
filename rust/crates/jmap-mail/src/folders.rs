// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! The `CamelStore` folder vfuncs: `get_folder_info_sync`, which describes the
//! account's folders, `get_folder_sync`, which opens one of them by path, and
//! the three — `get_inbox_folder_sync`, `get_trash_folder_sync`,
//! `get_junk_folder_sync` — that open one by purpose.
//!
//! They are one module because they are one question asked five ways: all five
//! read the folder listing [`JmapStore::folders`] keeps, and the last four exist
//! to turn something out of the first — a path, a role — back into the mailbox
//! it came from. What they do with the answer is where they part: the listing
//! marshals a whole subtree into C structs Camel frees, opening builds the
//! [`crate::folder`] object Camel keeps, and the three purposes delegate to the
//! opening so that every way of asking reaches the same object.
//!
//! ## The listing
//!
//! Everything it answers with exists already — [`JmapStore::folders`] keeps the
//! tree and decides whether to go and look, [`FolderInfoChain`] turns a tree
//! into the C forest Camel frees. What is left is the reading of the two
//! arguments those pieces do not take: `top`, the folder the answer is rooted
//! at, and `CAMEL_STORE_FOLDER_INFO_RECURSIVE`, the depth it is cut to.
//! [`Request`] is that reading, and it is a type of its own so that the decision
//! can be tested without a `CamelStore` to call the vfunc on.
//!
//! ### What Camel means by the arguments
//!
//! `camel_store_get_folder_info_sync`'s own documentation: "This fetches
//! information about the folder structure of @store, starting with @top […] If
//! @flags includes `CAMEL_STORE_FOLDER_INFO_RECURSIVE`, the returned tree will
//! include all levels of hierarchy below @top. If not, it will only include the
//! immediate subfolders of @top." A NULL or empty `top` is the account itself —
//! the wrapper makes the same test (`top == NULL || *top == '\0'`) for its own
//! purposes, so a store that read the two spellings differently would disagree
//! with the function calling it. The folder `top` names is part of the answer
//! rather than skipped: it is the head of the chain, which is what IMAPX
//! returns and what `camel_folder_info_build` produces from a set of paths
//! sharing that prefix.
//!
//! `RECURSIVE` is honoured here although IMAPX, the reference implementation,
//! has a `/* FIXME: obey other flags */` where it would be. Every real caller —
//! Evolution's folder cache and subscription editor, Camel's own
//! `camel_store_delete_folder_sync` — passes it, and the two calls that do not
//! are `camel_store_get_folder_info_sync`'s virtual-folder paths, which strip it
//! deliberately and want exactly the top level back. So obeying the documented
//! contract costs nothing a caller depends on, and saves a deep account from
//! marshalling its whole tree into C for a question about one level of it.
//!
//! `SUBSCRIBED` and `SUBSCRIPTION_LIST` are read too, and both are a filter on
//! the folders rather than a different request: `Mailbox/get` returns every
//! mailbox of the account with its `isSubscribed`, so this store has the
//! subscription list in hand and never needs the second, wider call an IMAP
//! store makes `LIST` beside `LSUB` for. See [`Request::new`] for what each
//! asks for and why an unsubscribed folder is sometimes still in the answer.
//!
//! Two flags are still not read. `FAST` is documented as deprecated and "most
//! backends will behave the same whether it is supplied or not", which is true
//! of this one because JMAP puts the counts in the mailbox anyway.
//! `NO_VIRTUAL` is not this vfunc's business at all: the wrapper adds and
//! removes vTrash and vJunk around the call.

use std::borrow::Cow;
use std::ptr;
use std::sync::Arc;

use eds_sys::{
    CAMEL_STORE_FOLDER_INFO_RECURSIVE, CAMEL_STORE_FOLDER_INFO_REFRESH,
    CAMEL_STORE_FOLDER_INFO_SUBSCRIBED, CAMEL_STORE_FOLDER_INFO_SUBSCRIPTION_LIST, CamelFolder,
    CamelFolderInfo, CamelStore, CamelStoreClass, CamelStoreGetFolderFlags,
    CamelStoreGetFolderInfoFlags, camel_store_get_folder_sync,
};
use gio_sys::GCancellable;
use glib_sys::{GError, gchar};
use jmap_backend_core::error::set_raw_gerror;
use jmap_backend_core::marshal::read_string;
use jmap_backend_core::trampoline::guard_ptr;
use jmap_mail_sync::{FolderInfo, FolderRole, FolderTree};

use crate::connect::StoreError;
use crate::folder::new_folder;
use crate::folder_info::{FolderInfoChain, c_string};
use crate::store::JmapStore;

/// The part of a store's folder tree one `get_folder_info_sync` call asks for.
pub struct Request<'a> {
    /// The sibling chain the answer is rooted at: the single folder `top`
    /// names, or every top-level folder when it names none.
    ///
    /// Empty for a `top` no folder answers to, which is a legitimate question
    /// with a legitimate empty answer — see [`Request::new`].
    ///
    /// Borrowed from the store's tree for a call that wants the folders as
    /// they are, owned for one that asked for a filtered view of them: a
    /// subtree with folders taken out of it is not a subtree of the tree the
    /// store holds, so it has to be built.
    pub roots: Cow<'a, [FolderInfo]>,
    /// How many levels of descendants below those roots belong in the answer;
    /// `None` for all of them.
    pub depth: Option<usize>,
}

impl<'a> Request<'a> {
    /// Reads the vfunc's `top` and `flags` against the tree the store holds.
    ///
    /// The depth differs by one between the two `top` cases, and for a reason
    /// that is easy to lose: "the immediate subfolders of `top`" is one level
    /// below a folder that is itself in the answer, but the account's top-level
    /// folders *are* the immediate subfolders of the root — the root is not a
    /// folder and is not returned — so there is no level left below them.
    ///
    /// A `top` that matches nothing yields no roots rather than an error. Camel
    /// documents the wrapper as able to "return NULL without setting a GError
    /// if no folders match the search criteria", and the case is ordinary: a
    /// folder another client deleted between one call and the next is asked
    /// for once more before Camel notices, and reporting that as a failure
    /// would turn someone else's tidying into a broken account.
    ///
    /// ## The two subscription flags
    ///
    /// `SUBSCRIBED` asks for the folders the user ticked, and is what
    /// Evolution's folder tree adds for a store that is `CamelSubscribable` —
    /// so it is what makes the tick in the subscription editor change what the
    /// user sees. It is applied to whatever `top` already chose, rather than
    /// instead of it: the two are separate halves of the same question, and a
    /// caller that asks about one subtree of a filtered account means the
    /// filtered part of that subtree.
    ///
    /// `SUBSCRIPTION_LIST` is the subscription editor's own question — which
    /// folders are there to tick — and is answered with all of them, which for
    /// this store is the listing it already has: `Mailbox/get` returns every
    /// mailbox of the account with its `isSubscribed`. It outranks `SUBSCRIBED`
    /// if a caller sets both, because an editor showing only what is already
    /// ticked is one nothing new can be ticked in.
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

    /// The forest this request is answered with, owned until it is handed over.
    pub fn answer(&self) -> FolderInfoChain {
        FolderInfoChain::from_forest(&self.roots, self.depth)
    }
}

/// The part of a sibling chain the user's ticks leave: every subscribed folder,
/// and every ancestor of one.
///
/// The ancestors are the whole of what makes this more than a filter.
/// `CamelFolderInfo` hangs a child off its parent, so there is no answer in
/// which `Work/Invoices` is present and `Work` is not — dropping an unticked
/// parent would drop the ticked folder underneath it, which is mail the user
/// asked to see and cannot reach. An IMAP server has the same problem and
/// answers it the same way, by returning the unsubscribed parents `LSUB`'s
/// children need.
///
/// Such a folder is *not* dressed up as anything else on the way out. It keeps
/// `subscribed: false`, so the listing does not put a tick in the subscription
/// editor the user never set, and it is deliberately not marked
/// `CAMEL_FOLDER_NOSELECT`, which Camel documents as "the folder cannot contain
/// messages" — a JMAP mailbox the user unticked holds mail like any other, and
/// claiming otherwise would be a lie Camel acts on. The visible consequence is
/// that unticking a folder with a ticked one below it leaves the folder in the
/// tree and openable; the alternative is worse.
///
/// What it does change is the children. A folder whose children were all
/// unticked has none *in this view*, and says so — unlike the depth cut in
/// [`FolderInfoChain::from_forest`], which leaves `CAMEL_FOLDER_CHILDREN` on a
/// folder whose children it left out, because those children exist and the
/// expander is how the caller asks for them.
///
/// Iteratively, for the reason `from_forest` gives: the depth of the tree comes
/// from a `parentId` chain a server chose.
fn ticked(siblings: &[FolderInfo]) -> Vec<FolderInfo> {
    // Pre-order first, each folder paired with its parent's place in that
    // order, because the answer has to be assembled the other way round: what
    // becomes of a folder depends on what became of everything below it.
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

    // And back again: reverse pre-order reaches every descendant before its
    // ancestor, so the children a folder keeps are settled by the time the
    // folder itself is decided.
    let mut kept: Vec<Vec<FolderInfo>> = vec![Vec::new(); order.len()];
    let mut roots: Vec<FolderInfo> = Vec::new();
    for (index, (folder, parent)) in order.into_iter().enumerate().rev() {
        let mut children = std::mem::take(&mut kept[index]);
        if !folder.subscribed && children.is_empty() {
            continue;
        }
        // They were collected by the same reversed walk, so they are in
        // reverse sibling order.
        children.reverse();
        // Field by field rather than `..folder.clone()`, which would clone the
        // subtree these children were just chosen from only to throw it away.
        let folder = FolderInfo {
            id: folder.id.clone(),
            path: folder.path.clone(),
            display_name: folder.display_name.clone(),
            role: folder.role,
            total: folder.total,
            unread: folder.unread,
            subscribed: folder.subscribed,
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
/// `class` must point at an initialised class struct that leads with a
/// `CamelStoreClass` — which is every descendant of `CamelStore`.
pub unsafe fn install_vfuncs(class: *mut CamelStoreClass) {
    // SAFETY: the contract above.
    let vfuncs = unsafe { &mut *class };
    vfuncs.get_folder_info_sync = Some(get_folder_info_sync);
    vfuncs.get_folder_sync = Some(get_folder_sync);
    vfuncs.get_inbox_folder_sync = Some(get_inbox_folder_sync);
    vfuncs.get_trash_folder_sync = Some(get_trash_folder_sync);
    vfuncs.get_junk_folder_sync = Some(get_junk_folder_sync);
}

/// Answers with the account's folders, or the part of them Camel asked for.
///
/// NULL is both the failure value and a legitimate answer — an account with no
/// folders, or a `top` that names none — which is why the error is what
/// separates them, and why nothing here returns NULL and sets one for a
/// question that simply had no folders in it.
///
/// `cancellable` is not observed, the same gap the address book backend
/// documents: [`Client`] takes its [`CancelFlag`] when it is built and offers
/// no way to re-point it, so only the connect is cancellable. The listing is
/// one or two round trips rather than a paged walk, which is why this is a gap
/// worth naming rather than one worth working around here; closing it is a
/// change to `jmap-client`.
///
/// [`Client`]: jmap_client::Client
/// [`CancelFlag`]: jmap_client::transport::CancelFlag
unsafe extern "C" fn get_folder_info_sync(
    store: *mut CamelStore,
    top: *const gchar,
    flags: CamelStoreGetFolderInfoFlags,
    _cancellable: *mut GCancellable,
    error: *mut *mut GError,
) -> *mut CamelFolderInfo {
    // SAFETY: Camel's contract for the vfunc: a valid instance of ours, a
    // NULL-or-valid string, and an out-parameter that is NULL or writable and
    // currently NULL.
    unsafe {
        guard_ptr("get_folder_info_sync", error, || {
            let Some(store) = JmapStore::borrow(store) else {
                return fail(error, &StoreError::Disconnected);
            };
            // Borrowed from Camel and NUL-terminated; `read_string` copies.
            let top = read_string(top);

            let tree = match store.folders(flags) {
                Ok(tree) => tree,
                Err(failure) => return fail(error, &failure),
            };

            // The tree is borrowed for exactly as long as the forest is being
            // built out of it, and the forest owns copies of everything it
            // took.
            Request::new(&tree, top.as_deref(), flags)
                .answer()
                .into_raw()
        })
    }
}

/// Opens one folder of the store: `camel_store_get_folder_sync`'s vfunc.
///
/// What it must *not* do is keep the folder. `CamelStore` owns a
/// `CamelObjectBag` of the folders it has open — public as
/// `camel_store_get_folders_bag`, keyed with the class's own
/// `hash_folder_name`/`equal_folder_name` — and the wrapper reserves this
/// folder's name in it before it reaches this function at all, so a second call
/// for the same path never gets here. A cache of our own would be a second
/// answer to a question Camel has already answered, and the way two
/// `CamelFolder`s over one mailbox — two summaries, two sets of flags — get
/// handed out.
///
/// The flags are not read. `CREATE` asks for a folder that does not exist to be
/// made, which for JMAP is a `Mailbox/set` and belongs to `create_folder_sync`;
/// `BODY_INDEX` asks for a body index this provider does not build; `PRIVATE`
/// is about vFolder membership, which is the wrapper's business; and `EXCL` is
/// documented as not honoured.
///
/// `cancellable` is not observed, the same gap [`get_folder_info_sync`]
/// documents and for the same reason.
unsafe extern "C" fn get_folder_sync(
    store: *mut CamelStore,
    folder_name: *const gchar,
    _flags: CamelStoreGetFolderFlags,
    _cancellable: *mut GCancellable,
    error: *mut *mut GError,
) -> *mut CamelFolder {
    // SAFETY: Camel's contract for the vfunc: a valid instance of ours, a
    // NUL-terminated string, and an out-parameter that is NULL or writable and
    // currently NULL.
    unsafe {
        guard_ptr("get_folder_sync", error, || {
            let Some(instance) = JmapStore::borrow(store) else {
                return fail(error, &StoreError::Disconnected);
            };
            // Borrowed from Camel and NUL-terminated; `read_string` copies, and
            // reads a NULL or empty name as no name — which no mailbox answers
            // to, because a path always has a component in it.
            let path = read_string(folder_name).unwrap_or_default();

            let tree = match tree_holding(instance, |tree| tree.find(&path).is_some()) {
                Ok(tree) => tree,
                Err(failure) => return fail(error, &failure),
            };
            let Some(mailbox) = tree.find(&path) else {
                return fail(error, &StoreError::NoFolder(path));
            };

            // SAFETY: `store` is the live `CamelStore` borrowed above, which is
            // what `new_folder` asks for.
            new_folder(store, mailbox)
        })
    }
}

/// Opens the account's inbox: `camel_store_get_inbox_folder_sync`'s vfunc.
///
/// Camel asks a store for this folder by *purpose*, and the purpose is the only
/// thing it can be answered from — which is why the vfunc is overridden rather
/// than inherited. `CamelStoreClass` does supply an implementation: it asks the
/// store's own `get_folder_sync` for a folder named `inbox`, in that case, and
/// IMAPX does the same thing one spelling up against `"INBOX"`. Both are IMAP
/// conventions rather than facts about mail stores. RFC 8621 §2 gives a mailbox
/// a `role` instead, and says nothing about that mailbox's name or where in the
/// hierarchy it sits: a JMAP account may perfectly well keep its inbox under a
/// per-address parent and call it something in the user's own language. An
/// account that *also* has an ordinary mailbox named "inbox" is the one the
/// inherited version gets quietly wrong, by running the user's incoming filters
/// over the wrong folder.
unsafe extern "C" fn get_inbox_folder_sync(
    store: *mut CamelStore,
    cancellable: *mut GCancellable,
    error: *mut *mut GError,
) -> *mut CamelFolder {
    // SAFETY: Camel's contract for the vfunc: a valid instance of ours, and an
    // out-parameter that is NULL or writable and currently NULL — which is
    // [`open_by_role`]'s contract too.
    unsafe {
        guard_ptr("get_inbox_folder_sync", error, || {
            open_by_role(store, FolderRole::Inbox, cancellable, error)
        })
    }
}

/// Opens the folder deleted mail is delivered into:
/// `camel_store_get_trash_folder_sync`'s vfunc.
///
/// The inherited implementation answers with a *virtual* folder — Camel's
/// vTrash, a search across the account for messages carrying the
/// `CAMEL_MESSAGE_DELETED` flag — and for a JMAP account that is the wrong
/// answer twice over. JMAP has no deleted keyword ([`crate::message_info`]'s
/// `FLAGS_FROM_JMAP` is where that is written down), so the flag is local to
/// this client: a message deleted here is in a folder no other client can see,
/// and a message another client moved to trash is not in it. Meanwhile the
/// account's *own* trash — the mailbox holding the `trash` role, which is where
/// the server and every other client put deleted mail — would sit next to it
/// under its own name.
///
/// So the role is the answer, and the virtual folders are turned off with it:
/// [`crate::store`]'s `instance_init` clears `CAMEL_STORE_VTRASH` and
/// `CAMEL_STORE_VJUNK`, because `camel_store_get_folder_info_sync` appends both
/// to every listing a store with those flags answers with.
///
/// What this vfunc does *not* do is make the mailbox. An account whose server
/// assigns no `trash` role has no trash folder, and `Mailbox/set` from in here
/// would be the provider inventing a folder in the user's account on the way to
/// answering a question about one.
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

/// And the folder spam is delivered into:
/// `camel_store_get_junk_folder_sync`'s vfunc.
///
/// The mirror of [`get_trash_folder_sync`], with one difference in the
/// reasoning: `$junk` *is* a JMAP keyword, so the vJunk folder the inherited
/// implementation answers with would not be empty — it would be a second spam
/// folder, populated differently from the account's own, and the two would
/// disagree the moment a server filed a message into the junk mailbox without
/// keywording it. The mailbox holding the `junk` role is the one the user sees
/// everywhere else.
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

/// The mailbox claiming `role`, as the folder Camel keeps for its path.
///
/// The three vfuncs above are this function under three names, because Camel
/// asks the same question three times: which of the account's mailboxes serves
/// this purpose. The role lookup is [`FolderTree::role`], which reads the role
/// this provider *assigned* — a contested role goes to one mailbox and only
/// one, so the folder opened here is the folder the listing gave that role's
/// `CAMEL_FOLDER_TYPE_*` to, and not a second mailbox claiming the same thing.
///
/// The folder itself is not built here. It is asked for by path through
/// `camel_store_get_folder_sync`, so that the answer goes through the store's
/// folder bag: Evolution reaches these folders both ways — by purpose when it
/// files or empties, by path when the user clicks one — and two `CamelFolder`s
/// over one mailbox would be two summaries and two sets of flags. Building one
/// here would be that bug.
///
/// A role no mailbox claims is reported rather than answered with a silent NULL,
/// although Camel documents NULL as meaning "no such folder exists" as well as
/// "it failed". The store knows *why* — the account has mailboxes, none of them
/// carries the role — and that is the difference between a user who can see
/// what to fix in their account and one whose deletes go nowhere for no stated
/// reason. It also keeps all three vfuncs answering alike.
///
/// `cancellable` *is* passed on, unlike in the two vfuncs above this section: it
/// is not observed by the listing this function may do itself, for the reason
/// [`get_folder_info_sync`] documents, but the call it delegates to is Camel's
/// own and has no such gap.
///
/// # Safety
///
/// `store` must be NULL or point at a live [`JmapStore`], and `error` must be
/// NULL or a writable location currently holding NULL.
unsafe fn open_by_role(
    store: *mut CamelStore,
    role: FolderRole,
    cancellable: *mut GCancellable,
    error: *mut *mut GError,
) -> *mut CamelFolder {
    // SAFETY: the contract above is what `JmapStore::borrow` and `fail` ask
    // for, and `camel_store_get_folder_sync` is handed the store borrowed here
    // with a NUL-terminated path that outlives the call. No flags on it:
    // `CREATE` would make a mailbox the tree says already exists.
    unsafe {
        let Some(instance) = JmapStore::borrow(store) else {
            return fail(error, &StoreError::Disconnected);
        };

        let tree = match tree_holding(instance, |tree| tree.role(role).is_some()) {
            Ok(tree) => tree,
            Err(failure) => return fail(error, &failure),
        };
        let Some(folder) = tree.role(role) else {
            return fail(error, &StoreError::NoRole(role));
        };
        let path = c_string(&folder.path);

        camel_store_get_folder_sync(store, path.as_ptr(), 0, cancellable, error)
    }
}

/// The store's folder tree, looked at again if it does not hold the folder the
/// caller is after.
///
/// The second look is what makes a mailbox created since the last listing
/// openable. Evolution reopens the folder the user last had selected when it
/// starts, from a URI in its own settings, before anything has asked the store
/// to refresh — and another client creating a folder while this one has an
/// account open is ordinary. Reporting a folder that plainly exists as missing
/// because our tree predates it would be a bug the user can only clear by
/// restarting.
///
/// The cost is one `Mailbox/changes` on the path that is about to fail anyway;
/// a hit — every folder the user clicks — is answered out of the held tree with
/// no request at all.
///
/// `wanted` is a question about the whole tree rather than a path, because the
/// callers ask different ones: opening a folder wants the path Camel named,
/// opening the inbox wants whichever mailbox claims the role, and
/// [`crate::subscribe`] wants the path a subscription change was aimed at.
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
