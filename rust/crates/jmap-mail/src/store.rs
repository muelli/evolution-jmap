// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! `CamelJmapStore`: the type that Camel instantiates for a JMAP mail account.
//!
//! This type holds the decisions that are expensive to change later:
//! - The parent type. Each folder vfunc is declared on one of the two
//!   candidates.
//! - The settings class, [`crate::settings`]. Without it, a JMAP account has
//!   no place to keep a server.
//! - The slots that hold the state. They are fields of the instance struct,
//!   thus part of the layout that the vfuncs read. They hold the connection,
//!   the folder listing read over it, and the two slots for JMAP Push.
//!
//! [`crate::service`] has the `CamelService` vfuncs that fill and empty the
//! connection slot. [`crate::folders`] has `get_folder_info_sync` of
//! `CamelStoreClass`, which reads the folder listing slot. `class_init` below
//! installs both.
//!
//! [`crate::subscribe`] has the three `CamelSubscribable` vfuncs, which also
//! read and write the folder listing slot. `interfaces` declares them, because
//! GObject fills an interface vtable, not the class of this crate.
//!
//! The two push slots belong to [`crate::push`]. They are filled and emptied
//! together with the connection. The docs of that module tell why there are
//! two.

use std::ffi::CStr;
use std::ptr;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, RwLock, RwLockReadGuard, RwLockWriteGuard};

use eds_sys::{
    CAMEL_STORE_FOLDER_INFO_REFRESH, CAMEL_STORE_SUPPORTS_INITIAL_SETUP, CAMEL_STORE_VJUNK,
    CAMEL_STORE_VTRASH, CamelOfflineStore, CamelOfflineStoreClass, CamelService, CamelServiceClass,
    CamelStore, CamelStoreClass, CamelStoreGetFolderInfoFlags, camel_offline_store_get_type,
    camel_service_get_type, camel_service_ref_session, camel_service_ref_settings,
    camel_store_get_flags, camel_store_set_flags,
};
use glib_sys::GType;
use gobject_sys::GObject;
use jmap_backend_core::instance::Slot;
#[cfg(feature = "testing")]
use jmap_backend_core::instance::zeroed_box;
use jmap_backend_core::marshal::{checked_borrow_ptr, dispatched_borrow};
use jmap_backend_core::owned::Owned;
use jmap_backend_core::push::{self, PushRefresh};
use jmap_backend_core::retry::retry_once_after;
use jmap_backend_core::subclass::{InterfaceDecl, ObjectSubclass, register_static};
use jmap_client::Credentials;
use jmap_mail_sync::{
    Filing, FolderInfo, FolderTree, FolderUpdate, KeywordChange, Keywords, MailSync,
    MessageSummary, MessageUpdate, SyncError,
};
use jmap_proto::mail::EmailQueryFilter;
use jmap_proto::methods::Filter;
use jmap_proto::{Id, State};

use crate::connect::StoreError;
use crate::oauth2;
use crate::push::FolderRefresh;
use crate::service::Connected;
use crate::settings::settings_type;
use crate::subscribe::Subscribable;

/// A folder listing and the state that it is current for.
///
/// The tree is in an [`Arc`] because it must live longer than the lock. A
/// caller that converts it into a `CamelFolderInfo` forest must not keep the
/// listing locked during the conversion. A copy per call walks each mailbox
/// for an answer that did not change.
struct Listing {
    state: State,
    tree: Arc<FolderTree>,
    /// Each in-place edit increments this value ([`JmapStore::set_subscribed`],
    /// [`JmapStore::create_folder`], [`JmapStore::delete_folder`],
    /// [`JmapStore::rename_folder`]). [`JmapStore::folders`] never does.
    ///
    /// `folders(REFRESH)` reads a `(state, tree)` snapshot. Then it sends
    /// `Mailbox/changes` with the `folders` slot unlocked, so one of the four
    /// edits can occur during that request. The answer is correct for the
    /// state that it asked from, but it can be stale when it arrives. It must
    /// not overwrite an edit made after the snapshot.
    ///
    /// Thus `folders` writes back only if the generation did not change. A
    /// different generation means that an edit changed the listing, and the
    /// edit stays (`jmap-mail/tests/folder_listing_race.rs` tests this).
    generation: u64,
}

/// The instance struct. It is `#[repr(C)]` and starts with the instance
/// struct of the parent. Thus a `*mut JmapStore` is a valid `CamelStore *`
/// for each Camel function.
#[repr(C)]
pub struct JmapStore {
    parent: CamelOfflineStore,
    /// The connection, from `connect_sync` to `disconnect_sync`.
    ///
    /// This is an `RwLock`, not a `Mutex`, for the same reason as in the
    /// address book backend. Camel uses a store from many threads at the same
    /// time: a folder refresh, a message fetch and a folder-list update can
    /// all run together. One exclusive lock makes each operation wait for the
    /// slowest. Only connect and disconnect replace the value, so only they
    /// need exclusive access.
    ///
    /// Because of the `Arc`, most callers below clone the connection and
    /// release the guard before the network request. Thus the write lock of
    /// `drop_connection` does not wait for a request in progress. The comment
    /// of each method tells if it does this. `MailSync` itself does not need
    /// `Clone`: the purpose is to share one instance.
    connection: Slot<RwLock<Option<Arc<MailSync>>>>,
    /// The last folder tree that the connection sent, and its state.
    ///
    /// This is a separate slot, not a field of the connection, so that a
    /// folder refresh and a reconnect do not wait for each other. An ordering
    /// rule connects the two: the store writes a listing while it still holds
    /// the read lock on the connection that read it. A
    /// [`store_connection`](JmapStore::store_connection) needs that lock
    /// exclusively. Thus it cannot occur between the request and the write, so
    /// a tree from the old connection cannot undo its clear.
    folders: Slot<RwLock<Option<Listing>>>,
    /// The JMAP Push subscription. It lives for the same time as the
    /// connection. A server that advertises an `eventSourceUrl` can tell the
    /// store that mail arrived, so the store does not poll on the refresh
    /// interval of the account.
    ///
    /// This is a plain `Mutex`, not an `RwLock` as above: no hot path reads
    /// it. The store only installs, takes and drops it, and these are all
    /// exclusive.
    push: Slot<Mutex<Option<PushRefresh>>>,
    /// The coalescing worker that does the refresh for a push. It has its own
    /// slot, separate from the subscription.
    ///
    /// This separation prevents a deadlock in [`JmapStore::stop_push`]. To
    /// stop the subscription, it joins the pump thread. At that time, the pump
    /// can be in [`crate::push::dispatch`], which locks this slot. Two locks
    /// that are taken in sequence and never nested cannot make a cycle. One
    /// lock can.
    refresher: Slot<Mutex<Option<FolderRefresh>>>,
}

impl JmapStore {
    /// Installs `sync` as the live connection and replaces the previous one.
    ///
    /// It replaces and does not refuse, because Camel reconnects a store when
    /// it thinks that the connection is lost. The old connection (and its
    /// socket) closes when this function returns.
    ///
    /// The folder listing read over the old connection is also removed. A
    /// reconnect occurs because the account changed, and the new server can
    /// be different. The old tree has paths, message counts and JMAP ids that
    /// each later request uses.
    pub fn store_connection(&self, sync: MailSync) {
        if let Some(connection) = self.connection() {
            let mut guard = write(connection);
            self.forget_folders();
            tracing::debug!("storing mail connection in store");
            *guard = Some(Arc::new(sync));
        }
        // Start the push subscription after the connection is installed: it
        // authenticates with this connection and refreshes over it. In the
        // other order, a pushed refresh can reach `refresh_info_sync` before
        // `store_connection` ran. Then it reports the account offline for a
        // change that did arrive.
        self.start_push();
    }

