// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! One message, in the shape a folder summary keeps it.
//!
//! `CamelFolderSummary` is a row per message: a uid, a flags word, a set of
//! user flags, the four header fields the message list shows (subject, from,
//! to, cc), two dates, a size, and the two identifiers threading needs. It is
//! not the message — the body is fetched separately, by the blob id kept here —
//! and it is what the folder's message-count and `get_message_info` vfuncs
//! answer out of, which is why a mailbox cannot be opened before this exists.
//!
//! JMAP hands the same information over as `Email` objects with every property
//! optional, because `Email/get` returns what was asked for and nothing else.
//! The mapping is therefore mostly about absence, and the rule throughout is
//! that a message with a field missing is a message with that field empty,
//! never a failed listing: the only thing that makes an `Email` unusable is
//! having no id, because the id *is* the Camel uid.
//!
//! ## What is not here
//!
//! - **`CAMEL_MESSAGE_DELETED`.** JMAP has no deleted keyword. Deleting mail is
//!   `Email/set` taking the message out of the mailbox (or putting it in the
//!   trash one), so there is nothing on the wire for this bit to come from. It
//!   stays what Camel makes it: a local mark the user sets, which this provider
//!   turns into a mailbox change when the folder is expunged.
//! - **`mlist`.** Camel's mailing-list column comes from `List-Id` and its
//!   half-dozen predecessors, which JMAP only serves as an explicitly requested
//!   `header:List-Id:asText` property. Worth asking for later — it is what
//!   Evolution's mailing-list filters read — but it is a header fetch, not part
//!   of this mapping.
//! - **The 64-bit digests Camel stores for threading.** Camel keeps a message
//!   id as `guint64` — a truncated MD5 of the header, per
//!   `CamelSummaryMessageID` — and a message's references as an array of the
//!   same, with no public function to compute one. Whatever digest this provider
//!   picks is only ever compared against digests it wrote itself, in its own
//!   summary, so the choice belongs to the layer that fills that summary. Here
//!   the headers stay text.

use jmap_client::limits;
use jmap_proto::Id;
use jmap_proto::mail::{Email, EmailAddress, keyword};

use crate::date::epoch_seconds;
use crate::error::SyncError;

/// The properties one `Email/get` has to ask for to fill a summary row.
///
/// Named explicitly rather than left to the server's default set, which RFC
/// 8621 §4.2 makes *everything* — including `bodyStructure`, `textBody` and the
/// preview-sized `bodyValues`. Fetching those to list a mailbox would multiply
/// the size of the answer by the size of the mail in it.
pub const SUMMARY_PROPERTIES: &[&str] = &[
    "id",
    "blobId",
    "threadId",
    "keywords",
    "size",
    "receivedAt",
    "sentAt",
    "messageId",
    "inReplyTo",
    "references",
    "from",
    "to",
    "cc",
    "subject",
    "hasAttachment",
    "preview",
];

/// The properties one `Email/get` has to ask for to find a message's bytes.
///
/// Three, and none of them is any of the sixteen above: fetching a message is
/// not fetching its summary again, and asking for a row's worth of properties
/// to learn one id would be paying for the message list a second time on every
/// message the user opens.
///
/// `size` is here for [`download_ceiling`] rather than for the summary, and it
/// is free: it rides the `Email/get` that had to be made anyway to learn the
/// `blobId`, so knowing how large the download will be costs no round trip.
pub const SOURCE_PROPERTIES: &[&str] = &["id", "blobId", "size"];

/// How many octets of blob to accept for a message whose row says it is
/// `advertised` octets long.
///
/// RFC 8621 §4.1.1 defines `size` as the octets of the raw data the `blobId`
/// refers to, which is exactly what the download returns, so the honest ceiling
/// is that number — and taking it means the memory one open message can cost is
/// bounded by something the account said before the download started, rather
/// than by a constant this crate guessed.
///
/// It is not taken *exactly*, and the margin is the interesting part. A server
/// that stores a message with bare-LF line endings and serves it with CRLF adds
/// one octet per line, and its `size` may well be counting the stored form;
/// refusing such a message would make a server's rounding into mail the user
/// cannot open, which is a far worse failure than buffering a few percent more
/// than expected. An eighth is generous cover for that — a mail line averages
/// well over eight octets — and the flat 64 KiB keeps the margin meaningful for
/// a short message, where an eighth of it is nothing. Neither term makes the
/// bound stop being the account's: it stays proportional to what the row said.
///
/// A row with no `size` gives nothing to be proportional to, and gets
/// [`limits::MAX_BLOB_BYTES`] — this repository's answer to "how large a
/// message will we open at all".
pub fn download_ceiling(advertised: Option<u64>) -> u64 {
    match advertised {
        // Saturating, so a server naming a size near `u64::MAX` gets a
        // meaningless ceiling rather than a wrapped one — it is not describing
        // a message, and the download will fail on its own terms.
        Some(size) => size.saturating_add(size / 8).saturating_add(64 * 1024),
        None => limits::MAX_BLOB_BYTES,
    }
}

