// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Property-based fuzzing of the untrusted-server boundary (Track A4,
//! `docs/ROADMAP.md`): a JMAP server is not trusted, so a response's
//! *shape* is hostile input, not a contract this crate's `Deserialize`
//! impls are allowed to assume. Every property here feeds arbitrary JSON
//! straight into an envelope type's deserialization; the only thing
//! asserted is that decoding never panics — an `Err` is a perfectly normal
//! outcome for garbage input, a panic is not.

use proptest::prelude::*;
use serde_json::Value;

/// A bounded-depth, bounded-breadth arbitrary JSON value.
///
/// The bounds exist to keep generation cheap, not to under-approximate
/// hostility: what this test hunts for is a `Deserialize` impl that
/// indexes, unwraps, or slices on an assumption about shape (a missing
/// field, a wrong type, a short array) rather than returning `Err`, and a
/// shallow-but-wide document exercises that exactly as well as a deep one.
/// Parser-level concerns like unbounded nesting depth are `serde_json`'s to
/// harden, not this crate's `Deserialize` impls'.
fn json_value() -> impl Strategy<Value = Value> {
    let leaf = prop_oneof![
        Just(Value::Null),
        any::<bool>().prop_map(Value::Bool),
        any::<i64>().prop_map(|n| Value::Number(n.into())),
        ".{0,16}".prop_map(Value::String),
    ];
    leaf.prop_recursive(4, 64, 8, |inner| {
        prop_oneof![
            prop::collection::vec(inner.clone(), 0..8).prop_map(Value::Array),
            prop::collection::btree_map(".{0,8}", inner, 0..8)
                .prop_map(|map| Value::Object(map.into_iter().collect())),
        ]
    })
}

proptest! {
    #[test]
    fn arbitrary_json_never_panics_deserializing_session(value in json_value()) {
        let text = value.to_string();
        let _ = serde_json::from_str::<jmap_proto::session::Session>(&text);
    }

    #[test]
    fn arbitrary_json_never_panics_deserializing_request(value in json_value()) {
        let text = value.to_string();
        let _ = serde_json::from_str::<jmap_proto::request::Request>(&text);
    }

    #[test]
    fn arbitrary_json_never_panics_deserializing_response(value in json_value()) {
        let text = value.to_string();
        let _ = serde_json::from_str::<jmap_proto::response::Response>(&text);
    }

    #[test]
    fn arbitrary_json_never_panics_deserializing_method_error(value in json_value()) {
        let text = value.to_string();
        let _ = serde_json::from_str::<jmap_proto::error::MethodError>(&text);
    }

    #[test]
    fn arbitrary_json_never_panics_deserializing_request_error(value in json_value()) {
        let text = value.to_string();
        let _ = serde_json::from_str::<jmap_proto::error::RequestError>(&text);
    }
}