    /// Subscribes to the JMAP Push stream of the account, if the server has
    /// one. [`crate::push`] tells what a push does, and why its refresh does
    /// not run on the thread where the push arrives.
    ///
    /// This does nothing if there is no connection, if the server advertises
    /// no `eventSourceUrl`, or on a [`JmapStore::detached`] test instance. A
    /// test instance is not a GObject, so it cannot have a weak reference. In
    /// all three cases, the account refreshes only when Camel asks.
    fn start_push(&self) {
        let Some(service) = self.service() else {
            return;
        };
        let store = service.cast::<CamelStore>();

        // Install the coalescing worker first. `start_for_with` starts its
        // pump before it returns, so the first push can arrive while this
        // function runs. A push that finds no worker is lost.
        //
        let pass = |object: *mut GObject, work: crate::push::Work| {
            // SAFETY: the object is the store for which `FolderRefresh` was
            // made. `self.service()` above checked that it is a `CamelService`;
            // it is this instance, so it is also a `CamelStore`. The worker
            // holds a strong reference for the full call.
            if work.refresh_messages {
                unsafe { crate::push::refresh_open_folders(object.cast()) };
            }
            // SAFETY: the same contract as above.
            if work.check_folder_list {
                unsafe { crate::push::refresh_folder_list_if_structural(object.cast()) };
            }
        };
        // SAFETY: `service` is this live instance as its C ancestor type. It
        // stays referenced for this call because `self` is borrowed from it.
        // The worker calls `pass` only with that same pointer.
        let refresher = unsafe { FolderRefresh::new(store.cast(), pass) };
        self.install_refresher(Some(refresher));

        // One subscription carries both halves of the mail push. A `Mailbox`
        // change and an `Email`/`EmailDelivery` change request different work
        // from Camel. Thus `dispatch` must know which types a `StateChange`
        // named, and `start_for_with` (not `start_for`) supplies them.
        let watched: Vec<&str> = crate::push::PUSHED_TYPES
            .iter()
            .chain(crate::push::FOLDER_LIST_TYPES)
            .copied()
            .collect();
        let action = |object, types: &[String]| {
            // SAFETY: `object` is a valid GObject, referenced as the call
            // below promises, and `dispatch` accepts exactly this type.
            unsafe { crate::push::dispatch(object, types) }
        };
        let started = self.connection().and_then(|connection| {
            let guard = read(connection);
            guard.as_ref().and_then(|sync| {
                // SAFETY: `store` is a valid GObject, referenced as above, and
                // `action` accepts exactly this type.
                unsafe {
                    push::start_for_with(
                        store.cast(),
                        sync.client(),
                        sync.account_id(),
                        &watched,
                        action,
                    )
                }
            })
        });

        if started.is_none() {
            // Without a stream, nothing asks the worker for a pass.
            self.install_refresher(None);
        }
        self.replace_push(started);
    }

    /// Installs `push` as the live subscription, and stops and drops the
    /// previous one. A reconnect needs this, because the old subscription
    /// authenticates with the replaced connection.
    pub fn store_push(&self, push: PushRefresh) {
        self.replace_push(Some(push));
    }

    /// Stops the push subscription, waits for its pump, and tells if there
    /// was a subscription.
    ///
    /// After this returns, nothing can *request* a refresh. A pass in progress
    /// continues to the end, on the store to which it holds a strong
    /// reference. Nothing here can wait for a network request (see
    /// [`crate::push`]).
    pub fn stop_push(&self) -> bool {
        // Stop the subscription first, without the refresher lock: the pump
        // thread that this joins can be in `crate::push::dispatch`, which
        // holds that lock.
        let stopped = self.replace_push(None);
        self.install_refresher(None);
        stopped
    }

    /// Tells if a push subscription is live.
    #[cfg(feature = "testing")]
    pub fn is_pushing(&self) -> bool {
        self.push
            .get()
            .is_some_and(|slot| push_lock(slot).is_some())
    }

    /// Replaces the `Authorization` header that the live push subscription
    /// sends on its next reconnect attempts. Without a subscription, it does
    /// nothing.
    ///
    /// `JmapStore::refresh_credentials` calls this after it installs a new
    /// OAuth 2.0 token on the connection. Thus a subscription that the server
    /// refused with the stale token uses the new one. It does not repeat the
    /// same failure until Camel reconnects.
    pub fn refresh_push_headers(&self, headers: Vec<(String, String)>) {
        if let Some(slot) = self.push.get()
            && let Some(push) = push_lock(slot).as_ref()
        {
            push.set_headers(headers);
        }
    }

    /// Requests a pass that does `work` from the coalescing worker.
    /// [`crate::push::dispatch`] calls this for each push that names a
    /// watched type. The comment of the `refresher` field tells why the
    /// worker has its own slot.
    pub fn request_folder_refresh(&self, work: crate::push::Work) {
        if let Some(slot) = self.refresher.get()
            && let Some(refresher) = refresher_lock(slot).as_ref()
        {
            refresher.request(work);
        }
    }

    /// Puts `push` in the subscription slot and tells if it replaced a
    /// subscription. Dropping the replaced subscription stops and joins its
    /// pump.
    fn replace_push(&self, push: Option<PushRefresh>) -> bool {
        match self.push.get() {
            Some(slot) => {
                let previous = std::mem::replace(&mut *push_lock(slot), push);
                previous.is_some()
            }
            None => false,
        }
    }

    fn install_refresher(&self, refresher: Option<FolderRefresh>) {
        if let Some(slot) = self.refresher.get() {
            *refresher_lock(slot) = refresher;
        }
    }

    /// Drops the connection and tells if there was one.
    ///
    /// On shutdown, Camel tells each store to disconnect, also a store that
    /// never connected. Thus "no connection" is a normal result, not a
    /// failure. `disconnect_sync` uses the result to know if it did work.
    ///
    /// The folder listing is also dropped. This changes no answer: without a
    /// connection, nothing can supply a tree, and a reconnect clears it
    /// again. But a disconnected account must not keep its full mailbox tree
    /// in memory until Evolution stops.
    pub fn drop_connection(&self) -> bool {
        // Stop push first, before its connection goes. Otherwise a push can
        // request a refresh from a store that has no connection to refresh
        // with.
        let unsubscribed = self.stop_push();
        tracing::debug!(unsubscribed, "stopping mail push before disconnecting");
        match self.connection() {
            Some(connection) => {
                let mut connection = write(connection);
                self.forget_folders();
                let dropped = connection.take().is_some();
                tracing::debug!(dropped, "dropping mail connection from store");
                dropped
            }
            None => false,
        }
    }

    /// Tells if an operation can find a connection.
    pub fn is_connected(&self) -> bool {
        self.connection()
            .is_some_and(|connection| read(connection).is_some())
    }

    /// This store as the `CamelService` that Camel gave to Evolution. It is
    /// the same instance that [`JmapStore::borrow`] returns. It is `None` on
    /// a [`JmapStore::detached`] test instance, which is not a `CamelService`.
    ///
    /// `Self` starts with `CamelOfflineStore`, which starts with `CamelStore`,
    /// which starts with `CamelService`. Thus `self` and this pointer refer to
    /// the same object, typed as its C ancestor. [`JmapStore::borrow`] makes
    /// the same cast in the opposite direction.
    ///
    /// The type check makes this safe on a detached instance too. Its first
    /// field, `GTypeInstance`, is `NULL` (all zero).
    /// `g_type_check_instance_is_a` reads that as "not this type" and does not
    /// dereference further. `jmap_backend_core::source::backend_source` uses
    /// the same guarantee for `ESource`.
    fn service(&self) -> Option<*mut CamelService> {
        let ptr = ptr::from_ref(self).cast::<CamelService>().cast_mut();
        // SAFETY: `ptr` is `self` read as its first field, `GTypeInstance`.
        // `checked_borrow_ptr` checks it against the target type before it
        // reads further, as the doc comment above tells.
        unsafe { checked_borrow_ptr(ptr, camel_service_get_type()) }
    }

