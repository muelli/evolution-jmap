// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! The connection and push lifecycle slot that every backend keeps.
//!
//! `JmapBookBackend` and `JmapCalBackend` each keep a live connection and an
//! optional push subscription behind the same pair of operations. This
//! module gives both backends one generic implementation of that pair,
//! parameterized by the backend's own sync type.
//!
//! The scheduled-refresh debounce (`refresh_queued`, `wait_for_queued`) stays
//! out: `jmap-backend-book` needs it to avoid a deadlock with a manual-query
//! book view, and `jmap-backend-cal` has no matching deadlock, so it is not
//! shared.

use crate::instance::Slot;
use crate::poison::{lock, read, write};
use crate::push::PushRefresh;
use jmap_object_sync::Changes;
use jmap_proto::{Id, State};
use std::sync::{Mutex, RwLock};

/// A backend's ability to list every item and report changes since a state.
///
/// `jmap-backend-book` and `jmap-backend-cal` each already have a type with
/// these four operations (`BookSync`, `CalSync`); this trait names the
/// shape they share so a vfunc body can be written once, generic over
/// `Self::Item` (`ContactInfo` or `ComponentInfo`). Marshalling an `Item`
/// into EDS out-parameters stays format-specific and out of this trait.
pub trait MetaSync {
    /// The error `list_existing` and `get_changes` report.
    type Error: std::error::Error;
    /// The listed and changed item: `ContactInfo` or `ComponentInfo`.
    type Item;

    /// The account this sync object was opened for.
    fn account_id(&self) -> &Id;
    /// The collection (address book or calendar) this sync object was opened for.
    fn collection_id(&self) -> &Id;
    /// Every item currently in the collection, with the state to resume from.
    fn list_existing(&self) -> Result<(State, Vec<Self::Item>), Self::Error>;
    /// Items changed or removed since `since`.
    fn get_changes(&self, since: &State) -> Result<Changes<Self::Item>, Self::Error>;
}

/// A connection slot and a push slot, generic over the backend's sync type.
///
/// Put one in a `#[repr(C)]` instance struct, the same way as a bare
/// [`Slot`]: both fields are empty at all-zero bytes, so `MetaState` itself
/// is valid at all-zero bytes.
pub struct MetaState<S> {
    /// Live connection; read-locked across concurrent vfuncs, write-locked on connect/disconnect.
    pub session: Slot<RwLock<Option<S>>>,
    /// Active push subscription for server-driven change notifications.
    pub push: Slot<Mutex<Option<PushRefresh>>>,
}

impl<S> MetaState<S> {
    /// A state with no connection and no push subscription.
    pub const fn new() -> Self {
        Self {
            session: Slot::new(),
            push: Slot::new(),
        }
    }

    /// Installs `sync` as the live connection, replacing any existing connection.
    pub fn store_connection(&self, sync: S) {
        if let Some(session) = self.session.get() {
            *write(session) = Some(sync);
        }
    }

    /// Installs `sync` as the live connection and starts the push subscription.
    pub fn install_connection(&self, sync: S, start_push: impl FnOnce(&S) -> Option<PushRefresh>) {
        self.store_connection(sync);
        let push = self.session.get().and_then(|session| {
            let guard = read(session);
            guard.as_ref().and_then(start_push)
        });
        if let Some(push) = push {
            self.store_push(push);
        }
    }

    /// Drops the connection, reporting whether there was one.
    pub fn drop_connection(&self) -> bool {
        match self.session.get() {
            Some(session) => write(session).take().is_some(),
            None => false,
        }
    }

    /// Whether an operation would find a connection.
    pub fn is_connected(&self) -> bool {
        self.session
            .get()
            .is_some_and(|session| read(session).is_some())
    }

    /// Installs `push` as the live push subscription, stopping and dropping
    /// any prior subscription.
    pub fn store_push(&self, push: PushRefresh) {
        if let Some(slot) = self.push.get() {
            *lock(slot) = Some(push);
        }
    }

    /// Stops the push subscription and waits for its thread, reporting
    /// whether there was one. Once this returns, no refresh can still be
    /// scheduled from it.
    pub fn stop_push(&self) -> bool {
        match self.push.get() {
            Some(slot) => lock(slot).take().is_some(),
            None => false,
        }
    }

    /// Whether a push subscription is live.
    #[cfg(feature = "testing")]
    pub fn is_pushing(&self) -> bool {
        self.push.get().is_some_and(|slot| lock(slot).is_some())
    }

    /// Replaces the authorization headers the live push subscription sends
    /// on future reconnect attempts, if a subscription is active.
    pub fn refresh_push_headers(&self, headers: Vec<(String, String)>) {
        if let Some(slot) = self.push.get()
            && let Some(push) = lock(slot).as_ref()
        {
            push.set_headers(headers);
        }
    }
}

impl<S> Default for MetaState<S> {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::MetaState;
    use crate::push::PushRefresh;
    use jmap_mock::MockServer;
    use std::sync::{Mutex, RwLock};

    /// A stand-in sync type: enough for `MetaState<S>` to store and compare, nothing more.
    #[derive(Debug, PartialEq, Eq, Clone)]
    struct StubSync(u32);

    fn detached() -> MetaState<StubSync> {
        let state = MetaState::new();
        state.session.init(RwLock::new(None));
        state.push.init(Mutex::new(None));
        state
    }

