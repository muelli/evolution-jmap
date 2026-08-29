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

/// The permissions the user has for a mailbox (RFC 8621 §2).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct MailboxRights {
    #[serde(default)]
    pub may_read_items: bool,
    #[serde(default)]
    pub may_add_items: bool,
    #[serde(default)]
    pub may_remove_items: bool,
    #[serde(default)]
    pub may_set_seen: bool,
    #[serde(default)]
    pub may_set_keywords: bool,
    #[serde(default)]
    pub may_create_child: bool,
    #[serde(default)]
    pub may_rename: bool,
    #[serde(default)]
    pub may_delete: bool,
    #[serde(default)]
    pub may_submit: bool,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

/// A thread of emails (RFC 8621 §3).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct Thread {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<Id>,
    #[serde(default)]
    pub email_ids: Vec<Id>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

/// Vacation response auto-responder settings (RFC 8621 §8).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct VacationResponse {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<Id>,
    #[serde(default)]
    pub is_enabled: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from_date: Option<UtcDate>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to_date: Option<UtcDate>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subject: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text_body: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub html_body: Option<String>,
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
    pub const ALL: &str = "all";
    pub const ARCHIVE: &str = "archive";
    pub const DRAFTS: &str = "drafts";
    pub const FLAGGED: &str = "flagged";
    pub const IMPORTANT: &str = "important";
    pub const INBOX: &str = "inbox";
    pub const JUNK: &str = "junk";
    pub const SENT: &str = "sent";
    pub const SUBSCRIBED: &str = "subscribed";
    pub const TRASH: &str = "trash";
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

/// One message handed to `Email/import` (RFC 8621 §4.8): the blob the raw
/// message was uploaded as, and what the account should file it under.
///
/// Every property is optional although the RFC makes `blobId` and `mailboxIds`
/// required, for the reason [`Email`]'s are: this type is what a request is
/// *deserialized into* as well as what a client fills in, and RFC 8621 §4.8 is
/// explicit that a `blobId`, `mailboxIds` or `keywords` that is "missing, wrong
/// type, id not found" is refused with an `invalidProperties` `SetError` — a
/// per-message refusal, in a method whose other messages must still be
/// imported. A required field here would turn one client mistake into a whole
/// call failing to parse, which is the answer the RFC reserves for a request
/// that is not an `Email/import` at all.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct EmailImport {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blob_id: Option<Id>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mailbox_ids: Option<BTreeMap<Id, bool>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub keywords: Option<BTreeMap<String, bool>>,
    /// The date the message should sort by. Absent leaves it to the server,
    /// which RFC 8621 §4.8 defines as the most recent `Received` header's date
    /// or, failing that, the time of the import.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub received_at: Option<UtcDate>,
}

impl EmailImport {
    /// A message to file in one mailbox with no keywords.
    pub fn new(blob_id: impl Into<Id>, mailbox_id: impl Into<Id>) -> Self {
        Self {
            blob_id: Some(blob_id.into()),
            mailbox_ids: Some([(mailbox_id.into(), true)].into()),
            keywords: None,
            received_at: None,
        }
    }

    pub fn keyword(mut self, keyword: impl Into<String>) -> Self {
        self.keywords
            .get_or_insert_with(BTreeMap::new)
            .insert(keyword.into(), true);
        self
    }

    pub fn received_at(mut self, received_at: impl Into<UtcDate>) -> Self {
        self.received_at = Some(received_at.into());
        self
    }
}

/// `Email/import` arguments (RFC 8621 §4.8).
///
/// Not a [`crate::methods::SetRequest`]: an import has no update and no destroy
/// half, its creations are `EmailImport`s rather than `Email`s, and the map is
/// named `emails` rather than `create`. What it does share is `ifInState` and
/// the `stateMismatch` that comes of it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EmailImportRequest {
    pub account_id: Id,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub if_in_state: Option<crate::state::State>,
    pub emails: BTreeMap<String, EmailImport>,
}

impl EmailImportRequest {
    pub fn new(account_id: impl Into<Id>) -> Self {
        Self {
            account_id: account_id.into(),
            if_in_state: None,
            emails: BTreeMap::new(),
        }
    }

    pub fn import(mut self, creation_id: impl Into<String>, email: EmailImport) -> Self {
        self.emails.insert(creation_id.into(), email);
        self
    }

    pub fn if_in_state(mut self, state: impl Into<crate::state::State>) -> Self {
        self.if_in_state = Some(state.into());
        self
    }
}