    /// Gets a new OAuth 2.0 access token for the account, installs it on the
    /// live connection, and tells if a retry of the operation is useful.
    ///
    /// This is the mail equivalent of `refresh_credentials` in the calendar
    /// and address book backends. For a 401 on a pooled connection, it
    /// refreshes the token and does not open a consent window immediately.
    /// Usually, the stored refresh token is still valid.
    ///
    /// It returns `false` in these cases:
    /// - The account does not use OAuth 2.0. A 401 for a Basic password or
    ///   an API token means that the stored secret is wrong, and a new fetch
    ///   gives the same result.
    /// - There is no `CamelService`: a [`JmapStore::detached`] test instance,
    ///   or a vfunc that runs before Camel completed the construction.
    /// - There is no session, or the token fetch fails.
    fn refresh_credentials(&self, sync: &MailSync) -> bool {
        let Some(service) = self.service() else {
            return false;
        };
        // SAFETY: `service` is a live `CamelService`, by the check in
        // `Self::service`. The settings reference is released when it drops.
        let settings = unsafe { Owned::from_raw(camel_service_ref_settings(service)) };
        // SAFETY: `settings` is NULL or the `CamelSettings` referenced above.
        let uses_oauth2 = settings
            .as_ref()
            .is_some_and(|settings| unsafe { oauth2::uses_oauth2(settings.as_ptr()) });
        if !uses_oauth2 {
            return false;
        }

        // SAFETY: `service` is a valid, registered `CamelService`, by the
        // check above. The session reference is released when it drops.
        let Some(session) = (unsafe { Owned::from_raw(camel_service_ref_session(service)) }) else {
            return false;
        };
        // SAFETY: a valid session and a valid, registered service. There is
        // no cancellable: the user did not ask to stop this refresh, and
        // `access_token` accepts NULL for "not cancellable".
        match unsafe { oauth2::access_token(session.as_ptr(), service, ptr::null_mut()) } {
            Ok(token) => {
                tracing::debug!("refreshed the mail connection's OAuth 2.0 access token");
                sync.client().set_credentials(Credentials::bearer(token));
                // Read the header back from the client and do not format it
                // again here. Thus the push stream and each method call send
                // the same header.
                if let Some(header) = sync.client().authorization_header() {
                    self.refresh_push_headers(vec![("Authorization".to_owned(), header)]);
                }
                true
            }
            Err(failure) => {
                tracing::debug!(
                    ?failure,
                    "refreshing the mail connection's access token failed"
                );
                false
            }
        }
    }

    /// The origin of the connected account: the API URL and the account id,
    /// or `None` when the store has no connection. See
    /// [`crate::summary::adopt_origin`] for its use.
    pub fn origin(&self) -> Option<String> {
        let connection = self.connection()?;
        let guard = read(connection);
        guard.as_ref().map(|sync| {
            format!(
                "{} {}",
                sync.client().session().api_url,
                sync.account_id().as_str()
            )
        })
    }

    /// For tests only. It gives access to the one part of
    /// [`JmapStore::refresh_credentials`] that a test can reach without a
    /// `CamelService`: new credentials on the pooled connection.
    #[cfg(feature = "testing")]
    pub fn inspect_connection<R>(&self, f: impl FnOnce(&MailSync) -> R) -> Option<R> {
        let connection = self.connection()?;
        let guard = read(connection);
        guard.as_ref().map(|sync| f(sync))
    }

    /// The folder tree of the account: the answer of `get_folder_info_sync`.
    ///
    /// `flags` comes from Camel without change. The important bit is
    /// `CAMEL_STORE_FOLDER_INFO_REFRESH`. Camel asks for the folder tree very
    /// frequently, and sets that bit only when it wants a new check.
    ///
    /// Without the bit, the held listing is the answer and no request occurs.
    /// With it, one `Mailbox/changes` decides if the tree must be read again.
    /// [`MailSync::folder_tree_since`] tells why a mailbox delta cannot be
    /// applied one folder at a time.
    ///
    /// The first call has no listing, so it lists the account for all flags.
    /// Otherwise, an account opens empty and shows no mail until a refresh.
    ///
    /// This function does not read the other flags. `SUBSCRIBED` and
    /// `SUBSCRIPTION_LIST` ask for the tree filtered to the subscribed
    /// folders: a filter on the tree, not a different request. `FAST` asks for
    /// no message counts, but JMAP includes them in the mailbox.
    pub fn folders(
        &self,
        flags: CamelStoreGetFolderInfoFlags,
    ) -> Result<Arc<FolderTree>, StoreError> {
        let (connection, folders) = self
            .connection()
            .zip(self.folder_listing())
            .ok_or(StoreError::Disconnected)?;

        // Hold this lock across the request, as the ordering rule of the
        // `folders` field requires: the connection that read a listing is
        // still current when the listing is written.
        let connection = read(connection);
        let sync = connection.as_ref().ok_or(StoreError::Disconnected)?;

        tracing::debug!(flags, "fetching folder tree");

        let held = read(folders).as_ref().map(|listing| {
            (
                listing.state.clone(),
                Arc::clone(&listing.tree),
                listing.generation,
            )
        });

        // `Some(generation)` if a listing exists: the write below replaces it
        // only if its generation did not change after this request started.
        // `None` for a first listing. No edit can race it, because the four
        // edit methods do nothing on an empty slot.
        let baseline_generation;
        let listing = match held {
            Some((_, tree, _)) if flags & CAMEL_STORE_FOLDER_INFO_REFRESH == 0 => return Ok(tree),
            Some((state, tree, generation)) => {
                baseline_generation = Some(generation);
                match retry_once_after(
                    || sync.folder_tree_since(&state),
                    SyncError::is_unauthorized,
                    || self.refresh_credentials(sync),
                ) {
                    // Keep the tree, do not make a new equal one. Camel
                    // compares the forests it gets to find created and deleted
                    // folders, and each caller above keeps the same `Arc`.
                    Ok(FolderUpdate::Unchanged(state)) => Listing {
                        state,
                        tree,
                        generation,
                    },
                    Ok(FolderUpdate::Rebuilt { state, tree }) => Listing {
                        state,
                        tree: Arc::new(tree),
                        generation,
                    },
                    Err(failure) => {
                        tracing::debug!(flags, ?failure, "fetching folder tree failed");
                        return Err(failure.into());
                    }
                }
            }
            None => {
                baseline_generation = None;
                match retry_once_after(
                    || sync.folder_tree(),
                    SyncError::is_unauthorized,
                    || self.refresh_credentials(sync),
                ) {
                    Ok((state, tree)) => Listing {
                        state,
                        tree: Arc::new(tree),
                        generation: 0,
                    },
                    Err(failure) => {
                        tracing::debug!(flags, ?failure, "fetching folder tree failed");
                        return Err(failure.into());
                    }
                }
            }
        };

        tracing::debug!(
            flags,
            state = listing.state.as_str(),
            count = listing.tree.len(),
            "fetched folder tree"
        );

        let mut guard = write(folders);
        // A concurrent edit (create, delete, rename or subscribe) changed the
        // generation during this request. Its write is more current than this
        // answer, which was correct when asked but is now stale. Keep the
        // edit, and give the caller the tree that the edit wrote.
        let stale = matches!(
            (&*guard, baseline_generation),
            (Some(current), Some(baseline)) if current.generation != baseline
        );
        let tree = if stale {
            guard
                .as_ref()
                .map(|current| Arc::clone(&current.tree))
                .unwrap_or_else(|| Arc::clone(&listing.tree))
        } else {
            let tree = Arc::clone(&listing.tree);
            *guard = Some(listing);
            tree
        };
        drop(guard);
        drop(connection);
        Ok(tree)
    }

