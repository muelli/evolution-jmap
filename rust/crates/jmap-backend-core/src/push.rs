// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Turning a server-pushed `StateChange` into the refresh EDS already runs.
//!
//! `docs/ROADMAP.md` item 28 is explicit that a push must "trigger the
//! EXISTING `get_changes_sync` path rather than growing a second sync
//! mechanism", and that evolution-ews is the precedent to study. It is a
//! short one: `ebb_ews_server_notification_cb` decides whether a server
//! notification concerns *this* backend's folder and, if it does, calls
//! `e_book_meta_backend_schedule_refresh` — nothing else. EDS takes it from
//! there: `schedule_refresh` coalesces (a refresh already running makes it a
//! no-op), dispatches through `e_book_backend_schedule_custom_operation`, and
//! the refresh reaches `get_changes_sync` with the stored sync tag. So a
//! burst of pushes costs one `/changes` round trip, and the whole of the sync
//! logic stays where it already is and is already tested.
//!
//! [`PushRefresh`] is that callback's Rust half, minus the EDS call itself:
//! it owns the [`EventSourceSubscription`], decides which pushes concern the
//! backend, and invokes an action the caller supplies. Keeping the action
//! injected is what makes this testable at all — a real `EBookMetaBackend`
//! needs a running `evolution-source-registry`, so the tests drive the pump
//! with a counting closure, while the backends pass one that calls
//! `e_book_meta_backend_schedule_refresh` through a
//! [`WeakBackend`](crate::weak::WeakBackend).
//!
//! Push is an accelerator, never the path to correctness: if the stream never
//! connects, or the server advertises no `eventSourceUrl`, or the action is
//! dropped on the floor because the backend went away, EDS's own periodic
//! refresh still runs. Nothing here reports an error to anybody, and that is
//! deliberate.
//!
//! ## Lifetime
//!
//! The pump thread outlives no backend: [`PushRefresh::stop`] cancels the
//! subscription and joins the thread, and `Drop` calls it, so by the time the
//! `PushRefresh` is gone the action can no longer run. The one case it cannot
//! join is when the action *itself* ended up dropping the backend's last
//! reference, so the backend's finalize — and therefore this `Drop` — is
//! running on the pump thread; joining there would deadlock on itself, so
//! that case detaches instead. It is safe to: the thread holds nothing that
//! `Drop` frees, and the cancellation it is about to observe is set first.

use std::thread::{self, JoinHandle};
use std::time::Duration;

use gobject_sys::GObject;
use jmap_client::eventsource::{SharedHeaders, expand_url};
use jmap_client::transport::CancelFlag;
use jmap_client::{Client, EventSourceSubscription};
use jmap_proto::Id;
use jmap_proto::push::StateChange;

use crate::weak::WeakBackend;

/// How long the pump waits for a push before looking at the cancellation
/// flag again. Only bounds how long [`PushRefresh::stop`] waits, since a
/// cancelled subscription also shuts its socket down, so this can be
/// generous rather than a busy-wait.
const POLL_INTERVAL: Duration = Duration::from_millis(200);

/// How often to ask the server for a `ping` event (RFC 8620 §7.3).
///
/// Not because anything reads them — [`jmap_client::eventsource`] discards
/// them — but because they are the only thing that makes a stream where
/// nothing is happening *write*, and a write is how either end learns the
/// connection has silently died (a NAT rebinding, a dropped VPN) instead of
/// waiting forever on a socket that will never deliver anything. Two minutes
/// is comfortably inside the shortest NAT idle timeouts in common use.
const PING_SECONDS: u32 = 120;

