// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! `CamelJmapStore`: the type Camel instantiates for a JMAP mail account.
//!
//! What it carries is the things a later increment cannot change cheaply: its
//! parent, because every folder vfunc is declared on one of the two candidates;
//! the settings class it is configured through — [`crate::settings`], without
//! which a JMAP account has nowhere to keep a server; and the slots its
//! state lives in, which are fields of the instance struct and therefore part
//! of a layout the vfuncs read through — the connection, the folder listing
//! read over it, and the two the account's JMAP Push subscription needs.
//!
//! The `CamelService` vfuncs that fill and empty the first of those slots are
//! [`crate::service`]; `CamelStoreClass`'s own `get_folder_info_sync`, which
//! reads the second, is [`crate::folders`]. Both are installed from `class_init`
//! below, and `CamelSubscribable`'s three — which read and write the second slot
//! too — are [`crate::subscribe`], declared from `interfaces` because an
//! interface's vtable is filled through GObject rather than through our class.
//! The last two slots are [`crate::push`]'s, filled and emptied alongside the
//! connection, and that module's docs are where the reason they are two lives.

use std::ffi::CStr;
use std::ptr;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, RwLock, RwLockReadGuard, RwLockWriteGuard};

use eds_sys::{
    CAMEL_STORE_FOLDER_INFO_REFRESH, CAMEL_STORE_VJUNK, CAMEL_STORE_VTRASH, CamelOfflineStore,
    CamelOfflineStoreClass, CamelService, CamelServiceClass, CamelStore, CamelStoreClass,
    CamelStoreGetFolderInfoFlags, camel_offline_store_get_type, camel_service_get_type,
    camel_service_ref_session, camel_service_ref_settings, camel_store_get_flags,
    camel_store_set_flags,
};
use glib_sys::GType;
use gobject_sys::{GObject, g_type_check_instance_is_a};
use jmap_backend_core::instance::Slot;
#[cfg(feature = "testing")]
use jmap_backend_core::instance::zeroed_box;
use jmap_backend_core::marshal::dispatched_borrow;
use jmap_backend_core::owned::Owned;
use jmap_backend_core::push::{self, PushRefresh};
use jmap_backend_core::retry::retry_once_after;
use jmap_backend_core::subclass::{InterfaceDecl, ObjectSubclass, register_static};
use jmap_client::Credentials;
use jmap_mail_sync::{
    Filing, FolderInfo, FolderTree, FolderUpdate, KeywordChange, Keywords, MailSync,
    MessageSummary, MessageUpdate, SyncError,
};
use jmap_proto::{Id, State};

use crate::connect::StoreError;
use crate::oauth2;
use crate::push::FolderRefresh;
use crate::service::Connected;
use crate::settings::settings_type;
use crate::subscribe::Subscribable;

/// A folder listing, and the state it is current as of.
///
/// The tree is behind an [`Arc`] because it outlives the lock: a caller
/// translating it into a `CamelFolderInfo` forest must not hold the store's
/// listing locked while it does, and copying the tree per call would be a walk
/// of every mailbox for an answer that did not change.
struct Listing {
    state: State,
    tree: Arc<FolderTree>,
}

/// The instance struct. `#[repr(C)]` leading with the parent's instance struct
/// is what makes a `*mut JmapStore` usable as the `CamelStore *` every Camel
/// function takes.
#[repr(C)]
pub struct JmapStore {
    parent: CamelOfflineStore,
    /// The connection, from `connect_sync` to `disconnect_sync`.
    ///
    /// An `RwLock` rather than a `Mutex`, for the same reason as the address
    /// book backend's: Camel drives a store from several threads at once — a
    /// folder refresh, a message fetch and a folder-list update are three
    /// different operations that may all be in flight — and serialising them
    /// behind one lock would make each wait on the slowest. Only connect and
    /// disconnect, which replace the value, need exclusive access.
    ///
    /// The `Arc` is what lets most callers below clone the connection out and
    /// drop the guard before making their network round trip, so
    /// `drop_connection`'s write lock does not wait on one already in flight
    /// — see each method's own comment for which ones do this and which
    /// cannot. `MailSync` itself has no reason to be `Clone`; sharing the one
    /// instance is the point.
    connection: Slot<RwLock<Option<Arc<MailSync>>>>,
    /// The folder tree the connection last answered with, and when.
    ///
    /// A slot of its own rather than a field of the connection, so that a
    /// folder refresh and a reconnect do not queue behind each other. What ties
    /// the two together is an ordering rule instead: a listing is stored while
    /// the connection it was read over is still read-locked, so a
    /// [`store_connection`](JmapStore::store_connection) — which needs that
    /// lock exclusively — cannot slip in between the request and the write and
    /// have its clearing undone by a tree the previous connection produced.
    folders: Slot<RwLock<Option<Listing>>>,
    /// The JMAP Push subscription, for the same span as the connection: a
    /// server that advertises an `eventSourceUrl` gets to say "mail arrived"
    /// instead of being asked on the account's refresh interval.
    ///
    /// A plain `Mutex` — unlike the two above, nothing reads this on the hot
    /// path; it is only installed, taken and dropped, all of which are
    /// exclusive anyway.
    push: Slot<Mutex<Option<PushRefresh>>>,
    /// The coalescing worker a push hands the actual refreshing to, in a slot
    /// of its own rather than beside the subscription above.
    ///
    /// That separation is what keeps [`JmapStore::stop_push`] from
    /// deadlocking: stopping the subscription joins its pump thread, and that
    /// pump may at that very moment be inside
    /// [`crate::push::dispatch`], which locks this. Two locks taken in
    /// sequence and never nested cannot make a cycle; one lock would.
    refresher: Slot<Mutex<Option<FolderRefresh>>>,
}

