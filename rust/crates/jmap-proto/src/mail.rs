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
    /// Permissions granted to specific principals on this mailbox (draft-ietf-jmap-mail-sharing §2).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub share_with: Option<BTreeMap<Id, MailboxRights>>,
    /// Server-computed permissions on this mailbox (RFC 8621 §2).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub my_rights: Option<MailboxRights>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl Mailbox {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            ..Self::default()
        }
    }

    pub fn with_parent_id(mut self, parent_id: impl Into<Id>) -> Self {
        self.parent_id = Some(parent_id.into());
        self
    }

    pub fn with_role(mut self, role: impl Into<String>) -> Self {
        self.role = Some(role.into());
        self
    }

    pub fn with_sort_order(mut self, sort_order: u32) -> Self {
        self.sort_order = Some(sort_order);
        self
    }

    pub fn is_subscribed(mut self, subscribed: bool) -> Self {
        self.is_subscribed = Some(subscribed);
        self
    }

    pub fn with_share_with(mut self, share_with: BTreeMap<Id, MailboxRights>) -> Self {
        self.share_with = Some(share_with);
        self
    }

    pub fn with_shared_principal(
        mut self,
        principal_id: impl Into<Id>,
        rights: MailboxRights,
    ) -> Self {
        self.share_with
            .get_or_insert_with(BTreeMap::new)
            .insert(principal_id.into(), rights);
        self
    }
}

/// Mail sharing capability properties (draft-ietf-jmap-mail-sharing §1.2).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct MailShareCapability {
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl MailShareCapability {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_extra(mut self, extra: BTreeMap<String, Value>) -> Self {
        self.extra = extra;
        self
    }
}

/// `Mailbox.myRights`, RFC 8621 §2.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct MailboxRights {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub may_read_items: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub may_add_items: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub may_remove_items: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub may_set_seen: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub may_set_keywords: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub may_create_child: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub may_rename: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub may_delete: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub may_submit: Option<bool>,
    /// Whether the user may administer and modify the shareWith property (draft-ietf-jmap-mail-sharing §2).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub may_share: Option<bool>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl MailboxRights {
    pub fn all() -> Self {
        Self {
            may_read_items: Some(true),
            may_add_items: Some(true),
            may_remove_items: Some(true),
            may_set_seen: Some(true),
            may_set_keywords: Some(true),
            may_create_child: Some(true),
            may_rename: Some(true),
            may_delete: Some(true),
            may_submit: Some(true),
            may_share: Some(true),
            extra: BTreeMap::new(),
        }
    }

    pub fn read_only() -> Self {
        Self {
            may_read_items: Some(true),
            ..Self::default()
        }
    }

    pub fn may_share(&self) -> bool {
        self.may_share.unwrap_or(false)
    }

    pub fn with_may_share(mut self, may_share: bool) -> Self {
        self.may_share = Some(may_share);
        self
    }
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

impl Thread {
    pub fn new(id: impl Into<Id>, email_ids: impl IntoIterator<Item = impl Into<Id>>) -> Self {
        Self {
            id: Some(id.into()),
            email_ids: email_ids.into_iter().map(Into::into).collect(),
            extra: BTreeMap::new(),
        }
    }
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

impl VacationResponse {
    pub fn new(is_enabled: bool) -> Self {
        Self {
            id: None,
            is_enabled,
            from_date: None,
            to_date: None,
            subject: None,
            text_body: None,
            html_body: None,
            extra: BTreeMap::new(),
        }
    }

    pub fn with_id(mut self, id: impl Into<Id>) -> Self {
        self.id = Some(id.into());
        self
    }

    pub fn with_from_date(mut self, from_date: impl Into<UtcDate>) -> Self {
        self.from_date = Some(from_date.into());
        self
    }

    pub fn with_to_date(mut self, to_date: impl Into<UtcDate>) -> Self {
        self.to_date = Some(to_date.into());
        self
    }

    pub fn with_subject(mut self, subject: impl Into<String>) -> Self {
        self.subject = Some(subject.into());
        self
    }

    pub fn with_text_body(mut self, text_body: impl Into<String>) -> Self {
        self.text_body = Some(text_body.into());
        self
    }