    /// Tells if the folder listing lets this mailbox accept new messages
    /// ([`FolderInfo::may_add_items`]).
    ///
    /// It calls [`Self::folders`] with `flags` of 0. If a listing exists, that
    /// reads the held tree and makes no request. An append has an open folder,
    /// thus a listing usually exists. A permission check with a request adds
    /// latency to each append.
    ///
    /// If no listing exists yet, `folders` lists the account from the server.
    /// The result is `None` if that fails, if the listing does not have the
    /// mailbox, or if the mailbox has no value. Each other reader of
    /// `myRights` here also treats this as "no reason to refuse".
    pub fn may_add_items(&self, mailbox: &Id) -> Option<bool> {
        let tree = self.folders(0).ok()?;
        tree.by_id(mailbox)?.may_add_items
    }

    /// The folder-list half of a pushed `Mailbox` change. It refreshes the
    /// held tree and tells if the change was structural. A structural change
    /// creates, destroys, renames, moves, subscribes or unsubscribes a
    /// mailbox; a count change is not structural. The module docs of
    /// [`crate::push`] tell that only a structural change must call
    /// `camel_store_folder_info_stale`.
    ///
    /// It returns `false` if no tree is held: Camel did not ask for a folder
    /// list yet, so no list can be stale.
    pub fn folder_list_changed_structurally(&self) -> bool {
        let Some(before) = self.cached_tree() else {
            return false;
        };
        match self.folders(CAMEL_STORE_FOLDER_INFO_REFRESH) {
            Ok(after) => !before.same_shape(&after),
            Err(failure) => {
                tracing::debug!(
                    ?failure,
                    "checking whether a pushed folder-list change was structural failed"
                );
                false
            }
        }
    }

    /// The held folder tree, if the account has a listing.
    fn cached_tree(&self) -> Option<Arc<FolderTree>> {
        let folders = self.folder_listing()?;
        read(folders)
            .as_ref()
            .map(|listing| Arc::clone(&listing.tree))
    }

    /// All messages in one mailbox of the account. The `refresh_info_sync` of
    /// a folder fills its summary from this.
    ///
    /// This is a method of the store, not of the folder, because the store has
    /// the connection. A `CamelFolder` holds a mailbox id and a pointer to its
    /// store, and each of its requests uses the client of the store. This
    /// function keeps no cache: the listing *is* the folder state, and the
    /// summary keeps it.
    ///
    /// Unlike [`JmapStore::folders`], this clones the connection and releases
    /// the lock before the request. It does not use the `folders` field, so
    /// the ordering rule of that field does not apply. A disconnect during the
    /// request does not wait for it (see the comment of the `connection`
    /// field). The request continues on the connection that was live when it
    /// started; a disconnect does not cancel it.
    ///
    /// The next refresh sends the returned state to
    /// [`JmapStore::messages_since`]. The folder keeps the state in its
    /// summary, so it stays after a restart.
    ///
    /// A folder without a state calls this: on the first refresh of a
    /// mailbox, and to recover when the server cannot calculate a delta from
    /// a state.
    pub fn messages(&self, mailbox: &Id) -> Result<(State, Vec<MessageSummary>), StoreError> {
        let connection = self.connection().ok_or(StoreError::Disconnected)?;
        let sync = {
            let guard = read(connection);
            let sync = guard.as_ref().ok_or(StoreError::Disconnected)?;
            Arc::clone(sync)
        };
        tracing::debug!(mailbox_id = mailbox.as_str(), "listing messages in mailbox");
        match retry_once_after(
            || sync.messages(mailbox),
            SyncError::is_unauthorized,
            || self.refresh_credentials(&sync),
        ) {
            Ok((state, list)) => {
                tracing::debug!(
                    mailbox_id = mailbox.as_str(),
                    state = state.as_str(),
                    count = list.len(),
                    "listed messages in mailbox"
                );
                Ok((state, list))
            }
            Err(failure) => {
                tracing::debug!(
                    mailbox_id = mailbox.as_str(),
                    ?failure,
                    "listing messages in mailbox failed"
                );
                Err(failure.into())
            }
        }
    }

    /// The current contents of one mailbox, relative to the state of the rows
    /// in the folder summary. Each refresh after the first uses this.
    ///
    /// It is on the store and locks the same as [`JmapStore::messages`]. It
    /// asks the same question at a lower cost: one `Email/changes`, where a
    /// listing is one query plus one `Email/get` for each page of the mailbox.
    ///
    /// The result is a [`MessageUpdate`], and the caller selects the action
    /// from its three variants. Only the folder holds the rows, so only the
    /// folder can tell a message that moved *into* the mailbox from a message
    /// that was there when its flags changed.
    ///
    /// `held` is the number of rows that the folder has. It goes to
    /// `MailSync::messages_since`, which uses it for one decision: when a
    /// delta is so large that a full listing costs less. The folder counts its
    /// summary at no cost; the layer below must ask the server.
    pub fn messages_since(
        &self,
        mailbox: &Id,
        since: &State,
        held: usize,
    ) -> Result<MessageUpdate, StoreError> {
        let connection = self.connection().ok_or(StoreError::Disconnected)?;
        let sync = {
            let guard = read(connection);
            let sync = guard.as_ref().ok_or(StoreError::Disconnected)?;
            Arc::clone(sync)
        };
        tracing::debug!(
            mailbox_id = mailbox.as_str(),
            since = since.as_str(),
            held,
            "fetching message changes"
        );
        match retry_once_after(
            || sync.messages_since(mailbox, since, held),
            SyncError::is_unauthorized,
            || self.refresh_credentials(&sync),
        ) {
            Ok(update) => {
                tracing::debug!(
                    mailbox_id = mailbox.as_str(),
                    since = since.as_str(),
                    "fetched message changes"
                );
                Ok(update)
            }
            Err(failure) => {
                tracing::debug!(
                    mailbox_id = mailbox.as_str(),
                    since = since.as_str(),
                    ?failure,
                    "fetching message changes failed"
                );
                Err(failure.into())
            }
        }
    }

    /// All `Quota` objects of the account. `get_quota_info_sync` answers the
    /// quota question of a folder from this.
    ///
    /// It is on the store and locks the same as [`JmapStore::messages`]. It
    /// applies to the account, not to one folder, for the reason that
    /// [`MailSync::quotas`] gives: a JMAP quota is not specific to a mailbox,
    /// so each folder of the account answers from the same list.
    pub fn quotas(&self) -> Result<Vec<jmap_proto::quota::Quota>, StoreError> {
        let connection = self.connection().ok_or(StoreError::Disconnected)?;
        let sync = {
            let guard = read(connection);
            let sync = guard.as_ref().ok_or(StoreError::Disconnected)?;
            Arc::clone(sync)
        };
        tracing::debug!("fetching account quotas");
        match retry_once_after(
            || sync.quotas(),
            SyncError::is_unauthorized,
            || self.refresh_credentials(&sync),
        ) {
            Ok(quotas) => {
                tracing::debug!(count = quotas.len(), "fetched account quotas");
                Ok(quotas)
            }
            Err(failure) => {
                tracing::debug!(?failure, "fetching account quotas failed");
                Err(failure.into())
            }
        }
    }

