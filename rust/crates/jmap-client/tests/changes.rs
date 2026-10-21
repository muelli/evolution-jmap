// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Incremental sync (`/changes`) semantics.

use std::collections::BTreeSet;

use jmap_client::limits::IdBudget;
use jmap_client::{ChangeSet, Client, Credentials, Error};
use jmap_mock::MockServer;
use jmap_proto::contacts::ContactCard;
use jmap_proto::{Id, State};
use serde_json::json;

/// `ChangeSet::is_empty` is a caller-facing shortcut over its three sets, not
/// exercised elsewhere in this crate: every other test reads the sets
/// directly (`full.created`, `middle.updated`, ...).
#[test]
fn change_set_is_empty_iff_all_three_sets_are() {
    let nothing = ChangeSet {
        new_state: State::new("s1"),
        created: BTreeSet::new(),
        updated: BTreeSet::new(),
        destroyed: BTreeSet::new(),
    };
    assert!(nothing.is_empty());

    let only_created = ChangeSet {
        created: BTreeSet::from([Id::new("a")]),
        ..nothing.clone()
    };
    assert!(!only_created.is_empty());

    let only_updated = ChangeSet {
        updated: BTreeSet::from([Id::new("a")]),
        ..nothing.clone()
    };
    assert!(!only_updated.is_empty());

    let only_destroyed = ChangeSet {
        destroyed: BTreeSet::from([Id::new("a")]),
        ..nothing
    };
    assert!(!only_destroyed.is_empty());
}

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

/// One account's edit history, and the changes a client sees over it.
struct Scenario {
    changes: ChangeSet,
    /// Created inside the window, then edited: visible as created.
    kept: Id,
    /// Created before the window, destroyed inside it: visible as destroyed.
    doomed: Id,
    /// Created and destroyed inside the window: visible not at all.
    born_and_gone: Id,
}

/// Play the same edits against a server that answers `/changes` in pages of
/// `page_size` (or, with `None`, one that never pages) and collect
/// everything that changed.
fn edit_history(page_size: Option<u64>) -> Scenario {
    let mut builder = MockServer::builder();
    if let Some(page_size) = page_size {
        builder = builder.changes_page_size(page_size);
    }
    let server = builder.start();
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

    let create = |name: &str| {
        client
            .contact_create(
                &account_id,
                &ContactCard::simple(book.clone(), name, "someone@example.com"),
            )
            .unwrap()
            .id
            .unwrap()
    };

    let doomed = create("Doomed");
    let since = client.contact_state(&account_id).unwrap();

    let kept = create("Keep Me");
    client
        .contact_update(&account_id, &kept, json!({"name/full": "Keep Me Longer"}))
        .unwrap();
    let born_and_gone = create("Ephemeral");
    client.contact_destroy(&account_id, &born_and_gone).unwrap();
    client.contact_destroy(&account_id, &doomed).unwrap();

    Scenario {
        changes: client
            .all_changes(&account_id, "ContactCard", &since)
            .unwrap(),
        kept,
        doomed,
        born_and_gone,
    }
}

#[test]
fn a_capped_response_says_there_is_more_and_where_to_resume() {
    let server = MockServer::builder().changes_page_size(1).start();
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
    let since = client.contact_state(&account_id).unwrap();

    let first_card = client
        .contact_create(
            &account_id,
            &ContactCard::simple(book.clone(), "First", "first@example.com"),
        )
        .unwrap()
        .id
        .unwrap();
    let second_card = client
        .contact_create(
            &account_id,
            &ContactCard::simple(book, "Second", "second@example.com"),
        )
        .unwrap()
        .id
        .unwrap();

    let first = client.changes(&account_id, "ContactCard", &since).unwrap();
    assert_eq!(first.created, vec![first_card]);
    assert!(first.has_more_changes, "one of two creates was withheld");
    assert_ne!(
        first.new_state, since,
        "a page the client cannot resume from is a page it must re-fetch forever"
    );

    let second = client
        .changes(&account_id, "ContactCard", &first.new_state)
        .unwrap();
    assert_eq!(second.created, vec![second_card]);
    assert!(!second.has_more_changes);
    assert_eq!(
        second.new_state,
        client.contact_state(&account_id).unwrap(),
        "the last page ends at the current state"
    );
}

#[test]
fn following_every_page_answers_what_one_page_would_have() {
    let whole = edit_history(None);

    assert_eq!(
        whole.changes.created,
        BTreeSet::from([whole.kept.clone()]),
        "a card created and then edited in-window is created, not updated"
    );
    assert!(whole.changes.updated.is_empty());
    assert_eq!(whole.changes.destroyed, BTreeSet::from([whole.doomed]));
    assert!(
        !whole.changes.created.contains(&whole.born_and_gone)
            && !whole.changes.destroyed.contains(&whole.born_and_gone),
        "a card the client never saw exist is not a card it must be told about"
    );

    // The point of the whole exercise: how the server chose to split its
    // answer is not something the caller can observe.
    let paged = edit_history(Some(1));
    assert_eq!(paged.changes, whole.changes);
}