    pub fn with_html_body(mut self, html_body: impl Into<String>) -> Self {
        self.html_body = Some(html_body.into());
        self
    }
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
    pub headers: Option<Vec<EmailHeader>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attachments: Option<Vec<EmailBodyPart>>,
    /// S/MIME signature and certificate verification status (RFC 9219 §4.1).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub smime_status: Option<String>,
    /// S/MIME verification errors and warnings (RFC 9219 §4.1).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub smime_errors: Option<Vec<String>>,
    /// Time at which the S/MIME signature was verified (RFC 9219 §4.1).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub smime_verified_at: Option<UtcDate>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl Email {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_id(mut self, id: impl Into<Id>) -> Self {
        self.id = Some(id.into());
        self
    }

    pub fn with_blob_id(mut self, blob_id: impl Into<Id>) -> Self {
        self.blob_id = Some(blob_id.into());
        self
    }

    pub fn with_thread_id(mut self, thread_id: impl Into<Id>) -> Self {
        self.thread_id = Some(thread_id.into());
        self
    }

    pub fn in_mailbox(mut self, mailbox_id: impl Into<Id>) -> Self {
        self.mailbox_ids
            .get_or_insert_with(BTreeMap::new)
            .insert(mailbox_id.into(), true);
        self
    }

    pub fn with_mailbox_ids(mut self, mailbox_ids: BTreeMap<Id, bool>) -> Self {
        self.mailbox_ids = Some(mailbox_ids);
        self
    }

    pub fn with_keyword(mut self, keyword: impl Into<String>) -> Self {
        self.keywords
            .get_or_insert_with(BTreeMap::new)
            .insert(keyword.into(), true);
        self
    }

    pub fn with_keywords(mut self, keywords: BTreeMap<String, bool>) -> Self {
        self.keywords = Some(keywords);
        self
    }

    pub fn with_size(mut self, size: u64) -> Self {
        self.size = Some(size);
        self
    }

    pub fn with_received_at(mut self, received_at: impl Into<UtcDate>) -> Self {
        self.received_at = Some(received_at.into());
        self
    }

    pub fn with_from(mut self, from: impl IntoIterator<Item = EmailAddress>) -> Self {
        self.from = Some(from.into_iter().collect());
        self
    }

    pub fn with_to(mut self, to: impl IntoIterator<Item = EmailAddress>) -> Self {
        self.to = Some(to.into_iter().collect());
        self
    }

    pub fn with_cc(mut self, cc: impl IntoIterator<Item = EmailAddress>) -> Self {
        self.cc = Some(cc.into_iter().collect());
        self
    }

    pub fn with_bcc(mut self, bcc: impl IntoIterator<Item = EmailAddress>) -> Self {
        self.bcc = Some(bcc.into_iter().collect());
        self
    }

    pub fn with_reply_to(mut self, reply_to: impl IntoIterator<Item = EmailAddress>) -> Self {
        self.reply_to = Some(reply_to.into_iter().collect());
        self
    }

    pub fn with_subject(mut self, subject: impl Into<String>) -> Self {
        self.subject = Some(subject.into());
        self
    }

    pub fn with_sent_at(mut self, sent_at: impl Into<String>) -> Self {
        self.sent_at = Some(sent_at.into());
        self
    }

    pub fn with_preview(mut self, preview: impl Into<String>) -> Self {
        self.preview = Some(preview.into());
        self
    }

    pub fn has_attachment(mut self, has: bool) -> Self {
        self.has_attachment = Some(has);
        self
    }

    pub fn with_header(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.headers
            .get_or_insert_with(Vec::new)
            .push(EmailHeader::new(name, value));
        self
    }

    pub fn with_headers(mut self, headers: impl IntoIterator<Item = EmailHeader>) -> Self {
        self.headers = Some(headers.into_iter().collect());
        self
    }

    pub fn with_body_structure(mut self, body_structure: EmailBodyPart) -> Self {
        self.body_structure = Some(body_structure);
        self
    }

    pub fn with_body_values(mut self, body_values: BTreeMap<String, EmailBodyValue>) -> Self {
        self.body_values = Some(body_values);
        self
    }

    pub fn with_text_body(mut self, text_body: impl IntoIterator<Item = EmailBodyPart>) -> Self {
        self.text_body = Some(text_body.into_iter().collect());
        self
    }

    pub fn with_html_body(mut self, html_body: impl IntoIterator<Item = EmailBodyPart>) -> Self {
        self.html_body = Some(html_body.into_iter().collect());
        self
    }

    pub fn with_attachments(
        mut self,
        attachments: impl IntoIterator<Item = EmailBodyPart>,
    ) -> Self {
        self.attachments = Some(attachments.into_iter().collect());
        self
    }

    pub fn with_smime_status(mut self, status: impl Into<String>) -> Self {
        self.smime_status = Some(status.into());
        self
    }