    /// The RFC 5322 bytes of one message, which `get_message_sync` parses.
    ///
    /// It is on the store for the same reason as [`JmapStore::messages`], and
    /// locks the same. It takes no mailbox: a JMAP email id identifies the
    /// message in the account, not in a folder. Thus the folder of a Camel uid
    /// adds no information. A message in two mailboxes is one message here,
    /// as it is on the server.
    pub fn message_source(&self, uid: &Id) -> Result<Vec<u8>, StoreError> {
        let connection = self.connection().ok_or(StoreError::Disconnected)?;
        let sync = {
            let guard = read(connection);
            let sync = guard.as_ref().ok_or(StoreError::Disconnected)?;
            Arc::clone(sync)
        };
        tracing::debug!(uid = uid.as_str(), "fetching message source");
        match retry_once_after(
            || sync.message_source(uid),
            SyncError::is_unauthorized,
            || self.refresh_credentials(&sync),
        ) {
            Ok(source) => {
                tracing::debug!(
                    uid = uid.as_str(),
                    size = source.len(),
                    "fetched message source"
                );
                Ok(source)
            }
            Err(failure) => {
                tracing::debug!(
                    uid = uid.as_str(),
                    ?failure,
                    "fetching message source failed"
                );
                Err(failure.into())
            }
        }
    }

    /// Writes the keyword change of one message to the server. This is the
    /// write half of a folder synchronisation.
    ///
    /// It is on the store for the same reason as the reads above, and locks
    /// the same. It takes no mailbox for the same reason as
    /// [`JmapStore::message_source`]: a JMAP email id identifies the message
    /// in the account.
    ///
    /// It takes a read lock, not a write lock, although it writes. The lock
    /// protects the *connection*, and a request only uses the connection.
    /// Many folders can synchronise at the same time, as Camel does when
    /// Evolution closes.
    pub fn set_keywords(&self, uid: &Id, change: &KeywordChange) -> Result<(), StoreError> {
        let connection = self.connection().ok_or(StoreError::Disconnected)?;
        let sync = {
            let guard = read(connection);
            let sync = guard.as_ref().ok_or(StoreError::Disconnected)?;
            Arc::clone(sync)
        };
        tracing::debug!(uid = uid.as_str(), "setting message keywords");
        match retry_once_after(
            || sync.set_keywords(uid, change),
            SyncError::is_unauthorized,
            || self.refresh_credentials(&sync),
        ) {
            Ok(()) => {
                tracing::debug!(uid = uid.as_str(), "set message keywords");
                Ok(())
            }
            Err(failure) => {
                tracing::debug!(
                    uid = uid.as_str(),
                    ?failure,
                    "setting message keywords failed"
                );
                Err(failure.into())
            }
        }
    }

    /// Files one message into a different mailbox. This is the write of
    /// `transfer_messages_to_sync`.
    ///
    /// It is on the store and locks the same as [`JmapStore::set_keywords`].
    /// It takes no mailbox argument for the same reason: the [`Filing`] has
    /// the two mailboxes, and the message id identifies the message in the
    /// account, not in a folder.
    pub fn file_message(&self, uid: &Id, filing: &Filing) -> Result<(), StoreError> {
        let connection = self.connection().ok_or(StoreError::Disconnected)?;
        let sync = {
            let guard = read(connection);
            let sync = guard.as_ref().ok_or(StoreError::Disconnected)?;
            Arc::clone(sync)
        };
        tracing::debug!(uid = uid.as_str(), "filing message");
        match retry_once_after(
            || sync.file_message(uid, filing),
            SyncError::is_unauthorized,
            || self.refresh_credentials(&sync),
        ) {
            Ok(()) => {
                tracing::debug!(uid = uid.as_str(), "filed message");
                Ok(())
            }
            Err(failure) => {
                tracing::debug!(uid = uid.as_str(), ?failure, "filing message failed");
                Err(failure.into())
            }
        }
    }

    /// The ids of the messages in `mailbox` that match `filter`, as the server
    /// calculates them. This is the request of `search_by_expression` and
    /// `search_by_uids` of a folder, after translation of the Camel search
    /// expression.
    pub fn search(
        &self,
        mailbox: &Id,
        filter: Filter<EmailQueryFilter>,
    ) -> Result<Vec<Id>, StoreError> {
        let connection = self.connection().ok_or(StoreError::Disconnected)?;
        let sync = {
            let guard = read(connection);
            let sync = guard.as_ref().ok_or(StoreError::Disconnected)?;
            Arc::clone(sync)
        };
        tracing::debug!(mailbox_id = mailbox.as_str(), "searching mailbox");
        match retry_once_after(
            || sync.search(mailbox, filter.clone()),
            SyncError::is_unauthorized,
            || self.refresh_credentials(&sync),
        ) {
            Ok(ids) => {
                tracing::debug!(
                    mailbox_id = mailbox.as_str(),
                    matches = ids.len(),
                    "searched mailbox"
                );
                Ok(ids)
            }
            Err(failure) => {
                tracing::debug!(mailbox_id = mailbox.as_str(), ?failure, "search failed");
                Err(failure.into())
            }
        }
    }

    /// Moves one message from a mailbox into the trash, or only out of the
    /// mailbox if a different mailbox holds it. This is the write of a delete
    /// outside the trash, for the reasons that [`MailSync::trash_message`]
    /// gives. It locks the same as [`JmapStore::expunge_message`].
    pub fn trash_message(&self, uid: &Id, mailbox: &Id, trash: &Id) -> Result<(), StoreError> {
        let connection = self.connection().ok_or(StoreError::Disconnected)?;
        let sync = {
            let guard = read(connection);
            let sync = guard.as_ref().ok_or(StoreError::Disconnected)?;
            Arc::clone(sync)
        };
        retry_once_after(
            || sync.trash_message(uid, mailbox, trash),
            SyncError::is_unauthorized,
            || self.refresh_credentials(&sync),
        )
        .map_err(Into::into)
    }

    /// Removes one message from a mailbox permanently. This is the write of
    /// `expunge_sync`.
    ///
    /// It is on the store and locks the same as [`JmapStore::file_message`].
    /// It takes the mailbox and the message, because an expunge needs both.
    /// The message id identifies the message in the account. The mailbox is
    /// the folder in which the user selected Expunge. The mailbox decides if
    /// the message is destroyed or only removed from that mailbox, for the
    /// reasons that [`MailSync::expunge_message`] gives.
    ///
    /// It takes a read lock although it destroys, for the reason that
    /// [`JmapStore::set_keywords`] gives: the lock protects the connection,
    /// and Evolution empties a trash while the account continues to refresh.
    pub fn expunge_message(&self, uid: &Id, mailbox: &Id) -> Result<(), StoreError> {
        let connection = self.connection().ok_or(StoreError::Disconnected)?;
        let sync = {
            let guard = read(connection);
            let sync = guard.as_ref().ok_or(StoreError::Disconnected)?;
            Arc::clone(sync)
        };
        tracing::debug!(
            uid = uid.as_str(),
            mailbox_id = mailbox.as_str(),
            "expunging message"
        );
        match retry_once_after(
            || sync.expunge_message(uid, mailbox),
            SyncError::is_unauthorized,
            || self.refresh_credentials(&sync),
        ) {
            Ok(()) => {
                tracing::debug!(
                    uid = uid.as_str(),
                    mailbox_id = mailbox.as_str(),
                    "expunged message"
                );
                Ok(())
            }
            Err(failure) => {
                tracing::debug!(
                    uid = uid.as_str(),
                    mailbox_id = mailbox.as_str(),
                    ?failure,
                    "expunging message failed"
                );
                Err(failure.into())
            }
        }
    }

