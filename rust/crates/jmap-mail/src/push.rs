// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! The mail half of JMAP Push: it changes a `StateChange` from the server into
//! the folder refresh that Camel already does.
//!
//! [`jmap_backend_core::push`] is the half that does not depend on the account
//! type. This module uses it without changes. That half owns the EventSource
//! subscription, decides which pushes are for this account, and calls an
//! injected C function on the backend object. This module supplies a
//! different kind of function.
//!
//! ## Why the address book design does not apply
//!
//! The EDS backends give a push directly to
//! `e_book_meta_backend_schedule_refresh`, which returns immediately. EDS
//! queues a custom operation, merges a second call into the running one, and
//! calls `get_changes_sync` on a separate thread. **Camel has no equivalent
//! call.** Its refresh entry point is `camel_folder_refresh_info_sync`. That
//! function is synchronous, works on one folder, and makes a network round
//! trip.
//!
//! evolution-ews has the same problem, and this module follows its solution.
//! In `src/EWS/camel/camel-ews-store.c`,
//! `camel_ews_store_server_notification_cb` does *not* refresh on the
//! notification thread. It sorts the notification. Then
//! `schedule_folder_update` merges the request into a low-priority one-second
//! timeout. It removes a pending timeout first, so a burst gives one update.
//! `run_update_thread` then starts a **detached** `GThread`. That thread calls
//! `camel_store_get_folder_sync` and `camel_folder_refresh_info_sync` for each
//! affected folder.
//!
//! Both parts are necessary here, for a reason that the EDS side does not
//! have. [`PushRefresh::stop`](jmap_backend_core::push::PushRefresh::stop)
//! *joins* its pump thread. A refresh in the callback of the pump would make
//! `disconnect_sync`, and thus the shutdown of Evolution, wait for one
//! network round trip per open folder. That round trip cannot be interrupted.
//! Thus:
//!
//! - the refresh runs on a separate thread, never on the pump, so `stop`
//!   returns quickly; and
//! - [`FolderRefresh`] merges requests as the EWS timeout does, but with
//!   book-keeping, not with a clock. While a pass runs, more pushes set a
//!   flag. The flag makes the running worker do **exactly one** more pass.
//!   Zero passes is not sufficient: a change that arrives while a pass reads
//!   the server can be missing from what that pass read. One pass per push is
//!   too many: the server also pushes each `Email/set` of this client back
//!   to it, so the writes of the client would cause a refresh storm.
//!
//! The worker gets the store through a
//! [`WeakBackend`](jmap_backend_core::weak::WeakBackend). If Camel releases
//! the store between two passes, the worker stops and does not touch it. A
//! store cannot be released *during* a pass, because the worker holds the
//! strong reference for the full pass.
//!
//! ## Which folders, and which types
//!
//! An RFC 8620 §7.1 `StateChange` names types per *account*. It names no
//! mailbox, so unlike the EWS `folder_id`, it gives nothing to narrow to.
//! [`refresh_open_folders`] refreshes the result of
//! `camel_store_dup_opened_folders`. These are the folders that something
//! holds a `CamelFolder` for. A refresh can change only their summaries.
//! Camel lists a folder that nobody has opened when somebody opens it.
//!
//! [`PUSHED_TYPES`] is the message half. The folder-list half is a `Mailbox`
//! change, but not each such push changes the folder list. RFC 8621 puts the
//! `totalEmails` and `unreadEmails` of a mailbox on the `Mailbox` object. Thus
//! a delivery changes the type state as visibly as a folder that is created,
//! destroyed, renamed, moved, subscribed or unsubscribed. The folder *tree* of
//! Camel must know only about the second kind, because the refresh of the open
//! folder already gives the counts to Camel. Thus
//! [`refresh_folder_list_if_structural`] gets the tree again and compares it
//! with [`FolderTree::change_to`](jmap_mail_sync::FolderTree::change_to):
//!
//! - New folders, destroyed folders and subscription changes get one signal
//!   for each folder (`folder-created`, `folder-deleted`, `folder-subscribed`,
//!   `folder-unsubscribed`). The other rows of the Evolution folder tree stay
//!   as they are, and the folder that the user has open stays selected.
//! - A rename, a move or a new role calls `camel_store_folder_info_stale`.
//!   Evolution then builds the full tree of the account again, and the
//!   selection is lost. The old paths of the descendants are not known, so
//!   `folder-renamed` cannot replace it.
//!
//! The new listing is a network round trip. Thus it runs on the worker of
//! [`FolderRefresh`] with the message refresh, not on the pump thread, for the
//! same reason.
//!
//! To tell a `Mailbox` push from an `Email` or `EmailDelivery` push, this
//! module uses [`jmap_backend_core::push::start_for_with`]. That function gives
//! the matched JMAP types to the action. The bare fn pointer of
//! [`jmap_backend_core::push::start_for`] does not get them. [`dispatch`] is
//! that action, and [`Work`] holds the two kinds of pass that it can request.
//! Thus [`PUSHED_TYPES`] does not name each type that the subscription
//! requests. See [`start_push`](crate::store::JmapStore) for the full list.

