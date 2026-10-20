// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Turning a server-pushed `StateChange` into the refresh EDS already runs.
//!
//! A push must "trigger the EXISTING `get_changes_sync` path rather than
//! growing a second sync mechanism", and evolution-ews is the precedent to
//! study. The precedent is short: `ebb_ews_server_notification_cb` decides
//! if a server notification concerns the folder of *this* backend. If it
//! does, it calls `e_book_meta_backend_schedule_refresh` and nothing else.
//! EDS does the rest: `schedule_refresh` coalesces (if a refresh already
//! runs, it does nothing), dispatches through
//! `e_book_backend_schedule_custom_operation`, and the refresh gets to
//! `get_changes_sync` with the stored sync tag. Thus a burst of pushes costs
//! one `/changes` round trip, and all of the sync logic stays where it is
//! now, with its tests.
//!
//! [`PushRefresh`] is the Rust half of that callback, without the EDS call.
//! Its pump thread owns the [`EventSourceSubscription`]. It decides which
//! pushes concern the backend, and calls an action that the caller supplies.
//! This injected action makes the module testable: a real `EBookMetaBackend`
//! needs a running `evolution-source-registry`. Thus the tests drive the pump
//! with a counting closure, and the backends pass a closure that calls
//! `e_book_meta_backend_schedule_refresh` through a
//! [`WeakBackend`](crate::weak::WeakBackend).
//!
//! Push makes refreshes faster, but correctness never depends on it. The
//! periodic refresh of EDS still runs if the stream never connects, if the
//! server advertises no `eventSourceUrl`, or if the action does nothing
//! because the backend is gone. Nothing here reports an error, and that is
//! deliberate.
//!
//! ## Lifetime
//!
//! The pump thread outlives no backend: [`PushRefresh::stop`] cancels the
//! subscription and joins the thread, and `Drop` calls it. Thus, after the
//! `PushRefresh` is gone, the action cannot run. One case cannot join: the
//! action *itself* drops the last reference to the backend, so the finalize
//! of the backend (and thus this `Drop`) runs on the pump thread. A join
//! there would deadlock on itself, so in that case the thread detaches. This
//! is safe: the thread holds nothing that `Drop` frees, and the cancellation
//! that the thread is about to see is already set.

use std::collections::BTreeMap;
use std::thread::{self, JoinHandle};
use std::time::Duration;

use gobject_sys::GObject;
use jmap_client::eventsource::{SharedHeaders, expand_url};
use jmap_client::transport::CancelFlag;
use jmap_client::{Client, EventSourceSubscription};
use jmap_proto::Id;
use jmap_proto::State;
use jmap_proto::push::StateChange;

use crate::weak::WeakBackend;

/// How long the pump waits for a push before it examines the cancellation
/// flag again. This only limits how long [`PushRefresh::stop`] waits,
/// because a cancelled subscription also closes its socket. Thus this value
/// can be generous; it is not a busy-wait.
const POLL_INTERVAL: Duration = Duration::from_millis(200);

/// How often to ask the server for a `ping` event (RFC 8620 §7.3).
///
/// Nothing reads the pings ([`jmap_client::eventsource`] discards them). But
/// they are the only thing that makes an idle stream *write*. A write is how
/// either end learns that the connection silently died (a NAT rebinding, a
/// dropped VPN), instead of an infinite wait on a socket that will never
/// deliver data. Two minutes is safely shorter than the shortest NAT idle
/// timeouts in common use.
const PING_SECONDS: u32 = 120;

/// Starts to push refreshes at `backend` if its server advertises a location
/// to listen to (`eventSourceUrl`, RFC 8620 §7.3). If not, returns `None`
/// without an error: on a server without push, the periodic refresh of EDS
/// stays the only trigger.
///
/// `schedule_refresh` is the EDS call to make, which is per backend
/// (`e_book_meta_backend_schedule_refresh` and its calendar twin). The call
/// goes through a [`WeakBackend`], so it does not run on a backend that EDS
/// has already released.
///
/// # Safety
///
/// `backend` must be a valid GObject with a strong reference held for the
/// duration of this call. `schedule_refresh` must accept a pointer to the
/// actual type of that object.
pub unsafe fn start_for(
    backend: *mut GObject,
    client: &Client,
    account_id: &Id,
    types: &[&str],
    schedule_refresh: unsafe extern "C" fn(*mut GObject),
) -> Option<PushRefresh> {
    let action = move |object: *mut GObject, _types: &[String]| {
        // SAFETY: `object` is a strong reference to the object of `backend`,
        // under the same contract as the doc of `start_for`. The caller
        // promised that `schedule_refresh` accepts the type of that object.
        unsafe { schedule_refresh(object) };
    };
    // SAFETY: forwarded to `start_for_with`, which has the same contract on
    // `backend`.
    unsafe { start_for_with(backend, client, account_id, types, action) }
}

