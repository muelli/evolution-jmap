// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Invariants of the mock's object store.

use jmap_mock::{ChangeKind, Store};
use jmap_proto::Id;

#[test]
fn id_allocation_unique_per_type() {
    let mut store: Store<String> = Store::new("X");
    let mut seen = std::collections::BTreeSet::new();
    for _ in 0..100 {
        assert!(seen.insert(store.alloc_id()), "duplicate id allocated");
    }
    assert!(seen.iter().all(|id| id.as_str().starts_with('X')));
}

#[test]
fn state_bumps_only_on_success() {
    let mut store: Store<String> = Store::new("X");
    let initial = store.state_counter();

    // A transaction that stages nothing must not advance state.
    store.transaction(|transaction| {
        assert!(!transaction.destroy(&Id::new("X999")));
    });
    assert_eq!(store.state_counter(), initial);

    // A transaction with one create advances state exactly once, however
    // many objects it touches.
    store.transaction(|transaction| {
        let a = transaction.alloc_id();
        transaction.create(a, "a".to_owned());
        let b = transaction.alloc_id();
        transaction.create(b, "b".to_owned());
    });
    assert_eq!(store.state_counter(), initial + 1);
}

#[test]
fn changes_log_matches_state_sequence() {
    let mut store: Store<String> = Store::new("X");

    let id = store.transaction(|transaction| {
        let id = transaction.alloc_id();
        transaction.create(id.clone(), "v1".to_owned());
        id
    });
    let after_create = store.state_counter();

    store.transaction(|transaction| {
        transaction.update(&id, "v2".to_owned());
    });
    store.transaction(|transaction| {
        transaction.destroy(&id);
    });

    let all: Vec<_> = store.changes_since(0).collect();
    assert_eq!(all.len(), 3);
    assert!(all.windows(2).all(|pair| pair[0].state < pair[1].state));
    assert_eq!(all[0].kind, ChangeKind::Created);
    assert_eq!(all[1].kind, ChangeKind::Updated);
    assert_eq!(all[2].kind, ChangeKind::Destroyed);
    assert!(all.iter().all(|change| change.id == id));

    // Slicing from a later state yields only later changes.
    let since_create: Vec<_> = store.changes_since(after_create).collect();
    assert_eq!(since_create.len(), 2);
    assert_eq!(since_create[0].kind, ChangeKind::Updated);
}
