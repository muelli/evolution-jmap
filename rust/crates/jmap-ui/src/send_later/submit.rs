// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! The blocking JMAP conversation for one scheduled send. It uploads the
//! message, imports it into Drafts as a draft, and submits it with an
//! RFC 4865 `HOLDFOR`. When the server accepts the submission,
//! `onSuccessUpdateEmail` moves the message from Drafts to Sent. Use this
//! module on worker threads only. `tests/send_later.rs` tests the full
//! sequence against the mock.
//!
//! The message goes to Drafts first on purpose, as in `Client::send_email`.
//! If the server refuses the submission, the message stays visible in Drafts
//! and is not lost. The error text that the user sees tells about this copy.
//!
//! ## Why Sent and not the `scheduled` mailbox
//!
//! RFC 9979 registers a `scheduled` role for messages that are "composed but
//! not yet sent". Thus a held message seems to belong there. A test against
//! Fastmail, which has such a mailbox, gave this result: the message stayed
//! in `scheduled` during the hold, and the server sent it on time. After
//! that, the message *stayed in `scheduled`* and did not appear in Sent. The
//! scheduled-send function of Fastmail controls that folder. A generic
//! RFC 8621 submission is not connected to that function, and nothing moves
//! the copy out.
//!
//! This code also cannot move the message later. `onSuccessUpdateEmail`
//! occurs one time, when the server *accepts* the submission. JMAP has no
//! hook at release time. The server deletes the submission record after
//! delivery, thus no marker remains for a later check. A full solution needs
//! Evolution to keep its own timer and to run at release time. This project
//! does not accept that bookkeeping, for snooze and for this feature.
//!
//! Thus the message goes to Sent when the server accepts it. It appears
//! there a short time before the server sends it. That error is smaller than
//! a sent message that never appears in Sent.

use jmap_backend_core::i18n::{translate, translate_with};
use jmap_proto::mail::{EmailImport, Envelope, Schedule, keyword, role};

use crate::link::AccountLink;

/// Tells the server to hold the message for `hold` seconds and then send it.
/// Returns the release time (`sendAt`) that the server reported.
pub fn schedule_send(
    link: &AccountLink,
    message: Vec<u8>,
    envelope: Envelope,
    hold: u64,
) -> Result<String, String> {
    let account_id = &link.features.account_id;

    let mailboxes = link
        .call(|client| client.mailbox_get(account_id))
        .map_err(|error| crate::link::describe(&error))?;
    let mailbox_id = |wanted: &str| {
        mailboxes
            .list
            .iter()
            .find(|mailbox| mailbox.role.as_deref() == Some(wanted))
            .and_then(|mailbox| mailbox.id.clone())
    };
    let drafts = mailbox_id(role::DRAFTS)
        .ok_or_else(|| translate(c"the server has no Drafts mailbox to stage the message in"))?;

    let identities = link
        .call(|client| client.identities(account_id))
        .map_err(|error| crate::link::describe(&error))?;
    let sender = envelope.mail_from.email.as_str();
    let identity_id = identities
        .iter()
        .find(|identity| identity.email.eq_ignore_ascii_case(sender))
        .or(identities.first())
        .and_then(|identity| identity.id.clone())
        .ok_or_else(|| translate(c"the account has no sending identity on the server"))?;

    let upload = link
        .call(|client| client.upload_blob(account_id, "message/rfc822", message.clone()))
        .map_err(|error| crate::link::describe(&error))?;
    let import = EmailImport::new(upload.blob_id, drafts.clone())
        .keyword(keyword::DRAFT)
        .keyword(keyword::SEEN);
    let imported = link
        .call(|client| client.email_import(account_id, &import))
        .map_err(|error| crate::link::describe(&error))?;
    let email_id = imported
        .id
        .ok_or_else(|| translate(c"the server imported the message without naming its id"))?;

    // The server applies this patch when it accepts the submission
    // (RFC 8621 §7.5). It removes the draft keyword and moves the message to
    // Sent, as Send does. Without a Sent mailbox, the message stays in Drafts.
    let mut on_success = serde_json::Map::new();
    on_success.insert(
        format!("keywords/{}", keyword::DRAFT),
        serde_json::Value::Null,
    );
    if let Some(sent) = mailbox_id(role::SENT) {
        on_success.insert(
            format!("mailboxIds/{}", drafts.as_str()),
            serde_json::Value::Null,
        );
        on_success.insert(
            format!("mailboxIds/{}", sent.as_str()),
            serde_json::Value::Bool(true),
        );
    }

    let submission = link
        .call(|client| {
            client.submit_email_at(
                account_id,
                &email_id,
                &identity_id,
                envelope.clone(),
                &Schedule::HoldFor(hold),
                Some(serde_json::Value::Object(on_success.clone())),
            )
        })
        .map_err(|error| crate::link::describe(&error))?;

    submission
        .send_at
        .map(|send_at| send_at.as_str().to_owned())
        .ok_or_else(|| {
            // The success message of the caller quotes `sendAt`, thus it must exist.
            translate_with(
                c"the server accepted the submission but named no release time (undoStatus %1$s)",
                &[submission.undo_status.as_deref().unwrap_or("unknown")],
            )
        })
}
