// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! The read side of the backend: what exists, what one card looks like, and
//! what changed — all against a live mock server rather than a fixture.

mod common;

use common::Fixture;
use serde_json::json;

#[test]
fn list_existing_returns_only_the_cards_in_this_address_book() {
    let fixture = Fixture::start();
    let mine = fixture.seed(&fixture.ours, "Vera Oldenburg", "vera@example.com");
    fixture.seed(&fixture.theirs, "Someone Else", "else@example.com");

    let (state, contacts) = fixture.sync().list_existing().unwrap();

    assert_eq!(contacts.len(), 1, "the other book must not leak in");
    assert_eq!(contacts[0].uid, mine.to_string());
    assert!(contacts[0].vcard.contains("FN:Vera Oldenburg"));
    assert!(!contacts[0].revision.is_empty());
    assert!(!state.as_str().is_empty());
}

#[test]
fn list_existing_on_an_empty_book_still_yields_a_state() {
    let fixture = Fixture::start();
    fixture.seed(&fixture.theirs, "Someone Else", "else@example.com");

    let (state, contacts) = fixture.sync().list_existing().unwrap();

    assert!(contacts.is_empty());
    assert!(!state.as_str().is_empty());
}

#[test]
fn the_revision_tracks_the_mapped_content_and_nothing_else() {
    let fixture = Fixture::start();
    let id = fixture.seed(&fixture.ours, "Vera Oldenburg", "vera@example.com");
    let sync = fixture.sync();
    let before = sync.load_contact(id.as_str()).unwrap().revision;

    // A property the vCard mapping drops: EDS cannot see it change, so
    // re-downloading every card because of it would be pure churn.
    fixture.patch(&id, json!({"notes": {"n1": {"note": "met at FOSDEM"}}}));
    assert_eq!(sync.load_contact(id.as_str()).unwrap().revision, before);

    // One it carries: the user's employer is on the card they are shown.
    fixture.patch(&id, json!({"organizations": {"o1": {"name": "Acme"}}}));
    let with_employer = sync.load_contact(id.as_str()).unwrap().revision;
    assert_ne!(with_employer, before);

    fixture.patch(&id, json!({"name/full": "Vera Oldenburg-Meier"}));
    assert_ne!(
        sync.load_contact(id.as_str()).unwrap().revision,
        with_employer
    );
}

#[test]
fn load_contact_reports_an_unknown_identifier_as_not_found() {
    let fixture = Fixture::start();
    let error = fixture.sync().load_contact("no-such-card").unwrap_err();

    assert!(
        matches!(&error, jmap_book_sync::SyncError::NotFound(uid) if uid == "no-such-card"),
        "{error:?}"
    );
}

#[test]
fn get_changes_reports_creations_updates_and_destructions() {
    let fixture = Fixture::start();
    // Present before the window: a card created *and* destroyed inside one
    // window is correctly reported in neither list, so it cannot stand in
    // for a destruction.
    let doomed = fixture.seed(&fixture.ours, "Ines Tollow", "ines@example.com");
    let edited = fixture.seed(&fixture.ours, "Ada Reinsch", "ada@example.com");
    let sync = fixture.sync();
    let (state, _) = sync.list_existing().unwrap();

    let created = fixture.seed(&fixture.ours, "Vera Oldenburg", "vera@example.com");
    fixture.patch(&edited, json!({"name/full": "Ada Reinsch-Brandt"}));
    sync.remove_contact(doomed.as_str()).unwrap();

    let changes = sync.get_changes(&state).unwrap();

    let mut changed: Vec<&str> = changes.changed.iter().map(|c| c.uid.as_str()).collect();
    changed.sort_unstable();
    let mut expected = vec![created.as_str(), edited.as_str()];
    expected.sort_unstable();
    assert_eq!(changed, expected);
    assert!(
        changes
            .changed
            .iter()
            .any(|c| c.vcard.contains("FN:Ada Reinsch-Brandt")),
        "the changed card is rendered, not just named"
    );
    assert_eq!(changes.removed, vec![doomed.to_string()]);
    assert_ne!(changes.new_state, state);

    // Nothing has happened since, so the follow-up delta is empty.
    let quiet = sync.get_changes(&changes.new_state).unwrap();
    assert!(quiet.changed.is_empty() && quiet.removed.is_empty());
}

#[test]
fn get_changes_ignores_cards_in_another_address_book() {
    let fixture = Fixture::start();
    let sync = fixture.sync();
    let (state, _) = sync.list_existing().unwrap();

    fixture.seed(&fixture.theirs, "Someone Else", "else@example.com");

    let changes = sync.get_changes(&state).unwrap();
    assert!(changes.changed.is_empty(), "{:?}", changes.changed);
    assert!(changes.removed.is_empty(), "{:?}", changes.removed);
}

#[test]
fn a_card_moved_to_another_address_book_is_reported_as_removed() {
    let fixture = Fixture::start();
    let moved = fixture.seed(&fixture.ours, "Vera Oldenburg", "vera@example.com");
    let sync = fixture.sync();
    let (state, _) = sync.list_existing().unwrap();

    fixture.patch(
        &moved,
        json!({"addressBookIds": {fixture.theirs.to_string(): true}}),
    );

    // It comes back as an update, not a destruction; reporting it as changed
    // would leave the book showing a contact it no longer contains.
    let changes = sync.get_changes(&state).unwrap();
    assert!(changes.changed.is_empty(), "{:?}", changes.changed);
    assert_eq!(changes.removed, vec![moved.to_string()]);
}

#[test]
fn remove_contact_destroys_the_card() {
    let fixture = Fixture::start();
    let id = fixture.seed(&fixture.ours, "Vera Oldenburg", "vera@example.com");
    let sync = fixture.sync();

    sync.remove_contact(id.as_str()).unwrap();

    assert!(sync.list_existing().unwrap().1.is_empty());
    assert!(sync.remove_contact(id.as_str()).is_err(), "already gone");
}
