// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Detection of changes to the folder list: what `Mailbox/changes` can tell,
//! and what it cannot tell.

use jmap_client::{Client, Credentials};
use jmap_mail_sync::{FolderTree, FolderUpdate, MailSync};
use jmap_mock::{MockServer, MockServerBuilder};
use jmap_proto::mail::role;
use jmap_proto::{Id, State};

struct Fixture {
    server: MockServer,
    account_id: Id,
}

impl Fixture {
    fn start() -> Self {
        Self::started_with(MockServer::builder())
    }

    fn started_with(builder: MockServerBuilder) -> Self {
        let server = builder.start();
        let account_id = server.account_id();
        Self { server, account_id }
    }

    fn sync(&self) -> MailSync {
        let client = Client::connect(self.server.origin(), Credentials::none()).unwrap();
        MailSync::new(client, self.account_id.clone())
    }

    /// Change the account as a different client does: as a state transition,
    /// which `Mailbox/changes` reports.
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

    // This is why a delta is not applied folder by folder. `Mailbox/changes`
    // names only the parent, but all paths below it changed too.
    let (_, tree) = rebuilt(sync.folder_tree_since(&state).unwrap());
    assert!(tree.find("Projects/Alpha").is_none());
    assert!(tree.find("Archive of Projects/Alpha").is_some());
}

#[test]
fn a_state_the_server_cannot_calculate_from_is_not_a_failure() {
    let fixture = Fixture::start();
    fixture.edit(|account| account.seed_mailbox("Inbox", Some(role::INBOX)));
    let sync = fixture.sync();

    // A state from a different server, or a state that this server forgot.
    // Camel cannot report this to its caller, so the store lists the account
    // again.
    let (_, tree) = rebuilt(sync.folder_tree_since(&State::new("nonsense")).unwrap());
    assert_eq!(tree.len(), 1);
}

/// The same unknown state, with the Stalwart answer: `invalidArguments`, not
/// the `cannotCalculateChanges` of RFC 8620 §5.2. Camel cannot report it, so
/// the result must also be a new listing, not a permanent failure.
#[test]
fn a_real_stalwart_shaped_unrecognized_state_is_not_a_failure_either() {
    let fixture =
        Fixture::started_with(MockServer::builder().stalwart_shaped_unrecognized_changes_state());
    fixture.edit(|account| account.seed_mailbox("Inbox", Some(role::INBOX)));
    let sync = fixture.sync();

    let (_, tree) = rebuilt(sync.folder_tree_since(&State::new("nonsense")).unwrap());
    assert_eq!(tree.len(), 1);
}
