// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! One message on its way out of the account.
//!
//! Everything else this crate writes changes mail the account holds. Sending is
//! the one operation whose point is somewhere else — and in JMAP it is still
//! two writes to the account's own store, because RFC 8621 §7 has a client
//! submit a message that already *is* an `Email`: there is no method that takes
//! bytes and an envelope and sends them. So the message is put into the account
//! first and handed to the submission machinery second, and this struct is what
//! [`MailSync::send_message`](crate::MailSync::send_message) needs to do both.
//!
//! ## Bytes, not properties
//!
//! [`Outgoing::source`] is the RFC 5322 message, which is why sending goes
//! through `Email/import` rather than the `Email/set` create that
//! [`Client::send_email`](jmap_client::Client::send_email) uses. A message
//! Evolution's composer built is already a MIME document — with its parts, its
//! encodings and possibly a signature over the whole of it — and a client that
//! took it apart into JMAP properties for the server to write out again would
//! be sending a different message than the one it was given. That is the same
//! judgement [`MailSync::import_message`](crate::MailSync::import_message) is
//! built on, made for the message the user just wrote.
//!
//! ## The envelope is not the headers
//!
//! [`Outgoing::envelope`] is carried separately because it *is* separate: the
//! addresses a message is delivered to are the SMTP envelope's, and the
//! headers are text inside the message. A `Bcc` recipient is exactly the case
//! where the two differ on purpose — the recipient is in the envelope and in no
//! header — so a client that let the server derive one from the other would
//! either drop the blind recipient or expose them to everyone else. Evolution
//! hands a transport the recipients as their own argument for this reason, and
//! they travel as their own field the whole way down.
//!
//! ## Staged, then filed
//!
//! [`Outgoing::staging`] is the mailbox the message sits in while it is being
//! sent — Drafts, for an account that has one — and [`Outgoing::destination`]
//! is where the server is asked to file it once the submission is accepted.
//! Two mailboxes rather than one, because a message that were imported straight
//! into Sent would be a message in Sent that may never go out. The move happens
//! in RFC 8621 §7.5's `onSuccessUpdateEmail`, applied by the server as part of
//! accepting the submission, so there is no window in which the message has
//! gone and the account still shows it as a draft.
//!
//! A submission the server *refuses* leaves the message in the staging mailbox,
//! still marked a draft. That is not a cleanup this crate skipped: the user's
//! message exists, unsent, in the place unsent messages live, which is where
//! they would look for it — and destroying it to keep the account tidy would
//! throw away work on behalf of a server that said no.
//!
//! ## Which two mailboxes those are
//!
//! [`OutgoingMailboxes`] answers that, and it is a question the caller cannot:
//! Camel's `send_to_sync` is handed a message and two address lists and nothing
//! about folders at all. So the pair is found in the account, by *role* — RFC
//! 8621 §2 puts a `role` on a mailbox for exactly this, and it is the only
//! thing that identifies one across the servers and the languages a user's
//! account may be in.

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
    /// The RFC 5322 bytes, exactly as they are to be delivered.
    pub source: Vec<u8>,
    /// The identity to submit through — one of the account's, from
    /// `Identity/get`. RFC 8621 §7 has the server check that the message's
    /// `From` agrees with it, which is what stops an account sending as
    /// somebody else.
    pub identity: Id,
    /// The SMTP envelope, or `None` to let the server derive one from the
    /// message's headers. A caller that was handed the recipients — every
    /// caller from Camel — names them here.
    pub envelope: Option<Envelope>,
    /// The mailbox the message is imported into while it is being sent.
    pub staging: Id,
    /// The mailbox the server files it into once it has gone, if the caller
    /// wants one. `None` leaves the message where it was staged: an account
    /// with no Sent mailbox has nowhere better to keep the copy, and a caller
    /// that saves its own copy elsewhere is asking for exactly that.
    pub destination: Option<Id>,
}

impl Outgoing {
    /// The keywords the message carries while it is being sent.
    ///
    /// `$draft`, because that is what it is until the server accepts it, and
    /// `$seen`, because the user wrote it: a sent message that came back unread
    /// would put a bold line in the sender's own Sent folder and a count on the
    /// folder in the tree.
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