/// Start pushing refreshes at `backend`, if the server it is connected to
/// advertises somewhere to listen (RFC 8620 §7.3's `eventSourceUrl`) — and
/// `None`, quietly, if it does not, since a server without push is simply one
/// where EDS's own periodic refresh remains the only trigger.
///
/// `schedule_refresh` is the EDS call to make, which is per-backend
/// (`e_book_meta_backend_schedule_refresh` and its calendar twin); it is
/// reached through a [`WeakBackend`], so it is skipped rather than run
/// against a backend EDS has already released.
///
/// # Safety
///
/// `backend` must be a valid GObject with a strong reference held for the
/// length of this call, and `schedule_refresh` must accept a pointer to that
/// object's actual type.
pub unsafe fn start_for(
    backend: *mut GObject,
    client: &Client,
    account_id: &Id,
    types: &[&str],
    schedule_refresh: unsafe extern "C" fn(*mut GObject),
) -> Option<PushRefresh> {
    let template = client.session().event_source_url.trim();
    if template.is_empty() {
        tracing::debug!("the server advertises no eventSourceUrl; push is unavailable");
        return None;
    }
    let url = expand_url(template, types, false, PING_SECONDS);
    let headers = client
        .authorization_header()
        .map(|value| vec![("Authorization".to_owned(), value)])
        .unwrap_or_default();
    // SAFETY: `backend` is valid and referenced, by this function's contract.
    let weak = unsafe { WeakBackend::new(backend) };

    tracing::debug!(account_id = account_id.as_str(), "subscribing to JMAP push");
    Some(PushRefresh::start(
        url,
        headers,
        account_id.clone(),
        types.iter().map(|name| (*name).to_owned()).collect(),
        move || {
            // SAFETY: `with_strong` runs this only while holding a strong
            // reference to the object `backend` pointed at, and the caller
            // promised `schedule_refresh` accepts that object's type.
            weak.with_strong(|object| unsafe { schedule_refresh(object) });
        },
    ))
}

/// A live JMAP Push subscription that refreshes one backend.
pub struct PushRefresh {
    cancel: CancelFlag,
    pump: Option<JoinHandle<()>>,
    headers: SharedHeaders,
}

impl PushRefresh {
    /// Subscribe to `url` — an `eventSourceUrl` already expanded by
    /// [`jmap_client::eventsource::expand_url`] — and run `refresh` whenever
    /// a pushed `StateChange` names `account_id` together with at least one
    /// of `types`.
    ///
    /// `types` is the same list the URL's `types` parameter asks the server
    /// to filter by; it is applied again here because the filtering is the
    /// server's courtesy, not its obligation (RFC 8620 §7.3 lets it push
    /// more), and because a `StateChange` covers every account the session
    /// can see, not just this backend's.
    ///
    /// An empty `types` accepts any type for the account, matching the `*`
    /// [`expand_url`](jmap_client::eventsource::expand_url) puts in the URL
    /// for the same input.
    pub fn start(
        url: String,
        headers: Vec<(String, String)>,
        account_id: Id,
        types: Vec<String>,
        refresh: impl Fn() + Send + 'static,
    ) -> Self {
        let cancel = CancelFlag::new();
        let headers = SharedHeaders::new(headers);
        let pump = {
            let cancel = cancel.clone();
            let headers = headers.clone();
            thread::spawn(move || {
                let subscription = EventSourceSubscription::start(url, headers, cancel.clone());
                while !cancel.is_cancelled() {
                    let Some(change) = subscription.recv_timeout(POLL_INTERVAL) else {
                        continue;
                    };
                    if concerns(&change, &account_id, &types) {
                        tracing::debug!(
                            account_id = account_id.as_str(),
                            "a pushed StateChange concerns this backend; scheduling a refresh"
                        );
                        refresh();
                    }
                }
            })
        };
        Self {
            cancel,
            pump: Some(pump),
            headers,
        }
    }

    /// Replace the `Authorization` header this subscription sends on future
    /// reconnect attempts — called right after a backend's own
    /// `refresh_credentials` installs a fresh OAuth 2.0 access token on the
    /// connection, so a subscription a stale token got refused on picks up
    /// the new one on its next reconnect instead of looping on the same
    /// failure until the backend itself reconnects (see
    /// [`jmap_client::eventsource`]'s own module doc, and
    /// `docs/ROADMAP.md` item 28).
    pub fn set_headers(&self, headers: Vec<(String, String)>) {
        self.headers.set(headers);
    }

    /// Stop listening and wait for the pump to finish, so that no further
    /// refresh can be triggered once this returns. Idempotent; `Drop` calls
    /// it.
    pub fn stop(&mut self) {
        self.cancel.cancel();
        let Some(pump) = self.pump.take() else {
            return;
        };
        if pump.thread().id() == thread::current().id() {
            // We are inside the pump's own `refresh` call: it dropped the
            // backend's last reference, so finalize — and this `stop` — are
            // running on the pump thread. Joining would be joining ourselves.
            // Detaching is enough: the cancellation above is already set, and
            // the pump checks it before it could call `refresh` again.
            tracing::debug!("push refresh stopped from its own pump thread; detaching");
            return;
        }
        let _ = pump.join();
    }
}