impl JmapStore {
    /// Installs `sync` as the live connection, replacing whatever was there.
    ///
    /// Replacing rather than refusing: Camel reconnects a store it believes has
    /// gone away, and the connection being replaced is exactly the one it
    /// believes that about. The old one is dropped — and its socket closed —
    /// when this returns, and the folder listing read over it goes too: a
    /// reconnect happens because something about the account changed, and the
    /// server behind the new connection may not be the one the old tree —
    /// paths, message counts, and the JMAP ids every later request is built
    /// from — describes.
    pub fn store_connection(&self, sync: MailSync) {
        if let Some(connection) = self.connection() {
            let mut guard = write(connection);
            self.forget_folders();
            tracing::debug!("storing mail connection in store");
            *guard = Some(Arc::new(sync));
        }
        // And only then the push subscription, which is authenticated with
        // the connection just installed and refreshes over it. The other
        // order has a window in which a pushed refresh reaches
        // `refresh_info_sync` before `store_connection` ran, which reports the
        // account offline for a change that had in fact arrived.
        self.start_push();
    }

    /// Subscribes to the account's JMAP Push stream, if the server offers one
    /// — see [`crate::push`] for what a push then does, and why the refresh
    /// it triggers is not run where the push arrives.
    ///
    /// A no-op with no connection, on a server advertising no
    /// `eventSourceUrl`, and on a [`JmapStore::detached`] test instance, which
    /// is not a GObject and so has nothing to hang a weak reference on. All
    /// three leave the account exactly as every JMAP account was before push
    /// existed: refreshed when Camel next asks.
    fn start_push(&self) {
        let Some(service) = self.service() else {
            return;
        };
        let store = service.cast::<CamelStore>();

        // The coalescing worker goes in first: `start_for` spawns its pump
        // before it returns, so the very first push may arrive while this
        // function is still running, and a push that finds no worker would be
        // silently dropped.
        //
        let pass = |object: *mut GObject| {
            // SAFETY: the store `FolderRefresh` was built for — checked to be
            // a `CamelService`, and so a `CamelStore`, just above — under the
            // strong reference the worker holds across the whole call.
            unsafe { crate::push::refresh_open_folders(object.cast()) };
        };
        // SAFETY: `service` is this live instance typed as its C ancestor —
        // referenced for this call because `self` is borrowed from it — and
        // the pass above is only ever called with that same pointer.
        let refresher = unsafe { FolderRefresh::new(store.cast(), pass) };
        self.install_refresher(Some(refresher));

        // Both halves of item 28's mail-side push in one subscription: a
        // `Mailbox` change and an `Email`/`EmailDelivery` change ask Camel for
        // different things, so `dispatch` needs to be told which one a given
        // `StateChange` actually named — `start_for_with`, not `start_for`,
        // is what carries that through.
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
            // No stream, so nothing will ever ask the worker for a pass.
            self.install_refresher(None);
        }
        self.replace_push(started);
    }

    /// Installs `push` as the live subscription, stopping and dropping
    /// whatever was there — which is what a reconnect wants, since the old
    /// subscription is authenticated with the connection that was just
    /// replaced.
    pub fn store_push(&self, push: PushRefresh) {
        self.replace_push(Some(push));
    }

    /// Stops the push subscription and waits for its pump, reporting whether
    /// there was one. Once this returns, no further refresh can be *asked*
    /// for; a pass already in flight is left to finish, on the store it holds
    /// a strong reference to, because nothing here may block on a network
    /// round trip (see [`crate::push`]).
    pub fn stop_push(&self) -> bool {
        // The subscription first, and without the refresher's lock held: the
        // pump thread this joins may be inside `schedule_refresh` holding
        // exactly that lock.
        let stopped = self.replace_push(None);
        self.install_refresher(None);
        stopped
    }

    /// Whether a push subscription is live.
    #[cfg(feature = "testing")]
    pub fn is_pushing(&self) -> bool {
        self.push
            .get()
            .is_some_and(|slot| push_lock(slot).is_some())
    }

    /// Replaces the `Authorization` header the live push subscription sends
    /// on its future reconnect attempts, if there is a subscription — a
    /// no-op otherwise. Called from `JmapStore::refresh_credentials` right
    /// after it installs a fresh OAuth 2.0 token on the connection, so a
    /// subscription refused with the stale one picks the new one up rather
    /// than looping on the same failure until Camel itself reconnects.
    pub fn refresh_push_headers(&self, headers: Vec<(String, String)>) {
        if let Some(slot) = self.push.get()
            && let Some(push) = push_lock(slot).as_ref()
        {
            push.set_headers(headers);
        }
    }

    /// Asks the coalescing worker for a refresh pass. What
    /// [`crate::push::dispatch`] does for an `Email`/`EmailDelivery` push,
    /// and the reason the worker lives in a slot of its own — see the
    /// field's own comment.
    pub fn request_folder_refresh(&self) {
        if let Some(slot) = self.refresher.get()
            && let Some(refresher) = refresher_lock(slot).as_ref()
        {
            refresher.request();
        }
    }

    /// Puts `push` in the subscription slot, reporting whether it displaced
    /// one. Dropping the displaced subscription stops and joins its pump.
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

    /// Drops the connection, reporting whether there was one.
    ///
    /// Camel asks a store to disconnect on shutdown whether or not it ever
    /// connected, so "there was none" is a normal outcome rather than a
    /// failure; it is still reported, because `disconnect_sync` is the caller
    /// that wants to know whether it did anything.
    ///
    /// The folder listing goes with it. That changes no answer — with no
    /// connection there is nothing that could serve a tree, and the reconnect
    /// clears it again anyway — but a disconnected account holding its whole
    /// mailbox tree in memory until Evolution quits is dead weight, and the
    /// point of a disconnect is that the account is not in use.
    pub fn drop_connection(&self) -> bool {
        // Stopped first, and before the connection it authenticates with
        // goes: a push arriving after this point would ask for a refresh over
        // a store that has nothing left to refresh with.
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

    /// Whether an operation would find a connection.
    pub fn is_connected(&self) -> bool {
        self.connection()
            .is_some_and(|connection| read(connection).is_some())
    }

    /// This store, as the `CamelService` Camel gave Evolution — the same
    /// instance [`JmapStore::borrow`] reaches by casting the other way — or
    /// `None` on a [`JmapStore::detached`] test instance, which is not one.
    ///
    /// `Self` leads with `CamelOfflineStore`, from `CamelStore`, from
    /// `CamelService`, so `self` and this pointer name the same object, just
    /// typed as its C ancestor — exactly the cast [`JmapStore::borrow`] makes
    /// in the other direction. The type check is what makes this sound on a
    /// detached instance too: its leading `GTypeInstance` field is `NULL`
    /// (all-zero), which `g_type_check_instance_is_a` reads as "not this
    /// type" without dereferencing further, the same guarantee
    /// `jmap_backend_core::source::backend_source` relies on for `ESource`.
    fn service(&self) -> Option<*mut CamelService> {
        let ptr = ptr::from_ref(self).cast::<CamelService>().cast_mut();
        // SAFETY: `ptr` is `self` reinterpreted as its leading `GTypeInstance`
        // field, checked against the target type before it is read any
        // further, per the doc comment above.
        let is_service =
            unsafe { g_type_check_instance_is_a(ptr.cast(), camel_service_get_type()) };
        (is_service != 0).then_some(ptr)
    }

    /// Fetches a fresh OAuth 2.0 access token for the account and installs it
    /// on the live connection, reporting whether an operation is now worth
    /// retrying.
    ///
    /// The mail-side counterpart of the calendar and address book backends'
    /// own `refresh_credentials`: a 401 on a pooled connection is answered by
    /// refreshing rather than escalating straight to a consent window, since
    /// the stored refresh token is usually still good.
    ///
    /// Two reasons this reports "nothing to refresh": not an OAuth 2.0
    /// account (a Basic-password or API-token 401 means the stored secret is
    /// wrong, which a re-fetch only reproduces), or no `CamelService` to ask
    /// — a [`JmapStore::detached`] test instance, or a vfunc reached before
    /// Camel finished constructing one.
    fn refresh_credentials(&self, sync: &MailSync) -> bool {
        let Some(service) = self.service() else {
            return false;
        };
        // SAFETY: `service` is a live `CamelService` by the check in
        // `Self::service`; the settings reference is released when it drops.
        let settings = unsafe { Owned::from_raw(camel_service_ref_settings(service)) };
        // SAFETY: `settings` is NULL or the `CamelSettings` just referenced.
        let uses_oauth2 = settings
            .as_ref()
            .is_some_and(|settings| unsafe { oauth2::uses_oauth2(settings.as_ptr()) });
        if !uses_oauth2 {
            return false;
        }

        // SAFETY: `service` is a valid, registered `CamelService` by the
        // check above; the session reference is released when it drops.
        let Some(session) = (unsafe { Owned::from_raw(camel_service_ref_session(service)) }) else {
            return false;
        };
        // SAFETY: a valid session and a valid, registered service; no
        // cancellable — this refresh is not the operation the user asked to
        // stop, and `access_token` accepts NULL for "not cancellable".
        match unsafe { oauth2::access_token(session.as_ptr(), service, ptr::null_mut()) } {
            Ok(token) => {
                tracing::debug!("refreshed the mail connection's OAuth 2.0 access token");
                sync.client().set_credentials(Credentials::bearer(token));
                // Read straight back off the client rather than formatted a
                // second time here, so the header the push stream sends and
                // the one every method call sends cannot drift apart.
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

    /// For the tests only, and specifically for the one thing
    /// [`JmapStore::refresh_credentials`] does that a test can reach without a
    /// `CamelService`: installing fresh credentials on the pooled connection.
    #[cfg(feature = "testing")]
    pub fn inspect_connection<R>(&self, f: impl FnOnce(&MailSync) -> R) -> Option<R> {
        let connection = self.connection()?;
        let guard = read(connection);
        guard.as_ref().map(|sync| f(sync))
    }

    /// The account's folder tree — what `get_folder_info_sync` answers with.
    ///
    /// `flags` is Camel's word verbatim, and the bit that matters here is
    /// `CAMEL_STORE_FOLDER_INFO_REFRESH`: Camel asks a store for its folder
    /// tree constantly, and sets that bit on the few of those calls that mean
    /// "go and look". Without it the listing already in hand is the answer, and
    /// no request is made at all. With it, one `Mailbox/changes` decides
    /// whether the tree has to be walked again — see
    /// [`MailSync::folder_tree_since`], which is where the rule that a mailbox
    /// delta cannot be applied folder by folder lives.
    ///
    /// The first call has nothing in hand and therefore lists whatever the
    /// flags say: an account that opened empty until something asked it to
    /// refresh would be an account with no mail in it.
    ///
    /// The other flags are not read yet. `SUBSCRIBED` and `SUBSCRIPTION_LIST`
    /// ask for the tree filtered to what the user subscribed to, which is a
    /// filter on the tree rather than a different request, and `FAST` asks for
    /// it without message counts, which JMAP includes in the mailbox anyway.
    pub fn folders(
        &self,
        flags: CamelStoreGetFolderInfoFlags,
    ) -> Result<Arc<FolderTree>, StoreError> {
        let (connection, folders) = self
            .connection()
            .zip(self.folder_listing())
            .ok_or(StoreError::Disconnected)?;

        // Held across the request, which is the ordering rule the `folders`
        // field documents: the connection a listing was read over is still ours
        // when the listing is written.
        let connection = read(connection);
        let sync = connection.as_ref().ok_or(StoreError::Disconnected)?;

        tracing::debug!(flags, "fetching folder tree");

        let held = read(folders)
            .as_ref()
            .map(|listing| (listing.state.clone(), Arc::clone(&listing.tree)));

        let listing = match held {
            Some((_, tree)) if flags & CAMEL_STORE_FOLDER_INFO_REFRESH == 0 => return Ok(tree),
            Some((state, tree)) => match retry_once_after(
                || sync.folder_tree_since(&state),
                SyncError::is_unauthorized,
                || self.refresh_credentials(sync),
            ) {
                // The tree is kept, not rebuilt from an equal one: Camel diffs
                // the forests it is handed to decide which folders to announce
                // as created or deleted, and every caller above holds the same
                // `Arc` as before.
                Ok(FolderUpdate::Unchanged(state)) => Listing { state, tree },
                Ok(FolderUpdate::Rebuilt { state, tree }) => Listing {
                    state,
                    tree: Arc::new(tree),
                },
                Err(failure) => {
                    tracing::debug!(flags, ?failure, "fetching folder tree failed");
                    return Err(failure.into());
                }
            },
            None => match retry_once_after(
                || sync.folder_tree(),
                SyncError::is_unauthorized,
                || self.refresh_credentials(sync),
            ) {
                Ok((state, tree)) => Listing {
                    state,
                    tree: Arc::new(tree),
                },
                Err(failure) => {
                    tracing::debug!(flags, ?failure, "fetching folder tree failed");
                    return Err(failure.into());
                }
            },
        };

        let tree = Arc::clone(&listing.tree);
        tracing::debug!(
            flags,
            state = listing.state.as_str(),
            count = listing.tree.len(),
            "fetched folder tree"
        );
        *write(folders) = Some(listing);
        drop(connection);
        Ok(tree)
    }

    /// Every message in one of the account's mailboxes — what a folder's
    /// `refresh_info_sync` fills its summary from.
    ///
    /// It is a method on the store rather than on the folder because the
    /// connection is: a `CamelFolder` holds a mailbox id and a back-pointer to
    /// the store it hangs off, and every request it makes goes out over the
    /// store's client. Nothing is cached here — a listing *is* the folder's
    /// state, and the summary is where it lives.
    ///
    /// The connection is cloned out and the lock released before the request
    /// goes out, unlike [`JmapStore::folders`]: nothing here touches the
    /// `folders` field, so the ordering rule that field's own comment states
    /// does not apply, and a disconnect arriving mid-request may now proceed
    /// without waiting for it — see the `connection` field's own comment. The
    /// request still runs against the connection that was live when it
    /// started; a disconnect that races it does not cancel it, only stops
    /// making it wait.
    ///
    /// The state the listing comes with is the one the *next* refresh asks
    /// [`JmapStore::messages_since`] from; the folder keeps it in its summary,
    /// which is what carries it across a restart.
    ///
    /// This call is what a folder with no state to ask from makes — its first
    /// refresh of a mailbox, and the recovery from a state the server will not
    /// calculate a delta from.
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

    /// What one mailbox looks like now, given the state a folder's summary says
    /// its rows are current as of — the refresh every one after the first
    /// makes.
    ///
    /// On the store and locked exactly like [`JmapStore::messages`], and it is
    /// the same question asked more cheaply: one `Email/changes` where a
    /// listing is one query plus one `Email/get` per page of the whole mailbox.
    /// What comes back is [`MessageUpdate`], whose three answers the caller
    /// dispatches on — the folder is the side that holds the rows, so it is the
    /// only side that can tell a message that moved *into* the mailbox from one
    /// that was sitting in it when its flags changed.
    ///
    /// `held` is how many rows the folder already has, passed through to
    /// `MailSync::messages_since` and used there for one thing: deciding when a
    /// delta has grown so large that listing the mailbox is the cheaper way to
    /// find out what is in it. The folder counts its own summary for free; the
    /// layer below would have to ask the server.
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

    /// Every `Quota` object of the account — what `get_quota_info_sync`
    /// answers a folder's quota question from.
    ///
    /// On the store and locked exactly like [`JmapStore::messages`], and
    /// account-wide rather than per-folder for the reason
    /// [`MailSync::quotas`]'s own doc gives: JMAP quotas are not scoped to a
    /// mailbox, so every folder in the account answers from the same list.
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

    /// The RFC 5322 bytes of one message — what `get_message_sync` will parse.
    ///
    /// On the store for the same reason as [`JmapStore::messages`], and locked
    /// the same way. It takes no mailbox: a JMAP email id identifies the
    /// message in the account, not in a folder, so the folder a Camel uid was
    /// read out of adds nothing to the question. The same message filed in two
    /// mailboxes is one message here, which is what it is on the server.
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

    /// Puts one message's keyword change on the server — the write half of what
    /// a folder synchronises.
    ///
    /// On the store for the same reason as the two reads above, and locked the
    /// same way; it takes no mailbox for the same reason [`JmapStore::
    /// message_source`] does not, because a JMAP email id identifies the message
    /// in the account.
    ///
    /// Read-locked rather than write-locked although it writes: what the lock
    /// guards is the *connection*, and a request is a use of one. Several
    /// folders may synchronise at once, which is what Camel does when Evolution
    /// closes.
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

    /// Files one message into another mailbox — the write behind
    /// `transfer_messages_to_sync`.
    ///
    /// On the store and locked exactly like [`JmapStore::set_keywords`], and
    /// taking no mailbox for the same reason: both of the mailboxes a filing
    /// names are in the [`Filing`], and the message is named by an id that
    /// identifies it in the account rather than in a folder.
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

    /// Makes one message leave a mailbox for good — the write behind
    /// `expunge_sync`.
    ///
    /// On the store and locked exactly like [`JmapStore::file_message`]. It
    /// takes the mailbox as well as the message because that is the whole of
    /// what an expunge is about: the message is named by an id that identifies
    /// it in the account, and the mailbox is the folder the user pressed
    /// Expunge in — which decides whether the message is destroyed or only
    /// unfiled, for the reasons [`MailSync::expunge_message`] gives.
    ///
    /// Read-locked although it destroys, for [`JmapStore::set_keywords`]'s
    /// reason: what the lock guards is the connection, and Evolution empties a
    /// trash while the rest of the account is still refreshing.
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

    /// Puts a message the account does not have into one of its mailboxes —
    /// the write behind `append_message_sync`.
    ///
    /// On the store and locked exactly like [`JmapStore::file_message`], and it
    /// is the one write here that takes a mailbox: `Email/import` names the
    /// message by the blob it uploads rather than by an id the account already
    /// holds, so where it is to be filed is the only thing identifying it.
    ///
    /// Read-locked although it uploads. It is the longest request this store
    /// makes, and holding the connection exclusively for it would stall every
    /// folder refresh in the account for the length of an attachment.
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
        // Cloned into the closure because `import_message` takes the bytes by
        // value (it uploads them as a blob) and a retry needs to send the same
        // ones again — `retry_once_after`'s `attempt` must be safe to run
        // twice, the same requirement every other retried call here meets by
        // taking its arguments as references instead.
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

    /// Says whether the user wants to see a folder — the write behind
    /// `CamelSubscribable`'s `subscribe_folder_sync` and
    /// `unsubscribe_folder_sync`.
    ///
    /// On the store and locked like the writes above, and then it does one
    /// thing none of them does: it edits the folder listing the store is
    /// holding. That is not a cache being kept warm, it is the answer to a
    /// different question. `folder_is_subscribed` is declared by Camel as a
    /// *non-blocking* method — Evolution asks it once per folder while drawing
    /// the tree — so the listing is the only thing that can ever answer it, and
    /// a store that wrote the subscription to the server and left its own
    /// listing saying the opposite would draw the tick straight back on.
    ///
    /// The edit is made through [`Arc::make_mut`], so a caller already walking
    /// the tree it was handed keeps walking the tree it was handed: a
    /// `CamelFolderInfo` forest is copied out of a borrowed tree, and one that
    /// mutated underneath that walk is what this rules out.
    ///
    /// A store with nothing listed yet gains nothing. The alternative would be
    /// a tree assembled from the single mailbox a write happened to name, which
    /// is an account with one folder in it.
    ///
    /// The state the listing is current as of is deliberately left where it
    /// was. The write did move the account on, so the next refresh finds a
    /// change and rebuilds — one listing more than strictly needed. The
    /// alternative is a store inventing a state string the server never handed
    /// it and then asking `Mailbox/changes` from it.
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

        // Only after the server agreed, and while the connection it agreed over
        // is still ours — the ordering rule the `folders` field documents.
        if let Some(folders) = self.folder_listing()
            && let Some(listing) = write(folders).as_mut()
        {
            Arc::make_mut(&mut listing.tree).set_subscribed(mailbox, subscribed);
        }
        tracing::debug!(
            mailbox_id = mailbox.as_str(),
            subscribed,
            "set folder subscription"
        );
        Ok(())
    }

    /// Makes a folder — the write behind `create_folder_sync`.
    ///
    /// Locked like the writes above, and it edits the held listing for
    /// [`JmapStore::set_subscribed`]'s reason turned one step further: Camel
    /// hands the folder this answers with to Evolution's folder tree and then
    /// opens it by path, and opening is answered out of the listing. A store
    /// that made the folder and did not record it would offer the user a folder
    /// it refuses to open until something refreshes the account.
    ///
    /// `parent` is the folder the new one hangs under, whole rather than by id,
    /// because the answer's path is built from the parent's — see
    /// [`MailSync::create_folder`], which is where that happens.
    ///
    /// A store with nothing listed yet gains nothing, and the state the listing
    /// is current as of is left where it was: both are the judgements
    /// [`JmapStore::set_subscribed`] documents, and for the same reasons.
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

        // Only after the server made it, and while the connection it made it
        // over is still ours — the ordering rule the `folders` field documents.
        if let Some(folders) = self.folder_listing()
            && let Some(listing) = write(folders).as_mut()
        {
            Arc::make_mut(&mut listing.tree).insert(created.clone());
        }
        tracing::debug!(folder_id = created.id.as_str(), name, "created mail folder");
        Ok(created)
    }

    /// Removes a folder — the write behind `delete_folder_sync`.
    ///
    /// The mirror of [`JmapStore::create_folder`] in every respect, including
    /// why the listing is edited at all: a folder that is gone from the account
    /// and still in the listing is one Camel will happily open again.
    ///
    /// By mailbox id, like every other write here, because the caller named the
    /// folder out of a listing and the path it had is the part another client's
    /// rename can already have invalidated.
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

        // As above: only after the server agreed.
        if let Some(folders) = self.folder_listing()
            && let Some(listing) = write(folders).as_mut()
        {
            Arc::make_mut(&mut listing.tree).remove(mailbox);
        }
        tracing::debug!(mailbox_id = mailbox.as_str(), "deleted mail folder");
        Ok(())
    }

    /// Renames a folder, and moves it — the write behind `rename_folder_sync`.
    ///
    /// Locked and ordered like the two writes above, and it edits the held
    /// listing for their reason with one more on top: a rename changes the path
    /// of everything *under* the folder as well, and every one of those paths
    /// is a key Camel opens a folder by. A store that renamed the folder and
    /// left its listing alone would answer `get_folder_sync` for the new paths
    /// with nothing and for the old ones with folders the account no longer has.
    ///
    /// `folder` is passed whole rather than by id because both halves are
    /// needed — the id for the write, the folder for the answer when the
    /// listing cannot supply one — and `parent` for the reason
    /// [`JmapStore::create_folder`] takes one: the path of the answer is built
    /// from the parent's.
    ///
    /// The answer is the folder as the listing now has it, subfolders and all,
    /// because that is what `camel_store_folder_renamed` is emitted with and
    /// Evolution walks the children of what it is handed. A store with no
    /// listing — one a disconnect emptied between the lookup and the write —
    /// answers with the folder it was given, at its new path and without the
    /// children whose paths only the tree edit knows. That is one announcement
    /// short of complete rather than wrong, and the account is being listed
    /// again anyway.
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

        // Only after the server agreed, and while the connection it agreed over
        // is still ours — the ordering rule the `folders` field documents.
        let renamed = self.folder_listing().and_then(|folders| {
            let mut folders = write(folders);
            let listing = folders.as_mut()?;
            let tree = Arc::make_mut(&mut listing.tree);
            tree.rename(&folder.id, &path, name);
            tree.find(&path).cloned()
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

    /// The folder listing the store is holding, if it is holding one — and
    /// nothing else: no request, and no connection needed to ask.
    ///
    /// [`JmapStore::folders`] answers the same question for a caller that may
    /// block, and lists the account when it has nothing in hand. This one is
    /// for the caller that may not: `CamelSubscribable`'s
    /// `folder_is_subscribed` is declared non-blocking and Evolution asks it
    /// once per folder while drawing the tree, so a request from in there would
    /// be a folder tree that stalls the UI thread once per row. `None` is
    /// therefore an answer — "nothing is known about this account yet" — and
    /// not a case to go and fix.
    pub fn held_folders(&self) -> Option<Arc<FolderTree>> {
        let folders = self.folder_listing()?;
        let listing = read(folders);
        listing.as_ref().map(|listing| Arc::clone(&listing.tree))
    }

    /// Drops the folder listing. Called with the connection lock held, by the
    /// two operations that make a listing stop describing the account the store
    /// is pointed at.
    fn forget_folders(&self) {
        if let Some(folders) = self.folder_listing() {
            *write(folders) = None;
        }
    }

    /// An instance outside the GObject type system: zeroed parent bytes and
    /// initialised slots, which is what `instance_init` leaves behind minus the
    /// GObject.
    ///
    /// This exists for the tests, and it is not a shortcut — Camel constructs a
    /// store through `camel_session_add_service`, which needs a `CamelSession`,
    /// which in Evolution is an `EMailSession` over a source registry on the
    /// session bus. Nothing but the slots may be touched through the result: the
    /// parent bytes are a valid bit pattern (every field is a pointer or an
    /// integer, and NULL is a pointer) but they are not a GObject, so passing
    /// one to any Camel function is undefined behaviour.
    #[cfg(feature = "testing")]
    pub fn detached() -> Box<Self> {
        // SAFETY: every field of the parent is a pointer or an integer, for
        // which all-zero is a valid value, and an all-zero `Slot` is its
        // documented empty state.
        let store: Box<Self> = unsafe { zeroed_box() };
        store.connection.init(RwLock::new(None));
        store.folders.init(RwLock::new(None));
        store.push.init(Mutex::new(None));
        store.refresher.init(Mutex::new(None));
        store
    }

    /// The Rust view of a `CamelStore *` Camel handed over.
    ///
    /// # Safety
    ///
    /// `store` must be NULL or point at an instance of this type. Camel only
    /// dispatches a class's vfuncs on instances of that class, so a vfunc's
    /// argument satisfies this; anything else has to check with
    /// `G_TYPE_CHECK_INSTANCE_TYPE` first.
    pub unsafe fn borrow<'a>(store: *mut CamelStore) -> Option<&'a Self> {
        // SAFETY: the doc comment above states the same contract.
        unsafe { dispatched_borrow(store) }
    }

    /// The connection slot, or `None` on an instance whose `instance_init` has
    /// not run or whose `finalize` already has.
    fn connection(&self) -> Option<&RwLock<Option<Arc<MailSync>>>> {
        self.connection.get()
    }

    /// The folder listing slot, with the same caveat.
    fn folder_listing(&self) -> Option<&RwLock<Option<Listing>>> {
        self.folders.get()
    }
}

/// The store as the service [`crate::service`]'s vfuncs connect.
///
/// Three methods that forward to the inherent ones above, because what the
/// vfuncs need is the part the store has in common with the transport and the
/// store's own callers need the part it does not: a `folders` that a
/// [`JmapStore::store_connection`] clears is a store concern, and one the trait
/// has no business naming.
///
// SAFETY: `JmapStore` is the instance struct of the type `store_type`
// registers, and that type derives from `CamelOfflineStore` — from
// `CamelStore`, from `CamelService`.
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

/// A poisoned lock means some other operation panicked while holding it. What
/// it guards is not damaged by that — a `MailSync` is an HTTP client and an
/// account id, a `Listing` is a tree and a state string — so carrying on is
/// better than taking the store down with whatever already went wrong.
///
/// `pub(crate)` for [`crate::transport`], which holds a connection of its own
/// under the same rule. Two copies of three lines would be two places for that
/// judgement to be made differently.
pub(crate) fn read<T>(lock: &RwLock<T>) -> RwLockReadGuard<'_, T> {
    lock.read().unwrap_or_else(PoisonError::into_inner)
}

pub(crate) fn write<T>(lock: &RwLock<T>) -> RwLockWriteGuard<'_, T> {
    lock.write().unwrap_or_else(PoisonError::into_inner)
}

