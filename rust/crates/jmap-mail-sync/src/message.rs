// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! One message, in the shape that a folder summary keeps it.
//!
//! `CamelFolderSummary` keeps one row per message: a uid, a flags word, user
//! flags, four header fields (subject, from, to, cc), two dates, a size and
//! two threading identifiers. The row does not contain the body. The provider
//! downloads the body separately, with the blob id in the row.
//!
//! The folder answers its message-count and `get_message_info` vfuncs from
//! these rows. Thus, the provider cannot open a mailbox before the rows exist.
//!
//! JMAP sends this data as `Email` objects, and each property is optional.
//! A missing field gives an empty field in the row, not a failed listing.
//! Only an `Email` without an id is unusable, because the id is the Camel uid.
//!
//! ## What is not here
//!
//! - **`CAMEL_MESSAGE_DELETED`.** JMAP has no deleted keyword. To delete mail,
//!   `Email/set` removes the message from its mailbox or moves it to the trash
//!   mailbox. Thus, this bit stays a local Camel mark. The provider changes
//!   the mailbox membership when the folder is expunged.
//! - **`mlist`.** Camel gets the mailing-list column from `List-Id` and older
//!   headers. JMAP serves these only as an explicitly requested property, for
//!   example `header:List-Id:asText`. Evolution's mailing-list filters read
//!   this column, so a later header fetch can supply it.
//! - **The 64-bit digests that Camel stores for threading.** Camel keeps a
//!   message id as a `guint64` (a truncated MD5 of the header, see
//!   `CamelSummaryMessageID`). It keeps references as an array of the same
//!   digests, and it has no public function that calculates one.
//!   The provider compares only digests that it wrote to its own summary.
//!   Thus, the layer that fills the summary selects the digest. Here, the
//!   headers stay text.

use jmap_client::limits;
use jmap_proto::Id;
use jmap_proto::mail::{Email, EmailAddress, keyword};

use crate::date::epoch_seconds;
use crate::error::SyncError;

/// The properties one `Email/get` has to ask for to fill a summary row.
///
/// The list is explicit. The default set in RFC 8621 §4.2 also contains
/// `textBody`, `htmlBody`, `attachments` and `bodyValues`, which a mailbox
/// listing does not need.
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
/// This is a subset of [`SUMMARY_PROPERTIES`]. Opening a message does not
/// fetch its full summary row again.
///
/// [`download_ceiling`] uses `size`. The same `Email/get` that gets the
/// `blobId` gets the size, so the size costs no round trip.
pub const SOURCE_PROPERTIES: &[&str] = &["id", "blobId", "size"];

/// How many octets of blob to accept for a message whose row says it is
/// `advertised` octets long.
///
/// RFC 8621 §4.1.1 defines `size` as the octets of the raw data that the
/// `blobId` refers to. The download returns that data. Thus, the server
/// limits the memory of one open message before the download starts.
///
/// The ceiling is `size + size / 8 + 64 KiB`. A server that stores bare-LF
/// line endings and serves CRLF adds one octet per line, and its `size` can
/// count the stored form. A mail line is usually longer than eight octets, so
/// one eighth covers this growth. The flat 64 KiB gives a margin to short
/// messages. The ceiling stays proportional to the advertised size.
///
/// A row without `size` gets [`limits::MAX_BLOB_BYTES`], the largest message
/// that this repository opens.
pub fn download_ceiling(advertised: Option<u64>) -> u64 {
    match advertised {
        // Saturating addition: a size near `u64::MAX` must not wrap to a
        // small ceiling. No message has that size.
        Some(size) => size.saturating_add(size / 8).saturating_add(64 * 1024),
        None => limits::MAX_BLOB_BYTES,
    }
}

/// The bits of Camel's flags word this provider can honestly set.
///
/// Each field is a `CamelMessageFlags` bit that a JMAP keyword or property
/// sets. `CAMEL_MESSAGE_DELETED` and `CAMEL_MESSAGE_FOLDER_FLAGGED` are local
/// state. Camel sets `CAMEL_MESSAGE_SECURE` and `CAMEL_MESSAGE_ANSWERED_ALL`
/// from a fetched message.
///
/// This crate has no Camel headers, so it uses a struct, not a Camel
/// bitfield. The folder layer converts the struct to the flags word.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct MessageFlags {
    /// `$seen`: the user read the message.
    pub seen: bool,
    /// `$answered`: the user replied to the message.
    pub answered: bool,
    /// `$flagged`: Evolution shows this as "important".
    pub flagged: bool,
    /// `$draft`: the message is not sent.
    pub draft: bool,
    /// `$forwarded`.
    pub forwarded: bool,
    /// `$junk`: the message is spam.
    pub junk: bool,
    /// `$notjunk`: an explicit "not spam" mark. This is not the same as no
    /// `$junk`, because it stops a filter that examines the message again.
    pub not_junk: bool,
    /// `hasAttachment`. This is the only bit that comes from an `Email`
    /// property, not from a keyword.
    pub attachments: bool,
}