    /// The patch RFC 8621 §7.5 applies to the message once the submission is
    /// accepted: it stops being a draft, and it moves to where sent mail is
    /// kept.
    ///
    /// One object rather than two writes, and not only to save a request:
    /// `onSuccessUpdateEmail` is applied by the server as part of accepting the
    /// submission, so a client that went away between the send and the tidying
    /// leaves no message behind claiming to be an unsent draft of something
    /// that has already gone out.
    ///
    /// The `$seen` this does *not* touch is deliberate — it was set at import,
    /// where it belongs, so that a message left in the staging mailbox by a
    /// refused submission is not also an unread one.
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

/// The two mailboxes a send needs, found in the account rather than named by
/// the caller.
///
/// They fill [`Outgoing::staging`] and [`Outgoing::destination`], and they are
/// looked up by role for the reason the module docs give. What follows is what
/// each role's absence means, which is where the judgements are.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutgoingMailboxes {
    /// Where the message is imported to while it is being sent.
    pub staging: Id,
    /// Where the server files it once the submission is accepted, if that is
    /// somewhere else.
    pub destination: Option<Id>,
    /// Whether the copy this send leaves behind is in the mailbox the account
    /// keeps sent mail in.
    ///
    /// It is the answer to Camel's `out_sent_message_saved`, which asks whether
    /// the transport has already saved the sent copy — a caller told `false`
    /// saves one of its own, and a caller told `true` does not.
    ///
    /// A field rather than something the caller derives, because neither id
    /// above answers it: [`Self::destination`] is `None` both for the account
    /// that stages *in* Sent, where the copy is saved, and for the account that
    /// has only a Drafts, where it is not. Reading "no destination" as "not
    /// saved" would have Evolution append a second copy of every message the
    /// first kind of account sends; reading it as "saved" would lose the sent
    /// copy of the second kind altogether.
    pub saves_sent_copy: bool,
}

impl OutgoingMailboxes {
    /// The pair this account sends through, or `None` if it has no mailbox an
    /// outgoing message may be put in.
    ///
    /// ## Drafts, then Sent, then nothing
    ///
    /// Drafts is the staging mailbox of an account that has one: it is where
    /// unsent mail belongs, and a submission the server refuses leaves the
    /// message exactly where the user would look for it.
    ///
    /// An account with no Drafts stages in Sent instead — the message is going
    /// there anyway, so the only thing lost is that a *refused* send leaves a
    /// message in Sent that never went out. It is still marked `$draft` when
    /// that happens (see [`Outgoing::accepted_patch`], which is what clears the
    /// keyword, and only on success), so it is distinguishable from mail that
    /// did go; and the alternative is refusing to send at all, which loses the
    /// message the user just wrote.
    ///
    /// An account with neither is one this crate will not send from, and
    /// falling back to the Inbox is the tempting wrong answer: the Inbox is
    /// where the *server* delivers, and importing the user's own outgoing mail
    /// into it would manufacture arrivals they have to sort out — for a message
    /// that may then be refused. A refusal here names something the user can
    /// act on. It also costs nothing, because it happens before the message is
    /// uploaded.
    ///
    /// ## No destination when it is the staging mailbox
    ///
    /// Not an optimisation but the same rule [`Filing::is_empty`] is built on:
    /// a message cannot be filed out of a mailbox into that same mailbox, and
    /// asking for it would put a pointer that is both `true` and `null` in one
    /// patch.
    ///
    /// ## Whether the copy counts as saved
    ///
    /// [`Self::saves_sent_copy`] is that question and it is not the one above,
    /// which is the whole reason it is carried separately: it is true exactly
    /// when the account *has* a Sent mailbox, because the message ends up in
    /// one either way — filed there from Drafts, or staged there to begin with.
    /// The absent destination means only that no move is needed.
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

/// Folds one patch object into another. Both are built here out of disjoint
/// properties — `keywords/…` and `mailboxIds/…` — so there is no key for one to
/// take from the other.
fn merge(patch: &mut Map<String, Value>, other: Value) {
    if let Value::Object(members) = other {
        patch.extend(members);
    }
}