/// The same rule for the two push slots, which are `Mutex` rather than
/// `RwLock` because nothing reads either on a hot path.
fn push_lock(slot: &Mutex<Option<PushRefresh>>) -> MutexGuard<'_, Option<PushRefresh>> {
    slot.lock().unwrap_or_else(PoisonError::into_inner)
}

fn refresher_lock(slot: &Mutex<Option<FolderRefresh>>) -> MutexGuard<'_, Option<FolderRefresh>> {
    slot.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The class struct, same rule one level up. It will grow overrides of
/// `CamelStoreClass`'s folder vfuncs; today it adds nothing of its own, which
/// is still not the same as *being* `CamelOfflineStoreClass` — the type needs
/// its own class for the overrides to have somewhere to go.
#[repr(C)]
pub struct JmapStoreClass {
    parent_class: CamelOfflineStoreClass,
}

// SAFETY: both structs are #[repr(C)] and lead with the CamelOfflineStore
// instance and class structs, whose layouts eds-sys's tests/layout.rs checks
// against `g_type_query`; CamelOfflineStore derives from CamelStore, from
// CamelService, from GObject.
unsafe impl ObjectSubclass for JmapStore {
    /// `CamelJmapStore`, not `JmapStore`: Camel's own stores are all
    /// `Camel<Protocol>Store`, and the type name is what a user sees in a
    /// GObject warning about the wrong store type.
    const NAME: &'static CStr = c"CamelJmapStore";
    type Instance = JmapStore;
    type Class = JmapStoreClass;