/// `edit_history`'s scenario never updates a card that existed *before* the
/// window without also creating or destroying it inside the window, so it
/// never reaches `ChangeSet::classify`'s "updated only" arm: every id it
/// folds is either created-then-updated (classified created) or destroyed.
/// This is the one case that arm exists for.
#[test]
fn a_card_from_before_the_window_edited_inside_it_classifies_as_updated() {
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

    let existing = client
        .contact_create(
            &account_id,
            &ContactCard::simple(book, "Existing", "existing@example.com"),
        )
        .unwrap()
        .id
        .unwrap();
    let since = client.contact_state(&account_id).unwrap();

    client
        .contact_update(
            &account_id,
            &existing,
            json!({"name/full": "Existing, Renamed"}),
        )
        .unwrap();

    let changes = client
        .all_changes(&account_id, "ContactCard", &since)
        .unwrap();
    assert_eq!(changes.updated, BTreeSet::from([existing]));
    assert!(changes.created.is_empty());
    assert!(changes.destroyed.is_empty());
}

/// A server that answers `/changes` in pages of one id, the state before three
/// new cards, and the ids of these cards.
fn three_new_cards() -> (MockServer, Id, State, Vec<Id>) {
    let server = MockServer::builder().changes_page_size(1).start();
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
    let since = client.contact_state(&account_id).unwrap();
    let ids = ["One", "Two", "Three"]
        .into_iter()
        .map(|name| {
            client
                .contact_create(
                    &account_id,
                    &ContactCard::simple(book.clone(), name, "card@example.com"),
                )
                .unwrap()
                .id
                .unwrap()
        })
        .collect();
    (server, account_id, since, ids)
}

/// The changes since `since`, read by a client with `budget`.
fn changes_within(
    server: &MockServer,
    account_id: &Id,
    since: &State,
    budget: IdBudget,
) -> Result<ChangeSet, Error> {
    Client::builder()
        .id_budget(budget)
        .connect(server.origin(), Credentials::none())
        .unwrap()
        .all_changes(account_id, "ContactCard", since)
}

fn assert_refused_for_budget(result: Result<ChangeSet, Error>) {
    match result {
        Err(Error::Protocol(message)) => assert!(
            message.contains("ContactCard/changes") && message.contains("budget"),
            "{message}"
        ),
        other => panic!("a stream past the budget was accepted: {other:?}"),
    }
}

#[test]
fn a_changes_stream_past_the_id_budget_is_refused() {
    let (server, account_id, since, ids) = three_new_cards();

    assert_refused_for_budget(changes_within(
        &server,
        &account_id,
        &since,
        IdBudget::new(2, usize::MAX),
    ));
    let exact = changes_within(&server, &account_id, &since, IdBudget::new(3, usize::MAX));
    assert_eq!(exact.unwrap().created, ids.into_iter().collect());
}

#[test]
fn a_changes_stream_past_the_id_byte_budget_is_refused() {
    let (server, account_id, since, ids) = three_new_cards();
    let bytes: usize = ids.iter().map(|id| id.as_str().len()).sum();

    assert_refused_for_budget(changes_within(
        &server,
        &account_id,
        &since,
        IdBudget::new(usize::MAX, bytes - 1),
    ));
    let exact = changes_within(
        &server,
        &account_id,
        &since,
        IdBudget::new(usize::MAX, bytes),
    );
    assert_eq!(exact.unwrap().created.len(), 3);
}

/// The budget counts each id once, so a server that repeats one id on many
/// pages does not use it up.
#[test]
fn an_id_on_many_pages_counts_once() {
    let (server, account_id, since, ids) = three_new_cards();
    let client = Client::connect(server.origin(), Credentials::none()).unwrap();
    for id in &ids {
        client
            .contact_update(&account_id, id, json!({"name/full": "Edited"}))
            .unwrap();
    }

    let changes = changes_within(&server, &account_id, &since, IdBudget::new(3, usize::MAX));
    assert_eq!(changes.unwrap().created, ids.into_iter().collect());
}

#[test]
fn the_default_budget_is_the_per_sync_one() {
    assert_eq!(IdBudget::default(), IdBudget::per_sync());
    assert_eq!(
        IdBudget::per_sync(),
        IdBudget::new(
            jmap_client::limits::MAX_SYNC_IDS,
            jmap_client::limits::MAX_SYNC_ID_BYTES
        )
    );
}
