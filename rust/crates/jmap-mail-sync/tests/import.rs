// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! The import of a message from the client into a mailbox, against a mock
//! server: the upload and the `Email/import`, the listing of the imported
//! message, and the two server refusals.
//!
//! The other message writes change a message that the account has. This call
//! (also used to stage a send) adds a message as *bytes*. Thus the tests
//! mostly check that the bytes stay the same: the listed row and the source
//! must both agree with the uploaded message.

use jmap_mail_sync::{Keywords, MessageFlags, MessageSummary, SyncError};
use jmap_mock::MockServer;
use jmap_proto::Id;
use jmap_proto::mail::email_import_error;

mod common;
use common::Fixture;

/// The RFC 5322 bytes of a message from Evolution: CRLF line endings, a header
/// block, a blank line, and a body.
const MESSAGE: &[u8] = b"From: Bob <bob@example.com>\r\n\
To: Alice <alice@example.com>\r\n\
Subject: Lunch?\r\n\
Message-ID: <lunch@example.com>\r\n\
Date: Thu, 15 Jan 2026 09:30:00 +0000\r\n\
\r\n\
One o'clock at the usual place.\r\n";

/// The single row of a mailbox. Fails with the count if it is not one row.
fn only(messages: Vec<MessageSummary>) -> MessageSummary {
    assert_eq!(messages.len(), 1, "one imported message");
    messages.into_iter().next().expect("the row just counted")
}

#[test]
fn an_imported_message_is_a_message_of_the_mailbox_it_was_filed_into() {
    let fixture = Fixture::start();
    let inbox = fixture.seed_mailbox("Inbox", None);

    let uid = fixture
        .sync()
        .import_message(&inbox, MESSAGE.to_vec(), &Keywords::default(), None)
        .expect("the message is imported");

    let row = only(fixture.listing(&inbox));
    // The uid from the import must be the uid in the listing, because the
    // caller finds the message again by that uid.
    assert_eq!(row.uid, uid);
    assert_eq!(row.subject.as_deref(), Some("Lunch?"));
    assert_eq!(row.size as usize, MESSAGE.len());
    assert_eq!(
        row.from.first().map(|from| from.email.as_str()),
        Some("bob@example.com")
    );
    assert_eq!(row.message_id.as_deref(), Some("lunch@example.com"));
}

#[test]
fn the_bytes_that_went_up_are_the_bytes_that_come_back_down() {
    let fixture = Fixture::start();
    let inbox = fixture.seed_mailbox("Inbox", None);
    let sync = fixture.sync();

    let uid = sync
        .import_message(&inbox, MESSAGE.to_vec(), &Keywords::default(), None)
        .expect("the message is imported");

    // This is the reason to import, not compose. A message rebuilt from its
    // properties has different bytes, and a signature on it fails.
    let source = sync.message_source(&uid).expect("the message is readable");
    assert_eq!(source, MESSAGE);
}

#[test]
fn the_keywords_a_row_carries_go_up_with_the_message() {
    let fixture = Fixture::start();
    let inbox = fixture.seed_mailbox("Inbox", None);
    let flags = MessageFlags {
        seen: true,
        flagged: true,
        ..MessageFlags::default()
    };

    fixture
        .sync()
        .import_message(
            &inbox,
            MESSAGE.to_vec(),
            &Keywords::new(&flags, &["Work".to_owned()]),
            None,
        )
        .expect("the message is imported");

    // Read the row as a folder does. This tests the full round trip through the
    // keyword mapping, not only the request.
    let row = only(fixture.listing(&inbox));
    assert_eq!(row.flags, flags);
    assert_eq!(row.tags, vec!["Work".to_owned()]);
}

#[test]
fn a_message_imported_with_no_keywords_arrives_carrying_none() {
    let fixture = Fixture::start();
    let inbox = fixture.seed_mailbox("Inbox", None);

    fixture
        .sync()
        .import_message(&inbox, MESSAGE.to_vec(), &Keywords::default(), None)
        .expect("the message is imported");

    // The message must be unread. If the append marks it as read, the user
    // does not see it in the folder.
    let row = only(fixture.listing(&inbox));
    assert_eq!(row.flags, MessageFlags::default());
    assert!(row.tags.is_empty(), "{:?}", row.tags);
}