    fn parent_type() -> GType {
        // SAFETY: no arguments, and the type initialises itself.
        unsafe { camel_offline_store_get_type() }
    }

    fn interfaces() -> Vec<InterfaceDecl> {
        // `CamelSubscribable` is three vfuncs and no properties, and Camel puts
        // no default behind any of them — so unlike the settings class's
        // `CamelNetworkSettings`, claiming this one without filling its vtable
        // would be a store whose subscription methods are calls through NULL.
        // [`crate::subscribe`] is what fills it.
        vec![InterfaceDecl::filled_by::<Subscribable>()]
    }

    fn class_init_types() -> Vec<GType> {
        // The settings type below, registered before this one rather than from
        // inside the class initialiser: `class_init` runs under GLib's
        // class-initialisation lock and cannot take the registration lock
        // without inverting the two.
        vec![settings_type()]
    }

    unsafe fn class_init(class: *mut Self::Class) {
        // Which class `camel_service_ref_settings` instantiates when nothing
        // has handed the service a settings object — and, more to the point,
        // which class Evolution's account editor and
        // `e_source_camel_configure_service` expect to configure. Inherited it
        // would be `CamelOfflineSettings`, which carries no server at all, so
        // this one line is what connects a JMAP account to a host.
        //
        // SAFETY: the class leads with CamelOfflineStoreClass, which derives
        // from CamelStoreClass, from CamelServiceClass — the contract above.
        let service = class.cast::<CamelServiceClass>();
        unsafe { (*service).settings_type = settings_type() };

        // Connect, authenticate, disconnect. They live in `crate::service`
        // rather than here because what they do is one operation split across
        // three slots by Camel's re-prompt loop, and reads as one file.
        // SAFETY: as above.
        unsafe { crate::service::install_vfuncs::<Self>(service) };

        // And the folder listing. `CamelStore` leaves `get_folder_info_sync`
        // NULL and `camel_store_get_folder_info_sync` refuses to call a store
        // that has not filled it in, so this line is the difference between an
        // account with folders and one with a runtime warning.
        //
        // SAFETY: the class leads with CamelOfflineStoreClass, which leads with
        // CamelStoreClass — the contract above.
        unsafe { crate::folders::install_vfuncs(class.cast::<CamelStoreClass>()) };

        // And the two that change which folders there are. Their own module
        // rather than `crate::folders`, which answers questions about the
        // account's folders where these two are what gains and loses one.
        //
        // SAFETY: as above.
        unsafe { crate::manage::install_vfuncs(class.cast::<CamelStoreClass>()) };
    }