use std::ptr;
use std::slice;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::thread;

use eds_sys::camel_store_dup_opened_folders;
use eds_sys::{
    CamelFolder, CamelStore, CamelSubscribable, camel_folder_refresh_info_sync,
    camel_store_folder_created, camel_store_folder_deleted, camel_store_folder_info_stale,
    camel_subscribable_folder_subscribed, camel_subscribable_folder_unsubscribed,
};
use glib_sys::g_ptr_array_free;
use glib_sys::{GError, GFALSE, GTRUE, g_clear_error};
use gobject_sys::GObject;
use jmap_backend_core::marshal::read_string;
use jmap_backend_core::owned::Owned;
use jmap_mail_sync::{FolderDelta, FolderListChange};

use crate::folder_info::FolderInfoChain;
use crate::store::JmapStore;

/// The JMAP data types that request a message refresh, which is a pass
/// through [`FolderRefresh`].
///
/// `EmailDelivery` is the delivery-only pseudo-type of RFC 8621 §5. Its state
/// changes when mail *arrives*, not when a flag changes. A mail client must
/// react to this push most of all. `Email` covers the other changes, for
/// example another client that marks a message as read or moves it. A refresh
/// must also get these. Of the two, `jmap-mock` tracks only `Email`, so the
/// tests use only `Email`. A real server needs both.
pub const PUSHED_TYPES: &[&str] = &["Email", "EmailDelivery"];

/// The JMAP data type that can change the folder *list* of Camel, not with
/// the message pass of [`FolderRefresh`]. Most `Mailbox` pushes only change
/// the counts of a delivery, so most do not. See the module docs and
/// [`refresh_folder_list_if_structural`] for which pushes do.
pub const FOLDER_LIST_TYPES: &[&str] = &["Mailbox"];

/// The work that a merged pass on the worker of [`FolderRefresh`] must do.
/// One `StateChange` can request one kind, both kinds or neither. For
/// neither, no pass starts. See [`Actions`] for the source of these values.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Work {
    /// Refresh the messages of each open folder, with [`refresh_open_folders`].
    pub refresh_messages: bool,
    /// Check if the folder list changed shape, with
    /// [`refresh_folder_list_if_structural`].
    pub check_folder_list: bool,
}

impl Work {
    fn is_empty(self) -> bool {
        !self.refresh_messages && !self.check_folder_list
    }

    fn union(self, other: Self) -> Self {
        Self {
            refresh_messages: self.refresh_messages || other.refresh_messages,
            check_folder_list: self.check_folder_list || other.check_folder_list,
        }
    }
}

/// The merging worker that the module docs describe. It does the work of
/// `schedule_folder_update` and `run_update_thread` in EWS, as one object.
///
/// Any thread can drop it at low cost. A drop does not cancel or join a
/// running pass. This is necessary, because the caller that drops it is
/// `disconnect_sync` or `finalize`, and these must not block. A running pass
/// holds its own reference to the shared state and its own strong reference
/// to the store. Thus it completes on a consistent store and then stops.
pub struct FolderRefresh {
    inner: Arc<Inner>,
}

