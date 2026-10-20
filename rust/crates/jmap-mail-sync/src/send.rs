// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! One outgoing message.
//!
//! In JMAP, a send is two writes to the account's own store. RFC 8621 §7
//! submits a message that is already an `Email`. No method takes bytes and
//! an envelope and sends them. Thus, the provider first puts the message into
//! the account, then submits it. This struct holds the data that
//! [`MailSync::send_message`](crate::MailSync::send_message) needs for the two
//! writes.
//!
//! ## Bytes, not properties
//!
//! [`Outgoing::source`] is the RFC 5322 message. Thus, a send uses
//! `Email/import`, not the `Email/set` create that
//! [`Client::send_email`](jmap_client::Client::send_email) uses. Evolution's
//! composer makes a complete MIME document, possibly with a signature over
//! all of it. If the client converts it into JMAP properties, the server
//! writes a different message. [`MailSync::import_message`](crate::MailSync::import_message)
//! uses the same rule.
//!
//! ## The envelope is not the headers
//!
//! [`Outgoing::envelope`] is a separate field, because the SMTP envelope
//! recipients and the message headers are different data. A `Bcc` recipient
//! is in the envelope and in no header. If the server derives one from the
//! other, it drops the blind recipient or shows that recipient to all others.
//! Evolution gives the recipients to a transport as a separate argument.
//!
//! ## Staged, then filed
//!
//! [`Outgoing::staging`] is the mailbox that holds the message during the
//! send (Drafts, if the account has one). [`Outgoing::destination`] is the
//! mailbox where the server files the message after it accepts the
//! submission. A message imported directly into Sent can stay in Sent and
//! never go out, so the send uses two mailboxes.
//!
//! The server applies the move through `onSuccessUpdateEmail` (RFC 8621 §7.5)
//! when it accepts the submission. Thus, a sent message never shows as a
//! draft.
//!
//! If the server refuses the submission, the message stays in the staging
//! mailbox with the `$draft` keyword. This is intentional: the unsent
//! message stays where the user looks for unsent mail, and the user's work
//! is not lost.
//!
//! ## How the provider finds the two mailboxes
//!
//! [`OutgoingMailboxes`] finds them. Camel's `send_to_sync` gets a message and
//! two address lists, but no folders. Thus, the provider finds the mailboxes
//! by *role* (RFC 8621 §2). Only the role identifies a mailbox across servers
//! and languages.

use jmap_proto::Id;
use jmap_proto::mail::Envelope;
use serde_json::{Map, Value};

use crate::folder::{FolderRole, FolderTree};
use crate::keywords::{KeywordChange, Keywords};
use crate::mailboxes::Filing;
use crate::message::MessageFlags;

/// A message to send, and everything sending it needs.
#[derive(Debug, Clone, PartialEq)]
pub struct Outgoing {
    /// The RFC 5322 bytes, exactly as the server must deliver them.
    pub source: Vec<u8>,
    /// The account identity for the submission, from `Identity/get`. Under
    /// RFC 8621 §7, the server checks the `From` header against this identity.
    pub identity: Id,
    /// The SMTP envelope. With `None`, the server derives the envelope from
    /// the headers. Callers from Camel get the recipients and set them here.
    pub envelope: Option<Envelope>,
    /// The mailbox that receives the imported message during the send.
    pub staging: Id,
    /// The mailbox where the server files the message after the send.
    /// `None` keeps the message in the staging mailbox, for example in an
    /// account without a Sent mailbox.
    pub destination: Option<Id>,
}

impl Outgoing {
    /// The keywords of the message during the send.
    ///
    /// `$draft` stays until the server accepts the submission. `$seen` is set
    /// because the user wrote the message: Sent must not show it as unread.
    pub(crate) fn staged_keywords() -> Keywords {
        Keywords::new(
            &MessageFlags {
                draft: true,
                seen: true,
                ..MessageFlags::default()
            },
            &[],
        )
    }

