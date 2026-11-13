// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! `EmailSeed::raw_source` lets a test hand the mock a verbatim
//! `message/rfc822` blob, for a MIME shape `rfc5322` cannot build itself
//! (for example `multipart/signed`). A real EDS/Camel client reads a
//! message by this raw blob, not by JMAP structured properties
//! (`jmap-mock/src/mail.rs`'s own `rfc5322` doc comment), so the raw bytes
//! a test supplies must reach the blob unchanged.

use jmap_mock::{EmailSeed, MockServer};

#[test]
fn a_raw_source_blob_reaches_storage_verbatim() {
    let server = MockServer::builder().start();
    let raw = b"From: sender@example.test\r\n\
Subject: raw\r\n\
MIME-Version: 1.0\r\n\
Content-Type: multipart/signed; protocol=\"application/pgp-signature\"\r\n\
\r\n\
this is not a shape rfc5322() ever produces on its own\r\n"
        .to_vec();

    let email_id = server.with_account_mut(server.account_id(), |account| {
        let inbox = account.seed_mailbox("Inbox", Some(jmap_proto::mail::role::INBOX));
        account.seed_email(
            EmailSeed::new(
                inbox,
                ("Sender", "sender@example.test"),
                "raw",
                "ignored: raw_source overrides the built body",
                "2026-01-02T03:04:05Z",
            )
            .raw_source(raw.clone()),
        )
    });

    let stored = server.with_account(server.account_id(), |account| {
        let email = account.emails.get(&email_id).expect("seeded email exists");
        let blob_id = email.blob_id.clone().expect("built email has a blobId");
        account
            .blobs
            .get(&blob_id)
            .expect("the blobId resolves to a stored blob")
            .data
            .clone()
    });

    assert_eq!(stored, raw, "the raw bytes must reach the blob unchanged");
}