/// `Email/import` response (RFC 8621 §4.8).
///
/// `created` carries the properties the server chose — the RFC names `id`,
/// `blobId`, `threadId` and `size` — as an [`Email`], whose properties are all
/// optional, so what the server left out stays left out rather than arriving as
/// a default.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EmailImportResponse {
    pub account_id: Id,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub old_state: Option<crate::state::State>,
    pub new_state: crate::state::State,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created: Option<BTreeMap<String, Email>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub not_created: Option<BTreeMap<String, crate::error::SetError>>,
}

/// The `SetError` type RFC 8621 §4.8 adds for `Email/import`.
///
/// The generic types of RFC 8620 §5.3 cannot say this one: the properties of
/// the `EmailImport` are all fine and the blob is there — what is wrong is the
/// *content* of the blob, which is not a message. A server is allowed to repair
/// such a blob instead and answer with a `blobId` of its own; refusing it is
/// the other branch of the same paragraph.
///
/// `alreadyExists` is the paragraph before it and is deliberately not here: it
/// belongs to a server that forbids two copies of one message in an account,
/// which RFC 8621 §4.8 leaves as a MAY.
pub mod email_import_error {
    pub const INVALID_EMAIL: &str = "invalidEmail";
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
    #[serde(default)]
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
    pub in_mailbox_other_than: Option<Vec<Id>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_size: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_size: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub has_keyword: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub not_keyword: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub has_attachment: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cc: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bcc: Option<String>,
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
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct EmailSubmission {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<Id>,
    #[serde(default)]
    pub identity_id: Id,
    #[serde(default)]
    pub email_id: Id,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread_id: Option<Id>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub envelope: Option<Envelope>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub send_at: Option<UtcDate>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub undo_status: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delivery_status: Option<BTreeMap<String, DeliveryStatus>>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

/// The delivery status of an email submission to one recipient (RFC 8621 §7.1.1).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeliveryStatus {
    #[serde(default)]
    pub smtp_reply: String,
    #[serde(default = "default_delivered")]
    pub delivered: String,
    #[serde(default = "default_displayed")]
    pub displayed: String,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

fn default_delivered() -> String {
    delivered::UNKNOWN.to_owned()
}

fn default_displayed() -> String {
    displayed::UNKNOWN.to_owned()
}

impl Default for DeliveryStatus {
    fn default() -> Self {
        Self {
            smtp_reply: String::new(),
            delivered: default_delivered(),
            displayed: default_displayed(),
            extra: BTreeMap::new(),
        }
    }
}

/// Delivered status values for `DeliveryStatus` (RFC 8621 §7.1.1).
pub mod delivered {
    pub const QUEUED: &str = "queued";
    pub const YES: &str = "yes";
    pub const NO: &str = "no";
    pub const UNKNOWN: &str = "unknown";
}

/// Displayed status values for `DeliveryStatus` (RFC 8621 §7.1.1).
pub mod displayed {
    pub const UNKNOWN: &str = "unknown";
    pub const YES: &str = "yes";
}

/// `EmailSubmission/query` filter conditions (RFC 8621 §7.3).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct EmailSubmissionQueryFilter {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub identity_ids: Option<Vec<Id>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub email_ids: Option<Vec<Id>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread_ids: Option<Vec<Id>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub undo_status: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub before: Option<UtcDate>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after: Option<UtcDate>,
}

/// Snippet of matching text in an email search (RFC 8621 §5).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct SearchSnippet {
    pub email_id: Id,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subject: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preview: Option<String>,
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
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct Envelope {
    pub mail_from: EnvelopeAddress,
    #[serde(default)]
    pub rcpt_to: Vec<EnvelopeAddress>,
}

/// One envelope address; `parameters` (SMTP extensions) is nullable and
/// always present on the wire.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct EnvelopeAddress {
    pub email: String,
    #[serde(default)]
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

/// The `SetError` types RFC 8621 §6.4 adds for `Identity/set`.
pub mod identity_set_error {
    pub const FORBIDDEN_FROM: &str = "forbiddenFrom";
    pub const CANNOT_DESTROY_DEFAULT: &str = "cannotDestroyDefault";
}

/// The `SetError` types RFC 8621 §7.5 adds for `EmailSubmission/set`.
pub mod email_submission_set_error {
    pub const CANNOT_UNSEND: &str = "cannotUnsend";
    pub const TOO_MANY_RECIPIENTS: &str = "tooManyRecipients";
    pub const NO_RECIPIENTS: &str = "noRecipients";
    pub const INVALID_RECIPIENTS: &str = "invalidRecipients";
    pub const FORBIDDEN_MAIL_FROM: &str = "forbiddenMailFrom";
    pub const FORBIDDEN_FROM: &str = "forbiddenFrom";
}

/// Standard undoStatus values (RFC 8621 §7).
pub mod undo_status {
    pub const PENDING: &str = "pending";
    pub const FINAL: &str = "final";
    pub const CANCELED: &str = "canceled";
}
