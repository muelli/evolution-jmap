// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! `MailSync::send_message` against a real JMAP server, with actual
//! intra-server delivery — the function `CamelTransport::send_sync` actually
//! calls, exercised end to end for the first time.
//!
//! `jmap-client/tests/live_server.rs::
//! send_email_delivers_to_a_second_account_on_the_real_server` already proves
//! `Client::send_email` (`Email/set` + `EmailSubmission/set`, chained) against
//! real Stalwart, including delivery to a second account. But nothing has
//! ever driven `MailSync::send_message` itself: the upload-then-stage-then-
//! submit sequencing through `import_message`, `MailSync::identity_for`'s
//! address-to-identity lookup, and `MailSync::outgoing_mailboxes`'s Drafts/
//! Sent staging decision — only `jmap-mockd` has (`jmap-mail-sync/tests/
//! send.rs`). This file is their live-server counterpart, following the same
//! two-account recipe `jmap-client`'s own send-email test uses.
//!
//! ## Running it
//!
//! Same environment as the other sync crates' live-server tests, plus the
//! recipient account from `docs/manual-test-live-server.md` step 3a — see
//! that file. In short, with `JMAP_LIVE_SERVER_URL`/`_WRITE_USER`/
//! `_WRITE_PASSWORD`/`_RECIPIENT_USER`/`_RECIPIENT_PASSWORD` already set up:
//!
//! ```console
//! $ cargo test -p evolution-jmap-mail-sync --test live_server_send -- --ignored
//! ```
//!
//! No `--features live-server` gate is needed here — like the other sync
//! crates, this crate has no such feature, and `#[ignore]` alone already
//! keeps it out of a plain `cargo test`.
//!
//! Skipped, not failed, when `JMAP_LIVE_SERVER_WRITE_USER`/`_PASSWORD` or
//! `JMAP_LIVE_SERVER_RECIPIENT_USER`/`_PASSWORD` are unset — the same
//! tolerance every write-path test in this repository gives an unconfigured
//! environment.

use std::env;
use std::time::Duration;

use jmap_client::{Client, Credentials};
use jmap_mail_sync::{FolderRole, MailSync, Outgoing};
use jmap_proto::mail::{Envelope, EnvelopeAddress};
use jmap_proto::session::CAPABILITY_MAIL;

/// A value unique to this process invocation, so a concurrent or prior run's
/// leftover message can never be mistaken for this run's own.
fn unique_suffix() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos()
}

/// Mirrors `jmap-cal-sync/tests/live_server.rs::connect_for_write` exactly.
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

/// Mirrors `jmap-client/tests/live_server.rs::connect_recipient` exactly.
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

/// Sends a message via `MailSync::send_message` from the write-test account
/// to the recipient account (both on `agent-livewrite.net`, per
/// `docs/manual-test-live-server.md`), and polls the recipient's own
/// `MailSync::messages` — not raw `Client::email_query`, to keep the
/// assertion on this crate's own read path — until the message actually
/// lands, proving delivery rather than only an accepted submission.
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

    // Local delivery is not necessarily synchronous with `send_message`
    // returning, so poll rather than assume it has already landed.
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
}
