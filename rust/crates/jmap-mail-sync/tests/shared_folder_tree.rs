// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! `MailSync::folder_tree_with_shared` against a mock server, item 163's
//! next slice after the pure merge in `tests/shared_tree.rs`.

use jmap_proto::Id;

mod common;
use common::Fixture;

#[test]
fn no_shared_accounts_leaves_the_tree_unchanged() {
    let fixture = Fixture::start();
    fixture.seed_mailbox("Inbox", Some("inbox"));

    let (_, tree) = fixture.sync().folder_tree_with_shared().unwrap();

    assert!(tree.find("Inbox").is_some());
    assert!(tree.find("Shared").is_none());
}

#[test]
fn a_shared_account_s_own_mailboxes_appear_under_shared() {
    let fixture = Fixture::start();
    fixture.seed_mailbox("Inbox", Some("inbox"));

    let shared = Id::new("A2");
    let projects = {
        let state = fixture.server.state();
        let mut state = state.lock().unwrap();
        let sharer = state.add_account(shared.clone(), "Bob");
        sharer.is_personal = false;
        sharer.seed_mailbox("Projects", None)
    };

    let (_, tree) = fixture.sync().folder_tree_with_shared().unwrap();

    assert!(
        tree.find("Inbox").is_some(),
        "the primary account's own folder stays"
    );
    let shared_projects = tree
        .find("Shared/Bob/Projects")
        .expect("the shared account's folder merges in under its owner");
    assert_eq!(shared_projects.id, projects);
}
