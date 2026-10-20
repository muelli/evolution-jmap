// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! The folder subscription of the user, as a `Mailbox/set`.
//!
//! Camel uses two vfuncs of `CamelSubscribable`: `subscribe_folder_sync` and
//! `unsubscribe_folder_sync`. JMAP uses one property for both: RFC 8621 §2
//! gives a `Mailbox` the property `isSubscribed`. The server records a
//! decision of the user, not of the account. Thus there is one method with two
//! values, not two methods.
//!
//! The tests check two rules:
//!
//! - The write goes to the *server*. Other clients of the user cannot see a
//!   subscription that only Evolution keeps, and that is the purpose of the
//!   property.
//! - The write changes nothing else: not the siblings of the mailbox and not
//!   its mail.

use jmap_client::{Client, Credentials};
use jmap_mail_sync::{MailSync, SyncError};
use jmap_mock::{EmailSeed, MockServer};
use jmap_proto::Id;
use jmap_proto::mail::role;

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

    /// Seed a top-level mailbox with the given name.
    fn seed_mailbox(&self, name: &str) -> Id {
        let state = self.server.state();
        let mut state = state.lock().unwrap();
        state
            .account_mut(&self.account_id)
            .unwrap()
            .seed_mailbox(name, None)
    }

    /// The subscription state of the folder at `path` in the folder tree.
    fn subscribed(&self, path: &str) -> Option<bool> {
        let (_, tree) = self.sync().folder_tree().unwrap();
        tree.find(path).map(|folder| folder.subscribed)
    }
}

#[test]
fn unsubscribing_reaches_the_server() {
    let fixture = Fixture::start();
    let inbox = fixture.seed_mailbox("Inbox");

    fixture.sync().set_subscribed(&inbox, false).unwrap();

    assert_eq!(fixture.subscribed("Inbox"), Some(false));
}

#[test]
fn subscribing_again_reaches_it_too() {
    let fixture = Fixture::start();
    let inbox = fixture.seed_mailbox("Inbox");
    let sync = fixture.sync();

    sync.set_subscribed(&inbox, false).unwrap();
    sync.set_subscribed(&inbox, true).unwrap();

    assert_eq!(fixture.subscribed("Inbox"), Some(true));
}

/// Camel subscribes a subscribed mailbox again each time the user opens the
/// subscription editor and pushes OK. Thus this must succeed, not fail.
#[test]
fn a_subscription_that_changes_nothing_is_still_a_success() {
    let fixture = Fixture::start();
    let inbox = fixture.seed_mailbox("Inbox");

    fixture.sync().set_subscribed(&inbox, true).unwrap();

    assert_eq!(fixture.subscribed("Inbox"), Some(true));
}

/// The property applies only to its mailbox, not to the sibling folders. This
/// test prevents a bug where one unsubscribe hides the full account.
#[test]
fn one_mailbox_unsubscribed_leaves_its_siblings_alone() {
    let fixture = Fixture::start();
    let inbox = fixture.seed_mailbox("Inbox");
    fixture.seed_mailbox("Sent");

    fixture.sync().set_subscribed(&inbox, false).unwrap();

    assert_eq!(fixture.subscribed("Inbox"), Some(false));
    assert_eq!(fixture.subscribed("Sent"), Some(true));
}

/// The property also does not apply to the folder contents. An unsubscribe
/// hides a folder from a client, but does not remove its mail.
#[test]
fn unsubscribing_does_not_touch_the_mail_in_the_folder() {
    let fixture = Fixture::start();
    let inbox = {
        let state = fixture.server.state();
        let mut state = state.lock().unwrap();
        let account = state.account_mut(&fixture.account_id).unwrap();
        let inbox = account.seed_mailbox("Inbox", Some(role::INBOX));
        account.seed_email(EmailSeed::new(
            inbox.clone(),
            ("Bob", "bob@example.com"),
            "Lunch?",
            "text",
            "2026-01-15T09:00:00Z",
        ));
        inbox
    };

    fixture.sync().set_subscribed(&inbox, false).unwrap();

    let (_, messages) = fixture.sync().messages(&inbox).unwrap();
    assert_eq!(messages.len(), 1);
}

/// A different client deleted the folder, but this client still lists it. An
/// unsubscribe of a deleted folder is normal, not a broken account. The layer
/// above can only tell this if this crate keeps the difference.
#[test]
fn a_mailbox_the_account_does_not_have_is_no_such_folder() {
    let fixture = Fixture::start();
    fixture.seed_mailbox("Inbox");

    let failure = fixture
        .sync()
        .set_subscribed(&Id::new("M404"), false)
        .unwrap_err();

    match failure {
        SyncError::NoSuchFolder(id) => assert_eq!(id.as_str(), "M404"),
        other => panic!("expected a missing folder, got {other}"),
    }
}
