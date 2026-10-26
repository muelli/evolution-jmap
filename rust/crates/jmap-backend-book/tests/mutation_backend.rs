// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Tests pinning mutant-targeted behavior in `jmap-backend-book::backend`.

use eds_sys::e_book_meta_backend_get_type;
use jmap_backend_book::backend::{JmapBookBackend, parent_class};
use jmap_backend_core::push::PushRefresh;
use jmap_backend_core::subclass::ObjectSubclass;
use jmap_client::eventsource::expand_url;
use jmap_mock::MockServer;

mod common;
use common::Fixture;

#[test]
fn inspect_connection_evaluates_closure_and_returns_result() {
    let fixture = Fixture::start();
    let backend = JmapBookBackend::detached();

    assert_eq!(backend.inspect_connection(|_| 42), None);
    assert!(!backend.is_connected());

    backend.store_connection(fixture.sync());
    assert!(backend.is_connected());

    let inspected =
        backend.inspect_connection(|s| (s.account_id().clone(), s.address_book_id().clone()));
    assert_eq!(
        inspected,
        Some((fixture.account_id.clone(), fixture.ours.clone()))
    );

    assert!(backend.drop_connection());
    assert!(!backend.is_connected());
    assert_eq!(backend.inspect_connection(|_| 99), None);
}

#[test]
fn drop_connection_reports_true_when_connected_and_false_when_not() {
    let fixture = Fixture::start();
    let backend = JmapBookBackend::detached();

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
    let server = MockServer::builder().bearer_token("fresh-token").start();
    let backend = JmapBookBackend::detached();

    assert!(!backend.is_pushing());
    assert!(!backend.stop_push());

    // Refreshing push headers on an inactive subscription is a safe no-op.
    backend.refresh_push_headers(vec![(
        "Authorization".to_owned(),
        "Bearer stale-token".to_owned(),
    )]);

    let url = expand_url(
        &format!("{}/eventsource", server.origin()),
        &["ContactCard"],
        false,
        0,
    );
    backend.store_push(PushRefresh::start(
        url,
        vec![("Authorization".to_owned(), "Bearer stale-token".to_owned())],
        server.account_id(),
        vec!["ContactCard".to_owned()],
        |_types: &[String]| {},
    ));
    assert!(backend.is_pushing());

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while server.unauthorized_responses() == 0 {
        assert!(
            std::time::Instant::now() < deadline,
            "the stale token was never refused"
        );
        std::thread::sleep(std::time::Duration::from_millis(10));
    }

    // Refresh headers while active to allow reconnection.
    backend.refresh_push_headers(vec![(
        "Authorization".to_owned(),
        "Bearer fresh-token".to_owned(),
    )]);

    server.wait_for_event_source_subscriber(std::time::Duration::from_secs(5));

    assert!(backend.stop_push());
    assert!(!backend.is_pushing());
    assert!(!backend.stop_push());
}

#[test]
fn parent_type_and_parent_class_return_valid_eds_meta_backend() {
    let parent_type = <JmapBookBackend as ObjectSubclass>::parent_type();
    assert_eq!(parent_type, unsafe { e_book_meta_backend_get_type() });
    let class = unsafe { gobject_sys::g_type_class_ref(parent_type) };
    assert!(parent_class().is_some());
    unsafe { gobject_sys::g_type_class_unref(class) };
}

#[test]
fn install_connection_without_push_stores_connection_only() {
    let fixture = Fixture::start();
    let backend = JmapBookBackend::detached();

    backend.install_connection(fixture.sync(), |_sync| None);

    assert!(backend.is_connected());
    assert!(!backend.is_pushing());

    let inspected = backend.inspect_connection(|s| s.account_id().clone());
    assert_eq!(inspected, Some(fixture.account_id));
}

#[test]
fn install_connection_with_push_stores_both_connection_and_push() {
    let fixture = Fixture::start();
    let backend = JmapBookBackend::detached();

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
