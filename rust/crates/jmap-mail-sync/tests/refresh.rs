// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Noticing that the folder list moved: what `Mailbox/changes` is good for,
//! and what it is not good enough for.

use jmap_client::{Client, Credentials};
use jmap_mail_sync::{FolderTree, FolderUpdate, MailSync};
use jmap_mock::MockServer;
use jmap_proto::mail::role;
use jmap_proto::{Id, State};

struct Fixture {
    server: MockServer,
    account_id: Id,
}

impl Fixture {
    fn start() -> Self {
        let server = MockServer::builder().start();
        let account_id = server.account_id();
        Self { server, account_id }
    }

    fn sync(&self) -> MailSync {
        let client = Client::connect(self.server.origin(), Credentials::none()).unwrap();
        MailSync::new(client, self.account_id.clone())
    }

    /// Mutate the account the way another client would — as a state
    /// transition, which is what `Mailbox/changes` reports.
    fn edit<R>(&self, edit: impl FnOnce(&mut jmap_mock::AccountState) -> R) -> R {
        let state = self.server.state();
        let mut state = state.lock().unwrap();
        edit(state.account_mut(&self.account_id).unwrap())
    }
}

fn rebuilt(update: FolderUpdate) -> (State, FolderTree) {
    match update {
        FolderUpdate::Rebuilt { state, tree } => (state, tree),
        FolderUpdate::Unchanged(state) => {
            panic!("expected a rebuilt tree, the server reported nothing new at {state}")
        }
    }
}

#[test]
fn the_folder_listing_carries_the_state_it_is_current_as_of() {
    let fixture = Fixture::start();
    fixture.edit(|account| account.seed_mailbox("Inbox", Some(role::INBOX)));
    let sync = fixture.sync();

    let (state, tree) = sync.folder_tree().unwrap();
    assert_eq!(tree.len(), 1);
    assert_eq!(
        state,
        fixture.edit(|account| account.mailboxes.state()),
        "the state is the one the listing was answered at, not one invented here"
    );
}

#[test]
fn an_account_nobody_touched_is_not_listed_again() {
    let fixture = Fixture::start();
    fixture.edit(|account| account.seed_mailbox("Inbox", Some(role::INBOX)));
    let sync = fixture.sync();
    let (state, _) = sync.folder_tree().unwrap();

    match sync.folder_tree_since(&state).unwrap() {
        FolderUpdate::Unchanged(unchanged) => assert_eq!(unchanged, state),
        FolderUpdate::Rebuilt { .. } => panic!("nothing changed, so nothing had to be re-listed"),
    }
}

#[test]
fn a_mailbox_another_client_made_brings_the_tree_back() {
    let fixture = Fixture::start();
    fixture.edit(|account| account.seed_mailbox("Inbox", Some(role::INBOX)));
    let sync = fixture.sync();
    let (state, _) = sync.folder_tree().unwrap();

    fixture.edit(|account| account.create_mailbox("Receipts", None, None));

    let (new_state, tree) = rebuilt(sync.folder_tree_since(&state).unwrap());
    assert!(tree.find("Receipts").is_some());
    assert_ne!(new_state, state);
    assert_eq!(
        new_state,
        fixture.edit(|account| account.mailboxes.state()),
        "the tree's state is the listing's, not the delta's: the listing is what was walked"
    );
}

#[test]
fn a_destroyed_mailbox_is_gone_from_the_tree_that_comes_back() {
    let fixture = Fixture::start();
    let doomed = fixture.edit(|account| {
        account.seed_mailbox("Inbox", Some(role::INBOX));
        account.seed_mailbox("Old Project", None)
    });
    let sync = fixture.sync();
    let (state, before) = sync.folder_tree().unwrap();
    assert!(before.find("Old Project").is_some());

    fixture.edit(|account| assert!(account.destroy_mailbox(&doomed)));

    let (_, tree) = rebuilt(sync.folder_tree_since(&state).unwrap());
    assert!(tree.find("Old Project").is_none());
}

#[test]
fn renaming_a_parent_moves_children_the_delta_never_names() {
    let fixture = Fixture::start();
    let parent = fixture.edit(|account| {
        let parent = account.seed_mailbox("Projects", None);
        account.seed_child_mailbox("Alpha", None, &parent);
        parent
    });
    let sync = fixture.sync();
    let (state, before) = sync.folder_tree().unwrap();
    assert!(before.find("Projects/Alpha").is_some());

    fixture.edit(|account| assert!(account.rename_mailbox(&parent, "Archive of Projects")));

    // The reason a delta is not applied folder by folder: `Mailbox/changes`
    // names the parent alone, and every path below it moved with it.
    let (_, tree) = rebuilt(sync.folder_tree_since(&state).unwrap());
    assert!(tree.find("Projects/Alpha").is_none());
    assert!(tree.find("Archive of Projects/Alpha").is_some());
}

#[test]
fn a_state_the_server_cannot_calculate_from_is_not_a_failure() {
    let fixture = Fixture::start();
    fixture.edit(|account| account.seed_mailbox("Inbox", Some(role::INBOX)));
    let sync = fixture.sync();

    // A state from another server, or one this one has forgotten. Camel has
    // no machinery for reporting that upwards — the store's only answer is to
    // list the account again, so that is the answer given here.
    let (_, tree) = rebuilt(sync.folder_tree_since(&State::new("nonsense")).unwrap());
    assert_eq!(tree.len(), 1);
}