#[test]
fn the_moment_the_message_was_received_is_the_moment_it_keeps() {
    let fixture = Fixture::start();
    let inbox = fixture.seed_mailbox("Inbox", None);
    // Camel gives seconds since the epoch, here 2026-01-15T09:30:00Z.
    let received_at = 1_768_469_400;

    fixture
        .sync()
        .import_message(
            &inbox,
            MESSAGE.to_vec(),
            &Keywords::default(),
            Some(received_at),
        )
        .expect("the message is imported");

    // The date goes out in the request and comes back in the listing. An error
    // in either direction puts the message at the wrong end of the folder.
    let row = only(fixture.listing(&inbox));
    assert_eq!(row.received_at, Some(received_at));
}

#[test]
fn a_message_imported_without_a_moment_is_dated_by_the_server() {
    let fixture = Fixture::start();
    let inbox = fixture.seed_mailbox("Inbox", None);

    fixture
        .sync()
        .import_message(&inbox, MESSAGE.to_vec(), &Keywords::default(), None)
        .expect("the message is imported");

    // RFC 8621 §4.8 lets the server set a missing `receivedAt`. Evolution sorts
    // a row with no date to the epoch. Thus send no date, not a zero.
    let row = only(fixture.listing(&inbox));
    let received_at = row.received_at.expect("the server dated the message");
    assert!(received_at > 0, "{received_at}");
}

#[test]
fn a_date_no_utc_date_can_name_is_left_to_the_server() {
    let fixture = Fixture::start();
    let inbox = fixture.seed_mailbox("Inbox", None);

    // Camel keeps a date as a signed 64-bit count of seconds. This value has
    // no year that a `UTCDate` can show. The import must not fail for this:
    // the user wants the message, and only the date is lost.
    let uid = fixture
        .sync()
        .import_message(
            &inbox,
            MESSAGE.to_vec(),
            &Keywords::default(),
            Some(i64::MAX),
        )
        .expect("an unwritable date does not stop the import");

    let row = only(fixture.listing(&inbox));
    assert_eq!(row.uid, uid);
    assert!(row.received_at.is_some(), "the server dated the message");
}

#[test]
fn bytes_that_are_not_a_message_are_refused_as_such() {
    let fixture = Fixture::start();
    let inbox = fixture.seed_mailbox("Inbox", None);

    let error = fixture
        .sync()
        .import_message(
            &inbox,
            b"\x00\x01 not a message at all".to_vec(),
            &Keywords::default(),
            None,
        )
        .expect_err("bytes that are not a message are not imported");

    // Keep the server refusal unchanged. `invalidEmail` tells the user about
    // the message, not about a broken account or a missing folder.
    match error {
        SyncError::Client(jmap_client::Error::Set(set_error)) => {
            assert_eq!(set_error.error_type, email_import_error::INVALID_EMAIL);
        }
        other => panic!("expected the server's refusal, got {other:?}"),
    }
    assert!(fixture.listing(&inbox).is_empty());
}

#[test]
fn a_mailbox_the_account_does_not_have_takes_no_message() {
    let fixture = Fixture::start();
    let inbox = fixture.seed_mailbox("Inbox", None);

    let error = fixture
        .sync()
        .import_message(
            &Id::new("M404"),
            MESSAGE.to_vec(),
            &Keywords::default(),
            None,
        )
        .expect_err("a mailbox that is not there holds no message");

    match error {
        SyncError::Client(jmap_client::Error::Set(_)) => {}
        other => panic!("expected the server's refusal, got {other:?}"),
    }
    // A refused import must not leave a message that no mailbox shows.
    assert!(fixture.listing(&inbox).is_empty());
}

