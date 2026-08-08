// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! The folder listing against a live mock server: the request `Mailbox/get`
//! actually is, and the counts a server actually reports.

use jmap_client::{Client, Credentials};
use jmap_mail_sync::{FolderRole, MailSync};
use jmap_mock::{EmailSeed, MockServer};
use jmap_proto::Id;
use jmap_proto::mail::{keyword, role};

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
}

#[test]
fn the_folder_tree_carries_the_mailboxes_the_server_reports() {
    let fixture = Fixture::start();
    {
        let state = fixture.server.state();
        let mut state = state.lock().unwrap();
        let account = state.account_mut(&fixture.account_id).unwrap();
        let inbox = account.seed_mailbox("Inbox", Some(role::INBOX));
        account.seed_mailbox("Sent", Some(role::SENT));
        account.seed_email(EmailSeed::new(
            inbox.clone(),
            ("Bob", "bob@example.com"),
            "Hello",
            "text",
            "2026-01-15T09:00:00Z",
        ));
        account.seed_email(
            EmailSeed::new(
                inbox,
                ("Bob", "bob@example.com"),
                "Read already",
                "text",
                "2026-01-15T10:00:00Z",
            )
            .keyword(keyword::SEEN),
        );
    }

    let tree = fixture.sync().folder_tree().unwrap();

    let inbox = tree.find("Inbox").expect("an inbox");
    assert_eq!(inbox.role, Some(FolderRole::Inbox));
    assert_eq!((inbox.total, inbox.unread), (2, 1));
    assert_eq!(
        tree.find("Sent").map(|folder| folder.role),
        Some(Some(FolderRole::Sent))
    );
    assert_eq!(tree.len(), 2);
}

#[test]
fn a_nested_mailbox_comes_back_nested() {
    let fixture = Fixture::start();
    let child = {
        let state = fixture.server.state();
        let mut state = state.lock().unwrap();
        let account = state.account_mut(&fixture.account_id).unwrap();
        let parent = account.seed_mailbox("Projects", None);
        account.seed_child_mailbox("JMAP", None, &parent)
    };

    let tree = fixture.sync().folder_tree().unwrap();

    assert_eq!(tree.roots().len(), 1, "the child must not also be a root");
    let folder = tree.find("Projects/JMAP").expect("the nested folder");
    assert_eq!(folder.id, child);
    assert_eq!(folder.display_name, "JMAP");
}

#[test]
fn an_account_without_mailboxes_yields_an_empty_tree() {
    let fixture = Fixture::start();

    let tree = fixture.sync().folder_tree().unwrap();

    assert!(tree.is_empty());
}
