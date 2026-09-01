// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Turning a server-pushed `StateChange` into the folder refresh Camel
//! already runs — the mail half of JMAP Push.
//!
//! [`jmap_backend_core::push`] is the account-independent half of this and is
//! reused unchanged: it owns the EventSource subscription, decides which
//! pushes concern this account, and calls an injected C function on the
//! backend object. What differs here is what that function can *be*.
//!
//! ## Why this is not the address book's shape
//!
//! The EDS backends hand a push straight to
//! `e_book_meta_backend_schedule_refresh`, which returns immediately: EDS
//! queues a custom operation, coalesces a second call into the running one,
//! and reaches `get_changes_sync` on a thread of its own. **Camel has no such
//! call.** Its refresh entry point is `camel_folder_refresh_info_sync`, which
//! is exactly what its name says — synchronous, per folder, and a network
//! round trip.
//!
//! evolution-ews hit the same wall and its answer is the precedent item 28
//! asks to be studied, so it was, before this was written. In
//! `src/EWS/camel/camel-ews-store.c`, `camel_ews_store_server_notification_cb`
//! does *not* refresh anything on the notification thread. It sorts the
//! notification, then `schedule_folder_update` coalesces onto a low-priority
//! one-second timeout (removing any pending one first, so a burst is one
//! update), and `run_update_thread` spawns a **detached** `GThread` which
//! walks the affected folders calling `camel_store_get_folder_sync` and
//! `camel_folder_refresh_info_sync`.
//!
//! Both halves of that are load-bearing here, for a reason the EDS side does
//! not have: [`PushRefresh::stop`](jmap_backend_core::push::PushRefresh::stop)
//! *joins* its pump thread, so a refresh run inline in the pump's callback
//! would make `disconnect_sync` — and therefore Evolution's shutdown — wait
//! out an uninterruptible network round trip per open folder. So:
//!
//! - the refresh runs on a thread of its own, never on the pump, which is
//!   what keeps `stop` prompt; and
//! - [`FolderRefresh`] coalesces the way EWS's timeout does, but by
//!   book-keeping rather than by clock: while a pass is in flight, further
//!   pushes set a flag that makes the running worker do **exactly one** more
//!   pass. Not zero — a change that arrived while a pass was reading the
//!   server may not be in what that pass read — and not one per push, which
//!   is the refresh storm a client's own writes would otherwise cause, since
//!   a server pushes our `Email/set` back at us like anyone else's.
//!
//! The worker reaches the store through a
//! [`WeakBackend`](jmap_backend_core::weak::WeakBackend), so a store Camel
//! released between two passes ends the worker rather than being touched, and
//! a store released *during* one cannot be: the strong reference is held
//! across the whole pass.
//!
//! ## Which folders, and which types
//!
//! An RFC 8620 §7.1 `StateChange` names types per *account*; there is no
//! mailbox in it, so unlike EWS's `folder_id` there is nothing to narrow to.
//! What [`refresh_open_folders`] refreshes is
//! `camel_store_dup_opened_folders` — the folders something is actually
//! holding a `CamelFolder` for, which is the set whose summaries a refresh
//! can change anything about. A folder nobody has opened is listed fresh when
//! somebody does.
//!
//! [`PUSHED_TYPES`] is the message half. The folder-list half — a `Mailbox`
//! change — needs a different Camel call, `camel_store_folder_info_stale`
//! (thread-safe and self-coalescing: it schedules a low-priority idle on the
//! store's own session and drops a second request already pending, so
//! calling it once per pushed `Mailbox` change costs nothing extra). Telling
//! the two apart needs [`jmap_backend_core::push::start_for_with`], which
//! hands the action the matched JMAP types instead of the bare fn pointer
//! [`jmap_backend_core::push::start_for`] uses — [`dispatch`] is that action.
//! [`PUSHED_TYPES`] therefore no longer names every type the subscription
//! asks about; see [`start_push`](crate::store::JmapStore) for the combined
//! list.