    /// The `onSuccessUpdateEmail` patch (RFC 8621 §7.5) for an accepted
    /// submission. It removes `$draft` and, if there is a destination, moves
    /// the message there. `None` if the patch is empty.
    ///
    /// The server applies this patch when it accepts the submission. Thus, if
    /// the client stops after the send, no sent message stays marked as draft.
    ///
    /// The patch does not change `$seen`. The import sets it, so a message
    /// that a refused submission leaves behind is not unread.
    pub(crate) fn accepted_patch(&self) -> Option<Value> {
        let sent = KeywordChange::between(
            &Self::staged_keywords(),
            &Keywords::new(
                &MessageFlags {
                    seen: true,
                    ..MessageFlags::default()
                },
                &[],
            ),
        );

        let mut patch = Map::new();
        merge(&mut patch, sent.patch());
        if let Some(destination) = &self.destination {
            merge(
                &mut patch,
                Filing::moved(self.staging.clone(), destination.clone()).patch(),
            );
        }
        (!patch.is_empty()).then_some(Value::Object(patch))
    }
}

/// The two mailboxes for a send, found in the account by role (see the module
/// docs). They fill [`Outgoing::staging`] and [`Outgoing::destination`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutgoingMailboxes {
    /// The mailbox that receives the imported message during the send.
    pub staging: Id,
    /// The mailbox where the server files the message after it accepts the
    /// submission. `None` if this is the staging mailbox.
    pub destination: Option<Id>,
    /// Whether the sent copy goes into the account's Sent mailbox.
    ///
    /// This is the value for Camel's `out_sent_message_saved`. If it is
    /// `false`, the caller saves its own copy.
    ///
    /// The caller cannot derive this from [`Self::destination`]. The
    /// destination is `None` for an account that stages in Sent (copy saved)
    /// and for an account with only Drafts (copy not saved). A wrong reading
    /// gives a duplicate sent copy or no sent copy.
    pub saves_sent_copy: bool,
}

impl OutgoingMailboxes {
    /// The two mailboxes of this account for a send, or `None` if no mailbox
    /// can hold an outgoing message.
    ///
    /// ## Drafts, then Sent, then nothing
    ///
    /// If the account has Drafts, Drafts is the staging mailbox. A refused
    /// submission then leaves the message where the user looks for it.
    ///
    /// Without Drafts, the staging mailbox is Sent. A refused send then leaves
    /// an unsent message in Sent. That message keeps `$draft`, because only
    /// `Outgoing::accepted_patch` removes it, and only on success.
    ///
    /// Without Drafts and Sent, the function returns `None`. The Inbox is not
    /// a fallback: the server delivers incoming mail there, and outgoing mail
    /// in the Inbox looks like new mail. The caller can then refuse the send
    /// before the upload.
    ///
    /// ## No destination when it is the staging mailbox
    ///
    /// This follows the rule of [`Filing::is_empty`]: a move from a mailbox
    /// into the same mailbox would set one pointer to both `true` and `null`.
    ///
    /// ## Whether the copy counts as saved
    ///
    /// [`Self::saves_sent_copy`] is true if the account has a Sent mailbox.
    /// The message gets to Sent by the move from Drafts or by staging there.
    /// No destination means only that no move is necessary.
    pub fn of(tree: &FolderTree) -> Option<Self> {
        let sent = tree.role(FolderRole::Sent).map(|folder| folder.id.clone());
        let drafts = tree
            .role(FolderRole::Drafts)
            .map(|folder| folder.id.clone());

        let staging = drafts.or_else(|| sent.clone())?;
        let saves_sent_copy = sent.is_some();
        let destination = sent.filter(|sent| *sent != staging);
        Some(Self {
            staging,
            destination,
            saves_sent_copy,
        })
    }
}

/// Merges one patch object into another. The callers merge patches with
/// disjoint keys (`keywords/…` and `mailboxIds/…`), so no key is overwritten.
fn merge(patch: &mut Map<String, Value>, other: Value) {
    if let Value::Object(members) = other {
        patch.extend(members);
    }
}
