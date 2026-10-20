// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Tests `MailSync::send_message` against a real JMAP server, with delivery
//! to a second account on the same server. `CamelTransport::send_sync` calls
//! this function.
//!
//! `send_email_delivers_to_a_second_account_on_the_real_server` in
//! `jmap-client/tests/live_server.rs` tests `Client::send_email` (`Email/set`
//! and `EmailSubmission/set`) with delivery to a second account. It does not
//! test `MailSync::send_message`: the upload, staging and submission through
//! `import_message`, the identity lookup of `MailSync::identity_for`, and the
//! Drafts and Sent selection of `MailSync::outgoing_mailboxes`.
//! `jmap-mail-sync/tests/send.rs` tests these against `jmap-mock` only. This
//! file is their live-server counterpart, with the same two-account setup as
//! the `jmap-client` send test.
//!
//! The first test also checks the premise of `Outgoing::accepted_patch` (see
//! `jmap-mail-sync/src/send.rs`). When the server accepts the submission, it
//! must also apply the `onSuccessUpdateEmail` patch of `$draft`, `$seen` and
//! `mailboxIds`. Thus the test lists the Drafts and Sent mailboxes of the
//! sender after the send. In `tests/send.rs`, this test does the same check
//! against `jmap-mock`:
//! `a_sent_message_leaves_the_mailbox_it_was_staged_in_for_the_one_it_is_filed_in`.
//!
//! The second test checks the reason for `Outgoing::envelope`: Stalwart must
//! deliver to `envelope.rcptTo` when no header contains the recipient address,
//! as for a Bcc recipient. In the first test, the envelope recipient is also
//! the `To` recipient.
//!
//! ## Running it
//!
//! See `docs/manual-test-live-server.md`, steps 3 and 3a. Set
//! `JMAP_LIVE_SERVER_URL`, `JMAP_LIVE_SERVER_WRITE_USER`,
//! `JMAP_LIVE_SERVER_WRITE_PASSWORD`, `JMAP_LIVE_SERVER_RECIPIENT_USER` and
//! `JMAP_LIVE_SERVER_RECIPIENT_PASSWORD`, then:
//!
//! ```console
//! $ cargo test -p evolution-jmap-mail-sync --test live_server_send -- --ignored
//! ```
//!
//! This crate has no `live-server` feature. `#[ignore]` keeps the test out of
//! a plain `cargo test`.
//!
//! The tests skip when `JMAP_LIVE_SERVER_WRITE_USER` or
//! `JMAP_LIVE_SERVER_RECIPIENT_USER` is unset. They panic when one of these
//! variables is set but its `_PASSWORD` variable or `JMAP_LIVE_SERVER_URL` is
//! unset.

use std::env;
use std::time::Duration;

use jmap_client::{Client, Credentials};
use jmap_mail_sync::{FolderRole, MailSync, Outgoing};
use jmap_proto::mail::{Envelope, EnvelopeAddress};
use jmap_proto::session::CAPABILITY_MAIL;

/// Returns a value unique to this run, so that a message from a different run
/// cannot match.
fn unique_suffix() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos()
}

/// Same as `connect_for_write` in `jmap-client/tests/live_server.rs`.
fn connect_for_write() -> Option<Client> {
    let user = env::var("JMAP_LIVE_SERVER_WRITE_USER").ok()?;
    let password = env::var("JMAP_LIVE_SERVER_WRITE_PASSWORD")
        .expect("JMAP_LIVE_SERVER_WRITE_USER is set but JMAP_LIVE_SERVER_WRITE_PASSWORD is not");
    let origin = env::var("JMAP_LIVE_SERVER_URL")
        .expect("set JMAP_LIVE_SERVER_URL alongside JMAP_LIVE_SERVER_WRITE_USER");
    let rebase = env::var("JMAP_LIVE_SERVER_REBASE_URLS").is_ok_and(|value| value != "0");

    Some(
        Client::builder()
            .rebase_urls_to_origin(rebase)
            .connect(&origin, Credentials::basic(user, password))
            .expect("could not fetch the session document for the write-test account"),
    )
}

/// Same as `connect_recipient` in `jmap-client/tests/live_server.rs`.
fn connect_recipient() -> Option<Client> {
    let user = env::var("JMAP_LIVE_SERVER_RECIPIENT_USER").ok()?;
    let password = env::var("JMAP_LIVE_SERVER_RECIPIENT_PASSWORD").expect(
        "JMAP_LIVE_SERVER_RECIPIENT_USER is set but JMAP_LIVE_SERVER_RECIPIENT_PASSWORD is not",
    );
    let origin = env::var("JMAP_LIVE_SERVER_URL")
        .expect("set JMAP_LIVE_SERVER_URL alongside JMAP_LIVE_SERVER_RECIPIENT_USER");
    let rebase = env::var("JMAP_LIVE_SERVER_REBASE_URLS").is_ok_and(|value| value != "0");

    Some(
        Client::builder()
            .rebase_urls_to_origin(rebase)
            .connect(&origin, Credentials::basic(user, password))
            .expect("could not fetch the session document for the recipient account"),
    )
}