    pub fn with_smime_errors(
        mut self,
        errors: impl IntoIterator<Item = impl Into<String>>,
    ) -> Self {
        self.smime_errors = Some(errors.into_iter().map(Into::into).collect());
        self
    }

    pub fn with_smime_verified_at(mut self, verified_at: impl Into<UtcDate>) -> Self {
        self.smime_verified_at = Some(verified_at.into());
        self
    }
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

impl EmailImportResponse {
    pub fn new(account_id: impl Into<Id>, new_state: impl Into<crate::state::State>) -> Self {
        Self {
            account_id: account_id.into(),
            old_state: None,
            new_state: new_state.into(),
            created: None,
            not_created: None,
        }
    }

    pub fn with_old_state(mut self, old_state: impl Into<crate::state::State>) -> Self {
        self.old_state = Some(old_state.into());
        self
    }

    pub fn with_created(mut self, created: BTreeMap<String, Email>) -> Self {
        self.created = Some(created);
        self
    }

    pub fn with_not_created(
        mut self,
        not_created: BTreeMap<String, crate::error::SetError>,
    ) -> Self {
        self.not_created = Some(not_created);
        self
    }
}

/// `Email/parse` arguments (RFC 8621 §4.7).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EmailParseRequest {
    pub account_id: Id,
    pub blob_ids: Vec<Id>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub properties: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body_properties: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub fetch_text_body_values: bool,
    #[serde(
        rename = "fetchHTMLBodyValues",
        default,
        skip_serializing_if = "is_false"
    )]
    pub fetch_html_body_values: bool,
    #[serde(default, skip_serializing_if = "is_false")]
    pub fetch_all_body_values: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_body_value_bytes: Option<u64>,
}

fn is_false(value: &bool) -> bool {
    !*value
}

impl EmailParseRequest {
    pub fn new(
        account_id: impl Into<Id>,
        blob_ids: impl IntoIterator<Item = impl Into<Id>>,
    ) -> Self {
        Self {
            account_id: account_id.into(),
            blob_ids: blob_ids.into_iter().map(Into::into).collect(),
            properties: None,
            body_properties: None,
            fetch_text_body_values: false,
            fetch_html_body_values: false,
            fetch_all_body_values: false,
            max_body_value_bytes: None,
        }
    }

    pub fn properties(mut self, properties: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.properties = Some(properties.into_iter().map(Into::into).collect());
        self
    }

    pub fn body_properties(
        mut self,
        body_properties: impl IntoIterator<Item = impl Into<String>>,
    ) -> Self {
        self.body_properties = Some(body_properties.into_iter().map(Into::into).collect());
        self
    }

    pub fn fetch_text_body_values(mut self) -> Self {
        self.fetch_text_body_values = true;
        self
    }

    pub fn fetch_html_body_values(mut self) -> Self {
        self.fetch_html_body_values = true;
        self
    }

    pub fn fetch_all_body_values(mut self) -> Self {
        self.fetch_all_body_values = true;
        self
    }

    pub fn max_body_value_bytes(mut self, max_bytes: u64) -> Self {
        self.max_body_value_bytes = Some(max_bytes);
        self
    }
}

/// `Email/parse` response (RFC 8621 §4.7).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EmailParseResponse {
    pub account_id: Id,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parsed: Option<BTreeMap<Id, Email>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub not_parsable: Option<Vec<Id>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub not_found: Option<Vec<Id>>,
}

impl EmailParseResponse {
    pub fn new(account_id: impl Into<Id>) -> Self {
        Self {
            account_id: account_id.into(),
            parsed: None,
            not_parsable: None,
            not_found: None,
        }
    }

    pub fn with_parsed(mut self, parsed: BTreeMap<Id, Email>) -> Self {
        self.parsed = Some(parsed);
        self
    }

    pub fn with_not_parsable(
        mut self,
        not_parsable: impl IntoIterator<Item = impl Into<Id>>,
    ) -> Self {
        self.not_parsable = Some(not_parsable.into_iter().map(Into::into).collect());
        self
    }

    pub fn with_not_found(mut self, not_found: impl IntoIterator<Item = impl Into<Id>>) -> Self {
        self.not_found = Some(not_found.into_iter().map(Into::into).collect());
        self
    }
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

/// A parsed email header (RFC 8621 §4.1.2.1).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct EmailHeader {
    pub name: String,
    pub value: String,
}

impl EmailHeader {
    pub fn new(name: impl Into<String>, value: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            value: value.into(),
        }
    }

    pub fn with_name(mut self, name: impl Into<String>) -> Self {
        self.name = name.into();
        self
    }

    pub fn with_value(mut self, value: impl Into<String>) -> Self {
        self.value = value.into();
        self
    }
}