/// What a folder summary keeps about one message.
#[derive(Debug, Clone, PartialEq)]
pub struct MessageSummary {
    /// The JMAP email id, which is also the Camel uid.
    ///
    /// The server assigns this id and it does not change, as Camel requires
    /// for a uid. A message in two mailboxes has the same id in both. This is
    /// correct, because a uid must be unique only in its folder.
    pub uid: Id,
    /// The blob that contains the RFC 5322 bytes of the message. If this is
    /// `None`, the message list shows the row, but the user cannot open it.
    pub blob_id: Option<Id>,
    /// The JMAP thread of this message. Camel threads by references, not by
    /// this id. The row keeps it as the server's thread assignment.
    pub thread_id: Option<Id>,
    pub flags: MessageFlags,
    /// The keywords that no flag covers, verbatim, in key order.
    ///
    /// These are Camel's user flags (Evolution's labels). They keep the
    /// leading `$` and the case, because a flag change sends the keyword back
    /// to the server, and a changed name is a different keyword.
    pub tags: Vec<String>,
    /// Camel counts a message's octets in 32 bits.
    pub size: u32,
    /// The time when the server received the message, in seconds since the
    /// epoch (Camel's `date_received`). `None` if the value is missing or
    /// unreadable.
    pub received_at: Option<i64>,
    /// The `Date` header, as seconds since the epoch. This is the sender's
    /// clock, so it can be very different from
    /// [`MessageSummary::received_at`].
    pub sent_at: Option<i64>,
    pub subject: Option<String>,
    /// The addresses as structures, not as the single string that Camel's
    /// summary stores. The folder layer formats them with
    /// `CamelInternetAddress`, which applies the RFC 5322 quoting and
    /// encoded-word rules.
    pub from: Vec<EmailAddress>,
    pub to: Vec<EmailAddress>,
    pub cc: Vec<EmailAddress>,
    /// The `Message-ID` header without angle brackets, as the server sends it.
    pub message_id: Option<String>,
    /// The `References` chain, oldest first, with the `In-Reply-To` parent at
    /// the end.
    pub references: Vec<String>,
    /// The server's preview of the body, for the preview line in the message
    /// list. With it, the provider does not fetch the body for a preview.
    pub preview: Option<String>,
}

impl MessageSummary {
    /// The summary row for one `Email/get` result.
    ///
    /// Fails only on an `Email` without an id. Other missing properties give
    /// empty fields.
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
            // A `Message-ID` header has one id. JMAP gives a list for a
            // message with more ids. Clients thread on the first id.
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

/// The flags and the remaining labels from one keyword set.
///
/// RFC 8621 §4.1.1 requires lowercase keywords, but the match ignores case.
/// Thus, a server that sends `$Seen` still marks the message as read.
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
        // RFC 8621 §4.1.1: the value is always true. Ignore a false value.
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

/// The ancestors of this message for Camel threading, oldest first, ending in
/// the parent.
///
/// Some mailers send `In-Reply-To` but not `References`. Without the
/// `In-Reply-To` parent, such a reply starts a new thread. The function
/// appends the parent only if the chain does not already contain it.
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

    /// The ceiling is above the advertised size, so a message of exactly that
    /// size arrives.
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

    /// The ceiling stays proportional to the advertised size: a small message
    /// does not get a large buffer.
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

    /// A message stored with bare LF and served with CRLF grows by one octet
    /// per line. With 64-octet lines, an 8 MiB message grows by more than the
    /// flat margin, so the proportional term must cover it.
    #[test]
    fn a_large_message_gets_room_for_its_line_endings_to_grow() {
        let size = 8 * 1024 * 1024;
        let served = size + size / 64;
        let ceiling = download_ceiling(Some(size));
        assert!(
            ceiling >= served,
            "an {size}-octet row served as {served} octets got a ceiling of {ceiling}"
        );
    }

    /// A short message gets the full flat margin.
    #[test]
    fn a_short_message_gets_the_flat_margin() {
        assert_eq!(download_ceiling(Some(0)), 64 * 1024);
    }

    /// A row without a size gets the fallback ceiling, not an unbounded read.
    #[test]
    fn a_row_that_states_no_size_gets_the_fallback_ceiling() {
        assert_eq!(download_ceiling(None), limits::MAX_BLOB_BYTES);
    }

    /// An impossible size saturates and does not wrap to a small ceiling.
    #[test]
    fn a_size_no_message_has_saturates_rather_than_wrapping() {
        assert_eq!(download_ceiling(Some(u64::MAX)), u64::MAX);
    }
}
