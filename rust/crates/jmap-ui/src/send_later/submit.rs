// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! The blocking JMAP conversation behind one scheduled send: upload the
//! message, import it into Drafts as a draft, submit it with an RFC 4865
//! `HOLDFOR`, and let `onSuccessUpdateEmail` move it Drafts → Sent the moment
//! the server accepts. Worker threads only; the mock proves the whole
//! round trip in `tests/send_later.rs`.
//!
//! The draft-first shape is deliberate and is `Client::send_email`'s own: a
//! submission the server refuses leaves the message visible in Drafts rather
//! than gone, and the user-facing error text names that residue.

use jmap_backend_core::i18n::{translate, translate_with};
use jmap_proto::mail::{EmailImport, Envelope, Schedule, keyword, role};

use crate::link::AccountLink;

/// Ask the server to hold the message for `hold` seconds and send it then.
/// Answers the release time the server reported (`sendAt`).
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

    // Applied when the submission is accepted (RFC 8621 §7.5): no longer a
    // draft, filed where sent mail goes — exactly what pressing Send leaves
    // behind. Without a sent-role mailbox the message simply stays in Drafts,
    // un-drafted.
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
            // Accepted but with no release time named: unheard of, but the
            // caller's success message quotes `sendAt`, so it has to exist.
            translate_with(
                c"the server accepted the submission but named no release time (undoStatus %1$s)",
                &[submission.undo_status.as_deref().unwrap_or("unknown")],
            )
        })
}