/// A name/address pair (RFC 8621 §4.1.2.3). `name` is nullable but always
/// present on the wire.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
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

    pub fn from_email(email: impl Into<String>) -> Self {
        Self {
            name: None,
            email: email.into(),
        }
    }

    pub fn with_name(mut self, name: impl Into<String>) -> Self {
        self.name = Some(name.into());
        self
    }

    pub fn with_email(mut self, email: impl Into<String>) -> Self {
        self.email = email.into();
        self
    }
}

/// An RFC 5322 address group (RFC 8621 §4.1.2.3).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct EmailAddressGroup {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default)]
    pub addresses: Vec<EmailAddress>,
}

impl EmailAddressGroup {
    pub fn new(addresses: impl IntoIterator<Item = EmailAddress>) -> Self {
        Self {
            name: None,
            addresses: addresses.into_iter().collect(),
        }
    }

    pub fn with_name(mut self, name: impl Into<String>) -> Self {
        self.name = Some(name.into());
        self
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub headers: Option<Vec<EmailHeader>>,
    /// S/MIME signature and certificate verification status (RFC 9219 §4.1).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub smime_status: Option<String>,
    /// S/MIME verification errors and warnings (RFC 9219 §4.1).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub smime_errors: Option<Vec<String>>,
    /// Time at which the S/MIME signature was verified (RFC 9219 §4.1).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub smime_verified_at: Option<UtcDate>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl EmailBodyPart {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_part_id(mut self, part_id: impl Into<String>) -> Self {
        self.part_id = Some(part_id.into());
        self
    }

    pub fn with_blob_id(mut self, blob_id: impl Into<Id>) -> Self {
        self.blob_id = Some(blob_id.into());
        self
    }

    pub fn with_size(mut self, size: u64) -> Self {
        self.size = Some(size);
        self
    }

    pub fn with_name(mut self, name: impl Into<String>) -> Self {
        self.name = Some(name.into());
        self
    }

    pub fn with_content_type(mut self, content_type: impl Into<String>) -> Self {
        self.content_type = Some(content_type.into());
        self
    }

    pub fn with_charset(mut self, charset: impl Into<String>) -> Self {
        self.charset = Some(charset.into());
        self
    }

    pub fn with_disposition(mut self, disposition: impl Into<String>) -> Self {
        self.disposition = Some(disposition.into());
        self
    }

    pub fn with_cid(mut self, cid: impl Into<String>) -> Self {
        self.cid = Some(cid.into());
        self
    }

    pub fn with_location(mut self, location: impl Into<String>) -> Self {
        self.location = Some(location.into());
        self
    }

    pub fn with_sub_parts(mut self, sub_parts: impl IntoIterator<Item = EmailBodyPart>) -> Self {
        self.sub_parts = Some(sub_parts.into_iter().collect());
        self
    }

    pub fn with_headers(mut self, headers: impl IntoIterator<Item = EmailHeader>) -> Self {
        self.headers = Some(headers.into_iter().collect());
        self
    }

    pub fn with_smime_status(mut self, status: impl Into<String>) -> Self {
        self.smime_status = Some(status.into());
        self
    }

    pub fn with_smime_errors(
        mut self,
        errors: impl IntoIterator<Item = impl Into<String>>,
    ) -> Self {
        self.smime_errors = Some(errors.into_iter().map(Into::into).collect());
        self
    }

    pub fn with_smime_verified_at(mut self, verified_at: impl Into<UtcDate>) -> Self {
        self.smime_verified_at = Some(verified_at.into());
        self
    }
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
    pub all_in_thread_have_keyword: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub some_in_thread_have_keyword: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub none_in_thread_have_keyword: Option<String>,
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
    pub header: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub before: Option<UtcDate>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after: Option<UtcDate>,
    /// S/MIME message presence filter (RFC 9219 §4.2).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub has_smime: Option<bool>,
    /// S/MIME verified message filter (RFC 9219 §4.2).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub has_verified_smime: Option<bool>,
}

impl EmailQueryFilter {
    pub fn in_mailbox(mailbox_id: impl Into<Id>) -> Self {
        Self {
            in_mailbox: Some(mailbox_id.into()),
            ..Self::default()
        }
    }

    pub fn in_mailbox_other_than(
        mut self,
        mailboxes: impl IntoIterator<Item = impl Into<Id>>,
    ) -> Self {
        self.in_mailbox_other_than = Some(mailboxes.into_iter().map(Into::into).collect());
        self
    }