impl Drop for PushRefresh {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Whether a pushed `StateChange` says something this backend should refresh
/// for: its own account, and — unless it asked for every type — a type it
/// asked about.
fn concerns(change: &StateChange, account_id: &Id, types: &[String]) -> bool {
    change
        .changed
        .get(account_id)
        .is_some_and(|state| types.is_empty() || types.iter().any(|name| state.contains_key(name)))
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Instant;

    use jmap_client::eventsource::expand_url;
    use jmap_proto::State;

    use super::*;

    fn change(account: &str, kind: &str) -> StateChange {
        let mut types = BTreeMap::new();
        types.insert(kind.to_owned(), State::new("s1"));
        let mut changed = BTreeMap::new();
        changed.insert(Id::new(account), types);
        StateChange::new(changed)
    }

    fn names(types: &[&str]) -> Vec<String> {
        types.iter().map(|name| (*name).to_owned()).collect()
    }

    #[test]
    fn a_change_to_a_watched_type_on_this_account_concerns_us() {
        assert!(concerns(
            &change("a1", "ContactCard"),
            &Id::new("a1"),
            &names(&["ContactCard"])
        ));
    }

    #[test]
    fn a_change_to_another_account_does_not() {
        assert!(!concerns(
            &change("a2", "ContactCard"),
            &Id::new("a1"),
            &names(&["ContactCard"])
        ));
    }

    #[test]
    fn a_change_to_a_type_we_did_not_ask_about_does_not() {
        assert!(!concerns(
            &change("a1", "Email"),
            &Id::new("a1"),
            &names(&["ContactCard"])
        ));
    }

    #[test]
    fn asking_for_no_types_in_particular_accepts_any_on_this_account() {
        assert!(concerns(&change("a1", "Email"), &Id::new("a1"), &[]));
    }

    /// The whole pump, over a real socket: a mutation in the mock broadcasts
    /// a `StateChange` on its own (`jmap-mock`'s automatic push), the reader
    /// receives it, and the refresh action runs.
    #[test]
    fn a_pushed_state_change_runs_the_refresh_action() {
        let server = jmap_mock::MockServer::builder().start();
        let url = expand_url(
            &format!("{}/eventsource", server.origin()),
            &["ContactCard"],
            false,
            0,
        );
        let refreshes = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&refreshes);
        let _push = PushRefresh::start(
            url,
            Vec::new(),
            Id::new("a1"),
            names(&["ContactCard"]),
            move || {
                counter.fetch_add(1, Ordering::SeqCst);
            },
        );

        server.wait_for_event_source_subscriber(Duration::from_secs(5));
        server.push_state_change(&change("a1", "ContactCard"));

        let deadline = Instant::now() + Duration::from_secs(5);
        while refreshes.load(Ordering::SeqCst) == 0 {
            assert!(Instant::now() < deadline, "the refresh action never ran");
            thread::sleep(Duration::from_millis(10));
        }
    }

    /// The negative control for the test above, over the same socket: a push
    /// naming an account this backend does not serve must not refresh it.
    /// The mock's own `types` filter cannot catch this one — it filters by
    /// type, and the type here is the one we asked for.
    #[test]
    fn a_push_for_another_account_does_not_run_the_refresh_action() {
        let server = jmap_mock::MockServer::builder().start();
        let url = expand_url(
            &format!("{}/eventsource", server.origin()),
            &["ContactCard"],
            false,
            0,
        );
        let refreshes = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&refreshes);
        let _push = PushRefresh::start(
            url,
            Vec::new(),
            Id::new("a1"),
            names(&["ContactCard"]),
            move || {
                counter.fetch_add(1, Ordering::SeqCst);
            },
        );

        server.wait_for_event_source_subscriber(Duration::from_secs(5));
        server.push_state_change(&change("someone-else", "ContactCard"));
        // Then one that does concern us, to prove the pump was running and
        // reading all along rather than merely slow — without this, the
        // assertion below would pass just as well on a broken reader.
        server.push_state_change(&change("a1", "ContactCard"));

