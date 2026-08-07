// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Incremental sync (`/changes`) semantics.

use jmap_client::{Client, Credentials};
use jmap_mock::MockServer;
use jmap_proto::contacts::ContactCard;
use serde_json::json;

#[test]
fn changes_since_state_tracks_crud() {
    let server = MockServer::builder().start();
    let account_id = server.account_id();
    let book = {
        let state = server.state();
        let mut state = state.lock().unwrap();
        state
            .account_mut(&account_id)
            .unwrap()
            .seed_address_book("Personal", true)
    };
    let client = Client::connect(server.origin(), Credentials::none()).unwrap();

    let state0 = client.contact_state(&account_id).unwrap();

    let kept = client
        .contact_create(
            &account_id,
            &ContactCard::simple(book.clone(), "Keep Me", "keep@example.com"),
        )
        .unwrap()
        .id
        .unwrap();
    let state_after_first_create = client.contact_state(&account_id).unwrap();

    let doomed = client
        .contact_create(
            &account_id,
            &ContactCard::simple(book, "Doomed", "doomed@example.com"),
        )
        .unwrap()
        .id
        .unwrap();
    let state_after_both_creates = client.contact_state(&account_id).unwrap();

    client
        .contact_update(&account_id, &kept, json!({"name/full": "Keep Me Longer"}))
        .unwrap();
    client.contact_destroy(&account_id, &doomed).unwrap();

    // Window covering everything: `kept` was created (and later updated) →
    // only "created"; `doomed` was created and destroyed → invisible.
    let full = client.changes(&account_id, "ContactCard", &state0).unwrap();
    assert_eq!(full.created, vec![kept.clone()]);
    assert!(full.updated.is_empty());
    assert!(full.destroyed.is_empty());

    // Window starting after `kept` existed: it shows as updated; `doomed`
    // (created + destroyed inside the window) still invisible.
    let middle = client
        .changes(&account_id, "ContactCard", &state_after_first_create)
        .unwrap();
    assert!(middle.created.is_empty(), "doomed was destroyed in-window");
    assert_eq!(middle.updated, vec![kept.clone()]);
    assert!(middle.destroyed.is_empty());

    // Window starting after both creates: update + destroy are visible as
    // such.
    let late = client
        .changes(&account_id, "ContactCard", &state_after_both_creates)
        .unwrap();
    assert!(late.created.is_empty());
    assert_eq!(late.updated, vec![kept]);
    assert_eq!(late.destroyed, vec![doomed]);

    // The response's newState equals the current state.
    let now = client.contact_state(&account_id).unwrap();
    assert_eq!(late.new_state, now);
}