    pub fn min_size(mut self, min_size: u64) -> Self {
        self.min_size = Some(min_size);
        self
    }

    pub fn max_size(mut self, max_size: u64) -> Self {
        self.max_size = Some(max_size);
        self
    }

    pub fn has_keyword(mut self, keyword: impl Into<String>) -> Self {
        self.has_keyword = Some(keyword.into());
        self
    }

    pub fn not_keyword(mut self, keyword: impl Into<String>) -> Self {
        self.not_keyword = Some(keyword.into());
        self
    }

    pub fn all_in_thread_have_keyword(mut self, keyword: impl Into<String>) -> Self {
        self.all_in_thread_have_keyword = Some(keyword.into());
        self
    }

    pub fn some_in_thread_have_keyword(mut self, keyword: impl Into<String>) -> Self {
        self.some_in_thread_have_keyword = Some(keyword.into());
        self
    }

    pub fn none_in_thread_have_keyword(mut self, keyword: impl Into<String>) -> Self {
        self.none_in_thread_have_keyword = Some(keyword.into());
        self
    }

    pub fn has_attachment(mut self, has: bool) -> Self {
        self.has_attachment = Some(has);
        self
    }

    pub fn text(mut self, text: impl Into<String>) -> Self {
        self.text = Some(text.into());
        self
    }

    pub fn body(mut self, body: impl Into<String>) -> Self {
        self.body = Some(body.into());
        self
    }

    pub fn from(mut self, from: impl Into<String>) -> Self {
        self.from = Some(from.into());
        self
    }

    pub fn to(mut self, to: impl Into<String>) -> Self {
        self.to = Some(to.into());
        self
    }

    pub fn cc(mut self, cc: impl Into<String>) -> Self {
        self.cc = Some(cc.into());
        self
    }

    pub fn bcc(mut self, bcc: impl Into<String>) -> Self {
        self.bcc = Some(bcc.into());
        self
    }

    pub fn subject(mut self, subject: impl Into<String>) -> Self {
        self.subject = Some(subject.into());
        self
    }

    pub fn header(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.header = Some(vec![name.into(), value.into()]);
        self
    }

    pub fn before(mut self, before: impl Into<UtcDate>) -> Self {
        self.before = Some(before.into());
        self
    }

    pub fn after(mut self, after: impl Into<UtcDate>) -> Self {
        self.after = Some(after.into());
        self
    }

    pub fn time_range(mut self, after: Option<UtcDate>, before: Option<UtcDate>) -> Self {
        self.after = after;
        self.before = before;
        self
    }

    pub fn with_has_smime(mut self, has: bool) -> Self {
        self.has_smime = Some(has);
        self
    }