    unsafe fn instance_init(instance: *mut Self::Instance) {
        // SAFETY: `instance` points at a zeroed instance struct of ours, and a
        // zeroed `Slot` is an empty one.
        unsafe {
            (*instance).connection.init(RwLock::new(None));
            (*instance).folders.init(RwLock::new(None));
            (*instance).push.init(Mutex::new(None));
            (*instance).refresher.init(Mutex::new(None));
        };

        // And what a JMAP account's trash and junk are: mailboxes of the
        // account, not searches over local flags. `CamelStore` starts every
        // store with `CAMEL_STORE_VTRASH | CAMEL_STORE_VJUNK`, which makes
        // `camel_store_get_folder_info_sync` append `.#evolution/Trash` and
        // `.#evolution/Junk` to every listing the store answers with — so a
        // provider that left them on and overrode the two getters (see
        // [`crate::folders`]) would show the user two trash folders and two junk
        // folders, one pair real and one pair a view of flags no other client
        // shares. Cleared here rather than per account, because it is a fact
        // about the protocol and not a setting: RFC 8621 gives a mailbox a role.
        //
        // Every other bit is left exactly as Camel set it — `CAN_EDIT_FOLDERS`
        // in particular, which [`crate::manage`] earns.
        //
        // SAFETY: the parent's `instance_init` has already run — GObject
        // initialises base to derived — so this is a `CamelStore` whose private
        // data exists, which is all either call touches.
        unsafe {
            let store = instance.cast::<CamelStore>();
            let flags = camel_store_get_flags(store) & !(CAMEL_STORE_VTRASH | CAMEL_STORE_VJUNK);
            camel_store_set_flags(store, flags);
        };
    }