/// A message larger than the `maxSizeUpload` of the account is refused before
/// the upload, and the refusal gives both numbers.
///
/// An import sends a full message, and an administrator can set a small limit.
/// The Camel layer must tell the user "this account takes at most N bytes".
/// Thus the crate keeps [`jmap_client::Error::TooLarge`] unchanged, as it
/// keeps the server refusals.
#[test]
fn a_message_over_the_accounts_upload_limit_is_not_imported() {
    let fixture = Fixture::started_with(MockServer::builder().size_upload(1024));
    let inbox = fixture.seed_mailbox("Inbox", None);

    let error = fixture
        .sync()
        .import_message(&inbox, vec![b'x'; 4096], &Keywords::default(), None)
        .expect_err("a message over the account's limit was imported");

    match error {
        SyncError::Client(jmap_client::Error::TooLarge { size, limit }) => {
            assert_eq!(size, 4096);
            assert_eq!(limit, 1024);
        }
        other => panic!("expected the upload limit, got {other:?}"),
    }

    // Nothing was uploaded and nothing was imported.
    assert!(fixture.listing(&inbox).is_empty());
    let state = fixture.server.state();
    let state = state.lock().unwrap();
    assert!(
        state.account(&fixture.account_id).unwrap().blobs.is_empty(),
        "a refused import still uploaded the message"
    );
}

#[test]
fn importing_a_message_with_multiple_recipients_and_custom_headers_preserves_summary() {
    let fixture = Fixture::start();
    let inbox = fixture.seed_mailbox("Inbox", None);

    let message_bytes = b"From: Alice <alice@example.com>\r\n\
To: Bob <bob@example.com>, \"Doe, Charlie\" <charlie@example.org>\r\n\
Cc: Dana <dana@example.net>\r\n\
Subject: Multi-recipient sync test\r\n\
Message-ID: <multi-2026@example.com>\r\n\
Date: Fri, 16 Jan 2026 14:00:00 +0000\r\n\
\r\n\
Detailed test body.\r\n";

    let flags = MessageFlags {
        seen: true,
        ..MessageFlags::default()
    };
    let tags = vec!["$label1".to_string()];
    let keywords = Keywords::new(&flags, &tags);

    let uid = fixture
        .sync()
        .import_message(&inbox, message_bytes.to_vec(), &keywords, None)
        .expect("the message is imported");

    let row = only(fixture.listing(&inbox));
    assert_eq!(row.uid, uid);
    assert_eq!(row.subject.as_deref(), Some("Multi-recipient sync test"));
    assert_eq!(row.message_id.as_deref(), Some("multi-2026@example.com"));
    assert_eq!(row.from.len(), 1);
    assert_eq!(row.from[0].email, "alice@example.com");
    assert_eq!(row.to.len(), 2);
    assert_eq!(row.to[0].email, "bob@example.com");
    assert_eq!(row.to[1].email, "charlie@example.org");
    assert_eq!(row.cc.len(), 1);
    assert_eq!(row.cc[0].email, "dana@example.net");
    assert!(row.flags.seen);
    assert!(row.tags.contains(&"$label1".to_string()));
}