/// The bits of Camel's flags word this provider can honestly set.
///
/// One field per `CamelMessageFlags` bit that a JMAP keyword or property says
/// something about, and no field for the rest: `CAMEL_MESSAGE_DELETED` and
/// `CAMEL_MESSAGE_FOLDER_FLAGGED` are local state, `CAMEL_MESSAGE_SECURE` and
/// `CAMEL_MESSAGE_ANSWERED_ALL` are conclusions Camel draws from a message it
/// has fetched. A struct rather than a bitfield of Camel's own, because this
/// crate has no Camel headers; turning it into the word is one match in the
/// folder layer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct MessageFlags {
    /// `$seen` — read.
    pub seen: bool,
    /// `$answered` — replied to.
    pub answered: bool,
    /// `$flagged` — what Evolution shows as "important".
    pub flagged: bool,
    /// `$draft` — unsent. Camel needs it to know not to offer a reply.
    pub draft: bool,
    /// `$forwarded`.
    pub forwarded: bool,
    /// `$junk` — the server's spam verdict.
    pub junk: bool,
    /// `$notjunk` — an explicit "not spam", which is not the same as the
    /// absence of `$junk`: it is what stops a filter reconsidering.
    pub not_junk: bool,
    /// `hasAttachment`, not a keyword: the one bit of the word that comes from
    /// a property of the message rather than from a label on it.
    pub attachments: bool,
}

/// What a folder summary keeps about one message.
#[derive(Debug, Clone, PartialEq)]
pub struct MessageSummary {
    /// The JMAP email id, which is also the Camel uid.
    ///
    /// A server-assigned immutable identifier is exactly what Camel wants a uid
    /// to be, so there is nothing to invent here — unlike the folder path,
    /// which had to be encoded out of a name. The same message filed in two
    /// mailboxes carries the same id in both, which is not a collision: a uid
    /// only has to be unique within its folder.
    pub uid: Id,
    /// Where the RFC 5322 bytes are downloaded from, when the user opens the
    /// message. `None` from a server that did not send one, which leaves a row
    /// that can be listed but not read.
    pub blob_id: Option<Id>,
    /// The JMAP thread this message belongs to. Camel threads by references
    /// rather than by an id, so this is not what its threading uses; it is kept
    /// because it is the server's own answer to the same question.
    pub thread_id: Option<Id>,
    pub flags: MessageFlags,
    /// The keywords no flag covers, verbatim, in the order a set puts them.
    ///
    /// Camel's user flags, which is what Evolution's labels are. Verbatim
    /// including the leading `$`, because a flag change sends the keyword back
    /// to the server and a renamed one would not be the same keyword.
    pub tags: Vec<String>,
    /// Camel counts a message's octets in 32 bits.
    pub size: u32,
    /// When the server received it, in seconds since the epoch — Camel's
    /// `date_received`. `None` if the server said nothing readable.
    pub received_at: Option<i64>,
    /// The `Date` header, as seconds since the epoch. Distinct from
    /// [`MessageSummary::received_at`]: this one is the sender's clock, at the
    /// sender's offset, and may be well before or absurdly after the other.
    pub sent_at: Option<i64>,
    pub subject: Option<String>,
    /// The addresses as structures, not as the single string Camel's summary
    /// stores. Formatting an address list is `CamelInternetAddress`'s job — it
    /// is where RFC 5322's quoting and encoded-word rules already live — so
    /// this crate hands over the parts and lets the folder layer put them
    /// together rather than reimplementing that.
    pub from: Vec<EmailAddress>,
    pub to: Vec<EmailAddress>,
    pub cc: Vec<EmailAddress>,
    /// The `Message-ID` header, angle brackets stripped, as the server sends
    /// it.
    pub message_id: Option<String>,
    /// The `References` chain, oldest first, with the `In-Reply-To` parent at
    /// the end of it.
    pub references: Vec<String>,
    /// The server's summary of the body — Camel keeps one too, for the message
    /// list's preview line, and taking the server's saves fetching the body to
    /// build our own.
    pub preview: Option<String>,
}