/// Sends a message from the write-test account to the recipient account and
/// polls `MailSync::messages` of the recipient until the message arrives.
/// This shows delivery, not only an accepted submission. Then it checks the
/// Drafts and Sent mailboxes of the sender.
#[test]
#[ignore = "needs a real JMAP server; see docs/manual-test-live-server.md"]
fn sending_a_message_delivers_to_a_second_account_on_the_real_server() {
    let Some(sender_client) = connect_for_write() else {
        eprintln!("JMAP_LIVE_SERVER_WRITE_USER/_PASSWORD not set; skipping the send test");
        return;
    };
    let Some(recipient_client) = connect_recipient() else {
        eprintln!("JMAP_LIVE_SERVER_RECIPIENT_USER/_PASSWORD not set; skipping the send test");
        return;
    };

    let sender_email = env::var("JMAP_LIVE_SERVER_WRITE_USER").unwrap();
    let recipient_email = env::var("JMAP_LIVE_SERVER_RECIPIENT_USER").unwrap();

    let sender_account_id = sender_client
        .primary_account(CAPABILITY_MAIL)
        .expect("the write-test account needs the mail capability");
    let sync = MailSync::new(sender_client, sender_account_id);

    let identity = sync
        .identity_for(&sender_email)
        .expect("the write-test account needs a sending identity for its own address");
    let mailboxes = sync
        .outgoing_mailboxes()
        .expect("the write-test account needs somewhere to stage an outgoing message");
    let staging = mailboxes.staging.clone();
    let destination = mailboxes
        .destination
        .clone()
        .expect("the write-test account needs a Sent mailbox to prove the filing half of this");

    let subject = format!("agent-mailsync-send-{}", unique_suffix());
    let message = format!(
        "From: {sender_email}\r\n\
         To: {recipient_email}\r\n\
         Subject: {subject}\r\n\
         Message-ID: <{subject}@agent-livewrite.net>\r\n\
         Date: Thu, 15 Jan 2026 09:30:00 +0000\r\n\
         \r\n\
         Sent via MailSync::send_message against a real server.\r\n"
    );

    let uid = sync
        .send_message(Outgoing {
            source: message.into_bytes(),
            identity,
            envelope: Some(Envelope {
                mail_from: EnvelopeAddress::new(sender_email.clone()),
                rcpt_to: vec![EnvelopeAddress::new(recipient_email.clone())],
            }),
            staging: mailboxes.staging,
            destination: mailboxes.destination,
        })
        .expect("MailSync::send_message failed against the real server");

    let recipient_account_id = recipient_client
        .primary_account(CAPABILITY_MAIL)
        .expect("the recipient account needs the mail capability");
    let recipient_sync = MailSync::new(recipient_client, recipient_account_id);
    let (_, tree) = recipient_sync
        .folder_tree()
        .expect("listing the recipient's folder tree failed");
    let inbox_id = tree
        .role(FolderRole::Inbox)
        .expect("the recipient account needs an Inbox")
        .id
        .clone();

    // Delivery can occur after `send_message` returns. Poll at most 20 times,
    // with 1 second between attempts.
    let mut delivered = None;
    for _ in 0..20 {
        let (_, messages) = recipient_sync
            .messages(&inbox_id)
            .expect("listing the recipient's Inbox failed");
        if let Some(row) = messages
            .into_iter()
            .find(|row| row.subject.as_deref() == Some(subject.as_str()))
        {
            delivered = Some(row);
            break;
        }
        std::thread::sleep(Duration::from_secs(1));
    }
    let delivered = delivered.unwrap_or_else(|| {
        panic!("the message sent via MailSync::send_message (uid {uid}) never showed up in the recipient's Inbox after 20s of polling")
    });

    assert_eq!(delivered.subject.as_deref(), Some(subject.as_str()));

    // The premise of `Outgoing::accepted_patch`: when the server accepts the
    // submission, it also applies the `$draft`, `$seen` and `mailboxIds`
    // patch.
    let (_, staging_messages) = sync
        .messages(&staging)
        .expect("listing the sender's own staging mailbox failed");
    assert!(
        staging_messages
            .iter()
            .all(|row| row.subject.as_deref() != Some(subject.as_str())),
        "the sent message (uid {uid}) is still left behind in the staging mailbox on the real server"
    );

    let (_, destination_messages) = sync
        .messages(&destination)
        .expect("listing the sender's own destination mailbox failed");
    let filed = destination_messages
        .into_iter()
        .find(|row| row.subject.as_deref() == Some(subject.as_str()))
        .unwrap_or_else(|| {
            panic!(
                "the sent message (uid {uid}) never showed up in the sender's own destination \
                 mailbox on the real server"
            )
        });
    assert!(
        !filed.flags.draft,
        "the sent message (uid {uid}) still carries $draft on the real server"
    );
    assert!(
        filed.flags.seen,
        "the sent message (uid {uid}) came back unread in the sender's own mailbox"
    );
}