/// The state that a worker thread shares with the [`FolderRefresh`] that
/// started it.
struct Inner {
    /// Gives the worker the store after the push callback that started it
    /// returns.
    weak: WeakStore,
    runs: Mutex<Runs>,
    /// One pass, which gets the [`Work`] that it must do. It is injected, so
    /// the tests in this module can test the merge on a plain `GObject`,
    /// without a Camel store, a `CamelSession` or a network.
    pass: Box<dyn Fn(*mut GObject, Work) + Send + Sync>,
}

/// Whether a pass runs, and which more [`Work`] pushes requested after it
/// started.
///
/// `pending` collects a union, not a count. One next pass answers all pushes
/// that arrive during a pass. That pass reads the current state of the
/// server, not a queue of deltas. Thus only the union of the requests is
/// necessary.
#[derive(Default)]
struct Runs {
    running: bool,
    pending: Work,
}

impl FolderRefresh {
    /// Prepares to refresh the open folders of `store` on request. `pass` is
    /// one refresh of all that its [`Work`] requests. It gets called while a
    /// strong reference to `store` is held.
    ///
    /// # Safety
    ///
    /// `store` must be a valid GObject with a strong reference held by the
    /// caller for the length of this call, and `pass` must accept a pointer
    /// to that object's actual type.
    pub unsafe fn new(
        store: *mut GObject,
        pass: impl Fn(*mut GObject, Work) + Send + Sync + 'static,
    ) -> Self {
        // SAFETY: `store` is valid and referenced, by this function's
        // contract.
        let weak = WeakStore(unsafe { jmap_backend_core::weak::WeakBackend::new(store) });
        Self {
            inner: Arc::new(Inner {
                weak,
                runs: Mutex::new(Runs::default()),
                pass: Box::new(pass),
            }),
        }
    }

    /// Requests a refresh that does at least `work`, and returns immediately.
    ///
    /// Starts a worker if no worker runs. Otherwise adds `work` to the next
    /// pass of the running worker. Empty `work` does nothing, so a caller need
    /// not check it first. It never blocks on a pass. It holds the lock only
    /// for the book-keeping. Thus the pump thread of
    /// [`jmap_backend_core::push`] can safely call it.
    pub fn request(&self, work: Work) {
        if work.is_empty() {
            return;
        }
        {
            let mut runs = lock(&self.inner.runs);
            if runs.running {
                runs.pending = runs.pending.union(work);
                tracing::debug!(
                    ?work,
                    "a folder refresh is already running; coalescing into it"
                );
                return;
            }
            runs.running = true;
        }
        let inner = Arc::clone(&self.inner);
        // Detached, as the EWS update thread is. Nothing joins it, and nothing
        // must join it (see the module docs).
        thread::spawn(move || inner.run(work));
    }

    /// Whether a pass runs or is pending. The tests have no other way to see
    /// the book-keeping of a detached thread.
    #[cfg(test)]
    fn busy(&self) -> bool {
        let runs = lock(&self.inner.runs);
        runs.running
    }
}

impl Inner {
    /// The full life of one worker: a pass, then another pass or the end.
    ///
    /// This function catches a panic in a pass, so the panic does not end the
    /// thread. The thread is detached and nothing waits for it, so the reason
    /// is the book-keeping. An unwind from here leaves `running` set, and
    /// nothing clears it. [`FolderRefresh::request`] then merges each request
    /// into a worker that does not exist. This silently disables push for the
    /// remaining life of the account. A failure that is rare, silent and
    /// permanent justifies a `catch_unwind`.
    fn run(&self, mut work: Work) {
        loop {
            let reached = jmap_backend_core::trampoline::guard(
                "push folder refresh",
                // A pass that panicked counts as a pass that ran. The store was
                // there, and the changes to the folders that it reached stay.
                // The worker must not try it again without end.
                Some(()),
                || self.weak.with_strong(|store| (self.pass)(store, work)),
            );
            if reached.is_none() {
                // Camel released the store between two passes. Nothing remains
                // to refresh or to give the book-keeping to. Clear it anyway:
                // if a `Drop` order keeps this `Inner` alive, it must not look
                // busy permanently.
                *lock(&self.runs) = Runs::default();
                tracing::debug!("the mail store went away; stopping its folder refresh");
                return;
            }
            let mut runs = lock(&self.runs);
            if !runs.pending.is_empty() {
                work = runs.pending;
                runs.pending = Work::default();
                continue;
            }
            runs.running = false;
            return;
        }
    }
}