    pub fn with_has_verified_smime(mut self, has: bool) -> Self {
        self.has_verified_smime = Some(has);
        self
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
    pub draft_mailbox_id: Option<Id>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub may_delete: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub may_send: Option<bool>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl Identity {
    pub fn new(name: impl Into<String>, email: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            email: email.into(),
            ..Self::default()
        }
    }

    pub fn with_reply_to(mut self, reply_to: impl IntoIterator<Item = EmailAddress>) -> Self {
        self.reply_to = Some(reply_to.into_iter().collect());
        self
    }

    pub fn with_bcc(mut self, bcc: impl IntoIterator<Item = EmailAddress>) -> Self {
        self.bcc = Some(bcc.into_iter().collect());
        self
    }

    pub fn with_text_signature(mut self, sig: impl Into<String>) -> Self {
        self.text_signature = Some(sig.into());
        self
    }

    pub fn with_html_signature(mut self, sig: impl Into<String>) -> Self {
        self.html_signature = Some(sig.into());
        self
    }

    pub fn with_draft_mailbox_id(mut self, draft_mailbox_id: impl Into<Id>) -> Self {
        self.draft_mailbox_id = Some(draft_mailbox_id.into());
        self
    }

    pub fn may_send(mut self, may_send: bool) -> Self {
        self.may_send = Some(may_send);
        self
    }
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

impl EmailSubmission {
    pub fn new(identity_id: impl Into<Id>, email_id: impl Into<Id>) -> Self {
        Self {
            identity_id: identity_id.into(),
            email_id: email_id.into(),
            ..Self::default()
        }
    }

    pub fn with_thread_id(mut self, thread_id: impl Into<Id>) -> Self {
        self.thread_id = Some(thread_id.into());
        self
    }

    pub fn with_envelope(mut self, envelope: Envelope) -> Self {
        self.envelope = Some(envelope);
        self
    }

    pub fn with_send_at(mut self, send_at: impl Into<UtcDate>) -> Self {
        self.send_at = Some(send_at.into());
        self
    }

    pub fn with_undo_status(mut self, undo_status: impl Into<String>) -> Self {
        self.undo_status = Some(undo_status.into());
        self
    }
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

impl EmailSubmissionQueryFilter {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_identity_ids(
        mut self,
        identity_ids: impl IntoIterator<Item = impl Into<Id>>,
    ) -> Self {
        self.identity_ids = Some(identity_ids.into_iter().map(Into::into).collect());
        self
    }

    pub fn with_email_ids(mut self, email_ids: impl IntoIterator<Item = impl Into<Id>>) -> Self {
        self.email_ids = Some(email_ids.into_iter().map(Into::into).collect());
        self
    }

    pub fn with_thread_ids(mut self, thread_ids: impl IntoIterator<Item = impl Into<Id>>) -> Self {
        self.thread_ids = Some(thread_ids.into_iter().map(Into::into).collect());
        self
    }

    pub fn with_undo_status(mut self, undo_status: impl Into<String>) -> Self {
        self.undo_status = Some(undo_status.into());
        self
    }

    pub fn before(mut self, before: impl Into<UtcDate>) -> Self {
        self.before = Some(before.into());
        self
    }

    pub fn after(mut self, after: impl Into<UtcDate>) -> Self {
        self.after = Some(after.into());
        self
    }

    pub fn time_range(mut self, after: Option<UtcDate>, before: Option<UtcDate>) -> Self {
        self.after = after;
        self.before = before;
        self
    }
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

impl SearchSnippet {
    pub fn new(email_id: impl Into<Id>) -> Self {
        Self {
            email_id: email_id.into(),
            subject: None,
            preview: None,
            extra: BTreeMap::new(),
        }
    }

    pub fn with_subject(mut self, subject: impl Into<String>) -> Self {
        self.subject = Some(subject.into());
        self
    }

    pub fn with_preview(mut self, preview: impl Into<String>) -> Self {
        self.preview = Some(preview.into());
        self
    }
}

/// `SearchSnippet/get` arguments (RFC 8621 §5.1).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchSnippetGetRequest {
    pub account_id: Id,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub filter: Option<EmailQueryFilter>,
    pub email_ids: Vec<Id>,
}

impl SearchSnippetGetRequest {
    pub fn new(
        account_id: impl Into<Id>,
        email_ids: impl IntoIterator<Item = impl Into<Id>>,
    ) -> Self {
        Self {
            account_id: account_id.into(),
            filter: None,
            email_ids: email_ids.into_iter().map(Into::into).collect(),
        }
    }

    pub fn with_filter(mut self, filter: EmailQueryFilter) -> Self {
        self.filter = Some(filter);
        self
    }
}

/// `SearchSnippet/get` response (RFC 8621 §5.1).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct SearchSnippetGetResponse {
    pub account_id: Id,
    #[serde(default)]
    pub list: Vec<SearchSnippet>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub not_found: Option<Vec<Id>>,
}

impl SearchSnippetGetResponse {
    pub fn new(account_id: impl Into<Id>, list: impl IntoIterator<Item = SearchSnippet>) -> Self {
        Self {
            account_id: account_id.into(),
            list: list.into_iter().collect(),
            not_found: None,
        }
    }

    pub fn with_not_found(mut self, not_found: impl IntoIterator<Item = impl Into<Id>>) -> Self {
        self.not_found = Some(not_found.into_iter().map(Into::into).collect());
        self
    }
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

impl EmailSubmissionSetRequest {
    pub fn new(set: crate::methods::SetRequest<EmailSubmission>) -> Self {
        Self {
            set,
            on_success_update_email: None,
            on_success_destroy_email: None,
        }
    }

    pub fn with_on_success_update_email(
        mut self,
        on_success_update_email: BTreeMap<String, Value>,
    ) -> Self {
        self.on_success_update_email = Some(on_success_update_email);
        self
    }

    pub fn with_on_success_destroy_email(
        mut self,
        on_success_destroy_email: impl IntoIterator<Item = impl Into<String>>,
    ) -> Self {
        self.on_success_destroy_email = Some(
            on_success_destroy_email
                .into_iter()
                .map(Into::into)
                .collect(),
        );
        self
    }
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

/// The `SetError` types RFC 8621 §4.6 adds for `Email/set`.
pub mod email_set_error {
    pub const BLOB_NOT_FOUND: &str = "blobNotFound";
    pub const TOO_MANY_KEYWORDS: &str = "tooManyKeywords";
    pub const TOO_MANY_MAILBOXES: &str = "tooManyMailboxes";
}

/// The `SetError` types RFC 8621 §7.5 adds for `EmailSubmission/set`.
pub mod email_submission_set_error {
    pub const CANNOT_UNSEND: &str = "cannotUnsend";
    pub const TOO_MANY_RECIPIENTS: &str = "tooManyRecipients";
    pub const NO_RECIPIENTS: &str = "noRecipients";
    pub const INVALID_RECIPIENTS: &str = "invalidRecipients";
    pub const FORBIDDEN_MAIL_FROM: &str = "forbiddenMailFrom";
    pub const FORBIDDEN_FROM: &str = "forbiddenFrom";
    pub const FORBIDDEN_TO_SEND: &str = "forbiddenToSend";
}

/// Standard undoStatus values (RFC 8621 §7).
pub mod undo_status {
    pub const PENDING: &str = "pending";
    pub const FINAL: &str = "final";
    pub const CANCELED: &str = "canceled";
}

/// Mail capability properties (RFC 8621 §1.3).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct MailCapability {
    #[serde(default)]
    pub max_size_attachments_per_email: u64,
    #[serde(default)]
    pub max_size_email_in_bytes: u64,
    #[serde(default)]
    pub max_size_body_value_bytes: u64,
    #[serde(default)]
    pub max_number_of_attachments_per_email: u64,
    #[serde(default)]
    pub max_number_of_recipients_per_email: u64,
    #[serde(default)]
    pub may_create_top_level_mailbox: bool,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl MailCapability {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_max_size_attachments_per_email(mut self, max: u64) -> Self {
        self.max_size_attachments_per_email = max;
        self
    }

    pub fn with_max_size_email_in_bytes(mut self, max: u64) -> Self {
        self.max_size_email_in_bytes = max;
        self
    }

    pub fn with_max_size_body_value_bytes(mut self, max: u64) -> Self {
        self.max_size_body_value_bytes = max;
        self
    }

    pub fn with_max_number_of_attachments_per_email(mut self, max: u64) -> Self {
        self.max_number_of_attachments_per_email = max;
        self
    }

    pub fn with_max_number_of_recipients_per_email(mut self, max: u64) -> Self {
        self.max_number_of_recipients_per_email = max;
        self
    }

    pub fn may_create_top_level_mailbox(mut self, may: bool) -> Self {
        self.may_create_top_level_mailbox = may;
        self
    }
}

/// Submission capability properties (RFC 8621 §1.4).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct SubmissionCapability {
    #[serde(default)]
    pub max_delayed_send: u64,
    #[serde(default)]
    pub submission_extensions: BTreeMap<String, Vec<String>>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl SubmissionCapability {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_max_delayed_send(mut self, max: u64) -> Self {
        self.max_delayed_send = max;
        self
    }

    pub fn with_submission_extensions(mut self, extensions: BTreeMap<String, Vec<String>>) -> Self {
        self.submission_extensions = extensions;
        self
    }
}

/// A Message Disposition Notification (RFC 9007 §2).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MDN {
    pub for_email_id: Id,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subject: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text_body: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub include_original_message: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reporting_ua: Option<String>,
    pub disposition: MDNDisposition,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mdn_gateway: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub original_recipient: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub final_recipient: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub original_message_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extension_fields: Option<BTreeMap<String, String>>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl MDN {
    pub fn new(for_email_id: impl Into<Id>, disposition: MDNDisposition) -> Self {
        Self {
            for_email_id: for_email_id.into(),
            subject: None,
            text_body: None,
            include_original_message: None,
            reporting_ua: None,
            disposition,
            mdn_gateway: None,
            original_recipient: None,
            final_recipient: None,
            original_message_id: None,
            error: None,
            extension_fields: None,
            extra: BTreeMap::new(),
        }
    }

    pub fn with_subject(mut self, subject: impl Into<String>) -> Self {
        self.subject = Some(subject.into());
        self
    }

    pub fn with_text_body(mut self, text_body: impl Into<String>) -> Self {
        self.text_body = Some(text_body.into());
        self
    }

    pub fn with_include_original_message(mut self, include_original_message: bool) -> Self {
        self.include_original_message = Some(include_original_message);
        self
    }

    pub fn with_reporting_ua(mut self, reporting_ua: impl Into<String>) -> Self {
        self.reporting_ua = Some(reporting_ua.into());
        self
    }

    pub fn with_final_recipient(mut self, final_recipient: impl Into<String>) -> Self {
        self.final_recipient = Some(final_recipient.into());
        self
    }

    pub fn with_original_message_id(mut self, original_message_id: impl Into<String>) -> Self {
        self.original_message_id = Some(original_message_id.into());
        self
    }
}

/// The disposition information of an MDN (RFC 9007 §2.1).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MDNDisposition {
    pub action_mode: String,
    pub sending_mode: String,
    #[serde(rename = "type")]
    pub disposition_type: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub modifiers: Option<Vec<String>>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl MDNDisposition {
    pub fn new(
        action_mode: impl Into<String>,
        sending_mode: impl Into<String>,
        disposition_type: impl Into<String>,
    ) -> Self {
        Self {
            action_mode: action_mode.into(),
            sending_mode: sending_mode.into(),
            disposition_type: disposition_type.into(),
            error: None,
            modifiers: None,
            extra: BTreeMap::new(),
        }
    }
}

/// `MDN/send` arguments (RFC 9007 §3.1).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MDNSendRequest {
    pub account_id: Id,
    pub send: BTreeMap<String, MDN>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub on_success_update_email: Option<BTreeMap<Id, Value>>,
}

impl MDNSendRequest {
    pub fn new(account_id: impl Into<Id>, send: BTreeMap<String, MDN>) -> Self {
        Self {
            account_id: account_id.into(),
            send,
            on_success_update_email: None,
        }
    }

    pub fn with_on_success_update_email(
        mut self,
        on_success_update_email: BTreeMap<Id, Value>,
    ) -> Self {
        self.on_success_update_email = Some(on_success_update_email);
        self
    }
}

/// `MDN/send` response (RFC 9007 §3.1).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MDNSendResponse {
    pub account_id: Id,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sent: Option<BTreeMap<String, MDN>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub not_sent: Option<BTreeMap<String, crate::error::SetError>>,
}

impl MDNSendResponse {
    pub fn new(account_id: impl Into<Id>) -> Self {
        Self {
            account_id: account_id.into(),
            sent: None,
            not_sent: None,
        }
    }

    pub fn with_sent(mut self, sent: BTreeMap<String, MDN>) -> Self {
        self.sent = Some(sent);
        self
    }

    pub fn with_not_sent(mut self, not_sent: BTreeMap<String, crate::error::SetError>) -> Self {
        self.not_sent = Some(not_sent);
        self
    }
}

/// Standard MDN action mode values (RFC 9007 §2.1, RFC 8098).
pub mod mdn_action_mode {
    pub const MANUAL_ACTION: &str = "manual-action";
    pub const AUTOMATIC_ACTION: &str = "automatic-action";
}

/// Standard MDN sending mode values (RFC 9007 §2.1, RFC 8098).
pub mod mdn_sending_mode {
    pub const MDN_SENT_MANUALLY: &str = "mdn-sent-manually";
    pub const MDN_SENT_AUTOMATICALLY: &str = "mdn-sent-automatically";
}

/// Standard MDN disposition type values (RFC 9007 §2.1, RFC 8098).
pub mod mdn_disposition_type {
    pub const DISPLAYED: &str = "displayed";
    pub const DELETED: &str = "deleted";
    pub const DISPATCHED: &str = "dispatched";
    pub const PROCESSED: &str = "processed";
}

/// The `SetError` types RFC 9007 §3.2 adds for `MDN/send`.
pub mod mdn_set_error {
    pub const MDN_ALREADY_SENT: &str = "mdnAlreadySent";
    pub const FORBIDDEN_FROM: &str = "forbiddenFrom";
}

/// MDN capability properties (RFC 9007 §1.3).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct MDNCapability {
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl MDNCapability {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_extra(mut self, extra: Value) -> Self {
        if let Value::Object(map) = extra {
            self.extra.extend(map);
        }
        self
    }
}

/// S/MIME Signature Verification capability (RFC 9219 §3).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct SmimeVerifyCapability {
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl SmimeVerifyCapability {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_extra(mut self, extra: Value) -> Self {
        if let Value::Object(map) = extra {
            self.extra.extend(map);
        }
        self
    }
}

/// Standard S/MIME signature verification status values (RFC 9219 §4.1).
pub mod smime_status {
    pub const UNKNOWN: &str = "unknown";
    pub const SIGNED: &str = "signed";
    pub const SIGNED_VERIFIED: &str = "signed/verified";
    pub const SIGNED_FAILED: &str = "signed/failed";
}