        let deadline = Instant::now() + Duration::from_secs(5);
        while refreshes.load(Ordering::SeqCst) == 0 {
            assert!(Instant::now() < deadline, "the second push never arrived");
            thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(
            refreshes.load(Ordering::SeqCst),
            1,
            "only the push naming this backend's account may refresh it"
        );
    }

    /// The header-refresh piece of item 28: a `PushRefresh` started with a
    /// stale `Authorization` header keeps being refused until
    /// [`PushRefresh::set_headers`] installs the fresh one, at which point
    /// the pump's own reconnect loop — already retrying with backoff — picks
    /// it up on its next attempt and the refresh action starts running.
    #[test]
    fn set_headers_lets_a_stalled_subscription_reconnect_and_resume_refreshing() {
        let server = jmap_mock::MockServer::builder()
            .bearer_token("fresh-token")
            .start();
        let url = expand_url(
            &format!("{}/eventsource", server.origin()),
            &["ContactCard"],
            false,
            0,
        );
        let refreshes = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&refreshes);
        let push = PushRefresh::start(
            url,
            vec![("Authorization".to_owned(), "Bearer stale-token".to_owned())],
            Id::new("a1"),
            names(&["ContactCard"]),
            move || {
                counter.fetch_add(1, Ordering::SeqCst);
            },
        );

        let deadline = Instant::now() + Duration::from_secs(5);
        while server.unauthorized_responses() == 0 {
            assert!(
                Instant::now() < deadline,
                "the stale token was never refused"
            );
            thread::sleep(Duration::from_millis(10));
        }

        push.set_headers(vec![(
            "Authorization".to_owned(),
            "Bearer fresh-token".to_owned(),
        )]);

        server.wait_for_event_source_subscriber(Duration::from_secs(5));
        server.push_state_change(&change("a1", "ContactCard"));

        let deadline = Instant::now() + Duration::from_secs(5);
        while refreshes.load(Ordering::SeqCst) == 0 {
            assert!(Instant::now() < deadline, "the refresh action never ran");
            thread::sleep(Duration::from_millis(10));
        }
    }

    #[test]
    fn stopping_waits_for_the_pump_and_does_not_block_on_a_live_connection() {
        let server = jmap_mock::MockServer::builder().start();
        let url = expand_url(
            &format!("{}/eventsource", server.origin()),
            &["ContactCard"],
            false,
            0,
        );
        let mut push = PushRefresh::start(url, Vec::new(), Id::new("a1"), Vec::new(), || {});
        server.wait_for_event_source_subscriber(Duration::from_secs(5));

        let started = Instant::now();
        push.stop();
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "stopping must not wait out a reconnect backoff"
        );
        // Idempotent: the backends call it from `disconnect_sync` and again
        // from `finalize`.
        push.stop();
    }

    /// `stop` reached from inside the action, which is what a `refresh` that
    /// drops the backend's last reference does — it runs finalize, which
    /// clears the slot this `PushRefresh` lives in, on the pump thread.
    /// Joining there would deadlock; the test would hang rather than fail,
    /// which is the honest way to catch it.
    #[test]
    fn stopping_from_inside_the_action_detaches_instead_of_joining_itself() {
        let server = jmap_mock::MockServer::builder().start();
        let url = expand_url(
            &format!("{}/eventsource", server.origin()),
            &["ContactCard"],
            false,
            0,
        );
        let slot: Arc<std::sync::Mutex<Option<PushRefresh>>> =
            Arc::new(std::sync::Mutex::new(None));
        let inner = Arc::clone(&slot);
        let stopped = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&stopped);
        let push = PushRefresh::start(url, Vec::new(), Id::new("a1"), Vec::new(), move || {
            // Stand-in for finalize: drop the `PushRefresh` from the
            // pump thread itself.
            drop(inner.lock().expect("slot lock").take());
            counter.fetch_add(1, Ordering::SeqCst);
        });
        *slot.lock().expect("slot lock") = Some(push);

        server.wait_for_event_source_subscriber(Duration::from_secs(5));
        server.push_state_change(&change("a1", "ContactCard"));

        let deadline = Instant::now() + Duration::from_secs(10);
        while stopped.load(Ordering::SeqCst) == 0 {
            assert!(
                Instant::now() < deadline,
                "the self-drop never completed, so it deadlocked on itself"
            );
            thread::sleep(Duration::from_millis(10));
        }
    }
}
