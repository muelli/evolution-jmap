// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! The store's half of JMAP Push: the subscription slot a live stream lives
//! in, and the two operations done to it from outside `jmap_mail::push` itself.
//!
//! What is not here is the wiring that starts a subscription for a real
//! account. `JmapStore::start_push` needs a GObject to hang a `GWeakRef` on.
//! A real `CamelJmapStore` needs a `CamelSession` over a source registry on
//! the session bus, which no test environment here has. This is
//! [`JmapStore::detached`]'s one documented limit, the same one
//! `stale_token.rs` records for the credentials half. So this file builds the
//! subscription the way `start_push` builds it and installs it by hand. This
//! makes every behaviour on this side of that call testable against a real
//! `jmap-mock` over a real socket.

use std::thread;
use std::time::{Duration, Instant};

use jmap_backend_core::push::PushRefresh;
use jmap_client::eventsource::expand_url;
use jmap_mail::store::JmapStore;
use jmap_mock::MockServer;

/// A subscription of the shape `JmapStore::start_push` makes, against
/// `server`, sending `token`. It refreshes nothing: what a refresh does is
/// `jmap_mail::push`'s own concern, tested there.
fn subscription(server: &MockServer, token: &str) -> PushRefresh {
    PushRefresh::start(
        expand_url(
            &format!("{}/eventsource", server.origin()),
            &["Email"],
            false,
            0,
        ),
        vec![("Authorization".to_owned(), format!("Bearer {token}"))],
        server.account_id(),
        vec!["Email".to_owned()],
        |_types: &[String]| {},
    )
}

#[test]
fn a_new_store_is_not_pushing() {
    let store = JmapStore::detached();

    assert!(
        !store.is_pushing(),
        "a store nothing has connected has no subscription"
    );
}

#[test]
fn stopping_takes_the_subscription_away() {
    let server = MockServer::builder().start();
    let store = JmapStore::detached();

    store.store_push(subscription(&server, "token"));
    assert!(store.is_pushing());

    assert!(store.stop_push(), "there was a subscription to stop");
    assert!(!store.is_pushing());
    assert!(
        !store.stop_push(),
        "and stopping a store that is not pushing is not a failure"
    );
}

/// Camel disconnects every service on shutdown, connected or not.
/// `disconnect_sync`, through `drop_connection`, must leave nothing behind
/// that could still refresh over a connection that is gone.
#[test]
fn disconnecting_stops_the_subscription_too() {
    let server = MockServer::builder().start();
    let store = JmapStore::detached();

    store.store_push(subscription(&server, "token"));
    store.drop_connection();

    assert!(
        !store.is_pushing(),
        "a disconnected store must not still be listening"
    );
}

/// The OAuth 2.0 half: a subscription whose token the server has stopped
/// accepting reconnects once, and only once, fresh headers are installed.
/// `refresh_credentials` installs fresh headers after it renews the
/// connection's own access token.
#[test]
fn refreshing_the_push_headers_lets_a_stalled_subscription_reconnect() {
    let server = MockServer::builder().bearer_token("fresh-token").start();
    let store = JmapStore::detached();

    store.store_push(subscription(&server, "stale-token"));

    let deadline = Instant::now() + Duration::from_secs(5);
    while server.unauthorized_responses() == 0 {
        assert!(
            Instant::now() < deadline,
            "the stale token was never refused"
        );
        thread::sleep(Duration::from_millis(10));
    }

    store.refresh_push_headers(vec![(
        "Authorization".to_owned(),
        "Bearer fresh-token".to_owned(),
    )]);

    server.wait_for_event_source_subscriber(Duration::from_secs(5));
}

/// A push that arrives at a store with no coalescing worker installed must be
/// a quiet no-op, not a panic across the `extern "C"` boundary, which would
/// abort all of Evolution. Every [`JmapStore::detached`] instance has no
/// worker installed, since `start_push` is what installs one.
#[test]
fn asking_a_store_with_no_worker_to_refresh_does_nothing() {
    let store = JmapStore::detached();

    store.request_folder_refresh(jmap_mail::push::Work {
        refresh_messages: true,
        check_folder_list: true,
    });
}
