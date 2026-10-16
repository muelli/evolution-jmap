// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

use jmap_proto::id::Id;
use jmap_proto::state::{State, UtcDate};

#[test]
fn id_from_owned_string_preserves_content() {
    let raw = String::from("mailbox-id-12345");
    let id = Id::from(raw.clone());
    assert_eq!(id.as_str(), "mailbox-id-12345");
    assert_eq!(id.to_string(), "mailbox-id-12345");
    assert_eq!(AsRef::<str>::as_ref(&id), "mailbox-id-12345");
    assert!(!id.is_empty());
}

#[test]
fn id_empty_predicate_identifies_empty_and_populated_identifiers() {
    let empty_id = Id::default();
    assert!(empty_id.is_empty());
    assert_eq!(empty_id.as_str(), "");

    let populated = Id::new("id-1");
    assert!(!populated.is_empty());
    assert_eq!(populated.as_str(), "id-1");
}

#[test]
fn id_conversions_and_display_roundtrip() {
    let id_ref = Id::from("custom-id");
    assert_eq!(id_ref.as_str(), "custom-id");

    let id_new_owned = Id::new(String::from("owned-id"));
    assert_eq!(id_new_owned.as_str(), "owned-id");

    let id_display = format!("{id_ref}");
    assert_eq!(id_display, "custom-id");
}

#[test]
fn id_transparent_serde_roundtrip() {
    let raw = serde_json::json!("msg-42");
    let id: Id = serde_json::from_value(raw.clone()).expect("deserialize id");
    assert_eq!(id.as_str(), "msg-42");

    let serialized = serde_json::to_value(&id).expect("serialize id");
    assert_eq!(serialized, raw);
}

#[test]
fn state_from_owned_string_preserves_content() {
    let raw = String::from("state-token-67890");
    let state = State::from(raw.clone());
    assert_eq!(state.as_str(), "state-token-67890");
    assert_eq!(state.to_string(), "state-token-67890");
    assert_eq!(AsRef::<str>::as_ref(&state), "state-token-67890");
}

#[test]
fn state_conversions_and_display_roundtrip() {
    let state_ref = State::from("ref-token");
    assert_eq!(state_ref.as_str(), "ref-token");

    let state_new_owned = State::new(String::from("owned-token"));
    assert_eq!(state_new_owned.as_str(), "owned-token");

    let state_display = format!("{state_ref}");
    assert_eq!(state_display, "ref-token");
}

#[test]
fn state_transparent_serde_roundtrip() {
    let raw = serde_json::json!("state-1234");
    let state: State = serde_json::from_value(raw.clone()).expect("deserialize state");
    assert_eq!(state.as_str(), "state-1234");

    let serialized = serde_json::to_value(&state).expect("serialize state");
    assert_eq!(serialized, raw);
}

#[test]
fn utc_date_from_owned_string_preserves_content() {
    let raw = String::from("2026-10-16T08:45:00Z");
    let date = UtcDate::from(raw.clone());
    assert_eq!(date.as_str(), "2026-10-16T08:45:00Z");
    assert_eq!(date.to_string(), "2026-10-16T08:45:00Z");
    assert_eq!(AsRef::<str>::as_ref(&date), "2026-10-16T08:45:00Z");
}

#[test]
fn utc_date_conversions_and_display_roundtrip() {
    let date_ref = UtcDate::from("2026-01-01T00:00:00Z");
    assert_eq!(date_ref.as_str(), "2026-01-01T00:00:00Z");

    let date_new_owned = UtcDate::new(String::from("2026-06-01T12:00:00Z"));
    assert_eq!(date_new_owned.as_str(), "2026-06-01T12:00:00Z");

    let date_display = format!("{date_ref}");
    assert_eq!(date_display, "2026-01-01T00:00:00Z");
}

#[test]
fn utc_date_transparent_serde_roundtrip() {
    let raw = serde_json::json!("2026-10-16T09:00:00Z");
    let date: UtcDate = serde_json::from_value(raw.clone()).expect("deserialize utc_date");
    assert_eq!(date.as_str(), "2026-10-16T09:00:00Z");

    let serialized = serde_json::to_value(&date).expect("serialize utc_date");
    assert_eq!(serialized, raw);
}