use std::ptr;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::thread;

use eds_sys::camel_store_dup_opened_folders;
use eds_sys::{
    CamelFolder, CamelStore, camel_folder_refresh_info_sync, camel_store_folder_info_stale,
};
use glib_sys::g_ptr_array_free;
use glib_sys::{GError, GFALSE, GTRUE, g_clear_error};
use gobject_sys::GObject;
use jmap_backend_core::marshal::read_string;
use jmap_backend_core::owned::Owned;

use crate::store::JmapStore;

/// The JMAP data types that ask for a message-level refresh — a pass through
/// [`FolderRefresh`], same as before this module learned to tell types apart.
///
/// `EmailDelivery` is RFC 8621 §5's delivery-only pseudo-type: its state moves
/// when mail *arrives* and not when a flag changes, which is the one push a
/// mail client exists to react to. `Email` covers the rest — another client
/// marking something read or filing it — which a refresh must also pick up.
/// `jmap-mock` tracks only `Email` of the two, so that is the one the tests
/// exercise; asking for both is what a real server needs.
pub const PUSHED_TYPES: &[&str] = &["Email", "EmailDelivery"];

/// The JMAP data type that asks Camel to mark the folder *list* stale —
/// `camel_store_folder_info_stale`, not [`FolderRefresh`], which only ever
/// re-reads folders already open.
pub const FOLDER_LIST_TYPES: &[&str] = &["Mailbox"];

/// The coalescing worker described in this module's docs: EWS's
/// `schedule_folder_update` and `run_update_thread`, as one object.
///
/// Cheap to drop from any thread — dropping it neither cancels nor joins a
/// pass in flight, which is the point: the caller that drops it is
/// `disconnect_sync` or `finalize`, and neither may block. A pass already
/// running holds its own reference to the shared state and its own strong
/// reference to the store, so it finishes against a consistent store and then
/// simply stops.
pub struct FolderRefresh {
    inner: Arc<Inner>,
}

/// What a worker thread and the [`FolderRefresh`] that spawned it share.
struct Inner {
    /// How the worker reaches the store after the push callback that started
    /// it has long returned.
    weak: WeakStore,
    runs: Mutex<Runs>,
    /// One pass. Injected rather than hard-wired to
    /// [`refresh_open_folders`] so the coalescing above can be tested against
    /// a plain `GObject`, with no Camel store, no `CamelSession` and no
    /// network — see this module's own tests.
    pass: Box<dyn Fn(*mut GObject) + Send + Sync>,
}

/// Whether a pass is running, and whether one more is owed.
///
/// `again` is a flag and not a count on purpose: two pushes that arrive
/// during one pass are both answered by the same following pass, because that
/// pass reads the server's current state and not a queue of deltas.
#[derive(Default)]
struct Runs {
    running: bool,
    again: bool,
}

