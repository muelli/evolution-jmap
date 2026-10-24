// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Smoke test a JMAP client against the in-memory mock server.

use jmap_client::{Client, Credentials};
use jmap_mock::MockServer;
use jmap_proto::mail::Mailbox;
use jmap_proto::session::CAPABILITY_MAIL;

fn main() {
    let server = MockServer::builder().start();
    let client = Client::connect(server.origin(), Credentials::none())
        .expect("client should connect to mock server");
    let account_id = client
        .primary_account(CAPABILITY_MAIL)
        .expect("mock session should include a primary mail account");

    client
        .mailbox_create(&account_id, &Mailbox::new("Client example mailbox"))
        .expect("Mailbox/set create should succeed");

    let mailboxes = client
        .mailbox_get(&account_id)
        .expect("Mailbox/get should succeed");
    assert!(
        mailboxes
            .list
            .iter()
            .any(|mailbox| mailbox.name == "Client example mailbox"),
        "created mailbox should be visible through Mailbox/get"
    );
}
