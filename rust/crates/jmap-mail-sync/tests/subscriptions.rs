// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Whether the user wants to see a folder, as the `Mailbox/set` that says so.
//!
//! Camel asks this with two vfuncs of `CamelSubscribable` —
//! `subscribe_folder_sync` and `unsubscribe_folder_sync` — and JMAP answers
//! both with the same property: RFC 8621 §2 gives a `Mailbox` an
//! `isSubscribed`, which is the server's record of a decision that belongs to
//! the user rather than to the account. So the two vfuncs are one update with
//! two values, which is why there is one method here and not two.
//!
//! The two things worth pinning are that the write reaches the *server* — a
//! subscription kept only in Evolution's own state is one the user's other
//! client never learns about, which is the whole reason the property exists —
//! and that it touches nothing else, neither the mailbox's siblings nor the
//! mail inside it.

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

    /// A top-level mailbox of the account, by name.
    fn seed_mailbox(&self, name: &str) -> Id {
        let state = self.server.state();
        let mut state = state.lock().unwrap();
        state
            .account_mut(&self.account_id)
            .unwrap()
            .seed_mailbox(name, None)
    }

    /// What the account's folder tree says about the folder at `path`.
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

/// A mailbox that is already subscribed being subscribed again is what Camel
/// does whenever the user reopens the subscription editor and presses OK, so it
/// has to be an ordinary success rather than a refusal.
#[test]
fn a_subscription_that_changes_nothing_is_still_a_success() {
    let fixture = Fixture::start();
    let inbox = fixture.seed_mailbox("Inbox");

    fixture.sync().set_subscribed(&inbox, true).unwrap();

    assert_eq!(fixture.subscribed("Inbox"), Some(true));
}

/// The property is the mailbox's own. A patch naming it must not be read as a
/// statement about the folders next to it — a subscription editor unticking one
/// folder and hiding the account is the bug this pins.
#[test]
fn one_mailbox_unsubscribed_leaves_its_siblings_alone() {
    let fixture = Fixture::start();
    let inbox = fixture.seed_mailbox("Inbox");
    fixture.seed_mailbox("Sent");

    fixture.sync().set_subscribed(&inbox, false).unwrap();

    assert_eq!(fixture.subscribed("Inbox"), Some(false));
    assert_eq!(fixture.subscribed("Sent"), Some(true));
}

/// And it says nothing about what is in the folder: unsubscribing hides a
/// folder from a client, it does not empty it.
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

/// A folder another client deleted while this one still lists it. The user
/// unticking it is then a decision about something that is already gone, which
/// is ordinary rather than a broken account — and the layer above can only say
/// so if the distinction survives this crate.
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