/// Like [`start_for`], but for a caller whose action must know which of
/// `types` the pushed `StateChange` actually named. A plain `unsafe extern
/// "C" fn(*mut GObject)` cannot carry that, because it must match the fixed
/// signature of a real vtable entry point (the case of `start_for`).
/// `jmap-mail` uses this: a `Mailbox` change and an `Email`/`EmailDelivery`
/// change ask Camel for two different things, and one `StateChange` can name
/// both.
///
/// # Safety
///
/// Same contract as [`start_for`]: `backend` must be a valid GObject with a
/// strong reference held for the duration of this call, and `action` must
/// accept a pointer to the actual type of that object.
pub unsafe fn start_for_with(
    backend: *mut GObject,
    client: &Client,
    account_id: &Id,
    types: &[&str],
    action: impl Fn(*mut GObject, &[String]) + Send + Sync + 'static,
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
        move |matched: &[String]| {
            // SAFETY: `with_strong` runs this only while it holds a strong
            // reference to the object that `backend` pointed to, and the
            // caller promised that `action` accepts the type of that object.
            weak.with_strong(|object| action(object, matched));
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
    /// Subscribes to `url` (an `eventSourceUrl` that
    /// [`jmap_client::eventsource::expand_url`] already expanded). Runs
    /// `refresh` each time a pushed `StateChange` names `account_id` together
    /// with at least one of `types` with a new state string.
    ///
    /// `types` is the same list that the `types` parameter of the URL asks
    /// the server to filter by. This function applies it again for two
    /// reasons. The filter is a courtesy of the server, not an obligation
    /// (RFC 8620 §7.3 lets it push more). Also, a `StateChange` covers each
    /// account that the session can see, not only the account of this
    /// backend.
    ///
    /// An empty `types` accepts any type for the account. This matches the
    /// `*` that [`expand_url`](jmap_client::eventsource::expand_url) puts in
    /// the URL for the same input.
    ///
    /// `refresh` receives the subset of `types` that the relevant
    /// `StateChange` named with a new state string (all such types for this
    /// account, if `types` is empty). Most callers ignore it and always
    /// refresh. But a caller that must know which JMAP type changed
    /// (`jmap-mail`, message vs. folder list) needs it.
    pub fn start(
        url: String,
        headers: Vec<(String, String)>,
        account_id: Id,
        types: Vec<String>,
        refresh: impl Fn(&[String]) + Send + 'static,
    ) -> Self {
        let cancel = CancelFlag::new();
        let headers = SharedHeaders::new(headers);
        let pump = {
            let cancel = cancel.clone();
            let headers = headers.clone();
            thread::spawn(move || {
                let subscription = EventSourceSubscription::start(url, headers, cancel.clone());
                let mut last_state: BTreeMap<String, State> = BTreeMap::new();
                while !cancel.is_cancelled() {
                    let Some(change) = subscription.recv_timeout(POLL_INTERVAL) else {
                        continue;
                    };
                    let matched = concerns(&change, &account_id, &types, &mut last_state);
                    if !matched.is_empty() {
                        tracing::debug!(
                            account_id = account_id.as_str(),
                            "a pushed StateChange concerns this backend; scheduling a refresh"
                        );
                        refresh(&matched);
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

    /// Replaces the `Authorization` header that this subscription sends on
    /// future reconnect attempts. A backend calls this immediately after its
    /// own `refresh_credentials` installs a new OAuth 2.0 access token on the
    /// connection. Thus a subscription that the server refused because of a
    /// stale token uses the new token on its next reconnect. It does not
    /// loop on the same failure until the backend itself reconnects (see the
    /// module doc of [`jmap_client::eventsource`]).
    pub fn set_headers(&self, headers: Vec<(String, String)>) {
        self.headers.set(headers);
    }

    /// Stops listening and waits for the pump to finish, so that no further
    /// refresh can start after this returns. Idempotent; `Drop` calls it.
    pub fn stop(&mut self) {
        self.cancel.cancel();
        let Some(pump) = self.pump.take() else {
            return;
        };
        if pump.thread().id() == thread::current().id() {
            // This is inside the `refresh` call of the pump: it dropped the
            // last reference to the backend, so finalize (and this `stop`)
            // runs on the pump thread. A join would join this thread itself.
            // Detach is sufficient: the cancellation above is already set, and
            // the pump examines it before it could call `refresh` again.
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

/// Which of `types` a pushed `StateChange` names for `account_id` with a
/// state string that is different from the last one seen. Empty if the
/// change does not concern this account, names none of the requested types,
/// or names only types whose state string did not change since the last
/// call. An empty `types` accepts each type that the account has, so each
/// type that the `StateChange` names for it is a candidate.
///
/// The `StateChange` of RFC 8620 §7.1 has a state string per (account,
/// type). This function compares it with `last_state`, and updates
/// `last_state` for each candidate, matched or not. Thus a server that
/// pushes the same state many times causes zero refreshes after the first.
/// The first time a type appears always counts as a change, because
/// `last_state` has no value to compare it with yet.
fn concerns(
    change: &StateChange,
    account_id: &Id,
    types: &[String],
    last_state: &mut BTreeMap<String, State>,
) -> Vec<String> {
    let Some(state) = change.changed.get(account_id) else {
        return Vec::new();
    };
    let candidates: Vec<&String> = if types.is_empty() {
        state.keys().collect()
    } else {
        types
            .iter()
            .filter(|name| state.contains_key(name.as_str()))
            .collect()
    };
    let mut matched = Vec::new();
    for name in candidates {
        let new_state = &state[name];
        let is_change = last_state.get(name) != Some(new_state);
        if is_change {
            matched.push(name.clone());
        }
        last_state.insert(name.clone(), new_state.clone());
    }
    matched
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Instant;

    use jmap_client::eventsource::expand_url;

    use super::*;

    fn change(account: &str, kind: &str) -> StateChange {
        change_with_state(account, kind, "s1")
    }

    fn change_with_state(account: &str, kind: &str, state: &str) -> StateChange {
        let mut types = BTreeMap::new();
        types.insert(kind.to_owned(), State::new(state));
        let mut changed = BTreeMap::new();
        changed.insert(Id::new(account), types);
        StateChange::new(changed)
    }

    fn names(types: &[&str]) -> Vec<String> {
        types.iter().map(|name| (*name).to_owned()).collect()
    }

    #[test]
    fn a_change_to_a_watched_type_on_this_account_concerns_us() {
        assert_eq!(
            concerns(
                &change("a1", "ContactCard"),
                &Id::new("a1"),
                &names(&["ContactCard"]),
                &mut BTreeMap::new()
            ),
            names(&["ContactCard"]),
            "the matched types must name the type that actually changed"
        );
    }

    #[test]
    fn a_change_to_another_account_does_not() {
        assert!(
            concerns(
                &change("a2", "ContactCard"),
                &Id::new("a1"),
                &names(&["ContactCard"]),
                &mut BTreeMap::new()
            )
            .is_empty()
        );
    }

    #[test]
    fn a_change_to_a_type_we_did_not_ask_about_does_not() {
        assert!(
            concerns(
                &change("a1", "Email"),
                &Id::new("a1"),
                &names(&["ContactCard"]),
                &mut BTreeMap::new()
            )
            .is_empty()
        );
    }

    #[test]
    fn asking_for_no_types_in_particular_accepts_any_on_this_account() {
        assert_eq!(
            concerns(
                &change("a1", "Email"),
                &Id::new("a1"),
                &[],
                &mut BTreeMap::new()
            ),
            names(&["Email"]),
            "an empty filter matches, and names, whichever type changed"
        );
    }

    #[test]
    fn a_change_naming_two_watched_types_reports_both() {
        let mut types = BTreeMap::new();
        types.insert("Email".to_owned(), State::new("s1"));
        types.insert("Mailbox".to_owned(), State::new("s1"));
        let mut changed = BTreeMap::new();
        changed.insert(Id::new("a1"), types);
        let change = StateChange::new(changed);

        let mut matched = concerns(
            &change,
            &Id::new("a1"),
            &names(&["Email", "Mailbox"]),
            &mut BTreeMap::new(),
        );
        matched.sort();
        assert_eq!(matched, names(&["Email", "Mailbox"]));
    }

    #[test]
    fn a_repeated_state_string_does_not_concern_us_again() {
        let mut last_state = BTreeMap::new();
        let first = change_with_state("a1", "ContactCard", "s1");
        assert_eq!(
            concerns(
                &first,
                &Id::new("a1"),
                &names(&["ContactCard"]),
                &mut last_state
            ),
            names(&["ContactCard"]),
            "the first sighting of a state string always counts as a change"
        );

        let repeat = change_with_state("a1", "ContactCard", "s1");
        assert!(
            concerns(
                &repeat,
                &Id::new("a1"),
                &names(&["ContactCard"]),
                &mut last_state
            )
            .is_empty(),
            "the same state string seen again must not concern us a second time"
        );

        let genuinely_new = change_with_state("a1", "ContactCard", "s2");
        assert_eq!(
            concerns(
                &genuinely_new,
                &Id::new("a1"),
                &names(&["ContactCard"]),
                &mut last_state
            ),
            names(&["ContactCard"]),
            "a state string that actually differs from the last one seen must concern us"
        );
    }

    #[test]
    fn each_type_tracks_its_own_last_seen_state_independently() {
        let mut last_state = BTreeMap::new();
        let mut types = BTreeMap::new();
        types.insert("Email".to_owned(), State::new("s1"));
        types.insert("Mailbox".to_owned(), State::new("s1"));
        let mut changed = BTreeMap::new();
        changed.insert(Id::new("a1"), types);
        let first = StateChange::new(changed);
        concerns(
            &first,
            &Id::new("a1"),
            &names(&["Email", "Mailbox"]),
            &mut last_state,
        );

        // Only the state string of Mailbox changed.
        let mut types = BTreeMap::new();
        types.insert("Email".to_owned(), State::new("s1"));
        types.insert("Mailbox".to_owned(), State::new("s2"));
        let mut changed = BTreeMap::new();
        changed.insert(Id::new("a1"), types);
        let second = StateChange::new(changed);
        assert_eq!(
            concerns(
                &second,
                &Id::new("a1"),
                &names(&["Email", "Mailbox"]),
                &mut last_state
            ),
            names(&["Mailbox"]),
            "an unchanged type must not be reported alongside a changed one"
        );
    }

    /// The full pump, over a real socket: the mock pushes a `StateChange`,
    /// the reader receives it, and the refresh action runs.
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
            move |_types: &[String]| {
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

    /// The negative control for the test above, also over a real socket: a
    /// push that names an account that this backend does not serve must not
    /// refresh it. The `types` filter of the mock cannot catch this case: it
    /// filters by type, and the type here is the requested type.
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
            move |_types: &[String]| {
                counter.fetch_add(1, Ordering::SeqCst);
            },
        );

        server.wait_for_event_source_subscriber(Duration::from_secs(5));
        server.push_state_change(&change("someone-else", "ContactCard"));
        // Then push one that concerns this backend. This proves that the
        // pump was running and reading all the time, not only slow. Without
        // it, the assertion below would also pass on a broken reader.
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

    /// The pump-level counterpart of the `concerns` unit tests above: if a
    /// server pushes the same state string two times, exactly one refresh
    /// must occur. A state string that is really different must still cause
    /// its own refresh. The `StateChange` of RFC 8620 §7.1 has a state string
    /// per (account, type), which the pump compares with the last one seen.
    #[test]
    fn a_repeated_push_state_does_not_refresh_twice_but_a_new_one_does() {
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
            move |_types: &[String]| {
                counter.fetch_add(1, Ordering::SeqCst);
            },
        );

        server.wait_for_event_source_subscriber(Duration::from_secs(5));
        server.push_state_change(&change_with_state("a1", "ContactCard", "s1"));

        let deadline = Instant::now() + Duration::from_secs(5);
        while refreshes.load(Ordering::SeqCst) < 1 {
            assert!(Instant::now() < deadline, "the first push never refreshed");
            thread::sleep(Duration::from_millis(10));
        }

        // The same state again must not cause a second refresh. Push a new
        // state immediately after it, and wait for THAT refresh. If the
        // repeat counted incorrectly, the count would be 3, not 2, after the
        // new state arrives.
        server.push_state_change(&change_with_state("a1", "ContactCard", "s1"));
        server.push_state_change(&change_with_state("a1", "ContactCard", "s2"));

        let deadline = Instant::now() + Duration::from_secs(5);
        while refreshes.load(Ordering::SeqCst) < 2 {
            assert!(Instant::now() < deadline, "the new state never refreshed");
            thread::sleep(Duration::from_millis(10));
        }

        // Give an incorrectly counted repeat time to arrive too.
        thread::sleep(Duration::from_millis(200));
        assert_eq!(
            refreshes.load(Ordering::SeqCst),
            2,
            "the repeated state must not have produced a third refresh"
        );
    }

    /// The server refuses a `PushRefresh` that starts with a stale
    /// `Authorization` header until [`PushRefresh::set_headers`] installs the
    /// new header. Then the reconnect loop of the pump (which already retries
    /// with backoff) uses it on its next attempt, and the refresh action
    /// starts to run.
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
            move |_types: &[String]| {
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
        let mut push = PushRefresh::start(
            url,
            Vec::new(),
            Id::new("a1"),
            Vec::new(),
            |_types: &[String]| {},
        );
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

    /// `stop` called from inside the action. This occurs when a `refresh`
    /// drops the last reference to the backend: it runs finalize on the pump
    /// thread, and finalize clears the slot that holds this `PushRefresh`.
    /// A join there would deadlock. The test would then hang, not fail,
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
        let push = PushRefresh::start(
            url,
            Vec::new(),
            Id::new("a1"),
            Vec::new(),
            move |_types: &[String]| {
                // Stand-in for finalize: drop the `PushRefresh` from the
                // pump thread itself.
                drop(inner.lock().expect("slot lock").take());
                counter.fetch_add(1, Ordering::SeqCst);
            },
        );
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

    /// `stop` must actually cancel the pump, not only return quickly: a push
    /// that arrives after `stop` returns must never get to the refresh action.
    #[test]
    fn stopping_prevents_a_later_push_from_running_the_refresh_action() {
        let server = jmap_mock::MockServer::builder().start();
        let url = expand_url(
            &format!("{}/eventsource", server.origin()),
            &["ContactCard"],
            false,
            0,
        );
        let refreshes = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&refreshes);
        let mut push = PushRefresh::start(
            url,
            Vec::new(),
            Id::new("a1"),
            names(&["ContactCard"]),
            move |_types: &[String]| {
                counter.fetch_add(1, Ordering::SeqCst);
            },
        );
        server.wait_for_event_source_subscriber(Duration::from_secs(5));

        push.stop();
        server.push_state_change(&change("a1", "ContactCard"));

        thread::sleep(Duration::from_millis(500));
        assert_eq!(
            refreshes.load(Ordering::SeqCst),
            0,
            "a push arriving after stop() returned must not run the refresh action"
        );
    }

    /// `Drop` must actually stop the pump, not only return: a push that
    /// arrives after a `PushRefresh` is dropped must never get to the refresh
    /// action.

    #[test]
    fn dropping_a_pushrefresh_stops_the_pump_so_a_later_push_does_not_refresh() {
        let server = jmap_mock::MockServer::builder().start();
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
            Vec::new(),
            Id::new("a1"),
            names(&["ContactCard"]),
            move |_types: &[String]| {
                counter.fetch_add(1, Ordering::SeqCst);
            },
        );
        server.wait_for_event_source_subscriber(Duration::from_secs(5));

        drop(push);
        server.push_state_change(&change("a1", "ContactCard"));

        thread::sleep(Duration::from_millis(500));
        assert_eq!(
            refreshes.load(Ordering::SeqCst),
            0,
            "a push arriving after the PushRefresh is dropped must not run the refresh action"
        );
    }
}