/// A `WeakBackend` with a name that tells what it points to here.
struct WeakStore(jmap_backend_core::weak::WeakBackend);

impl WeakStore {
    fn with_strong<R>(&self, f: impl FnOnce(*mut GObject) -> R) -> Option<R> {
        self.0.with_strong(f)
    }
}

/// One refresh pass: the server lists again each folder that the store has
/// open now.
///
/// This function logs and skips errors and does not propagate them. After an
/// error, it continues with the next folder. Push only makes updates faster.
/// The periodic refresh of Camel gets to a folder that this pass could not
/// refresh, so it is not an account failure. Nothing waits for a result
/// from here.
///
/// This function gives no `GCancellable`. It has none, because a server push
/// starts it, not an operation that the user can cancel.
/// `camel_folder_refresh_info_sync` accepts NULL for this case.
///
/// # Safety
///
/// `store` must be a live `CamelStore` with a strong reference held for the
/// length of this call. [`FolderRefresh`] holds this reference for a pass.
pub unsafe fn refresh_open_folders(store: *mut CamelStore) {
    // SAFETY: a live `CamelStore` by the contract of this function. The call
    // transfers the array and a reference to each folder in it to this code.
    // The code below frees them as `camel_store_dup_opened_folders` documents.
    let folders = unsafe { camel_store_dup_opened_folders(store) };
    if folders.is_null() {
        return;
    }
    // SAFETY: this code now owns this `GPtrArray`, so both fields are readable.
    let count = unsafe { (*folders).len } as usize;
    tracing::debug!(
        count,
        "refreshing the mail store's open folders after a push"
    );
    for index in 0..count {
        // SAFETY: `index` is less than the length of the array. Each element
        // of this array is a `CamelFolder *` (`transfer full`). The reference
        // moves into `folder`, and its drop releases it, not a manual
        // `g_object_unref` at the end of the loop.
        let folder =
            unsafe { Owned::from_raw((*(*folders).pdata.add(index)).cast::<CamelFolder>()) };
        let Some(folder) = folder else { continue };
        let mut error: *mut GError = ptr::null_mut();
        // SAFETY: a live folder from the array, no cancellable (see above),
        // and a valid location for an error.
        let refreshed = unsafe {
            camel_folder_refresh_info_sync(folder.as_ptr(), ptr::null_mut(), &raw mut error)
        };
        if refreshed == GFALSE {
            // SAFETY: `error` is NULL or a `GError` that this call now owns.
            // Its `message` is a NUL-terminated string that it owns.
            let message =
                unsafe { error.as_ref() }.and_then(|error| unsafe { read_string(error.message) });
            tracing::debug!(
                ?message,
                "a pushed folder refresh failed; leaving it to polling"
            );
            // SAFETY: `error` is NULL or an owned `GError`; cleared once.
            unsafe { g_clear_error(&raw mut error) };
        }
    }
    // SAFETY: this frees the array itself. The loop already released its
    // elements. `TRUE` frees the backing storage, which this code now owns.
    unsafe { g_ptr_array_free(folders, GTRUE) };
}

/// The folder-list half of [`Work`]. It gets the tree again and tells Camel
/// what changed (see the module docs).
///
/// This is a network round trip ([`crate::store::JmapStore::folders`], with
/// `Mailbox/changes`). Thus it runs on the worker of [`FolderRefresh`], never
/// on the pump thread, for the same reason as [`refresh_open_folders`].
///
/// # Safety
///
/// `store` must be a live [`JmapStore`], under the same contract as
/// [`dispatch`].
pub unsafe fn refresh_folder_list_if_structural(store: *mut CamelStore) {
    // SAFETY: contract above.
    let Some(jmap_store) = (unsafe { JmapStore::borrow(store) }) else {
        return;
    };
    match jmap_store.pushed_folder_list_change() {
        FolderListChange::Same => {}
        FolderListChange::Delta(delta) => {
            tracing::debug!(
                destroyed = delta.destroyed.len(),
                created = delta.created.len(),
                resubscribed = delta.resubscribed.len(),
                "a pushed Mailbox change added or removed folders; announcing each"
            );
            // SAFETY: as below.
            unsafe { announce(store, &delta) };
        }
        FolderListChange::Reshaped => {
            tracing::debug!(
                "a pushed Mailbox change was structural; marking the folder list stale"
            );
            // SAFETY: `store` is a live `CamelStore`, by the contract of this
            // function. `camel_store_folder_info_stale` only requires
            // `CAMEL_IS_STORE(store)`, and a live `JmapStore` satisfies it.
            unsafe { camel_store_folder_info_stale(store) };
        }
    }
}

