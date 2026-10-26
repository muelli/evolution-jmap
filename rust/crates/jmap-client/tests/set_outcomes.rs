// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Tests for `/set` response outcome helpers: `expect_created`, `expect_updated`,
//! and `expect_destroyed`.

use std::collections::BTreeMap;

use jmap_client::proto::error::SetError;
use jmap_client::proto::methods::SetResponse;
use jmap_client::{Error, Id, expect_created, expect_destroyed, expect_updated};

#[test]
fn expect_created_returns_object_on_success() {
    let mut response = SetResponse::<String>::new("acc1", "state1");
    let mut created = BTreeMap::new();
    created.insert("c1".to_string(), "hello".to_string());
    response.created = Some(created);

    let result = expect_created(&response, "c1").expect("should return created item");
    assert_eq!(result, "hello");
}

#[test]
fn expect_created_returns_set_error_on_rejection() {
    let mut response = SetResponse::<String>::new("acc1", "state1");
    let mut not_created = BTreeMap::new();
    not_created.insert("c1".to_string(), SetError::new("invalidProperties"));
    response.not_created = Some(not_created);

    let err = expect_created(&response, "c1").expect_err("should return error");
    match err {
        Error::Set(set_err) => assert_eq!(set_err.error_type, "invalidProperties"),
        other => panic!("expected Error::Set, got {other:?}"),
    }
}

#[test]
fn expect_created_returns_protocol_error_when_omitted() {
    let response = SetResponse::<String>::new("acc1", "state1");
    let err = expect_created(&response, "c1").expect_err("should return protocol error");
    match err {
        Error::Protocol(msg) => {
            assert_eq!(msg, "/set response reports neither success nor failure");
        }
        other => panic!("expected Error::Protocol, got {other:?}"),
    }
}

#[test]
fn expect_updated_succeeds_when_id_present_in_updated() {
    let mut response = SetResponse::<()>::new("acc1", "state1");
    let mut updated = BTreeMap::new();
    let id = Id::new("item1");
    updated.insert(id.clone(), None);
    response.updated = Some(updated);

    expect_updated(&response, &id).expect("should succeed");
}

#[test]
fn expect_updated_returns_set_error_on_rejection() {
    let mut response = SetResponse::<()>::new("acc1", "state1");
    let mut not_updated = BTreeMap::new();
    let id = Id::new("item1");
    not_updated.insert(id.clone(), SetError::new("notFound"));
    response.not_updated = Some(not_updated);

    let err = expect_updated(&response, &id).expect_err("should return error");
    match err {
        Error::Set(set_err) => assert_eq!(set_err.error_type, "notFound"),
        other => panic!("expected Error::Set, got {other:?}"),
    }
}

#[test]
fn expect_updated_returns_protocol_error_when_omitted() {
    let response = SetResponse::<()>::new("acc1", "state1");
    let id = Id::new("item1");
    let err = expect_updated(&response, &id).expect_err("should return protocol error");
    match err {
        Error::Protocol(msg) => {
            assert!(msg.contains("reports neither success nor failure"));
            assert!(msg.contains("item1"));
        }
        other => panic!("expected Error::Protocol, got {other:?}"),
    }
}

#[test]
fn expect_destroyed_succeeds_when_id_present_in_destroyed() {
    let mut response = SetResponse::<()>::new("acc1", "state1");
    let id = Id::new("item1");
    response.destroyed = Some(vec![id.clone()]);

    expect_destroyed(&response, &id).expect("should succeed");
}

#[test]
fn expect_destroyed_returns_set_error_on_rejection() {
    let mut response = SetResponse::<()>::new("acc1", "state1");
    let mut not_destroyed = BTreeMap::new();
    let id = Id::new("item1");
    not_destroyed.insert(id.clone(), SetError::new("forbidden"));
    response.not_destroyed = Some(not_destroyed);

    let err = expect_destroyed(&response, &id).expect_err("should return error");
    match err {
        Error::Set(set_err) => assert_eq!(set_err.error_type, "forbidden"),
        other => panic!("expected Error::Set, got {other:?}"),
    }
}

#[test]
fn expect_destroyed_returns_protocol_error_when_omitted() {
    let response = SetResponse::<()>::new("acc1", "state1");
    let id = Id::new("item1");
    let err = expect_destroyed(&response, &id).expect_err("should return protocol error");
    match err {
        Error::Protocol(msg) => {
            assert!(msg.contains("reports neither success nor failure"));
            assert!(msg.contains("item1"));
        }
        other => panic!("expected Error::Protocol, got {other:?}"),
    }
}
