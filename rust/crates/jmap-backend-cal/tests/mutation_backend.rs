// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Tests pinning mutant-targeted behavior in `jmap-backend-cal::backend`.

use jmap_backend_cal::backend::JmapCalBackend;
use jmap_backend_core::push::PushRefresh;
use jmap_cal_sync::CalSync;
use jmap_client::eventsource::expand_url;
use jmap_client::{Client, Credentials};
use jmap_mock::MockServer;
use jmap_proto::Id;

struct Fixture {
    server: MockServer,
    account_id: Id,
    calendar: Id,
}

impl Fixture {
    fn start() -> Self {
        let server = MockServer::builder().start();
        let account_id = server.account_id();
        let calendar = {
            let state = server.state();
            let mut state = state.lock().unwrap();
            state
                .account_mut(&account_id)
                .unwrap()
                .seed_calendar("Personal", true)
        };
        Self {
            server,
            account_id,
            calendar,
        }
    }

    fn sync(&self) -> CalSync {
        let client = Client::connect(self.server.origin(), Credentials::none()).unwrap();
        CalSync::new(client, self.account_id.clone(), self.calendar.clone())
    }

    fn push_subscription(&self) -> PushRefresh {
        let url = expand_url(
            &format!("{}/eventsource", self.server.origin()),
            &["CalendarEvent"],
            false,
            0,
        );
        PushRefresh::start(
            url,
            Vec::new(),
            self.account_id.clone(),
            vec!["CalendarEvent".to_owned()],
            |_types: &[String]| {},
        )
    }
}

#[test]
fn inspect_connection_evaluates_closure_and_returns_result() {
    let fixture = Fixture::start();
    let backend = JmapCalBackend::detached();

    assert_eq!(backend.inspect_connection(|_| 42), None);
    assert!(!backend.is_connected());

    backend.store_connection(fixture.sync());
    assert!(backend.is_connected());

    let inspected =
        backend.inspect_connection(|s| (s.account_id().clone(), s.calendar_id().clone()));
    assert_eq!(
        inspected,
        Some((fixture.account_id.clone(), fixture.calendar.clone()))
    );

    assert!(backend.drop_connection());
    assert!(!backend.is_connected());
    assert_eq!(backend.inspect_connection(|_| 99), None);
}

#[test]
fn drop_connection_reports_true_when_connected_and_false_when_not() {
    let fixture = Fixture::start();
    let backend = JmapCalBackend::detached();

    assert!(!backend.drop_connection());
    assert!(!backend.is_connected());

    backend.store_connection(fixture.sync());
    assert!(backend.is_connected());

    assert!(backend.drop_connection());
    assert!(!backend.is_connected());
    assert!(!backend.drop_connection());
}

#[test]
fn push_lifecycle_store_is_pushing_stop_and_refresh_headers() {
    let fixture = Fixture::start();
    let backend = JmapCalBackend::detached();

    assert!(!backend.is_pushing());
    assert!(!backend.stop_push());

    // Refreshing push headers on an inactive subscription is a safe no-op.
    backend.refresh_push_headers(vec![(
        "Authorization".to_owned(),
        "Bearer token".to_owned(),
    )]);

    let push = fixture.push_subscription();
    backend.store_push(push);
    assert!(backend.is_pushing());

    // Refresh headers while active.
    backend.refresh_push_headers(vec![(
        "Authorization".to_owned(),
        "Bearer updated".to_owned(),
    )]);

    assert!(backend.stop_push());
    assert!(!backend.is_pushing());
    assert!(!backend.stop_push());
}

#[test]
fn install_connection_without_push_stores_connection_only() {
    let fixture = Fixture::start();
    let backend = JmapCalBackend::detached();

    backend.install_connection(fixture.sync(), |_sync| None);

    assert!(backend.is_connected());
    assert!(!backend.is_pushing());

    let inspected = backend.inspect_connection(|s| s.account_id().clone());
    assert_eq!(inspected, Some(fixture.account_id));
}

#[test]
fn install_connection_with_push_stores_both_connection_and_push() {
    let fixture = Fixture::start();
    let backend = JmapCalBackend::detached();

    backend.install_connection(fixture.sync(), |_sync| Some(fixture.push_subscription()));

    assert!(backend.is_connected());
    assert!(backend.is_pushing());

    let inspected = backend.inspect_connection(|s| s.account_id().clone());
    assert_eq!(inspected, Some(fixture.account_id));

    assert!(backend.stop_push());
    assert!(!backend.is_pushing());
    assert!(backend.drop_connection());
    assert!(!backend.is_connected());
}

#[test]
fn last_known_color_slot_initializes_empty_preserves_identity_and_stores_value() {
    let backend = JmapCalBackend::detached();

    let slot1 = backend
        .last_known_color()
        .expect("last_known_color initialized");
    assert_eq!(*slot1.read().unwrap(), None);

    let slot2 = backend
        .last_known_color()
        .expect("last_known_color initialized");
    assert!(std::ptr::eq(slot1, slot2));

    *slot1.write().unwrap() = Some("#62a0ea".to_string());
    assert_eq!(*slot2.read().unwrap(), Some("#62a0ea".to_string()));
}
