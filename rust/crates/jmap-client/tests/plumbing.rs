// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Client ↔ mock-server plumbing: session discovery, authentication,
//! Core/echo.

use jmap_client::{Client, Credentials, Error};
use jmap_mock::MockServer;
use jmap_proto::session;
use serde_json::json;

#[test]
fn core_echo_roundtrip() {
    let server = MockServer::builder().start();
    let client = Client::connect(server.origin(), Credentials::none()).unwrap();

    let payload = json!({"hello": "world", "nested": {"n": 1}});
    let echoed = client.echo(payload.clone()).unwrap();
    assert_eq!(echoed, payload);
}

#[test]
fn session_discovery_capabilities() {
    let server = MockServer::builder().start();
    let client = Client::connect(server.origin(), Credentials::none()).unwrap();

    let s = client.session();
    assert!(s.capabilities.contains_key(session::CAPABILITY_CORE));
    // Core describes the server, not a data type — every other capability
    // must also name a primary account.
    for capability in [
        session::CAPABILITY_MAIL,
        session::CAPABILITY_SUBMISSION,
        session::CAPABILITY_CONTACTS,
        session::CAPABILITY_CALENDARS,
    ] {
        assert!(
            s.capabilities.contains_key(capability),
            "session lacks {capability}"
        );
        assert!(
            s.primary_account(capability).is_some(),
            "no primary account for {capability}"
        );
    }
    assert!(!s.api_url.is_empty());
    assert!(!s.username.is_empty());
}

#[test]
fn auth_basic_ok() {
    let server = MockServer::builder().basic_auth("alice", "sekret").start();
    let client = Client::connect(server.origin(), Credentials::basic("alice", "sekret")).unwrap();
    assert_eq!(
        client.echo(json!({"ok": true})).unwrap(),
        json!({"ok": true})
    );
}

#[test]
fn auth_basic_rejected_401() {
    let server = MockServer::builder().basic_auth("alice", "sekret").start();

    let denied = Client::connect(server.origin(), Credentials::basic("alice", "wrong"));
    match denied {
        Err(Error::Http { status: 401, .. }) => {}
        other => panic!("expected 401, got {other:?}"),
    }

    let anonymous = Client::connect(server.origin(), Credentials::none());
    match anonymous {
        Err(Error::Http { status: 401, .. }) => {}
        other => panic!("expected 401, got {other:?}"),
    }
}

#[test]
fn auth_bearer_ok() {
    let server = MockServer::builder().bearer_token("tok-123").start();

    let client = Client::connect(server.origin(), Credentials::bearer("tok-123")).unwrap();
    assert_eq!(client.echo(json!({"n": 42})).unwrap(), json!({"n": 42}));

    let denied = Client::connect(server.origin(), Credentials::bearer("tok-999"));
    match denied {
        Err(Error::Http { status: 401, .. }) => {}
        other => panic!("expected 401, got {other:?}"),
    }
}

#[test]
fn unknown_method_returns_error() {
    let server = MockServer::builder().start();
    let client = Client::connect(server.origin(), Credentials::none()).unwrap();

    let request = jmap_proto::request::Request::new([session::CAPABILITY_CORE])
        .call("Nonexistent/method", &json!({}), "c0")
        .unwrap();
    let response = client.api_call(&request).unwrap();

    let invocation = &response.method_responses[0];
    assert!(invocation.is_error());
    let error: jmap_proto::error::MethodError = invocation.parse().unwrap();
    assert_eq!(error.error_type, jmap_proto::error::method::UNKNOWN_METHOD);
}