    /// Puts a new message into one mailbox of the account. This is the write
    /// of `append_message_sync`.
    ///
    /// It is on the store and locks the same as [`JmapStore::file_message`].
    /// `Email/import` identifies the message by the blob that it uploads, not
    /// by an id in the account. Thus the target mailbox is the only other
    /// argument that identifies the operation.
    ///
    /// It takes a read lock although it uploads. This is the longest request
    /// of this store. An exclusive lock stops each folder refresh in the
    /// account for the duration of an attachment upload.
    pub fn import_message(
        &self,
        mailbox: &Id,
        source: Vec<u8>,
        keywords: &Keywords,
        received_at: Option<i64>,
    ) -> Result<Id, StoreError> {
        let connection = self.connection().ok_or(StoreError::Disconnected)?;
        let sync = {
            let guard = read(connection);
            let sync = guard.as_ref().ok_or(StoreError::Disconnected)?;
            Arc::clone(sync)
        };
        tracing::debug!(
            mailbox_id = mailbox.as_str(),
            size = source.len(),
            "importing message"
        );
        // The closure clones the bytes, because `import_message` takes them by
        // value (it uploads them as a blob) and a retry must send them again.
        // The `attempt` of `retry_once_after` must be safe to run two times.
        // Each other retried call here meets this by taking references.
        match retry_once_after(
            || sync.import_message(mailbox, source.clone(), keywords, received_at),
            SyncError::is_unauthorized,
            || self.refresh_credentials(&sync),
        ) {
            Ok(id) => {
                tracing::debug!(
                    mailbox_id = mailbox.as_str(),
                    uid = id.as_str(),
                    "imported message"
                );
                Ok(id)
            }
            Err(failure) => {
                tracing::debug!(
                    mailbox_id = mailbox.as_str(),
                    ?failure,
                    "importing message failed"
                );
                Err(failure.into())
            }
        }
    }

    /// Sets if the user wants to see a folder. This is the write of
    /// `subscribe_folder_sync` and `unsubscribe_folder_sync` of
    /// `CamelSubscribable`.
    ///
    /// It takes a read lock like the writes above, but it keeps the lock
    /// across the request. Then it also edits the held folder listing. This
    /// edit is not for cache performance: it gives the answer to a different
    /// question.
    ///
    /// Camel declares `folder_is_subscribed` as *non-blocking*, and Evolution
    /// calls it once for each folder when it draws the tree. Thus only the
    /// listing can answer it. If the listing did not change, Evolution shows
    /// the old subscription state again.
    ///
    /// The edit uses [`Arc::make_mut`]. Thus a caller that walks a tree it got
    /// earlier continues on that same, unchanged tree. A `CamelFolderInfo`
    /// forest is copied from a borrowed tree, and the tree must not change
    /// during that copy.
    ///
    /// If no listing exists yet, nothing is added. Otherwise the tree has only
    /// the one mailbox that the write named, which is an account with one
    /// folder.
    ///
    /// The state of the listing does not change, intentionally. The write
    /// changed the account, so the next refresh finds a change and reads the
    /// tree again: one listing more than necessary. The alternative is a store
    /// that makes a state string that the server did not send, and asks
    /// `Mailbox/changes` from it.
    pub fn set_subscribed(&self, mailbox: &Id, subscribed: bool) -> Result<(), StoreError> {
        let connection = self.connection().ok_or(StoreError::Disconnected)?;
        let connection = read(connection);
        let sync = connection.as_ref().ok_or(StoreError::Disconnected)?;
        tracing::debug!(
            mailbox_id = mailbox.as_str(),
            subscribed,
            "setting folder subscription"
        );
        if let Err(failure) = retry_once_after(
            || sync.set_subscribed(mailbox, subscribed),
            SyncError::is_unauthorized,
            || self.refresh_credentials(sync),
        ) {
            tracing::debug!(
                mailbox_id = mailbox.as_str(),
                subscribed,
                ?failure,
                "setting folder subscription failed"
            );
            return Err(failure.into());
        }

        // Edit only after the server accepted, and while the connection that
        // it used is still current. This is the ordering rule of the
        // `folders` field.
        if let Some(folders) = self.folder_listing()
            && let Some(listing) = write(folders).as_mut()
        {
            Arc::make_mut(&mut listing.tree).set_subscribed(mailbox, subscribed);
            listing.generation = listing.generation.wrapping_add(1);
        }
        tracing::debug!(
            mailbox_id = mailbox.as_str(),
            subscribed,
            "set folder subscription"
        );
        Ok(())
    }

    /// Creates a folder. This is the write of `create_folder_sync`.
    ///
    /// It locks the same as [`JmapStore::set_subscribed`]. It edits the held
    /// listing for a stronger reason: Camel gives the returned folder to the
    /// folder tree of Evolution and then opens it by path. The open uses the
    /// listing. Without the edit, the user sees a folder that the store
    /// refuses to open until the account refreshes.
    ///
    /// `parent` is the folder above the new one. It is a full folder, not an
    /// id, because the path of the result comes from the path of the parent.
    /// [`MailSync::create_folder`] makes that path.
    ///
    /// If no listing exists yet, nothing is added, and the state of the
    /// listing does not change. [`JmapStore::set_subscribed`] gives the
    /// reasons for both.
    pub fn create_folder(
        &self,
        parent: Option<&FolderInfo>,
        name: &str,
    ) -> Result<FolderInfo, StoreError> {
        let connection = self.connection().ok_or(StoreError::Disconnected)?;
        let connection = read(connection);
        let sync = connection.as_ref().ok_or(StoreError::Disconnected)?;
        tracing::debug!(
            parent_id = parent.map(|p| p.id.as_str()),
            name,
            "creating mail folder"
        );
        let created = match retry_once_after(
            || sync.create_folder(parent, name),
            SyncError::is_unauthorized,
            || self.refresh_credentials(sync),
        ) {
            Ok(created) => created,
            Err(failure) => {
                tracing::debug!(
                    parent_id = parent.map(|p| p.id.as_str()),
                    name,
                    ?failure,
                    "creating mail folder failed"
                );
                return Err(failure.into());
            }
        };

        // Edit only after the server created the folder, and while the
        // connection that it used is still current. This is the ordering rule
        // of the `folders` field.
        if let Some(folders) = self.folder_listing()
            && let Some(listing) = write(folders).as_mut()
        {
            Arc::make_mut(&mut listing.tree).insert(created.clone());
            listing.generation = listing.generation.wrapping_add(1);
        }
        tracing::debug!(folder_id = created.id.as_str(), name, "created mail folder");
        Ok(created)
    }

    /// Deletes a folder. This is the write of `delete_folder_sync`.
    ///
    /// It is the opposite of [`JmapStore::create_folder`] in all aspects, also
    /// in the reason for the listing edit. Without the edit, Camel can open a
    /// folder that the account does not have.
    ///
    /// It takes a mailbox id, not a path. The caller got the folder from a
    /// listing, and a rename by a different client can make that path
    /// incorrect.
    pub fn delete_folder(&self, mailbox: &Id) -> Result<(), StoreError> {
        let connection = self.connection().ok_or(StoreError::Disconnected)?;
        let connection = read(connection);
        let sync = connection.as_ref().ok_or(StoreError::Disconnected)?;
        tracing::debug!(mailbox_id = mailbox.as_str(), "deleting mail folder");
        if let Err(failure) = retry_once_after(
            || sync.delete_folder(mailbox),
            SyncError::is_unauthorized,
            || self.refresh_credentials(sync),
        ) {
            tracing::debug!(
                mailbox_id = mailbox.as_str(),
                ?failure,
                "deleting mail folder failed"
            );
            return Err(failure.into());
        }

        // As above: edit only after the server accepted.
        if let Some(folders) = self.folder_listing()
            && let Some(listing) = write(folders).as_mut()
        {
            Arc::make_mut(&mut listing.tree).remove(mailbox);
            listing.generation = listing.generation.wrapping_add(1);
        }
        tracing::debug!(mailbox_id = mailbox.as_str(), "deleted mail folder");
        Ok(())
    }