/// Sends a message whose headers name only the sender. Only the envelope
/// names the recipient, as for a Bcc recipient.
///
/// `jmap-mail/src/envelope.rs` keeps the envelope apart from the headers for
/// this case. A server that delivers from the headers, not from
/// `envelope.rcptTo`, does not deliver to a Bcc recipient.
#[test]
#[ignore = "needs a real JMAP server; see docs/manual-test-live-server.md"]
fn an_envelope_recipient_absent_from_every_header_is_still_delivered() {
    let Some(sender_client) = connect_for_write() else {
        eprintln!("JMAP_LIVE_SERVER_WRITE_USER/_PASSWORD not set; skipping the send test");
        return;
    };
    let Some(recipient_client) = connect_recipient() else {
        eprintln!("JMAP_LIVE_SERVER_RECIPIENT_USER/_PASSWORD not set; skipping the send test");
        return;
    };

    let sender_email = env::var("JMAP_LIVE_SERVER_WRITE_USER").unwrap();
    let recipient_email = env::var("JMAP_LIVE_SERVER_RECIPIENT_USER").unwrap();

    let sender_account_id = sender_client
        .primary_account(CAPABILITY_MAIL)
        .expect("the write-test account needs the mail capability");
    let sync = MailSync::new(sender_client, sender_account_id);

    let identity = sync
        .identity_for(&sender_email)
        .expect("the write-test account needs a sending identity for its own address");
    let mailboxes = sync
        .outgoing_mailboxes()
        .expect("the write-test account needs somewhere to stage an outgoing message");

    let subject = format!("agent-mailsync-bcc-{}", unique_suffix());
    // `To` names the sender. The recipient address is not in the source
    // bytes, as for a real Bcc.
    let message = format!(
        "From: {sender_email}\r\n\
         To: {sender_email}\r\n\
         Subject: {subject}\r\n\
         Message-ID: <{subject}@agent-livewrite.net>\r\n\
         Date: Thu, 15 Jan 2026 09:30:00 +0000\r\n\
         \r\n\
         Sent via MailSync::send_message's envelope, with no header naming the recipient.\r\n"
    );

    let uid = sync
        .send_message(Outgoing {
            source: message.into_bytes(),
            identity,
            envelope: Some(Envelope {
                mail_from: EnvelopeAddress::new(sender_email.clone()),
                rcpt_to: vec![EnvelopeAddress::new(recipient_email.clone())],
            }),
            staging: mailboxes.staging,
            destination: mailboxes.destination,
        })
        .expect("MailSync::send_message failed against the real server");

    let recipient_account_id = recipient_client
        .primary_account(CAPABILITY_MAIL)
        .expect("the recipient account needs the mail capability");
    let recipient_sync = MailSync::new(recipient_client, recipient_account_id);
    let (_, tree) = recipient_sync
        .folder_tree()
        .expect("listing the recipient's folder tree failed");
    let inbox_id = tree
        .role(FolderRole::Inbox)
        .expect("the recipient account needs an Inbox")
        .id
        .clone();

    // Delivery can occur after `send_message` returns. Poll at most 20 times,
    // with 1 second between attempts.
    let mut delivered = None;
    for _ in 0..20 {
        let (_, messages) = recipient_sync
            .messages(&inbox_id)
            .expect("listing the recipient's Inbox failed");
        if let Some(row) = messages
            .into_iter()
            .find(|row| row.subject.as_deref() == Some(subject.as_str()))
        {
            delivered = Some(row);
            break;
        }
        std::thread::sleep(Duration::from_secs(1));
    }
    let delivered = delivered.unwrap_or_else(|| {
        panic!(
            "the message sent via MailSync::send_message (uid {uid}) with an envelope-only \
             recipient never showed up in that recipient's Inbox after 20s of polling — the \
             real server appears to deliver from headers, not the envelope"
        )
    });

    assert_eq!(delivered.subject.as_deref(), Some(subject.as_str()));
    assert!(
        delivered.to.iter().all(|to| to.email != recipient_email),
        "the delivered message's own To header names the envelope-only recipient \
         ({recipient_email}); it should name only the sender, the same way a real Bcc \
         recipient's address never appears in the headers"
    );
}
