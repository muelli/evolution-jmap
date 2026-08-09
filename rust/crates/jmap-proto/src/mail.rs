// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! RFC 8621 mail types: `Mailbox`, `Email`, `Identity`, `EmailSubmission`.
//!
//! Every object carries a `#[serde(flatten)] extra` map so properties this
//! crate does not model survive round-trips.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::id::Id;
use crate::state::UtcDate;

/// A mailbox (folder/label), RFC 8621 §2.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct Mailbox {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<Id>,
    #[serde(default)]
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_id: Option<Id>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sort_order: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total_emails: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unread_emails: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total_threads: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unread_threads: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub is_subscribed: Option<bool>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

/// The two `SetError` types RFC 8621 §2.5 adds for `Mailbox/set`.
///
/// Both are refusals to *destroy*, and both exist because the generic types of
/// RFC 8620 §5.3 cannot say what a client would have to do about them: a
/// mailbox that still holds another mailbox, and one that still holds mail, are
/// removable once the user has decided what becomes of what is inside. A
/// `forbidden` would read as "not yours to delete", which is a different
/// conversation with the user.
pub mod mailbox_set_error {
    pub const HAS_CHILD: &str = "mailboxHasChild";
    pub const HAS_EMAIL: &str = "mailboxHasEmail";
}

/// Well-known mailbox roles (RFC 8457 registry, referenced by RFC 8621).
pub mod role {
    pub const INBOX: &str = "inbox";
    pub const DRAFTS: &str = "drafts";
    pub const SENT: &str = "sent";
    pub const TRASH: &str = "trash";
    pub const JUNK: &str = "junk";
    pub const ARCHIVE: &str = "archive";
}

/// An email message, RFC 8621 §4. All properties are optional because
/// `Email/get` returns only what was asked for and `Email/set` sends only
/// what the client provides.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct Email {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<Id>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blob_id: Option<Id>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread_id: Option<Id>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mailbox_ids: Option<BTreeMap<Id, bool>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub keywords: Option<BTreeMap<String, bool>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub received_at: Option<UtcDate>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message_id: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub in_reply_to: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub references: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sender: Option<Vec<EmailAddress>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from: Option<Vec<EmailAddress>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to: Option<Vec<EmailAddress>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cc: Option<Vec<EmailAddress>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bcc: Option<Vec<EmailAddress>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reply_to: Option<Vec<EmailAddress>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subject: Option<String>,
    /// Date header; unlike `receivedAt` this may carry a zone offset.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sent_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub has_attachment: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preview: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body_structure: Option<EmailBodyPart>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body_values: Option<BTreeMap<String, EmailBodyValue>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text_body: Option<Vec<EmailBodyPart>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub html_body: Option<Vec<EmailBodyPart>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attachments: Option<Vec<EmailBodyPart>>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

/// Well-known keywords (RFC 8621 §4.1.1, which defers the rest of the set to
/// the IMAP keywords registry of RFC 5788).
pub mod keyword {
    pub const SEEN: &str = "$seen";
    pub const DRAFT: &str = "$draft";
    pub const FLAGGED: &str = "$flagged";
    pub const ANSWERED: &str = "$answered";
    pub const FORWARDED: &str = "$forwarded";
    pub const JUNK: &str = "$junk";
    pub const NOT_JUNK: &str = "$notjunk";
}

/// A name/address pair (RFC 8621 §4.1.2.3). `name` is nullable but always
/// present on the wire.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EmailAddress {
    pub name: Option<String>,
    pub email: String,
}

impl EmailAddress {
    pub fn new(name: Option<&str>, email: &str) -> Self {
        Self {
            name: name.map(str::to_owned),
            email: email.to_owned(),
        }
    }
}

/// A node in the MIME tree (RFC 8621 §4.1.4).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct EmailBodyPart {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub part_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blob_id: Option<Id>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(rename = "type", default, skip_serializing_if = "Option::is_none")]
    pub content_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub charset: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub disposition: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cid: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub location: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sub_parts: Option<Vec<EmailBodyPart>>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

/// Decoded body content addressed by `partId` (RFC 8621 §4.1.4).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EmailBodyValue {
    pub value: String,
    #[serde(default)]
    pub is_encoding_problem: bool,
    #[serde(default)]
    pub is_truncated: bool,
}

impl EmailBodyValue {
    pub fn new(value: impl Into<String>) -> Self {
        Self {
            value: value.into(),
            is_encoding_problem: false,
            is_truncated: false,
        }
    }
}

/// `Email/query` filter conditions (RFC 8621 §4.4.1). Flat conditions only —
/// no AND/OR/NOT operator nesting yet.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct EmailQueryFilter {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub in_mailbox: Option<Id>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub has_keyword: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub not_keyword: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subject: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub before: Option<UtcDate>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after: Option<UtcDate>,
}

impl EmailQueryFilter {
    pub fn in_mailbox(mailbox_id: impl Into<Id>) -> Self {
        Self {
            in_mailbox: Some(mailbox_id.into()),
            ..Self::default()
        }
    }
}

/// A sending identity (RFC 8621 §6).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct Identity {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<Id>,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub email: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reply_to: Option<Vec<EmailAddress>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bcc: Option<Vec<EmailAddress>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text_signature: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub html_signature: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub may_delete: Option<bool>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

/// A submission of an email for delivery (RFC 8621 §7).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EmailSubmission {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<Id>,
    pub identity_id: Id,
    pub email_id: Id,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread_id: Option<Id>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub envelope: Option<Envelope>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub send_at: Option<UtcDate>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub undo_status: Option<String>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

/// `EmailSubmission/set` arguments: the standard `/set` shape plus the
/// `onSuccess*` extensions (RFC 8621 §7.5). Keys of `onSuccessUpdateEmail`
/// are submission ids or `#`-prefixed creation ids from the same call.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EmailSubmissionSetRequest {
    #[serde(flatten)]
    pub set: crate::methods::SetRequest<EmailSubmission>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub on_success_update_email: Option<BTreeMap<String, Value>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub on_success_destroy_email: Option<Vec<String>>,
}

/// SMTP envelope (RFC 8621 §7).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Envelope {
    pub mail_from: EnvelopeAddress,
    pub rcpt_to: Vec<EnvelopeAddress>,
}

/// One envelope address; `parameters` (SMTP extensions) is nullable and
/// always present on the wire.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EnvelopeAddress {
    pub email: String,
    pub parameters: Option<Value>,
}

impl EnvelopeAddress {
    pub fn new(email: impl Into<String>) -> Self {
        Self {
            email: email.into(),
            parameters: None,
        }
    }
}