/// Sends one signal for each folder of `delta`, as the vfuncs of
/// [`crate::manage`] and [`crate::subscribe`] do for a change on this
/// computer. The Evolution folder tree of a subscribable store adds and
/// removes rows only on `folder-subscribed` and `folder-unsubscribed`.
///
/// # Safety
///
/// `store` must be a live [`JmapStore`].
unsafe fn announce(store: *mut CamelStore, delta: &FolderDelta) {
    let subscribable = store.cast::<CamelSubscribable>();
    // SAFETY: `store` is live and implements `CamelSubscribable`. Each
    // emitter queues its signal on the session and copies the chain, so the
    // chain must live only for the call.
    unsafe {
        for folder in &delta.destroyed {
            let chain = FolderInfoChain::from_forest(slice::from_ref(folder), Some(0));
            if folder.subscribed {
                camel_subscribable_folder_unsubscribed(subscribable, chain.as_ptr());
            }
            camel_store_folder_deleted(store, chain.as_ptr());
        }
        for folder in &delta.created {
            let chain = FolderInfoChain::from_forest(slice::from_ref(folder), Some(0));
            camel_store_folder_created(store, chain.as_ptr());
            if folder.subscribed {
                camel_subscribable_folder_subscribed(subscribable, chain.as_ptr());
            }
        }
        for folder in &delta.resubscribed {
            let chain = FolderInfoChain::from_forest(slice::from_ref(folder), Some(0));
            if folder.subscribed {
                camel_subscribable_folder_subscribed(subscribable, chain.as_ptr());
            } else {
                camel_subscribable_folder_unsubscribed(subscribable, chain.as_ptr());
            }
        }
    }
}

/// The Camel half of a push. It gets the pushed JMAP types that changed.
/// [`jmap_backend_core::push::start_for_with`] calls this action. One
/// `StateChange` can name a message change and a folder-list change together,
/// and the two request different [`Work`] from the worker.
///
/// This function cannot keep the merge state itself. It finds that state
/// with the other store state, on the instance that the push started for.
///
/// # Safety
///
/// `object` must be a live [`JmapStore`].
/// [`jmap_backend_core::push::start_for_with`] guarantees this: it calls this
/// function only under a strong reference from a `GWeakRef` on the instance
/// that it got.
pub unsafe fn dispatch(object: *mut GObject, types: &[String]) {
    let actions = actions_for(types);
    let work = Work {
        refresh_messages: actions.request_message_refresh,
        check_folder_list: actions.mark_folder_list_stale,
    };
    // A panic here must not stop the pump thread. This is the same reason
    // that `catch_unwind` wraps the pass in `Inner::run`.
    jmap_backend_core::trampoline::guard("push dispatch", (), || {
        if !work.is_empty()
            // SAFETY: a live instance of the store type of this crate, by the
            // contract of this function.
            && let Some(store) = unsafe { JmapStore::borrow(object.cast::<CamelStore>()) }
        {
            store.request_folder_refresh(work);
        }
    });
}

/// The Camel calls that the matched `types` of a pushed `StateChange`
/// request. This is the decision half of [`dispatch`]. It is pure and
/// separate, so tests do not need a live `CamelStore` or `JmapStore` GObject.
/// The test environment of this crate cannot make either (see the tests in
/// this module and the doc comment of `tests/push.rs`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Actions {
    /// A `Mailbox` change: the folder list can have changed.
    mark_folder_list_stale: bool,
    /// An `Email` or `EmailDelivery` change: the messages of an open folder
    /// can have changed.
    request_message_refresh: bool,
}