    #[test]
    fn store_and_drop_connection_report_whether_one_was_present() {
        let state = detached();

        assert!(!state.is_connected());
        assert!(!state.drop_connection());

        state.store_connection(StubSync(1));
        assert!(state.is_connected());

        assert!(state.drop_connection());
        assert!(!state.is_connected());
        assert!(!state.drop_connection());
    }

    #[test]
    fn install_connection_without_push_stores_connection_only() {
        let state = detached();

        state.install_connection(StubSync(7), |_sync| None);

        assert!(state.is_connected());
        assert!(!state.is_pushing());
    }

    #[test]
    fn push_lifecycle_store_is_pushing_stop_and_refresh_headers() {
        let server = MockServer::builder().bearer_token("fresh-token").start();
        let state = detached();

        assert!(!state.is_pushing());
        assert!(!state.stop_push());

        // Refreshing push headers on an inactive subscription is a safe no-op.
        state.refresh_push_headers(vec![(
            "Authorization".to_owned(),
            "Bearer stale-token".to_owned(),
        )]);

        let url = jmap_client::eventsource::expand_url(
            &format!("{}/eventsource", server.origin()),
            &["ContactCard"],
            false,
            0,
        );
        state.store_push(PushRefresh::start(
            url,
            vec![("Authorization".to_owned(), "Bearer stale-token".to_owned())],
            server.account_id(),
            vec!["ContactCard".to_owned()],
            |_types: &[String]| {},
        ));
        assert!(state.is_pushing());

        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while server.unauthorized_responses() == 0 {
            assert!(
                std::time::Instant::now() < deadline,
                "the stale token was never refused"
            );
            std::thread::sleep(std::time::Duration::from_millis(10));
        }

        state.refresh_push_headers(vec![(
            "Authorization".to_owned(),
            "Bearer fresh-token".to_owned(),
        )]);

        server.wait_for_event_source_subscriber(std::time::Duration::from_secs(5));

        assert!(state.stop_push());
        assert!(!state.is_pushing());
        assert!(!state.stop_push());
    }

    #[test]
    fn install_connection_with_push_stores_both_connection_and_push() {
        let server = MockServer::builder().bearer_token("fresh-token").start();
        let state = detached();

        let url = jmap_client::eventsource::expand_url(
            &format!("{}/eventsource", server.origin()),
            &["ContactCard"],
            false,
            0,
        );
        state.install_connection(StubSync(3), |_sync| {
            Some(PushRefresh::start(
                url.clone(),
                vec![],
                server.account_id(),
                vec!["ContactCard".to_owned()],
                |_types: &[String]| {},
            ))
        });

        assert!(state.is_connected());
        assert!(state.is_pushing());

        assert!(state.stop_push());
        assert!(!state.is_pushing());
        assert!(state.drop_connection());
        assert!(!state.is_connected());
    }
}

#[cfg(test)]
mod meta_sync_tests {
    use super::MetaSync;
    use jmap_object_sync::Changes;
    use jmap_proto::{Id, State};
    use std::convert::Infallible;

    /// A stand-in item: enough to tell listed and changed items apart.
    #[derive(Debug, PartialEq, Eq, Clone)]
    struct StubItem(u32);

    /// A stand-in sync type with canned answers, enough to prove `MetaSync`
    /// is usable generically, without naming `BookSync` or `CalSync`.
    struct StubSync {
        account_id: Id,
        collection_id: Id,
        existing: Vec<StubItem>,
    }

    impl MetaSync for StubSync {
        type Error = Infallible;
        type Item = StubItem;

        fn account_id(&self) -> &Id {
            &self.account_id
        }

        fn collection_id(&self) -> &Id {
            &self.collection_id
        }

        fn list_existing(&self) -> Result<(State, Vec<StubItem>), Infallible> {
            Ok((State::from("s1".to_owned()), self.existing.clone()))
        }

        fn get_changes(&self, since: &State) -> Result<Changes<StubItem>, Infallible> {
            assert_eq!(since.as_str(), "s1");
            Ok(Changes {
                new_state: State::from("s2".to_owned()),
                changed: vec![StubItem(2)],
                removed: vec!["1".to_owned()],
            })
        }
    }

    /// Exercises `MetaSync` only through the trait, the way a shared vfunc
    /// body would: this is the point of the trait, not just its stub impl.
    fn ids<T: MetaSync>(sync: &T) -> (String, String) {
        (
            sync.account_id().as_str().to_owned(),
            sync.collection_id().as_str().to_owned(),
        )
    }

    #[test]
    fn list_existing_and_get_changes_work_through_the_trait() {
        let sync = StubSync {
            account_id: Id::from("acct".to_owned()),
            collection_id: Id::from("coll".to_owned()),
            existing: vec![StubItem(1)],
        };

        assert_eq!(ids(&sync), ("acct".to_owned(), "coll".to_owned()));

        let (state, existing) = sync.list_existing().unwrap();
        assert_eq!(state.as_str(), "s1");
        assert_eq!(existing, vec![StubItem(1)]);

        let changes = sync.get_changes(&state).unwrap();
        assert_eq!(changes.new_state.as_str(), "s2");
        assert_eq!(changes.changed, vec![StubItem(2)]);
        assert_eq!(changes.removed, vec!["1".to_owned()]);
    }
}