    /// Renames and moves a folder. This is the write of `rename_folder_sync`.
    ///
    /// It locks and orders the same as the two writes above, and edits the
    /// held listing for their reason. There is one more reason: a rename also
    /// changes the path of each folder *below* it, and Camel opens a folder by
    /// its path. Without the edit, `get_folder_sync` finds nothing for the new
    /// paths, and finds old folders for the old paths.
    ///
    /// `folder` is a full folder, not an id, because both parts are necessary.
    /// The write uses the id. The result uses the folder if the listing cannot
    /// supply one. `parent` is a full folder for the reason that
    /// [`JmapStore::create_folder`] gives: the path of the result comes from
    /// the path of the parent.
    ///
    /// The result is the folder as the listing now has it, with all its
    /// subfolders. `camel_store_folder_renamed` sends this folder, and
    /// Evolution walks its children.
    ///
    /// The listing can be empty, for example if a disconnect cleared it after
    /// the caller found the folder. Then the result is the given folder at its
    /// new path, without children: only the tree edit knows their paths. This
    /// result is incomplete, not wrong, and the account is listed again.
    pub fn rename_folder(
        &self,
        folder: &FolderInfo,
        parent: Option<&FolderInfo>,
        name: &str,
    ) -> Result<FolderInfo, StoreError> {
        let connection = self.connection().ok_or(StoreError::Disconnected)?;
        let connection = read(connection);
        let sync = connection.as_ref().ok_or(StoreError::Disconnected)?;
        tracing::debug!(
            mailbox_id = folder.id.as_str(),
            parent_id = parent.map(|p| p.id.as_str()),
            name,
            "renaming mail folder"
        );
        let path = match retry_once_after(
            || sync.rename_folder(&folder.id, parent, name),
            SyncError::is_unauthorized,
            || self.refresh_credentials(sync),
        ) {
            Ok(path) => path,
            Err(failure) => {
                tracing::debug!(
                    mailbox_id = folder.id.as_str(),
                    name,
                    ?failure,
                    "renaming mail folder failed"
                );
                return Err(failure.into());
            }
        };

        // Edit only after the server accepted, and while the connection that
        // it used is still current. This is the ordering rule of the
        // `folders` field.
        let renamed = self.folder_listing().and_then(|folders| {
            let mut folders = write(folders);
            let listing = folders.as_mut()?;
            let tree = Arc::make_mut(&mut listing.tree);
            tree.rename(&folder.id, &path, name);
            listing.generation = listing.generation.wrapping_add(1);
            listing.tree.find(&path).cloned()
        });

        let result = renamed.unwrap_or_else(|| FolderInfo {
            path: path.clone(),
            display_name: name.to_owned(),
            children: Vec::new(),
            ..folder.clone()
        });
        tracing::debug!(
            mailbox_id = folder.id.as_str(),
            path = result.path.as_str(),
            name,
            "renamed mail folder"
        );
        Ok(result)
    }

    /// The held folder listing, if there is one. It makes no request and
    /// needs no connection.
    ///
    /// [`JmapStore::folders`] answers the same question for a caller that can
    /// block, and lists the account if it has no listing. This function is
    /// for a caller that cannot block. Camel declares `folder_is_subscribed`
    /// of `CamelSubscribable` as non-blocking, and Evolution calls it once for
    /// each folder when it draws the tree. A request there stops the UI thread
    /// once for each row.
    ///
    /// Thus `None` is a valid answer ("no information about this account
    /// yet"), not a problem to correct.
    pub fn held_folders(&self) -> Option<Arc<FolderTree>> {
        let folders = self.folder_listing()?;
        let listing = read(folders);
        listing.as_ref().map(|listing| Arc::clone(&listing.tree))
    }

    /// Drops the folder listing. The two operations that make the listing
    /// incorrect for the account call this while they hold the connection
    /// lock.
    fn forget_folders(&self) {
        if let Some(folders) = self.folder_listing() {
            *write(folders) = None;
        }
    }

    /// An instance outside the GObject type system: zeroed parent bytes and
    /// initialised slots. This is the result of `instance_init` without the
    /// GObject.
    ///
    /// This is for tests, and it is necessary. Camel makes a store through
    /// `camel_session_add_service`, which needs a `CamelSession`. In
    /// Evolution, that is an `EMailSession` over a source registry on the
    /// session bus.
    ///
    /// Use only the slots of the result. The parent bytes are a valid bit
    /// pattern (each field is a pointer or an integer, and NULL is a valid
    /// pointer), but they are not a GObject. Passing them to a Camel function
    /// is undefined behaviour.
    #[cfg(feature = "testing")]
    pub fn detached() -> Box<Self> {
        // SAFETY: each field of the parent is a pointer or an integer, for
        // which all zero is a valid value. An all-zero `Slot` is its
        // documented empty state.
        let store: Box<Self> = unsafe { zeroed_box() };
        store.connection.init(RwLock::new(None));
        store.folders.init(RwLock::new(None));
        store.push.init(Mutex::new(None));
        store.refresher.init(Mutex::new(None));
        store
    }

    /// The Rust view of a `CamelStore *` from Camel.
    ///
    /// # Safety
    ///
    /// `store` must be NULL or point at an instance of this type. Camel calls
    /// the vfuncs of a class only on instances of that class, so a vfunc
    /// argument satisfies this. Check any other pointer with
    /// `G_TYPE_CHECK_INSTANCE_TYPE` first.
    pub unsafe fn borrow<'a>(store: *mut CamelStore) -> Option<&'a Self> {
        // SAFETY: the doc comment above gives the same contract.
        unsafe { dispatched_borrow(store) }
    }

    /// The connection slot. It is `None` on an instance before its
    /// `instance_init` or after its `finalize`.
    fn connection(&self) -> Option<&RwLock<Option<Arc<MailSync>>>> {
        self.connection.get()
    }

    /// The folder listing slot, with the same condition.
    fn folder_listing(&self) -> Option<&RwLock<Option<Listing>>> {
        self.folders.get()
    }
}

/// The store as the service that the vfuncs of [`crate::service`] connect.
///
/// The three methods forward to the inherent methods above. The vfuncs need
/// only the part that the store shares with the transport. A `folders` slot
/// that [`JmapStore::store_connection`] clears is specific to the store, so
/// the trait does not name it.
///
// SAFETY: `JmapStore` is the instance struct of the type that `store_type`
// registers. That type derives from `CamelOfflineStore`, which derives from
// `CamelStore`, which derives from `CamelService`.
unsafe impl Connected for JmapStore {
    fn hold_connection(&self, sync: MailSync) {
        self.store_connection(sync);
    }

    fn release_connection(&self) {
        self.drop_connection();
    }

    fn holds_connection(&self) -> bool {
        self.is_connected()
    }
}

/// A poisoned lock means that a different operation panicked while it held
/// the lock. That does not damage the protected value: a `MailSync` is an
/// HTTP client and an account id, and a `Listing` is a tree and a state
/// string. Thus it is better to continue than to stop the store.
///
/// This is `pub(crate)` for [`crate::transport`], which holds its own
/// connection under the same rule. One copy keeps this decision in one place.
pub(crate) fn read<T>(lock: &RwLock<T>) -> RwLockReadGuard<'_, T> {
    lock.read().unwrap_or_else(PoisonError::into_inner)
}