#[test]
fn importing_a_multipart_message_with_attachment_and_html_alternate_preserves_content() {
    let fixture = Fixture::start();
    let inbox = fixture.seed_mailbox("Inbox", None);

    let multipart_bytes = b"From: Sender <sender@example.com>\r\n\
To: Recipient <recipient@example.com>\r\n\
Subject: Invoice and Statement\r\n\
Message-ID: <invoice-statement-2026@example.com>\r\n\
Date: Fri, 16 Jan 2026 15:45:00 +0000\r\n\
MIME-Version: 1.0\r\n\
Content-Type: multipart/mixed; boundary=\"mix_bound_987\"\r\n\
\r\n\
--mix_bound_987\r\n\
Content-Type: multipart/alternative; boundary=\"alt_bound_987\"\r\n\
\r\n\
--alt_bound_987\r\n\
Content-Type: text/plain; charset=\"utf-8\"\r\n\
\r\n\
Please review your invoice attached.\r\n\
\r\n\
--alt_bound_987\r\n\
Content-Type: text/html; charset=\"utf-8\"\r\n\
\r\n\
<p>Please review your invoice attached.</p>\r\n\
\r\n\
--alt_bound_987--\r\n\
\r\n\
--mix_bound_987\r\n\
Content-Type: application/pdf; name=\"invoice.pdf\"\r\n\
Content-Disposition: attachment; filename=\"invoice.pdf\"\r\n\
Content-Transfer-Encoding: base64\r\n\
\r\n\
JVBERi0xLjQKJcfsj6IKMSAwIG9iago8PAovVHlwZSAvQ2F0YWxvZwovUGFnZXMgMiAw\r\n\
\r\n\
--mix_bound_987--\r\n";

    let flags = MessageFlags {
        flagged: true,
        ..MessageFlags::default()
    };
    let keywords = Keywords::new(&flags, &["billing".to_string()]);

    let uid = fixture
        .sync()
        .import_message(&inbox, multipart_bytes.to_vec(), &keywords, None)
        .expect("multipart message is imported");

    let row = only(fixture.listing(&inbox));
    assert_eq!(row.uid, uid);
    assert_eq!(row.subject.as_deref(), Some("Invoice and Statement"));
    assert_eq!(
        row.message_id.as_deref(),
        Some("invoice-statement-2026@example.com")
    );
    assert_eq!(row.from.len(), 1);
    assert_eq!(row.from[0].email, "sender@example.com");
    assert_eq!(row.size as usize, multipart_bytes.len());
    assert!(row.flags.flagged);
    assert!(row.tags.contains(&"billing".to_string()));

    let fetched_source = fixture
        .sync()
        .message_source(&uid)
        .expect("fetched binary source");
    assert_eq!(fetched_source, multipart_bytes);
}

#[test]
fn importing_a_message_with_base64_attachment_and_qp_body_preserves_wire_format() {
    let fixture = Fixture::start();
    let archive = fixture.seed_mailbox("Archive", None);

    let wire_bytes = b"From: Developer <dev@example.org>\r\n\
To: Reviewer <rev@example.org>\r\n\
Subject: Patch Submission v2\r\n\
Message-ID: <patch-v2-2026@example.org>\r\n\
Date: Fri, 16 Jan 2026 18:15:00 +0000\r\n\
MIME-Version: 1.0\r\n\
Content-Type: multipart/mixed; boundary=\"_patch_boundary_002_\"\r\n\
\r\n\
--_patch_boundary_002_\r\n\
Content-Type: text/plain; charset=\"utf-8\"\r\n\
Content-Transfer-Encoding: quoted-printable\r\n\
\r\n\
Here is the updated patch for review=\r\n\
 with revised error quark assertions.\r\n\
\r\n\
--_patch_boundary_002_\r\n\
Content-Type: text/x-patch; name=\"changes.diff\"\r\n\
Content-Disposition: attachment; filename=\"changes.diff\"\r\n\
Content-Transfer-Encoding: base64\r\n\
\r\n\
LS0tIGEvc3JjL21haW4ucnMKKysrIGIvc3JjL21haW4ucnMKQEAgLTEsMyArMSwzIEBACi0g\r\n\
b2xkX2Z1bmMoKTsKKyBuZXdfZnVuYygpOwo=\r\n\
\r\n\
--_patch_boundary_002_--\r\n";

    let flags = MessageFlags {
        seen: true,
        answered: true,
        ..MessageFlags::default()
    };
    let tags = vec!["patches".to_string(), "reviewed".to_string()];
    let keywords = Keywords::new(&flags, &tags);

    let uid = fixture
        .sync()
        .import_message(&archive, wire_bytes.to_vec(), &keywords, None)
        .expect("imported patch message");

    let row = only(fixture.listing(&archive));
    assert_eq!(row.uid, uid);
    assert_eq!(row.subject.as_deref(), Some("Patch Submission v2"));
    assert_eq!(row.message_id.as_deref(), Some("patch-v2-2026@example.org"));
    assert_eq!(row.from.len(), 1);
    assert_eq!(row.from[0].email, "dev@example.org");
    assert_eq!(row.size as usize, wire_bytes.len());
    assert!(row.flags.seen);
    assert!(row.flags.answered);
    assert!(row.tags.contains(&"patches".to_string()));
    assert!(row.tags.contains(&"reviewed".to_string()));

    let read_back = fixture
        .sync()
        .message_source(&uid)
        .expect("readable source");
    assert_eq!(read_back, wire_bytes);
}