    unsafe fn finalize(instance: *mut Self::Instance) {
        // Before the connection, and before anything else here: the push slot
        // holds a thread that can call back into this instance, and clearing
        // it stops and joins that thread. Camel does not promise a
        // `disconnect_sync` before it drops a store — a service that never
        // connected is finalized without one — so this, not that, is what
        // guarantees the pump is gone.
        //
        // SAFETY: the instance is being finalized, so nothing can still reach
        // it and no borrow handed out by `get` is alive.
        unsafe {
            (*instance).push.clear();
            (*instance).refresher.clear();
        };
        // SAFETY: as above. Without this the connection — and its socket —
        // outlives the account, and the folder listing leaks with it.
        unsafe {
            (*instance).connection.clear();
            (*instance).folders.clear();
        };
    }
}

/// Registers the store type, or returns it if it is already registered.
///
/// Statically, unlike the EDS backends' types: a Camel provider is not a
/// `GTypeModule`. Camel dlopens the module and never closes it, and the
/// provider struct it keeps a pointer to names these `GType`s forever, so there
/// is no unload for a dynamic type to be unregistered by — and a type that
/// *could* be unloaded here would be one Camel could still be asked to
/// instantiate.
pub fn store_type() -> GType {
    register_static::<JmapStore>()
}