pub(crate) fn write<T>(lock: &RwLock<T>) -> RwLockWriteGuard<'_, T> {
    lock.write().unwrap_or_else(PoisonError::into_inner)
}

/// The same rule for the two push slots. They are `Mutex`, not `RwLock`,
/// because no hot path reads them.
fn push_lock(slot: &Mutex<Option<PushRefresh>>) -> MutexGuard<'_, Option<PushRefresh>> {
    slot.lock().unwrap_or_else(PoisonError::into_inner)
}

fn refresher_lock(slot: &Mutex<Option<FolderRefresh>>) -> MutexGuard<'_, Option<FolderRefresh>> {
    slot.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The class struct, with the same layout rule as the instance struct. It
/// adds no fields: `class_init` writes the overrides into the inherited
/// `CamelStoreClass` and `CamelServiceClass` vtables. But the type needs its
/// own class, not `CamelOfflineStoreClass`, as the location of the overrides.
#[repr(C)]
pub struct JmapStoreClass {
    parent_class: CamelOfflineStoreClass,
}

// SAFETY: both structs are #[repr(C)] and start with the CamelOfflineStore
// instance and class structs. tests/layout.rs of eds-sys checks those layouts
// against `g_type_query`. CamelOfflineStore derives from CamelStore, which
// derives from CamelService, which derives from GObject.
unsafe impl ObjectSubclass for JmapStore {
    /// `CamelJmapStore`, not `JmapStore`: all stores of Camel have the name
    /// `Camel<Protocol>Store`. A user sees the type name in a GObject warning
    /// about an incorrect store type.
    const NAME: &'static CStr = c"CamelJmapStore";
    type Instance = JmapStore;
    type Class = JmapStoreClass;

    fn parent_type() -> GType {
        // SAFETY: no arguments, and the type initialises itself.
        unsafe { camel_offline_store_get_type() }
    }

    fn interfaces() -> Vec<InterfaceDecl> {
        // `CamelSubscribable` has three vfuncs and no properties, and Camel
        // gives no default for them. This is different from
        // `CamelNetworkSettings` of the settings class. If the type claims
        // this interface and does not fill its vtable, each subscription
        // method calls through NULL. [`crate::subscribe`] fills it.
        vec![InterfaceDecl::filled_by::<Subscribable>()]
    }

    fn class_init_types() -> Vec<GType> {
        // Register the settings type before this type, not in the class
        // initialiser. `class_init` runs under the GLib class-initialisation
        // lock, and it cannot take the registration lock without an inverted
        // lock order.
        vec![settings_type()]
    }

    unsafe fn class_init(class: *mut Self::Class) {
        // This sets the class that `camel_service_ref_settings` instantiates
        // when the service has no settings object. It is also the class that
        // the account editor of Evolution and
        // `e_source_camel_configure_service` configure. The inherited value is
        // `CamelOfflineSettings`, which has no server. Thus this line connects
        // a JMAP account to a host.
        //
        // SAFETY: the class starts with CamelOfflineStoreClass, which derives
        // from CamelStoreClass, which derives from CamelServiceClass. This is
        // the contract above.
        let service = class.cast::<CamelServiceClass>();
        unsafe { (*service).settings_type = settings_type() };

        // Name, connect, authenticate and disconnect. They are in
        // `crate::service`, not here: Camel splits one connect operation
        // across three slots for its re-prompt loop, and one file shows it
        // as one operation.
        // SAFETY: as above.
        unsafe { crate::service::install_vfuncs::<Self>(service) };

        // The folder listing. `CamelStore` leaves `get_folder_info_sync`
        // NULL, and `camel_store_get_folder_info_sync` does not call a store
        // that did not fill it. Without this line, the account has no folders
        // and shows a runtime warning.
        //
        // SAFETY: the class starts with CamelOfflineStoreClass, which starts
        // with CamelStoreClass. This is the contract above.
        unsafe { crate::folders::install_vfuncs(class.cast::<CamelStoreClass>()) };

        // The three vfuncs that create, delete and rename folders. They have
        // their own module, not `crate::folders`: that module answers
        // questions about the folders, and these vfuncs change them.
        //
        // SAFETY: as above.
        unsafe { crate::manage::install_vfuncs(class.cast::<CamelStoreClass>()) };
    }

    unsafe fn instance_init(instance: *mut Self::Instance) {
        // SAFETY: `instance` points at a zeroed instance struct of this type,
        // and a zeroed `Slot` is empty.
        unsafe {
            (*instance).connection.init(RwLock::new(None));
            (*instance).folders.init(RwLock::new(None));
            (*instance).push.init(Mutex::new(None));
            (*instance).refresher.init(Mutex::new(None));
        };

        // On a JMAP account, trash and junk are mailboxes of the account, not
        // searches on local flags. `CamelStore` starts each store with
        // `CAMEL_STORE_VTRASH | CAMEL_STORE_VJUNK`. With these bits,
        // `camel_store_get_folder_info_sync` adds `.#evolution/Trash` and
        // `.#evolution/Junk` to each listing.
        //
        // A provider that keeps these bits and overrides the two getters (see
        // [`crate::folders`]) shows two trash and two junk folders. One pair is
        // real, and one pair is a view of flags that no other client shares.
        // Clear them here, not per account: RFC 8621 gives a mailbox a role,
        // so this is a fact of the protocol, not a setting.
        //
        // Set `CAMEL_STORE_SUPPORTS_INITIAL_SETUP` for the opposite reason. It
        // advertises the vfunc that [`crate::folders`] installs. Without it,
        // the account wizard of Evolution never calls `initial_setup_sync`.
        //
        // All other bits stay as Camel set them. This includes
        // `CAN_EDIT_FOLDERS`, which [`crate::manage`] supports.
        //
        // SAFETY: the `instance_init` of the parent already ran (GObject
        // initialises from base to derived). Thus this is a `CamelStore` with
        // private data, which is all that the two calls use.
        unsafe {
            let store = instance.cast::<CamelStore>();
            let flags = (camel_store_get_flags(store) & !(CAMEL_STORE_VTRASH | CAMEL_STORE_VJUNK))
                | CAMEL_STORE_SUPPORTS_INITIAL_SETUP;
            camel_store_set_flags(store, flags);
        };
    }

    unsafe fn finalize(instance: *mut Self::Instance) {
        // Clear push first, before the connection and all other fields. The
        // push slot holds a thread that can call this instance, and the clear
        // stops and joins that thread. Camel does not always call
        // `disconnect_sync` before it drops a store: a service that never
        // connected is finalized without it. Thus this code, not
        // `disconnect_sync`, makes sure that the pump stops.
        //
        // SAFETY: the instance is in finalization, so nothing can still reach
        // it, and no borrow from `get` is alive.
        unsafe {
            (*instance).push.clear();
            (*instance).refresher.clear();
        };
        // SAFETY: as above. Without this, the connection (and its socket)
        // lives longer than the account, and the folder listing leaks too.
        unsafe {
            (*instance).connection.clear();
            (*instance).folders.clear();
        };
    }
}

/// Registers the store type, or returns it if it is already registered.
///
/// The registration is static, unlike the EDS backend types: a Camel provider
/// is not a `GTypeModule`. Camel loads the module with dlopen and never
/// closes it. The provider struct keeps a pointer to these `GType`s
/// permanently. Thus no unload can unregister a dynamic type. A type that
/// *can* unload here is one that Camel can still try to instantiate.
pub fn store_type() -> GType {
    register_static::<JmapStore>()
}
