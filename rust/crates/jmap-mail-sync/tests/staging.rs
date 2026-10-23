// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! The staging and destination mailboxes of an outgoing message, against a
//! mock server.
//!
//! [`Outgoing`] has two mailbox ids: the import mailbox during the send, and
//! the mailbox for the message after the server accepts the submission. A
//! `CamelTransport` gets neither. The Camel `send_to_sync` gets a message and
//! two address lists, but no folders. Thus the method under test finds the two
//! mailboxes in the account.
//!
//! It finds them by *role*, never by name. RFC 8621 §2 gives a mailbox a
//! `role` for this purpose. An account with drafts in `Entwürfe` is a normal
//! account that can send.

use jmap_mail_sync::SyncError;

mod common;
use common::Fixture;

#[test]
fn a_message_waits_in_drafts_and_is_filed_in_sent() {
    let fixture = Fixture::start();
    fixture.seed_mailbox("Inbox", Some("inbox"));
    let drafts = fixture.seed_mailbox("Drafts", Some("drafts"));
    let sent = fixture.seed_mailbox("Sent", Some("sent"));

    let mailboxes = fixture
        .sync()
        .outgoing_mailboxes()
        .expect("the account can send");

    // The usual account. Staging and destination are different, because a
    // message in Sent was sent, and a message in submission was not.
    assert_eq!(mailboxes.staging, drafts);
    assert_eq!(mailboxes.destination, Some(sent));
    // The server files the copy of the user, so the caller must not keep a
    // second copy.
    assert!(mailboxes.saves_sent_copy);
}

#[test]
fn the_mailboxes_are_the_ones_the_account_gave_the_role_to() {
    let fixture = Fixture::start();
    // A German account, with two decoys that have English names and no role.
    // A match by name puts outgoing mail in a random folder called Drafts,
    // and it does not use the Sent mailbox of the server.
    fixture.seed_mailbox("Drafts", None);
    fixture.seed_mailbox("Sent", None);
    let drafts = fixture.seed_mailbox("Entwürfe", Some("drafts"));
    let sent = fixture.seed_mailbox("Gesendet", Some("sent"));

    let mailboxes = fixture
        .sync()
        .outgoing_mailboxes()
        .expect("the account can send");

    assert_eq!(mailboxes.staging, drafts);
    assert_eq!(mailboxes.destination, Some(sent));
}

#[test]
fn an_account_with_no_sent_mailbox_leaves_the_message_where_it_waited() {
    let fixture = Fixture::start();
    fixture.seed_mailbox("Inbox", Some("inbox"));
    let drafts = fixture.seed_mailbox("Drafts", Some("drafts"));

    let mailboxes = fixture
        .sync()
        .outgoing_mailboxes()
        .expect("the account can send");

    // No destination, not a guess. RFC 8621 §4.6 requires at least one mailbox,
    // so the copy stays in Drafts. [`Outgoing::accepted_patch`] removes its
    // draft flag.
    assert_eq!(mailboxes.staging, drafts);
    assert_eq!(mailboxes.destination, None);
    // The only case where the sent copy is *not* in a Sent mailbox, because
    // there is none. The caller gets this information, and Evolution keeps its
    // own copy as configured.
    assert!(!mailboxes.saves_sent_copy);
}

#[test]
fn an_account_with_no_drafts_waits_in_the_mailbox_it_will_be_filed_in() {
    let fixture = Fixture::start();
    fixture.seed_mailbox("Inbox", Some("inbox"));
    let sent = fixture.seed_mailbox("Sent", Some("sent"));

    let mailboxes = fixture
        .sync()
        .outgoing_mailboxes()
        .expect("the account can send");

    // Without Drafts, the message stages in Sent, which is its destination.
    // Thus there is no destination: a move into the same mailbox is not
    // possible.
    assert_eq!(mailboxes.staging, sent);
    assert_eq!(mailboxes.destination, None);
    // This case is why the field exists, and not `destination.is_some()`. There
    // is no destination, but the copy *is* in Sent, because it stages there.
    // If the caller reads "not saved", Evolution appends a second copy of each
    // sent message.
    assert!(mailboxes.saves_sent_copy);
}

#[test]
fn an_account_with_nowhere_to_put_an_outgoing_message_cannot_send() {
    let fixture = Fixture::start();
    // The account has folders, but none of them is for mail that the user
    // writes.
    fixture.seed_mailbox("Inbox", Some("inbox"));
    fixture.seed_mailbox("Archive", Some("archive"));

    let error = fixture
        .sync()
        .outgoing_mailboxes()
        .expect_err("an account with no drafts and no sent mailbox can send");

    // Not the Inbox, and not the first folder. The server *delivers* to the
    // Inbox. An import of outgoing mail there makes false arrivals, also for a
    // message that the server possibly does not send.
    match error {
        SyncError::NoOutgoingFolder => {}
        other => panic!("expected the account to have nowhere to send from, got {other:?}"),
    }
}

#[test]
fn the_mailboxes_are_read_from_the_account_at_every_send() {
    let fixture = Fixture::start();
    let drafts = fixture.seed_mailbox("Drafts", Some("drafts"));
    let sync = fixture.sync();

    let before = sync.outgoing_mailboxes().expect("the account can send");
    assert_eq!(before.destination, None);
    assert!(!before.saves_sent_copy);

    // A different client (or the user in a different window) adds a Sent
    // mailbox. No cache keeps the tree, as for [`MailSync::identity_for`]: an
    // old listing can file sent mail in a deleted mailbox, or not find a new
    // mailbox.
    let sent = fixture.seed_mailbox("Sent", Some("sent"));

    let after = sync.outgoing_mailboxes().expect("the account can send");
    assert_eq!(after.staging, drafts);
    assert_eq!(after.destination, Some(sent));
    assert!(after.saves_sent_copy);
}