fn actions_for(types: &[String]) -> Actions {
    Actions {
        mark_folder_list_stale: types
            .iter()
            .any(|name| FOLDER_LIST_TYPES.contains(&name.as_str())),
        request_message_refresh: types
            .iter()
            .any(|name| PUSHED_TYPES.contains(&name.as_str())),
    }
}

/// A poisoned lock means that a worker panicked during a pass. The lock
/// guards two booleans, and a panic cannot damage them. Thus the book-keeping
/// continues and does not stop the account after the first failure. The locks
/// of [`crate::store`] follow the same rule.
fn lock(runs: &Mutex<Runs>) -> MutexGuard<'_, Runs> {
    runs.lock().unwrap_or_else(PoisonError::into_inner)
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::mpsc::{Receiver, Sender, channel};
    use std::time::{Duration, Instant};

    use gobject_sys::{G_TYPE_OBJECT, g_object_new_with_properties, g_object_unref};

    use super::*;

    fn strings(names: &[&str]) -> Vec<String> {
        names.iter().map(|name| (*name).to_owned()).collect()
    }

    /// The main property of the split: a `Mailbox` change makes Camel mark the
    /// folder list stale, and does nothing else.
    #[test]
    fn a_mailbox_change_marks_the_folder_list_stale_only() {
        assert_eq!(
            actions_for(&strings(&["Mailbox"])),
            Actions {
                mark_folder_list_stale: true,
                request_message_refresh: false,
            }
        );
    }

    /// An `Email` or `EmailDelivery` change requests a message refresh, and
    /// does not change the folder list.
    #[test]
    fn a_message_change_requests_a_refresh_only() {
        for kind in ["Email", "EmailDelivery"] {
            assert_eq!(
                actions_for(&strings(&[kind])),
                Actions {
                    mark_folder_list_stale: false,
                    request_message_refresh: true,
                },
                "{kind} must request a message refresh and nothing else"
            );
        }
    }

    /// One push can name both kinds. For this reason, `dispatch` must get the
    /// matched types, not only a bare fn pointer.
    #[test]
    fn a_push_naming_both_kinds_asks_for_both_actions() {
        assert_eq!(
            actions_for(&strings(&["Mailbox", "Email"])),
            Actions {
                mark_folder_list_stale: true,
                request_message_refresh: true,
            }
        );
    }

    /// A type that this module does not watch requests nothing. In practice
    /// this does not occur, because `start_push` makes the subscription match
    /// only `PUSHED_TYPES` and `FOLDER_LIST_TYPES`.
    #[test]
    fn an_unwatched_type_asks_for_nothing() {
        assert_eq!(
            actions_for(&strings(&["ContactCard"])),
            Actions {
                mark_folder_list_stale: false,
                request_message_refresh: false,
            }
        );
    }

    /// A bare `GObject` for the weak reference. [`FolderRefresh`] does not
    /// require a specific object type. A real `CamelJmapStore` needs a
    /// `CamelSession` with a source registry on the session bus, and no test
    /// environment here has one.
    fn plain_object() -> *mut GObject {
        // SAFETY: this sets no properties, so the count is zero and both
        // arrays can be NULL.
        unsafe { g_object_new_with_properties(G_TYPE_OBJECT, 0, ptr::null_mut(), ptr::null()) }
    }

    /// A pass that reports its start and then blocks until the test releases
    /// it. Thus the test controls the overlap that the merge handles, without
    /// a race.
    struct Gate {
        started: Receiver<()>,
        release: Sender<()>,
        passes: Arc<AtomicUsize>,
    }

    /// The `Work` that these tests request when the kind is not important.
    /// The merge that they test is the same for each kind.
    const ANY_WORK: Work = Work {
        refresh_messages: true,
        check_folder_list: false,
    };

    fn gated() -> (Gate, impl Fn(*mut GObject, Work) + Send + Sync + 'static) {
        let (started_tx, started) = channel();
        let (release, held) = channel::<()>();
        let held = Mutex::new(held);
        let passes = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&passes);
        let pass = move |_: *mut GObject, _: Work| {
            counter.fetch_add(1, Ordering::SeqCst);
            started_tx.send(()).expect("the test is still listening");
            held.lock()
                .expect("only one pass runs at a time")
                .recv()
                .expect("the test releases every pass it starts");
        };
        (
            Gate {
                started,
                release,
                passes,
            },
            pass,
        )
    }

    impl Gate {
        fn await_start(&self) {
            self.started
                .recv_timeout(Duration::from_secs(5))
                .expect("a pass must start");
        }

        fn release_one(&self) {
            self.release.send(()).expect("a pass is waiting");
        }

        fn passes(&self) -> usize {
            self.passes.load(Ordering::SeqCst)
        }
    }

    /// The main property of this module: N pushes that arrive during one pass
    /// cause exactly one more pass, not N passes and not zero.
    #[test]
    fn pushes_arriving_during_a_pass_coalesce_into_exactly_one_more() {
        let object = plain_object();
        let (gate, pass) = gated();
        // SAFETY: a new object, and this test holds one reference.
        let refresh = unsafe { FolderRefresh::new(object, pass) };

        refresh.request(ANY_WORK);
        gate.await_start();
        assert_eq!(gate.passes(), 1, "the first request runs a pass");

        // Three more pushes, while the first pass is still blocked.
        refresh.request(ANY_WORK);
        refresh.request(ANY_WORK);
        refresh.request(ANY_WORK);
        assert_eq!(
            gate.passes(),
            1,
            "no request during a pass may start a second one"
        );

        gate.release_one();
        gate.await_start();
        assert_eq!(gate.passes(), 2, "the three coalesce into one further pass");

        gate.release_one();
        let deadline = Instant::now() + Duration::from_secs(5);
        while refresh.busy() {
            assert!(
                Instant::now() < deadline,
                "the worker must stop once nothing more is owed"
            );
            thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(gate.passes(), 2, "and no further pass runs unasked");

        // SAFETY: this releases the last reference, which this test owns.
        unsafe { g_object_unref(object) };
    }

    /// Each request that does not overlap gets its own pass. The merge must
    /// not lose a push that arrives after the last pass completes.
    #[test]
    fn requests_that_do_not_overlap_each_run_a_pass() {
        let object = plain_object();
        let (gate, pass) = gated();
        // SAFETY: a new object, and this test holds one reference.
        let refresh = unsafe { FolderRefresh::new(object, pass) };

        for expected in 1..=3 {
            refresh.request(ANY_WORK);
            gate.await_start();
            assert_eq!(gate.passes(), expected);
            gate.release_one();
            let deadline = Instant::now() + Duration::from_secs(5);
            while refresh.busy() {
                assert!(Instant::now() < deadline, "the worker must finish a pass");
                thread::sleep(Duration::from_millis(5));
            }
        }

        // SAFETY: this releases the last reference, which this test owns.
        unsafe { g_object_unref(object) };
    }

    /// A request for no work must not start a worker. [`dispatch`] needs this,
    /// so an [`Actions`] with only false values does nothing.
    #[test]
    fn requesting_no_work_starts_no_pass() {
        let object = plain_object();
        let (_gate, pass) = gated();
        // SAFETY: a new object, and this test holds one reference.
        let refresh = unsafe { FolderRefresh::new(object, pass) };

        refresh.request(Work::default());
        assert!(!refresh.busy(), "no work was asked for");

        // SAFETY: this releases the last reference, which this test owns.
        unsafe { g_object_unref(object) };
    }

    /// The type in which [`gated_recording`] gives back the passes it saw.
    type Seen = Arc<Mutex<Vec<Work>>>;

    /// Like [`gated`], but also records the [`Work`] that each pass got. The
    /// union test below checks these records.
    fn gated_recording() -> (
        Gate,
        Seen,
        impl Fn(*mut GObject, Work) + Send + Sync + 'static,
    ) {
        let (started_tx, started) = channel();
        let (release, held) = channel::<()>();
        let held = Mutex::new(held);
        let passes = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&passes);
        let seen = Arc::new(Mutex::new(Vec::new()));
        let recorded = Arc::clone(&seen);
        let pass = move |_: *mut GObject, work: Work| {
            recorded.lock().expect("not poisoned").push(work);
            counter.fetch_add(1, Ordering::SeqCst);
            started_tx.send(()).expect("the test is still listening");
            held.lock()
                .expect("only one pass runs at a time")
                .recv()
                .expect("the test releases every pass it starts");
        };
        (
            Gate {
                started,
                release,
                passes,
            },
            seen,
            pass,
        )
    }

    /// A `Mailbox` push and an `Email` push arrive while one pass runs. Neither
    /// kind of work must get lost. The one merged pass that answers both must
    /// do both. `Work::union` does this, and this test pins it.
    #[test]
    fn work_requested_during_a_pass_unions_into_the_next_one() {
        let object = plain_object();
        let (gate, seen, pass) = gated_recording();
        // SAFETY: a new object, and this test holds one reference.
        let refresh = unsafe { FolderRefresh::new(object, pass) };

        let check_only = Work {
            refresh_messages: false,
            check_folder_list: true,
        };
        let messages_only = Work {
            refresh_messages: true,
            check_folder_list: false,
        };

        refresh.request(check_only);
        gate.await_start();

        // Both arrive while the first pass is still blocked. Thus the merged
        // pass must do the union, not only the last request.
        refresh.request(messages_only);
        refresh.request(check_only);
        gate.release_one();
        gate.await_start();

        gate.release_one();
        let deadline = Instant::now() + Duration::from_secs(5);
        while refresh.busy() {
            assert!(Instant::now() < deadline, "the worker must finish");
            thread::sleep(Duration::from_millis(5));
        }

        assert_eq!(
            *seen.lock().expect("not poisoned"),
            vec![
                check_only,
                Work {
                    refresh_messages: true,
                    check_folder_list: true,
                }
            ],
            "the coalesced pass must do both kinds of work, not just one"
        );

        // SAFETY: this releases the last reference, which this test owns.
        unsafe { g_object_unref(object) };
    }

    /// A pass that panics must not stop push for the account. The worker
    /// resets its book-keeping, so the *next* push still starts a pass.
    #[test]
    fn a_panicking_pass_does_not_wedge_the_account() {
        let object = plain_object();
        let passes = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&passes);
        // SAFETY: a new object, and this test holds one reference.
        let refresh = unsafe {
            FolderRefresh::new(object, move |_, _| {
                counter.fetch_add(1, Ordering::SeqCst);
                panic!("a folder refresh went wrong");
            })
        };

        for expected in 1..=2 {
            refresh.request(ANY_WORK);
            let deadline = Instant::now() + Duration::from_secs(5);
            while refresh.busy() {
                assert!(Instant::now() < deadline, "the worker must not hang");
                thread::sleep(Duration::from_millis(5));
            }
            assert_eq!(
                passes.load(Ordering::SeqCst),
                expected,
                "a push after a panicking pass must still run one"
            );
        }

        // By design, each panicking pass keeps the reference that
        // `WeakBackend::with_strong` took. This test releases those two and
        // its own.
        // SAFETY: the object is live, and this test holds all three references.
        unsafe {
            assert_eq!((*object).ref_count, 3, "each panicking pass keeps one");
            for _ in 0..3 {
                g_object_unref(object);
            }
        }
    }

    /// A push that arrives after Camel released the store refreshes nothing.
    /// The weak reference to the store prevents this use-after-free.
    #[test]
    fn a_request_after_the_store_is_gone_runs_no_pass() {
        let object = plain_object();
        let passes = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&passes);
        // SAFETY: a new object, and this test holds one reference.
        let refresh = unsafe {
            FolderRefresh::new(object, move |_, _| {
                counter.fetch_add(1, Ordering::SeqCst);
            })
        };
        // SAFETY: this releases the last reference, which this test owns.
        // Thus the object is gone before the push arrives.
        unsafe { g_object_unref(object) };

        refresh.request(ANY_WORK);
        let deadline = Instant::now() + Duration::from_secs(5);
        while refresh.busy() {
            assert!(Instant::now() < deadline, "the worker must give up");
            thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(
            passes.load(Ordering::SeqCst),
            0,
            "a released store must not be refreshed"
        );
    }
}