impl FolderRefresh {
    /// Prepare to refresh `store`'s open folders on demand. `pass` is one
    /// refresh of everything, called with a strong reference to `store` held.
    ///
    /// # Safety
    ///
    /// `store` must be a valid GObject with a strong reference held by the
    /// caller for the length of this call, and `pass` must accept a pointer
    /// to that object's actual type.
    pub unsafe fn new(
        store: *mut GObject,
        pass: impl Fn(*mut GObject) + Send + Sync + 'static,
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

    /// Ask for a refresh, and return immediately.
    ///
    /// Starts a worker if none is running; otherwise notes that the running
    /// one owes one more pass. Never blocks on a pass, and never waits on the
    /// lock for longer than the book-keeping — which is what makes it safe to
    /// call from [`jmap_backend_core::push`]'s pump thread.
    pub fn request(&self) {
        {
            let mut runs = lock(&self.inner.runs);
            if runs.running {
                runs.again = true;
                tracing::debug!("a folder refresh is already running; coalescing into it");
                return;
            }
            runs.running = true;
        }
        let inner = Arc::clone(&self.inner);
        // Detached, like EWS's own update thread: nothing joins it, and
        // nothing may — see the module docs.
        thread::spawn(move || inner.run());
    }

    /// Whether a pass is in flight or owed. For the tests, which have no
    /// other way to see a detached thread's book-keeping.
    #[cfg(test)]
    fn busy(&self) -> bool {
        let runs = lock(&self.inner.runs);
        runs.running || runs.again
    }
}

impl Inner {
    /// One worker's whole life: pass, then either another pass or done.
    ///
    /// A panicking pass is caught rather than allowed to end the thread. Not
    /// for the thread's sake — a worker is detached and nothing waits on it —
    /// but for the book-keeping's: unwinding out of here would leave `running`
    /// set with nobody left to clear it, and [`FolderRefresh::request`] would
    /// then coalesce into a worker that no longer exists, silently disabling
    /// push for the rest of the account's life. Rare, silent and permanent is
    /// the combination worth spending a `catch_unwind` on.
    fn run(&self) {
        loop {
            let reached = jmap_backend_core::trampoline::guard(
                "push folder refresh",
                // A pass that panicked is treated as one that ran: the store
                // was there, and whatever it did to the folders it got to is
                // done. What must not happen is retrying it forever.
                Some(()),
                || self.weak.with_strong(|store| (self.pass)(store)),
            );
            if reached.is_none() {
                // Camel released the store between two passes. There is
                // nothing to refresh, and nothing to hand the book-keeping
                // back to either — but clear it anyway, both flags, since a
                // `Drop` order that leaves this `Inner` alive should not leave
                // it looking permanently busy.
                *lock(&self.runs) = Runs::default();
                tracing::debug!("the mail store went away; stopping its folder refresh");
                return;
            }
            let mut runs = lock(&self.runs);
            if runs.again {
                runs.again = false;
                continue;
            }
            runs.running = false;
            return;
        }
    }
}

/// A `WeakBackend` under a name that says what it points at here.
struct WeakStore(jmap_backend_core::weak::WeakBackend);

impl WeakStore {
    fn with_strong<R>(&self, f: impl FnOnce(*mut GObject) -> R) -> Option<R> {
        self.0.with_strong(f)
    }
}

/// One refresh pass: every folder the store currently has open, listed again
/// against the server.
///
/// Errors are logged and skipped rather than propagated, and the walk carries
/// on past one: push is an accelerator, so a folder that could not be
/// refreshed is a folder Camel's own periodic refresh will get to, not an
/// account failure to report. Nobody is waiting for an answer from here.
///
/// No `GCancellable` is passed. There is none to pass — this runs from a
/// server push and not from an operation the user could cancel — and
/// `camel_folder_refresh_info_sync` accepts NULL for exactly that case.
///
/// # Safety
///
/// `store` must be a live `CamelStore` with a strong reference held for the
/// length of this call, which is what [`FolderRefresh`] holds across a pass.
pub unsafe fn refresh_open_folders(store: *mut CamelStore) {
    // SAFETY: a live `CamelStore` by this function's contract. The array and
    // a reference to every folder in it are transferred to us — freed below,
    // in the shape `camel_store_dup_opened_folders`' own documentation gives.
    let folders = unsafe { camel_store_dup_opened_folders(store) };
    if folders.is_null() {
        return;
    }
    // SAFETY: a `GPtrArray` just handed over, so both fields are readable.
    let count = unsafe { (*folders).len } as usize;
    tracing::debug!(
        count,
        "refreshing the mail store's open folders after a push"
    );
    for index in 0..count {
        // SAFETY: `index` is below the array's own length, every element of
        // this particular array is a `CamelFolder *` (`transfer full`), and
        // the reference moves into `folder`, released on drop rather than by
        // a manual `g_object_unref` at the bottom of the loop.
        let folder =
            unsafe { Owned::from_raw((*(*folders).pdata.add(index)).cast::<CamelFolder>()) };
        let Some(folder) = folder else { continue };
        let mut error: *mut GError = ptr::null_mut();
        // SAFETY: a live folder from the array, no cancellable (documented
        // above), and a valid place to put an error.
        let refreshed = unsafe {
            camel_folder_refresh_info_sync(folder.as_ptr(), ptr::null_mut(), &raw mut error)
        };
        if refreshed == GFALSE {
            // SAFETY: `error` is NULL or a `GError` this call now owns, whose
            // `message` is a NUL-terminated string owned by it.
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
    // SAFETY: freeing the array itself, whose elements we have already
    // released; `TRUE` frees the backing storage, which is ours now.
    unsafe { g_ptr_array_free(folders, GTRUE) };
}

/// The Camel half of a push, told which of the pushed JMAP types actually
/// changed — the action [`jmap_backend_core::push::start_for_with`] calls,
/// since a single `StateChange` can name both a message and a folder-list
/// change at once and the two need different Camel calls.
///
/// It cannot carry the coalescing state itself, so it finds it where the rest
/// of the store's state lives: on the instance the push was started for.
///
/// # Safety
///
/// `object` must be a live [`JmapStore`], which is what
/// [`jmap_backend_core::push::start_for_with`] guarantees: it only calls this
/// under a strong reference taken from a `GWeakRef` on the instance it was
/// given.
pub unsafe fn dispatch(object: *mut GObject, types: &[String]) {
    let actions = actions_for(types);
    // A panic here must not take the whole pump thread down with it, the
    // same reason `Inner::run`'s pass above is wrapped in `catch_unwind`.
    jmap_backend_core::trampoline::guard("push dispatch", (), || {
        if actions.mark_folder_list_stale {
            tracing::debug!("a pushed Mailbox change; marking the folder list stale");
            // SAFETY: `object` is a live `CamelStore`, by this function's
            // contract; `camel_store_folder_info_stale` only requires
            // `CAMEL_IS_STORE(store)`, which a live `JmapStore` satisfies.
            unsafe { camel_store_folder_info_stale(object.cast()) };
        }
        if actions.request_message_refresh
            // SAFETY: a live instance of this crate's store type, by this
            // function's contract.
            && let Some(store) = unsafe { JmapStore::borrow(object.cast::<CamelStore>()) }
        {
            store.request_folder_refresh();
        }
    });
}

/// Which Camel calls a pushed `StateChange`'s matched `types` ask for — the
/// decision half of [`dispatch`], kept pure and separate from it so it is
/// testable without a live `CamelStore`/`JmapStore` GObject, neither of which
/// this crate's test environment can construct (see this module's own tests,
/// and `tests/push.rs`'s doc comment for why).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Actions {
    /// A `Mailbox` change: the folder list itself may have changed.
    mark_folder_list_stale: bool,
    /// An `Email` or `EmailDelivery` change: some open folder's messages may
    /// have changed.
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

/// A poisoned lock means a worker panicked mid-pass. What it guards is two
/// booleans, which that cannot damage, so the book-keeping carries on rather
/// than taking the account down with whatever already went wrong — the same
/// rule [`crate::store`]'s own locks follow.
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

    /// The property the whole split exists for: a `Mailbox` change asks Camel
    /// to mark the folder list stale, and nothing else.
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

    /// An `Email` or `EmailDelivery` change asks for a message refresh, and
    /// does not touch the folder list.
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

    /// A single push can name both — the whole reason `dispatch` needs to be
    /// told which types matched, instead of one bare fn pointer.
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

    /// A type this module does not watch for — unreachable in practice, since
    /// `start_push` only ever asks the subscription to match `PUSHED_TYPES`
    /// plus `FOLDER_LIST_TYPES` — asks for nothing rather than guessing.
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

    /// A bare `GObject` to hang the weak reference on: [`FolderRefresh`] says
    /// nothing about *which* object, and a real `CamelJmapStore` needs a
    /// `CamelSession` over a source registry on the session bus, which no
    /// test environment here has.
    fn plain_object() -> *mut GObject {
        // SAFETY: no properties are being set, so the count is zero and both
        // arrays may be NULL.
        unsafe { g_object_new_with_properties(G_TYPE_OBJECT, 0, ptr::null_mut(), ptr::null()) }
    }

    /// A pass that reports when it starts and then blocks until the test lets
    /// it finish, so the overlap the coalescing is about is arranged rather
    /// than raced for.
    struct Gate {
        started: Receiver<()>,
        release: Sender<()>,
        passes: Arc<AtomicUsize>,
    }

    fn gated() -> (Gate, impl Fn(*mut GObject) + Send + Sync + 'static) {
        let (started_tx, started) = channel();
        let (release, held) = channel::<()>();
        let held = Mutex::new(held);
        let passes = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&passes);
        let pass = move |_: *mut GObject| {
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

    /// The property the whole module exists for: N pushes arriving while one
    /// pass is in flight cost exactly one more pass, not N and not none.
    #[test]
    fn pushes_arriving_during_a_pass_coalesce_into_exactly_one_more() {
        let object = plain_object();
        let (gate, pass) = gated();
        // SAFETY: freshly constructed, one reference held here.
        let refresh = unsafe { FolderRefresh::new(object, pass) };

        refresh.request();
        gate.await_start();
        assert_eq!(gate.passes(), 1, "the first request runs a pass");

        // Three more pushes, all while the first pass is still blocked.
        refresh.request();
        refresh.request();
        refresh.request();
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

        // SAFETY: releasing this test's own reference, the last one.
        unsafe { g_object_unref(object) };
    }

    /// Requests that do not overlap each get their own pass — the coalescing
    /// must not swallow a push that arrived after the last pass finished.
    #[test]
    fn requests_that_do_not_overlap_each_run_a_pass() {
        let object = plain_object();
        let (gate, pass) = gated();
        // SAFETY: freshly constructed, one reference held here.
        let refresh = unsafe { FolderRefresh::new(object, pass) };

        for expected in 1..=3 {
            refresh.request();
            gate.await_start();
            assert_eq!(gate.passes(), expected);
            gate.release_one();
            let deadline = Instant::now() + Duration::from_secs(5);
            while refresh.busy() {
                assert!(Instant::now() < deadline, "the worker must finish a pass");
                thread::sleep(Duration::from_millis(5));
            }
        }

        // SAFETY: releasing this test's own reference, the last one.
        unsafe { g_object_unref(object) };
    }

    /// A pass that panics must not take the account's push with it: the
    /// worker's book-keeping is unwound, so the *next* push still starts one.
    #[test]
    fn a_panicking_pass_does_not_wedge_the_account() {
        let object = plain_object();
        let passes = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&passes);
        // SAFETY: freshly constructed, one reference held here.
        let refresh = unsafe {
            FolderRefresh::new(object, move |_| {
                counter.fetch_add(1, Ordering::SeqCst);
                panic!("a folder refresh went wrong");
            })
        };

        for expected in 1..=2 {
            refresh.request();
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

        // SAFETY: releasing this test's own reference, the last one.
        unsafe { g_object_unref(object) };
    }

    /// A push that arrives after Camel released the store refreshes nothing —
    /// the use-after-free this reaches the store weakly to avoid.
    #[test]
    fn a_request_after_the_store_is_gone_runs_no_pass() {
        let object = plain_object();
        let passes = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&passes);
        // SAFETY: freshly constructed, one reference held here.
        let refresh = unsafe {
            FolderRefresh::new(object, move |_| {
                counter.fetch_add(1, Ordering::SeqCst);
            })
        };
        // SAFETY: releasing this test's own reference, the last one, so the
        // object is gone before the push arrives.
        unsafe { g_object_unref(object) };

        refresh.request();
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