impl MessageSummary {
    /// The summary row for one `Email/get` result.
    ///
    /// Fails only on an `Email` with no id: everything else a server can leave
    /// out leaves a usable row.
    pub fn from_email(email: &Email) -> Result<Self, SyncError> {
        let uid = email
            .id
            .clone()
            .ok_or_else(|| SyncError::protocol("Email/get returned a message without an id"))?;
        let (flags, tags) = read_keywords(email);

        Ok(Self {
            uid,
            blob_id: email.blob_id.clone(),
            thread_id: email.thread_id.clone(),
            flags,
            tags,
            size: u32::try_from(email.size.unwrap_or(0)).unwrap_or(u32::MAX),
            received_at: email
                .received_at
                .as_ref()
                .and_then(|received_at| epoch_seconds(received_at.as_str())),
            sent_at: email.sent_at.as_deref().and_then(epoch_seconds),
            subject: email.subject.clone(),
            from: email.from.clone().unwrap_or_default(),
            to: email.to.clone().unwrap_or_default(),
            cc: email.cc.clone().unwrap_or_default(),
            // A `Message-ID` header holds one id; the list is JMAP's way of
            // reporting a message that arrived with more, and the first is the
            // one every other client will thread on.
            message_id: email
                .message_id
                .as_ref()
                .and_then(|ids| ids.first())
                .cloned(),
            references: references(email),
            preview: email.preview.clone(),
        })
    }
}

/// The flags word and the leftover labels, out of one keyword set.
///
/// Keywords are matched case-insensitively: RFC 8621 §4.1.1 restricts them to
/// lower case, and a server that shouts `$Seen` should still not leave every
/// message unread and every mailbox labelled.
fn read_keywords(email: &Email) -> (MessageFlags, Vec<String>) {
    let mut flags = MessageFlags {
        attachments: email.has_attachment.unwrap_or(false),
        ..MessageFlags::default()
    };
    let mut tags = Vec::new();

    let Some(keywords) = &email.keywords else {
        return (flags, tags);
    };
    for (name, set) in keywords {
        // RFC 8621 §4.1.1: the value is always true, and false means the
        // keyword is not set — a server that sends one is saying nothing.
        if !set {
            continue;
        }
        match name.to_ascii_lowercase().as_str() {
            keyword::SEEN => flags.seen = true,
            keyword::ANSWERED => flags.answered = true,
            keyword::FLAGGED => flags.flagged = true,
            keyword::DRAFT => flags.draft = true,
            keyword::FORWARDED => flags.forwarded = true,
            keyword::JUNK => flags.junk = true,
            keyword::NOT_JUNK => flags.not_junk = true,
            _ => tags.push(name.clone()),
        }
    }
    (flags, tags)
}

/// The thread this message hangs off, as Camel wants it: the ancestors oldest
/// first, ending in the parent.
///
/// `References` alone is not enough. A reply is required to carry its parent in
/// `In-Reply-To` but only *should* carry the chain in `References`, and mailers
/// exist that send the first and not the second — a message whose parent is
/// only in `In-Reply-To` would otherwise thread as a new conversation. Appended
/// rather than prepended, and only when the chain does not name it already:
/// a well-formed `References` ends in the parent, and a duplicate would be an
/// ancestor listed twice.
fn references(email: &Email) -> Vec<String> {
    let mut references = email.references.clone().unwrap_or_default();
    if let Some(parent) = email.in_reply_to.as_ref().and_then(|ids| ids.first())
        && !references.contains(parent)
    {
        references.push(parent.clone());
    }
    references
}

#[cfg(test)]
mod ceiling_tests {
    use super::*;

    /// The number a download is held to comes from the row, and is above it —
    /// a message of exactly the size it advertises has to arrive.
    #[test]
    fn the_ceiling_a_row_gets_is_above_the_size_it_states() {
        for size in [0, 1, 1024, 11 * 1024 * 1024, 900 * 1024 * 1024] {
            let ceiling = download_ceiling(Some(size));
            assert!(
                ceiling > size,
                "a {size}-octet message needs room for {size} octets, got {ceiling}"
            );
        }
    }

    /// And it stays proportional to that row rather than becoming a constant:
    /// a small message does not license a large buffer.
    #[test]
    fn the_ceiling_stays_within_reach_of_the_size_it_states() {
        assert!(
            download_ceiling(Some(1024)) < limits::MAX_BLOB_BYTES,
            "a one-kilobyte message must not license the fallback ceiling"
        );
        let big = 100 * 1024 * 1024;
        assert!(
            download_ceiling(Some(big)) < big * 2,
            "the margin is slack, not a doubling"
        );
    }

    /// A row that states no size gives nothing to be proportional to, and gets
    /// this repository's answer instead of an unbounded read.
    #[test]
    fn a_row_that_states_no_size_gets_the_fallback_ceiling() {
        assert_eq!(download_ceiling(None), limits::MAX_BLOB_BYTES);
    }

    /// A size no message has does not wrap into a *small* ceiling, which would
    /// turn a nonsense row into a refusal of the mail behind it for the wrong
    /// reason.
    #[test]
    fn a_size_no_message_has_saturates_rather_than_wrapping() {
        assert_eq!(download_ceiling(Some(u64::MAX)), u64::MAX);
    }
}
