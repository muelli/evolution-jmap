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
#[non_exhaustive]
pub struct Mailbox {
    /// The server-assigned id. Absent on a `Mailbox/set create` argument.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<Id>,
    /// The user-visible name of the mailbox.
    #[serde(default)]
    pub name: String,
    /// The id of the parent mailbox, or `None` at the top level.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_id: Option<Id>,
    /// The mailbox's role, or `None` if it has none (see `role`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    /// A number the client may use to order mailboxes for display.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sort_order: Option<u32>,
    /// The number of emails in this mailbox.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total_emails: Option<u64>,
    /// The number of emails in this mailbox that are not marked `$seen`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unread_emails: Option<u64>,
    /// The number of threads with at least one email in this mailbox.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total_threads: Option<u64>,
    /// The number of threads with at least one unread email in this mailbox.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unread_threads: Option<u64>,
    /// True if the user has subscribed to this mailbox.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub is_subscribed: Option<bool>,
    /// Permissions granted to specific principals on this mailbox (draft-ietf-jmap-mail-sharing §2).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub share_with: Option<BTreeMap<Id, MailboxRights>>,
    /// Server-computed permissions on this mailbox (RFC 8621 §2).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub my_rights: Option<MailboxRights>,
    /// Extra properties the server sent that this struct does not name.
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl Mailbox {
    /// Create a mailbox with the given name and no other properties set.
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            ..Self::default()
        }
    }

    /// Set the server-assigned id.
    pub fn with_id(mut self, id: impl Into<Id>) -> Self {
        self.id = Some(id.into());
        self
    }

    /// Set the id of the parent mailbox.
    pub fn with_parent_id(mut self, parent_id: impl Into<Id>) -> Self {
        self.parent_id = Some(parent_id.into());
        self
    }

    /// Set the mailbox's role.
    pub fn with_role(mut self, role: impl Into<String>) -> Self {
        self.role = Some(role.into());
        self
    }

    /// Set the display sort order.
    pub fn with_sort_order(mut self, sort_order: u32) -> Self {
        self.sort_order = Some(sort_order);
        self
    }

    /// Set the number of emails in this mailbox.
    pub fn with_total_emails(mut self, total_emails: u64) -> Self {
        self.total_emails = Some(total_emails);
        self
    }

    /// Set the number of unread emails in this mailbox.
    pub fn with_unread_emails(mut self, unread_emails: u64) -> Self {
        self.unread_emails = Some(unread_emails);
        self
    }

    /// Set the number of threads with at least one email in this mailbox.
    pub fn with_total_threads(mut self, total_threads: u64) -> Self {
        self.total_threads = Some(total_threads);
        self
    }

    /// Set the number of threads with at least one unread email in this mailbox.
    pub fn with_unread_threads(mut self, unread_threads: u64) -> Self {
        self.unread_threads = Some(unread_threads);
        self
    }

    /// Set whether the user has subscribed to this mailbox.
    pub fn is_subscribed(mut self, subscribed: bool) -> Self {
        self.is_subscribed = Some(subscribed);
        self
    }

    /// Set the server-computed permissions on this mailbox.
    pub fn with_my_rights(mut self, my_rights: MailboxRights) -> Self {
        self.my_rights = Some(my_rights);
        self
    }

    /// Set the permissions granted to specific principals, keyed by principal id.
    pub fn with_share_with(mut self, share_with: BTreeMap<Id, MailboxRights>) -> Self {
        self.share_with = Some(share_with);
        self
    }

    /// Grant the given principal these rights on this mailbox.
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
#[non_exhaustive]
pub struct MailShareCapability {
    /// Extra properties the server sent that this struct does not name.
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl MailShareCapability {
    /// Create an empty mail sharing capability.
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the extra properties the server sent beyond the named fields.
    pub fn with_extra(mut self, extra: BTreeMap<String, Value>) -> Self {
        self.extra = extra;
        self
    }
}

/// `Mailbox.myRights`, RFC 8621 §2.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct MailboxRights {
    /// True if the account may see the emails in this mailbox.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub may_read_items: Option<bool>,
    /// True if the account may add emails to this mailbox.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub may_add_items: Option<bool>,
    /// True if the account may remove emails from this mailbox.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub may_remove_items: Option<bool>,
    /// True if the account may mark emails in this mailbox as seen or unseen.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub may_set_seen: Option<bool>,
    /// True if the account may set other keywords on emails in this mailbox.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub may_set_keywords: Option<bool>,
    /// True if the account may create a child mailbox under this one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub may_create_child: Option<bool>,
    /// True if the account may rename this mailbox.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub may_rename: Option<bool>,
    /// True if the account may delete this mailbox.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub may_delete: Option<bool>,
    /// True if the account may submit emails filed in this mailbox for sending.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub may_submit: Option<bool>,
    /// Whether the user may administer and modify the shareWith property (draft-ietf-jmap-mail-sharing §2).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub may_share: Option<bool>,
    /// Extra properties the server sent that this struct does not name.
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl MailboxRights {
    /// Create a `MailboxRights` with every right granted.
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

    /// Create a `MailboxRights` with only `mayReadItems` granted.
    pub fn read_only() -> Self {
        Self {
            may_read_items: Some(true),
            ..Self::default()
        }
    }

    /// Create a `MailboxRights` with no rights granted.
    pub fn new() -> Self {
        Self::default()
    }

    /// True if the account may administer and modify the shareWith property.
    pub fn may_share(&self) -> bool {
        self.may_share.unwrap_or(false)
    }

    /// Set whether the account may see the emails in this mailbox.
    pub fn with_may_read_items(mut self, may: bool) -> Self {
        self.may_read_items = Some(may);
        self
    }

    /// Set whether the account may add emails to this mailbox.
    pub fn with_may_add_items(mut self, may: bool) -> Self {
        self.may_add_items = Some(may);
        self
    }

    /// Set whether the account may remove emails from this mailbox.
    pub fn with_may_remove_items(mut self, may: bool) -> Self {
        self.may_remove_items = Some(may);
        self
    }

    /// Set whether the account may mark emails in this mailbox as seen or unseen.
    pub fn with_may_set_seen(mut self, may: bool) -> Self {
        self.may_set_seen = Some(may);
        self
    }

    /// Set whether the account may set other keywords on emails in this mailbox.
    pub fn with_may_set_keywords(mut self, may: bool) -> Self {
        self.may_set_keywords = Some(may);
        self
    }

    /// Set whether the account may create a child mailbox under this one.
    pub fn with_may_create_child(mut self, may: bool) -> Self {
        self.may_create_child = Some(may);
        self
    }

    /// Set whether the account may rename this mailbox.
    pub fn with_may_rename(mut self, may: bool) -> Self {
        self.may_rename = Some(may);
        self
    }

    /// Set whether the account may delete this mailbox.
    pub fn with_may_delete(mut self, may: bool) -> Self {
        self.may_delete = Some(may);
        self
    }

    /// Set whether the account may submit emails filed in this mailbox for sending.
    pub fn with_may_submit(mut self, may: bool) -> Self {
        self.may_submit = Some(may);
        self
    }

    /// Set whether the account may administer and modify the shareWith property.
    pub fn with_may_share(mut self, may_share: bool) -> Self {
        self.may_share = Some(may_share);
        self
    }
}

/// A thread of emails (RFC 8621 §3).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct Thread {
    /// The server-assigned id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<Id>,
    /// The ids of the emails in this thread, sorted by date.
    #[serde(default)]
    pub email_ids: Vec<Id>,
    /// Extra properties the server sent that this struct does not name.
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl Thread {
    /// Create a thread with the given id and emails.
    pub fn new(id: impl Into<Id>, email_ids: impl IntoIterator<Item = impl Into<Id>>) -> Self {
        Self {
            id: Some(id.into()),
            email_ids: email_ids.into_iter().map(Into::into).collect(),
            extra: BTreeMap::new(),
        }
    }

    /// Set the server-assigned id.
    pub fn with_id(mut self, id: impl Into<Id>) -> Self {
        self.id = Some(id.into());
        self
    }

    /// Set the ids of the emails in this thread.
    pub fn with_email_ids(mut self, email_ids: impl IntoIterator<Item = impl Into<Id>>) -> Self {
        self.email_ids = email_ids.into_iter().map(Into::into).collect();
        self
    }

    /// Add one email id to this thread.
    pub fn with_email_id(mut self, email_id: impl Into<Id>) -> Self {
        self.email_ids.push(email_id.into());
        self
    }

    /// Set the extra properties the server sent beyond the named fields.
    pub fn with_extra(mut self, extra: BTreeMap<String, Value>) -> Self {
        self.extra = extra;
        self
    }
}

/// Vacation response auto-responder settings (RFC 8621 §8).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct VacationResponse {
    /// The server-assigned id. There is only ever one, a singleton object.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<Id>,
    /// True if the vacation responder is active.
    #[serde(default)]
    pub is_enabled: bool,
    /// The responder sends replies only at or after this date, if set.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from_date: Option<UtcDate>,
    /// The responder sends replies only before this date, if set.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to_date: Option<UtcDate>,
    /// The subject line of the auto-reply, or `None` to use a server default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subject: Option<String>,
    /// The plain text body of the auto-reply.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text_body: Option<String>,
    /// The HTML body of the auto-reply.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub html_body: Option<String>,
    /// Extra properties the server sent that this struct does not name.
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl VacationResponse {
    /// Create a vacation response with no dates or body text set.
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

    /// Set the server-assigned id.
    pub fn with_id(mut self, id: impl Into<Id>) -> Self {
        self.id = Some(id.into());
        self
    }

    /// Set whether the vacation responder is active.
    pub fn with_is_enabled(mut self, is_enabled: bool) -> Self {
        self.is_enabled = is_enabled;
        self
    }

    /// Set the date the responder starts sending replies.
    pub fn with_from_date(mut self, from_date: impl Into<UtcDate>) -> Self {
        self.from_date = Some(from_date.into());
        self
    }

    /// Set the date the responder stops sending replies.
    pub fn with_to_date(mut self, to_date: impl Into<UtcDate>) -> Self {
        self.to_date = Some(to_date.into());
        self
    }

    /// Set the subject line of the auto-reply.
    pub fn with_subject(mut self, subject: impl Into<String>) -> Self {
        self.subject = Some(subject.into());
        self
    }

    /// Set the plain text body of the auto-reply.
    pub fn with_text_body(mut self, text_body: impl Into<String>) -> Self {
        self.text_body = Some(text_body.into());
        self
    }

    /// Set the HTML body of the auto-reply.
    pub fn with_html_body(mut self, html_body: impl Into<String>) -> Self {
        self.html_body = Some(html_body.into());
        self
    }

    /// Set the extra properties the server sent beyond the named fields.
    pub fn with_extra(mut self, extra: BTreeMap<String, Value>) -> Self {
        self.extra = extra;
        self
    }
}

/// Vacation response capability properties (RFC 8621 §1.5).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct VacationResponseCapability {
    /// Extra properties the server sent that this struct does not name.
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl VacationResponseCapability {
    /// Create an empty vacation response capability.
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the extra properties the server sent beyond the named fields.
    pub fn with_extra(mut self, extra: BTreeMap<String, Value>) -> Self {
        self.extra = extra;
        self
    }
}

/// The `SetError` types added for `Mailbox/set` destructions (RFC 8621 §2.5).
pub mod mailbox_set_error {
    /// The set error type returned when destroying a mailbox that has child mailboxes.
    pub const HAS_CHILD: &str = "mailboxHasChild";
    /// The set error type returned when destroying a mailbox that still has emails in it.
    pub const HAS_EMAIL: &str = "mailboxHasEmail";
}

/// Well-known mailbox roles (RFC 8457 registry, referenced by RFC 8621).
pub mod role {
    /// A virtual mailbox holding every email in the account.
    pub const ALL: &str = "all";
    /// Messages the user has moved out of their inbox for long-term storage.
    pub const ARCHIVE: &str = "archive";
    /// Messages the user is composing but has not yet sent.
    pub const DRAFTS: &str = "drafts";
    /// Messages the user has flagged for attention.
    pub const FLAGGED: &str = "flagged";
    /// Messages the user or server has marked important.
    pub const IMPORTANT: &str = "important";
    /// The mailbox new messages arrive in by default.
    pub const INBOX: &str = "inbox";
    /// Messages identified as spam.
    pub const JUNK: &str = "junk";
    /// Held for later sending (RFC 9979 §8.2).
    pub const SCHEDULED: &str = "scheduled";
    /// Messages the user has sent.
    pub const SENT: &str = "sent";
    /// Where snoozed messages wait (RFC 9979 §8.1: the role is standard,
    /// the snoozing mechanism is not; see [`super::SnoozeDetails`]).
    pub const SNOOZED: &str = "snoozed";
    /// Marks a mailbox the user has subscribed to via IMAP (RFC 6154).
    pub const SUBSCRIBED: &str = "subscribed";
    /// Messages the user has deleted.
    pub const TRASH: &str = "trash";
}

/// When a snoozed message wakes and where it goes (`Email.snoozed`, per
/// draft-ietf-extra-email-snooze §4 as implemented by Cyrus).
///
/// Moving a message to [`role::SNOOZED`] moves it to `move_to_mailbox_id`
/// at `until` and applies `set_keywords`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct SnoozeDetails {
    /// When the snoozed message wakes.
    pub until: UtcDate,
    /// The mailbox the message moves to when it wakes, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub move_to_mailbox_id: Option<Id>,
    /// Keywords to set on the message when it wakes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub set_keywords: Option<BTreeMap<String, bool>>,
    /// Extra properties the server sent that this struct does not name.
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl SnoozeDetails {
    /// Create snooze details that wake at the given time with no mailbox move or keyword change.
    pub fn new(until: impl Into<UtcDate>) -> Self {
        Self {
            until: until.into(),
            move_to_mailbox_id: None,
            set_keywords: None,
            extra: BTreeMap::new(),
        }
    }

    /// Set the mailbox the message moves to when it wakes.
    pub fn with_move_to_mailbox_id(mut self, mailbox_id: impl Into<Id>) -> Self {
        self.move_to_mailbox_id = Some(mailbox_id.into());
        self
    }

    /// Set the keywords to apply to the message when it wakes.
    pub fn with_set_keywords(mut self, keywords: BTreeMap<String, bool>) -> Self {
        self.set_keywords = Some(keywords);
        self
    }
}

/// An email message, RFC 8621 §4. All properties are optional because
/// `Email/get` returns only what was asked for and `Email/set` sends only
/// what the client provides.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct Email {
    /// The server-assigned id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<Id>,
    /// The id of the blob holding the raw RFC 5322 message.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blob_id: Option<Id>,
    /// The id of the thread this email belongs to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread_id: Option<Id>,
    /// The mailboxes this email is filed in, keyed by mailbox id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mailbox_ids: Option<BTreeMap<Id, bool>>,
    /// The keywords set on this email, such as `$seen` or `$flagged`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub keywords: Option<BTreeMap<String, bool>>,
    /// Vendor extension, absent on the wire unless the account offers
    /// [`crate::session::CAPABILITY_CYRUS_MAIL`]; see [`SnoozeDetails`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub snoozed: Option<SnoozeDetails>,
    /// The size of the raw message in bytes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<u64>,
    /// When the server received this email.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub received_at: Option<UtcDate>,
    /// The `Message-ID` header's content, without angle brackets.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message_id: Option<Vec<String>>,
    /// The `In-Reply-To` header's content, without angle brackets.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub in_reply_to: Option<Vec<String>>,
    /// The `References` header's content, without angle brackets.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub references: Option<Vec<String>>,
    /// The `Sender` header's addresses.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sender: Option<Vec<EmailAddress>>,
    /// The `From` header's addresses.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from: Option<Vec<EmailAddress>>,
    /// The `To` header's addresses.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to: Option<Vec<EmailAddress>>,
    /// The `Cc` header's addresses.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cc: Option<Vec<EmailAddress>>,
    /// The `Bcc` header's addresses.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bcc: Option<Vec<EmailAddress>>,
    /// The `Reply-To` header's addresses.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reply_to: Option<Vec<EmailAddress>>,
    /// The `Subject` header's content, decoded.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subject: Option<String>,
    /// Date header; unlike `receivedAt` this may carry a zone offset.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sent_at: Option<String>,
    /// True if the server considers this email to have an attachment.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub has_attachment: Option<bool>,
    /// A short, server-generated plain text preview of the email body.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preview: Option<String>,
    /// The MIME structure of the message, as a tree of body parts.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body_structure: Option<EmailBodyPart>,
    /// The decoded text content of body parts, keyed by part id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body_values: Option<BTreeMap<String, EmailBodyValue>>,
    /// The plain text body parts to show the user by default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text_body: Option<Vec<EmailBodyPart>>,
    /// The HTML body parts to show the user by default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub html_body: Option<Vec<EmailBodyPart>>,
    /// Every header on the message, in wire order.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub headers: Option<Vec<EmailHeader>>,
    /// The body parts the server considers attachments.
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
    /// Extra properties the server sent that this struct does not name.
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl Email {
    /// Create an email with every property unset.
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the server-assigned id.
    pub fn with_id(mut self, id: impl Into<Id>) -> Self {
        self.id = Some(id.into());
        self
    }

    /// Set the id of the blob holding the raw message.
    pub fn with_blob_id(mut self, blob_id: impl Into<Id>) -> Self {
        self.blob_id = Some(blob_id.into());
        self
    }

    /// Set the id of the thread this email belongs to.
    pub fn with_thread_id(mut self, thread_id: impl Into<Id>) -> Self {
        self.thread_id = Some(thread_id.into());
        self
    }

    /// File this email in the given mailbox.
    pub fn in_mailbox(mut self, mailbox_id: impl Into<Id>) -> Self {
        self.mailbox_ids
            .get_or_insert_with(BTreeMap::new)
            .insert(mailbox_id.into(), true);
        self
    }

    /// Set the mailboxes this email is filed in.
    pub fn with_mailbox_ids(mut self, mailbox_ids: BTreeMap<Id, bool>) -> Self {
        self.mailbox_ids = Some(mailbox_ids);
        self
    }

    /// Set one keyword on this email.
    pub fn with_keyword(mut self, keyword: impl Into<String>) -> Self {
        self.keywords
            .get_or_insert_with(BTreeMap::new)
            .insert(keyword.into(), true);
        self
    }

    /// Set the keywords on this email.
    pub fn with_keywords(mut self, keywords: BTreeMap<String, bool>) -> Self {
        self.keywords = Some(keywords);
        self
    }

    /// Set the size of the raw message in bytes.
    pub fn with_size(mut self, size: u64) -> Self {
        self.size = Some(size);
        self
    }

    /// Set when the server received this email.
    pub fn with_received_at(mut self, received_at: impl Into<UtcDate>) -> Self {
        self.received_at = Some(received_at.into());
        self
    }

    /// Set the `Sender` header's addresses.
    pub fn with_sender(mut self, sender: impl IntoIterator<Item = EmailAddress>) -> Self {
        self.sender = Some(sender.into_iter().collect());
        self
    }

    /// Set the `Message-ID` header's content.
    pub fn with_message_id(
        mut self,
        message_id: impl IntoIterator<Item = impl Into<String>>,
    ) -> Self {
        self.message_id = Some(message_id.into_iter().map(Into::into).collect());
        self
    }

    /// Set the `In-Reply-To` header's content.
    pub fn with_in_reply_to(
        mut self,
        in_reply_to: impl IntoIterator<Item = impl Into<String>>,
    ) -> Self {
        self.in_reply_to = Some(in_reply_to.into_iter().map(Into::into).collect());
        self
    }

    /// Set the `References` header's content.
    pub fn with_references(
        mut self,
        references: impl IntoIterator<Item = impl Into<String>>,
    ) -> Self {
        self.references = Some(references.into_iter().map(Into::into).collect());
        self
    }

    /// Set the snooze details for this email.
    pub fn with_snoozed(mut self, snoozed: SnoozeDetails) -> Self {
        self.snoozed = Some(snoozed);
        self
    }

    /// Set the `From` header's addresses.
    pub fn with_from(mut self, from: impl IntoIterator<Item = EmailAddress>) -> Self {
        self.from = Some(from.into_iter().collect());
        self
    }

    /// Set the `To` header's addresses.
    pub fn with_to(mut self, to: impl IntoIterator<Item = EmailAddress>) -> Self {
        self.to = Some(to.into_iter().collect());
        self
    }

    /// Set the `Cc` header's addresses.
    pub fn with_cc(mut self, cc: impl IntoIterator<Item = EmailAddress>) -> Self {
        self.cc = Some(cc.into_iter().collect());
        self
    }

    /// Set the `Bcc` header's addresses.
    pub fn with_bcc(mut self, bcc: impl IntoIterator<Item = EmailAddress>) -> Self {
        self.bcc = Some(bcc.into_iter().collect());
        self
    }

    /// Set the `Reply-To` header's addresses.
    pub fn with_reply_to(mut self, reply_to: impl IntoIterator<Item = EmailAddress>) -> Self {
        self.reply_to = Some(reply_to.into_iter().collect());
        self
    }

    /// Set the `Subject` header's content.
    pub fn with_subject(mut self, subject: impl Into<String>) -> Self {
        self.subject = Some(subject.into());
        self
    }

    /// Set the `Date` header's content.
    pub fn with_sent_at(mut self, sent_at: impl Into<String>) -> Self {
        self.sent_at = Some(sent_at.into());
        self
    }

    /// Set the short plain text preview of the email body.
    pub fn with_preview(mut self, preview: impl Into<String>) -> Self {
        self.preview = Some(preview.into());
        self
    }

    /// Set whether the server considers this email to have an attachment.
    pub fn has_attachment(mut self, has: bool) -> Self {
        self.has_attachment = Some(has);
        self
    }

    /// Add one header to this email.
    pub fn with_header(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.headers
            .get_or_insert_with(Vec::new)
            .push(EmailHeader::new(name, value));
        self
    }

    /// Set every header on the message.
    pub fn with_headers(mut self, headers: impl IntoIterator<Item = EmailHeader>) -> Self {
        self.headers = Some(headers.into_iter().collect());
        self
    }

    /// Set the MIME structure of the message.
    pub fn with_body_structure(mut self, body_structure: EmailBodyPart) -> Self {
        self.body_structure = Some(body_structure);
        self
    }

    /// Set the decoded text content of body parts, keyed by part id.
    pub fn with_body_values(mut self, body_values: BTreeMap<String, EmailBodyValue>) -> Self {
        self.body_values = Some(body_values);
        self
    }

    /// Set the plain text body parts to show the user by default.
    pub fn with_text_body(mut self, text_body: impl IntoIterator<Item = EmailBodyPart>) -> Self {
        self.text_body = Some(text_body.into_iter().collect());
        self
    }

    /// Set the HTML body parts to show the user by default.
    pub fn with_html_body(mut self, html_body: impl IntoIterator<Item = EmailBodyPart>) -> Self {
        self.html_body = Some(html_body.into_iter().collect());
        self
    }

    /// Set the body parts the server considers attachments.
    pub fn with_attachments(
        mut self,
        attachments: impl IntoIterator<Item = EmailBodyPart>,
    ) -> Self {
        self.attachments = Some(attachments.into_iter().collect());
        self
    }

    /// Set the S/MIME signature verification status.
    pub fn with_smime_status(mut self, status: impl Into<String>) -> Self {
        self.smime_status = Some(status.into());
        self
    }

    /// Set the S/MIME verification errors and warnings.
    pub fn with_smime_errors(
        mut self,
        errors: impl IntoIterator<Item = impl Into<String>>,
    ) -> Self {
        self.smime_errors = Some(errors.into_iter().map(Into::into).collect());
        self
    }

    /// Set when the S/MIME signature was verified.
    pub fn with_smime_verified_at(mut self, verified_at: impl Into<UtcDate>) -> Self {
        self.smime_verified_at = Some(verified_at.into());
        self
    }
}

/// One message handed to `Email/import` (RFC 8621 §4.8).
/// Fields are optional on deserialization to isolate per-message invalidProperties errors.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct EmailImport {
    /// The id of the blob holding the raw message to import.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blob_id: Option<Id>,
    /// The mailboxes to file the imported message in.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mailbox_ids: Option<BTreeMap<Id, bool>>,
    /// The keywords to set on the imported message.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub keywords: Option<BTreeMap<String, bool>>,
    /// The sort date for the message.
    /// Defaults to the newest `Received` header date or import time (RFC 8621 §4.8).
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

    /// Add one keyword to the imported message.
    pub fn keyword(mut self, keyword: impl Into<String>) -> Self {
        self.keywords
            .get_or_insert_with(BTreeMap::new)
            .insert(keyword.into(), true);
        self
    }

    /// Set the sort date for the imported message.
    pub fn received_at(mut self, received_at: impl Into<UtcDate>) -> Self {
        self.received_at = Some(received_at.into());
        self
    }

    /// Set the id of the blob holding the raw message.
    pub fn with_blob_id(mut self, blob_id: impl Into<Id>) -> Self {
        self.blob_id = Some(blob_id.into());
        self
    }

    /// Set the mailboxes to file the imported message in.
    pub fn with_mailbox_ids(mut self, mailbox_ids: BTreeMap<Id, bool>) -> Self {
        self.mailbox_ids = Some(mailbox_ids);
        self
    }
}

/// `Email/import` arguments (RFC 8621 §4.8).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct EmailImportRequest {
    /// The account to import into.
    pub account_id: Id,
    /// Rejects the request unless the account is in this state.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub if_in_state: Option<crate::state::State>,
    /// The messages to import, keyed by a client-chosen creation id.
    pub emails: BTreeMap<String, EmailImport>,
}

impl EmailImportRequest {
    /// Create an import request with no messages yet.
    pub fn new(account_id: impl Into<Id>) -> Self {
        Self {
            account_id: account_id.into(),
            if_in_state: None,
            emails: BTreeMap::new(),
        }
    }

    /// Add one message to import under the given creation id.
    pub fn import(mut self, creation_id: impl Into<String>, email: EmailImport) -> Self {
        self.emails.insert(creation_id.into(), email);
        self
    }

    /// Reject the request unless the account is in this state.
    pub fn if_in_state(mut self, state: impl Into<crate::state::State>) -> Self {
        self.if_in_state = Some(state.into());
        self
    }
}

/// `Email/import` response (RFC 8621 §4.8).
///
/// `created` carries server-chosen properties (`id`, `blobId`, `threadId`, `size`)
/// as an [`Email`] with optional fields.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct EmailImportResponse {
    /// The account the messages were imported into.
    pub account_id: Id,
    /// The state string before this request, if the server tracked one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub old_state: Option<crate::state::State>,
    /// The state `Email/get` will now return.
    pub new_state: crate::state::State,
    /// The imported messages, keyed by their creation id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created: Option<BTreeMap<String, Email>>,
    /// The messages that failed to import, keyed by their creation id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub not_created: Option<BTreeMap<String, crate::error::SetError>>,
}

impl EmailImportResponse {
    /// Create a response with no created or failed messages yet.
    pub fn new(account_id: impl Into<Id>, new_state: impl Into<crate::state::State>) -> Self {
        Self {
            account_id: account_id.into(),
            old_state: None,
            new_state: new_state.into(),
            created: None,
            not_created: None,
        }
    }

    /// Set the state string before this request.
    pub fn with_old_state(mut self, old_state: impl Into<crate::state::State>) -> Self {
        self.old_state = Some(old_state.into());
        self
    }

    /// Set the imported messages, keyed by their creation id.
    pub fn with_created(mut self, created: BTreeMap<String, Email>) -> Self {
        self.created = Some(created);
        self
    }

    /// Set the messages that failed to import, keyed by their creation id.
    pub fn with_not_created(
        mut self,
        not_created: BTreeMap<String, crate::error::SetError>,
    ) -> Self {
        self.not_created = Some(not_created);
        self
    }
}

/// `Email/get` arguments (RFC 8621 §4.2).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct EmailGetRequest {
    /// The account to fetch emails from.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account_id: Option<Id>,
    /// The ids to fetch, or `None` for every object.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ids: Option<Vec<Id>>,
    /// A back-reference that resolves to `ids` from an earlier method's response.
    #[serde(rename = "#ids", default, skip_serializing_if = "Option::is_none")]
    pub ids_ref: Option<crate::request::ResultReference>,
    /// The properties to return for each email, or `None` for every property.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub properties: Option<Vec<String>>,
    /// The properties to include on each returned body part.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body_properties: Option<Vec<String>>,
    /// Fetch the content of `textBody` parts into `bodyValues`.
    #[serde(default, skip_serializing_if = "is_false")]
    pub fetch_text_body_values: bool,
    /// Fetch the content of `htmlBody` parts into `bodyValues`.
    #[serde(
        rename = "fetchHTMLBodyValues",
        default,
        skip_serializing_if = "is_false"
    )]
    pub fetch_html_body_values: bool,
    /// Fetch the content of every body part into `bodyValues`.
    #[serde(default, skip_serializing_if = "is_false")]
    pub fetch_all_body_values: bool,
    /// Truncate each fetched body value to at most this many bytes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_body_value_bytes: Option<u64>,
}

impl EmailGetRequest {
    /// Create a request with no account, ids or properties set.
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the account to fetch emails from.
    pub fn with_account_id(mut self, account_id: impl Into<Id>) -> Self {
        self.account_id = Some(account_id.into());
        self
    }

    /// Set the ids to fetch.
    pub fn with_ids(mut self, ids: impl IntoIterator<Item = impl Into<Id>>) -> Self {
        self.ids = Some(ids.into_iter().map(Into::into).collect());
        self
    }

    /// Set the properties to return for each email.
    pub fn with_properties(
        mut self,
        properties: impl IntoIterator<Item = impl Into<String>>,
    ) -> Self {
        self.properties = Some(properties.into_iter().map(Into::into).collect());
        self
    }

    /// Set the properties to include on each returned body part.
    pub fn with_body_properties(
        mut self,
        body_properties: impl IntoIterator<Item = impl Into<String>>,
    ) -> Self {
        self.body_properties = Some(body_properties.into_iter().map(Into::into).collect());
        self
    }

    /// Fetch the content of `textBody` parts into `bodyValues`.
    pub fn fetch_text_body_values(mut self) -> Self {
        self.fetch_text_body_values = true;
        self
    }

    /// Fetch the content of `htmlBody` parts into `bodyValues`.
    pub fn fetch_html_body_values(mut self) -> Self {
        self.fetch_html_body_values = true;
        self
    }

    /// Fetch the content of every body part into `bodyValues`.
    pub fn fetch_all_body_values(mut self) -> Self {
        self.fetch_all_body_values = true;
        self
    }

    /// Set the maximum size, in bytes, of each fetched body value.
    pub fn with_max_body_value_bytes(mut self, max: u64) -> Self {
        self.max_body_value_bytes = Some(max);
        self
    }
}

/// `Email/parse` arguments (RFC 8621 §4.7).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct EmailParseRequest {
    /// The account the blobs belong to.
    pub account_id: Id,
    /// The ids of the blobs to parse as RFC 5322 messages.
    pub blob_ids: Vec<Id>,
    /// The properties to return for each parsed email, or `None` for every property.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub properties: Option<Vec<String>>,
    /// The properties to include on each returned body part.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body_properties: Option<Vec<String>>,
    /// Fetch the content of `textBody` parts into `bodyValues`.
    #[serde(default, skip_serializing_if = "is_false")]
    pub fetch_text_body_values: bool,
    /// Fetch the content of `htmlBody` parts into `bodyValues`.
    #[serde(
        rename = "fetchHTMLBodyValues",
        default,
        skip_serializing_if = "is_false"
    )]
    pub fetch_html_body_values: bool,
    /// Fetch the content of every body part into `bodyValues`.
    #[serde(default, skip_serializing_if = "is_false")]
    pub fetch_all_body_values: bool,
    /// Truncate each fetched body value to at most this many bytes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_body_value_bytes: Option<u64>,
}

fn is_false(value: &bool) -> bool {
    !*value
}

impl EmailParseRequest {
    /// Create a request to parse the given blobs.
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

    /// Set the properties to return for each parsed email.
    pub fn properties(mut self, properties: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.properties = Some(properties.into_iter().map(Into::into).collect());
        self
    }

    /// Set the properties to include on each returned body part.
    pub fn body_properties(
        mut self,
        body_properties: impl IntoIterator<Item = impl Into<String>>,
    ) -> Self {
        self.body_properties = Some(body_properties.into_iter().map(Into::into).collect());
        self
    }

    /// Fetch the content of `textBody` parts into `bodyValues`.
    pub fn fetch_text_body_values(mut self) -> Self {
        self.fetch_text_body_values = true;
        self
    }

    /// Fetch the content of `htmlBody` parts into `bodyValues`.
    pub fn fetch_html_body_values(mut self) -> Self {
        self.fetch_html_body_values = true;
        self
    }

    /// Fetch the content of every body part into `bodyValues`.
    pub fn fetch_all_body_values(mut self) -> Self {
        self.fetch_all_body_values = true;
        self
    }

    /// Set the maximum size, in bytes, of each fetched body value.
    pub fn max_body_value_bytes(mut self, max_bytes: u64) -> Self {
        self.max_body_value_bytes = Some(max_bytes);
        self
    }
}

/// `Email/parse` response (RFC 8621 §4.7).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct EmailParseResponse {
    /// The account the parsed blobs belong to.
    pub account_id: Id,
    /// The parsed emails, keyed by blob id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parsed: Option<BTreeMap<Id, Email>>,
    /// The blob ids that were not parsable RFC 5322 messages.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub not_parsable: Option<Vec<Id>>,
    /// The blob ids that do not exist.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub not_found: Option<Vec<Id>>,
}

impl EmailParseResponse {
    /// Create a response with nothing parsed, not parsable or not found yet.
    pub fn new(account_id: impl Into<Id>) -> Self {
        Self {
            account_id: account_id.into(),
            parsed: None,
            not_parsable: None,
            not_found: None,
        }
    }

    /// Set the parsed emails, keyed by blob id.
    pub fn with_parsed(mut self, parsed: BTreeMap<Id, Email>) -> Self {
        self.parsed = Some(parsed);
        self
    }

    /// Set the blob ids that were not parsable RFC 5322 messages.
    pub fn with_not_parsable(
        mut self,
        not_parsable: impl IntoIterator<Item = impl Into<Id>>,
    ) -> Self {
        self.not_parsable = Some(not_parsable.into_iter().map(Into::into).collect());
        self
    }

    /// Set the blob ids that do not exist.
    pub fn with_not_found(mut self, not_found: impl IntoIterator<Item = impl Into<Id>>) -> Self {
        self.not_found = Some(not_found.into_iter().map(Into::into).collect());
        self
    }
}

/// The `SetError` type RFC 8621 §4.8 adds for `Email/import`.
/// `invalidEmail` indicates the blob content is not a parsable RFC 5322 message.
pub mod email_import_error {
    /// The set error type returned when a blob is not a parsable RFC 5322 message.
    pub const INVALID_EMAIL: &str = "invalidEmail";
}

/// Well-known keywords (RFC 8621 §4.1.1, which defers the rest of the set to
/// the IMAP keywords registry of RFC 5788).
pub mod keyword {
    /// The message has been read.
    pub const SEEN: &str = "$seen";
    /// The message is a draft.
    pub const DRAFT: &str = "$draft";
    /// The user has flagged the message for attention.
    pub const FLAGGED: &str = "$flagged";
    /// The user has replied to the message.
    pub const ANSWERED: &str = "$answered";
    /// The user has forwarded the message.
    pub const FORWARDED: &str = "$forwarded";
    /// The user has marked the message as spam.
    pub const JUNK: &str = "$junk";
    /// The user has marked the message as not spam.
    pub const NOT_JUNK: &str = "$notjunk";
}

/// A parsed email header (RFC 8621 §4.1.2.1).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[non_exhaustive]
pub struct EmailHeader {
    /// The header field name, such as `Subject`.
    pub name: String,
    /// The raw, undecoded header field value.
    pub value: String,
}

impl EmailHeader {
    /// Create a header with the given name and value.
    pub fn new(name: impl Into<String>, value: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            value: value.into(),
        }
    }

    /// Set the header field name.
    pub fn with_name(mut self, name: impl Into<String>) -> Self {
        self.name = name.into();
        self
    }

    /// Set the raw header field value.
    pub fn with_value(mut self, value: impl Into<String>) -> Self {
        self.value = value.into();
        self
    }
}

/// A name/address pair (RFC 8621 §4.1.2.3). `name` is nullable but always
/// present on the wire.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[non_exhaustive]
pub struct EmailAddress {
    /// The display name, or `None` if the address has none.
    #[serde(default)]
    pub name: Option<String>,
    /// The email address.
    pub email: String,
}

impl EmailAddress {
    /// Create an address with the given display name and email.
    pub fn new(name: Option<&str>, email: &str) -> Self {
        Self {
            name: name.map(str::to_owned),
            email: email.to_owned(),
        }
    }

    /// Create an address with no display name.
    pub fn from_email(email: impl Into<String>) -> Self {
        Self {
            name: None,
            email: email.into(),
        }
    }

    /// Set the display name.
    pub fn with_name(mut self, name: impl Into<String>) -> Self {
        self.name = Some(name.into());
        self
    }

    /// Set the email address.
    pub fn with_email(mut self, email: impl Into<String>) -> Self {
        self.email = email.into();
        self
    }
}

/// An RFC 5322 address group (RFC 8621 §4.1.2.3).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[non_exhaustive]
pub struct EmailAddressGroup {
    /// The group's display name, or `None` if it has none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// The addresses in this group.
    #[serde(default)]
    pub addresses: Vec<EmailAddress>,
}

impl EmailAddressGroup {
    /// Create a group with the given addresses and no name.
    pub fn new(addresses: impl IntoIterator<Item = EmailAddress>) -> Self {
        Self {
            name: None,
            addresses: addresses.into_iter().collect(),
        }
    }

    /// Set the group's display name.
    pub fn with_name(mut self, name: impl Into<String>) -> Self {
        self.name = Some(name.into());
        self
    }

    /// Set the addresses in this group.
    pub fn with_addresses(mut self, addresses: impl IntoIterator<Item = EmailAddress>) -> Self {
        self.addresses = addresses.into_iter().collect();
        self
    }

    /// Add one address to this group.
    pub fn with_address(mut self, address: EmailAddress) -> Self {
        self.addresses.push(address);
        self
    }
}

/// A node in the MIME tree (RFC 8621 §4.1.4).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct EmailBodyPart {
    /// An identifier for the part within the structure, unique within the message.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub part_id: Option<String>,
    /// The id of the blob holding this part's content.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blob_id: Option<Id>,
    /// The size of this part's content, in bytes, before any decoding.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<u64>,
    /// The filename from the `Content-Disposition` or `Content-Type` header, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// The `Content-Type` header's value.
    #[serde(rename = "type", default, skip_serializing_if = "Option::is_none")]
    pub content_type: Option<String>,
    /// The `charset` parameter of the `Content-Type` header, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub charset: Option<String>,
    /// The `Content-Disposition` header's value, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub disposition: Option<String>,
    /// The `Content-Id` header's value, without angle brackets, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cid: Option<String>,
    /// The `Content-Location` header's value, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub location: Option<String>,
    /// The child parts of a multipart body.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sub_parts: Option<Vec<EmailBodyPart>>,
    /// The headers of this body part, in wire order.
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
    /// Extra properties the server sent that this struct does not name.
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl EmailBodyPart {
    /// Create a body part with every property unset.
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the part identifier.
    pub fn with_part_id(mut self, part_id: impl Into<String>) -> Self {
        self.part_id = Some(part_id.into());
        self
    }

    /// Set the id of the blob holding this part's content.
    pub fn with_blob_id(mut self, blob_id: impl Into<Id>) -> Self {
        self.blob_id = Some(blob_id.into());
        self
    }

    /// Set the size of this part's content in bytes.
    pub fn with_size(mut self, size: u64) -> Self {
        self.size = Some(size);
        self
    }

    /// Set the filename.
    pub fn with_name(mut self, name: impl Into<String>) -> Self {
        self.name = Some(name.into());
        self
    }

    /// Set the `Content-Type` header's value.
    pub fn with_content_type(mut self, content_type: impl Into<String>) -> Self {
        self.content_type = Some(content_type.into());
        self
    }

    /// Set the `charset` parameter of the `Content-Type` header.
    pub fn with_charset(mut self, charset: impl Into<String>) -> Self {
        self.charset = Some(charset.into());
        self
    }

    /// Set the `Content-Disposition` header's value.
    pub fn with_disposition(mut self, disposition: impl Into<String>) -> Self {
        self.disposition = Some(disposition.into());
        self
    }

    /// Set the `Content-Id` header's value.
    pub fn with_cid(mut self, cid: impl Into<String>) -> Self {
        self.cid = Some(cid.into());
        self
    }

    /// Set the `Content-Location` header's value.
    pub fn with_location(mut self, location: impl Into<String>) -> Self {
        self.location = Some(location.into());
        self
    }

    /// Set the child parts of a multipart body.
    pub fn with_sub_parts(mut self, sub_parts: impl IntoIterator<Item = EmailBodyPart>) -> Self {
        self.sub_parts = Some(sub_parts.into_iter().collect());
        self
    }

    /// Set the headers of this body part.
    pub fn with_headers(mut self, headers: impl IntoIterator<Item = EmailHeader>) -> Self {
        self.headers = Some(headers.into_iter().collect());
        self
    }

    /// Set the S/MIME signature verification status.
    pub fn with_smime_status(mut self, status: impl Into<String>) -> Self {
        self.smime_status = Some(status.into());
        self
    }

    /// Set the S/MIME verification errors and warnings.
    pub fn with_smime_errors(
        mut self,
        errors: impl IntoIterator<Item = impl Into<String>>,
    ) -> Self {
        self.smime_errors = Some(errors.into_iter().map(Into::into).collect());
        self
    }

    /// Set when the S/MIME signature was verified.
    pub fn with_smime_verified_at(mut self, verified_at: impl Into<UtcDate>) -> Self {
        self.smime_verified_at = Some(verified_at.into());
        self
    }
}

/// Decoded body content addressed by `partId` (RFC 8621 §4.1.4).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct EmailBodyValue {
    /// The decoded text content of the body part.
    pub value: String,
    /// True if the server replaced malformed characters while decoding.
    #[serde(default)]
    pub is_encoding_problem: bool,
    /// True if the content was cut short at `maxBodyValueBytes`.
    #[serde(default)]
    pub is_truncated: bool,
}

impl EmailBodyValue {
    /// Create a body value with the given content, not truncated or malformed.
    pub fn new(value: impl Into<String>) -> Self {
        Self {
            value: value.into(),
            is_encoding_problem: false,
            is_truncated: false,
        }
    }
}

/// `Email/query` filter conditions (RFC 8621 §4.4.1). This type is a leaf
/// condition; `methods::Filter<EmailQueryFilter>` adds AND/OR/NOT nesting
/// (RFC 8620 §5.5) on top of it, which `Email/query`'s request type uses.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct EmailQueryFilter {
    /// Matches emails filed in this mailbox.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub in_mailbox: Option<Id>,
    /// Matches emails not filed in any of these mailboxes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub in_mailbox_other_than: Option<Vec<Id>>,
    /// Matches emails at least this many bytes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_size: Option<u64>,
    /// Matches emails at most this many bytes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_size: Option<u64>,
    /// Matches emails that have this keyword.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub has_keyword: Option<String>,
    /// Matches emails that do not have this keyword.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub not_keyword: Option<String>,
    /// Matches emails where every message in the thread has this keyword.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub all_in_thread_have_keyword: Option<String>,
    /// Matches emails where at least one message in the thread has this keyword.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub some_in_thread_have_keyword: Option<String>,
    /// Matches emails where no message in the thread has this keyword.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub none_in_thread_have_keyword: Option<String>,
    /// Matches emails the server considers to have an attachment.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub has_attachment: Option<bool>,
    /// Matches emails where this text appears in any searchable field.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    /// Matches emails where this text appears in the body.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body: Option<String>,
    /// Matches emails where this text appears in the `From` header.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from: Option<String>,
    /// Matches emails where this text appears in the `To` header.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to: Option<String>,
    /// Matches emails where this text appears in the `Cc` header.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cc: Option<String>,
    /// Matches emails where this text appears in the `Bcc` header.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bcc: Option<String>,
    /// Matches emails where this text appears in the `Subject` header.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subject: Option<String>,
    /// Matches emails with a header whose name, and optionally value, equal these strings.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub header: Option<Vec<String>>,
    /// Matches emails received strictly before this date.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub before: Option<UtcDate>,
    /// Matches emails received at or after this date.
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
    /// Create a filter matching emails filed in this mailbox.
    pub fn in_mailbox(mailbox_id: impl Into<Id>) -> Self {
        Self {
            in_mailbox: Some(mailbox_id.into()),
            ..Self::default()
        }
    }

    /// Set the mailboxes an email must not be filed in to match.
    pub fn in_mailbox_other_than(
        mut self,
        mailboxes: impl IntoIterator<Item = impl Into<Id>>,
    ) -> Self {
        self.in_mailbox_other_than = Some(mailboxes.into_iter().map(Into::into).collect());
        self
    }

    /// Set the minimum size, in bytes, to match.
    pub fn min_size(mut self, min_size: u64) -> Self {
        self.min_size = Some(min_size);
        self
    }

    /// Set the maximum size, in bytes, to match.
    pub fn max_size(mut self, max_size: u64) -> Self {
        self.max_size = Some(max_size);
        self
    }

    /// Set the keyword an email must have to match.
    pub fn has_keyword(mut self, keyword: impl Into<String>) -> Self {
        self.has_keyword = Some(keyword.into());
        self
    }

    /// Set the keyword an email must not have to match.
    pub fn not_keyword(mut self, keyword: impl Into<String>) -> Self {
        self.not_keyword = Some(keyword.into());
        self
    }

    /// Set the keyword every message in the thread must have to match.
    pub fn all_in_thread_have_keyword(mut self, keyword: impl Into<String>) -> Self {
        self.all_in_thread_have_keyword = Some(keyword.into());
        self
    }

    /// Set the keyword at least one message in the thread must have to match.
    pub fn some_in_thread_have_keyword(mut self, keyword: impl Into<String>) -> Self {
        self.some_in_thread_have_keyword = Some(keyword.into());
        self
    }

    /// Set the keyword no message in the thread may have to match.
    pub fn none_in_thread_have_keyword(mut self, keyword: impl Into<String>) -> Self {
        self.none_in_thread_have_keyword = Some(keyword.into());
        self
    }

    /// Set whether a match must have an attachment.
    pub fn has_attachment(mut self, has: bool) -> Self {
        self.has_attachment = Some(has);
        self
    }

    /// Set the text to search for in any searchable field.
    pub fn text(mut self, text: impl Into<String>) -> Self {
        self.text = Some(text.into());
        self
    }

    /// Set the text to search for in the body.
    pub fn body(mut self, body: impl Into<String>) -> Self {
        self.body = Some(body.into());
        self
    }

    /// Set the text to search for in the `From` header.
    pub fn from(mut self, from: impl Into<String>) -> Self {
        self.from = Some(from.into());
        self
    }

    /// Set the text to search for in the `To` header.
    pub fn to(mut self, to: impl Into<String>) -> Self {
        self.to = Some(to.into());
        self
    }

    /// Set the text to search for in the `Cc` header.
    pub fn cc(mut self, cc: impl Into<String>) -> Self {
        self.cc = Some(cc.into());
        self
    }

    /// Set the text to search for in the `Bcc` header.
    pub fn bcc(mut self, bcc: impl Into<String>) -> Self {
        self.bcc = Some(bcc.into());
        self
    }

    /// Set the text to search for in the `Subject` header.
    pub fn subject(mut self, subject: impl Into<String>) -> Self {
        self.subject = Some(subject.into());
        self
    }

    /// Set the header name and value to match.
    pub fn header(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.header = Some(vec![name.into(), value.into()]);
        self
    }

    /// Set the date emails must be received strictly before to match.
    pub fn before(mut self, before: impl Into<UtcDate>) -> Self {
        self.before = Some(before.into());
        self
    }

    /// Set the date emails must be received at or after to match.
    pub fn after(mut self, after: impl Into<UtcDate>) -> Self {
        self.after = Some(after.into());
        self
    }

    /// Set both the `after` and `before` bounds at once.
    pub fn time_range(mut self, after: Option<UtcDate>, before: Option<UtcDate>) -> Self {
        self.after = after;
        self.before = before;
        self
    }

    /// Set whether a match must be an S/MIME message.
    pub fn with_has_smime(mut self, has: bool) -> Self {
        self.has_smime = Some(has);
        self
    }

    /// Set whether a match must be a verified S/MIME message.
    pub fn with_has_verified_smime(mut self, has: bool) -> Self {
        self.has_verified_smime = Some(has);
        self
    }
}

/// A sending identity (RFC 8621 §6).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct Identity {
    /// The server-assigned id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<Id>,
    /// The display name to use in the `From` header when sending as this identity.
    #[serde(default)]
    pub name: String,
    /// The email address to send as.
    #[serde(default)]
    pub email: String,
    /// The default `Reply-To` header's addresses for messages sent with this identity.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reply_to: Option<Vec<EmailAddress>>,
    /// The default `Bcc` header's addresses for messages sent with this identity.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bcc: Option<Vec<EmailAddress>>,
    /// The plain text signature to append to messages sent with this identity.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text_signature: Option<String>,
    /// The HTML signature to append to messages sent with this identity.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub html_signature: Option<String>,
    /// The mailbox drafts sent with this identity are filed in by default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub draft_mailbox_id: Option<Id>,
    /// True if the requesting account may delete this identity.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub may_delete: Option<bool>,
    /// True if the requesting account may send using this identity.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub may_send: Option<bool>,
    /// Extra properties the server sent that this struct does not name.
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl Identity {
    /// Create an identity with the given display name and email address.
    pub fn new(name: impl Into<String>, email: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            email: email.into(),
            ..Self::default()
        }
    }

    /// Set the server-assigned id.
    pub fn with_id(mut self, id: impl Into<Id>) -> Self {
        self.id = Some(id.into());
        self
    }

    /// Set the default `Reply-To` header's addresses.
    pub fn with_reply_to(mut self, reply_to: impl IntoIterator<Item = EmailAddress>) -> Self {
        self.reply_to = Some(reply_to.into_iter().collect());
        self
    }

    /// Set the default `Bcc` header's addresses.
    pub fn with_bcc(mut self, bcc: impl IntoIterator<Item = EmailAddress>) -> Self {
        self.bcc = Some(bcc.into_iter().collect());
        self
    }

    /// Set the plain text signature.
    pub fn with_text_signature(mut self, sig: impl Into<String>) -> Self {
        self.text_signature = Some(sig.into());
        self
    }

    /// Set the HTML signature.
    pub fn with_html_signature(mut self, sig: impl Into<String>) -> Self {
        self.html_signature = Some(sig.into());
        self
    }

    /// Set the mailbox drafts are filed in by default.
    pub fn with_draft_mailbox_id(mut self, draft_mailbox_id: impl Into<Id>) -> Self {
        self.draft_mailbox_id = Some(draft_mailbox_id.into());
        self
    }

    /// Set whether the requesting account may send using this identity.
    pub fn may_send(mut self, may_send: bool) -> Self {
        self.may_send = Some(may_send);
        self
    }

    /// Set whether the requesting account may delete this identity.
    pub fn with_may_delete(mut self, may_delete: bool) -> Self {
        self.may_delete = Some(may_delete);
        self
    }
}

/// A submission of an email for delivery (RFC 8621 §7).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct EmailSubmission {
    /// The server-assigned id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<Id>,
    /// The identity to submit the email as.
    #[serde(default)]
    pub identity_id: Id,
    /// The id of the email to submit.
    #[serde(default)]
    pub email_id: Id,
    /// The id of the thread the submitted email belongs to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread_id: Option<Id>,
    /// The SMTP envelope to use, or `None` to derive one from the message.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub envelope: Option<Envelope>,
    /// When the server received the submission request.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub send_at: Option<UtcDate>,
    /// Whether the submission can still be cancelled (see `undo_status`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub undo_status: Option<String>,
    /// Per-recipient delivery status, keyed by email address, once known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delivery_status: Option<BTreeMap<String, DeliveryStatus>>,
    /// Extra properties the server sent that this struct does not name.
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl EmailSubmission {
    /// Create a submission of the given email under the given identity.
    pub fn new(identity_id: impl Into<Id>, email_id: impl Into<Id>) -> Self {
        Self {
            identity_id: identity_id.into(),
            email_id: email_id.into(),
            ..Self::default()
        }
    }

    /// Set the server-assigned id.
    pub fn with_id(mut self, id: impl Into<Id>) -> Self {
        self.id = Some(id.into());
        self
    }

    /// Set the id of the thread the submitted email belongs to.
    pub fn with_thread_id(mut self, thread_id: impl Into<Id>) -> Self {
        self.thread_id = Some(thread_id.into());
        self
    }

    /// Set the SMTP envelope to use.
    pub fn with_envelope(mut self, envelope: Envelope) -> Self {
        self.envelope = Some(envelope);
        self
    }

    /// Set whether the submission can still be cancelled.
    pub fn with_undo_status(mut self, undo_status: impl Into<String>) -> Self {
        self.undo_status = Some(undo_status.into());
        self
    }

    /// Set the per-recipient delivery status, keyed by email address.
    pub fn with_delivery_status(
        mut self,
        delivery_status: BTreeMap<String, DeliveryStatus>,
    ) -> Self {
        self.delivery_status = Some(delivery_status);
        self
    }
}

/// The delivery status of an email submission to one recipient (RFC 8621 §7.1.1).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct DeliveryStatus {
    /// The raw SMTP reply the server received for this recipient.
    #[serde(default)]
    pub smtp_reply: String,
    /// Whether the message was delivered to the recipient (see `delivered`).
    #[serde(default = "default_delivered")]
    pub delivered: String,
    /// Whether the recipient has been notified the message is ready (see `displayed`).
    #[serde(default = "default_displayed")]
    pub displayed: String,
    /// Extra properties the server sent that this struct does not name.
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

fn default_delivered() -> String {
    delivered::UNKNOWN.to_owned()
}

fn default_displayed() -> String {
    displayed::UNKNOWN.to_owned()
}

impl DeliveryStatus {
    /// Create a delivery status from the given SMTP reply, with delivered and displayed unknown.
    pub fn new(smtp_reply: impl Into<String>) -> Self {
        Self {
            smtp_reply: smtp_reply.into(),
            delivered: default_delivered(),
            displayed: default_displayed(),
            extra: BTreeMap::new(),
        }
    }

    /// Set the raw SMTP reply.
    pub fn with_smtp_reply(mut self, smtp_reply: impl Into<String>) -> Self {
        self.smtp_reply = smtp_reply.into();
        self
    }

    /// Set whether the message was delivered.
    pub fn with_delivered(mut self, delivered: impl Into<String>) -> Self {
        self.delivered = delivered.into();
        self
    }

    /// Set whether the recipient has been notified.
    pub fn with_displayed(mut self, displayed: impl Into<String>) -> Self {
        self.displayed = displayed.into();
        self
    }

    /// Set the extra properties the server sent beyond the named fields.
    pub fn with_extra(mut self, extra: BTreeMap<String, Value>) -> Self {
        self.extra = extra;
        self
    }
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
    /// The message is still queued for delivery.
    pub const QUEUED: &str = "queued";
    /// The message was delivered.
    pub const YES: &str = "yes";
    /// The message could not be delivered.
    pub const NO: &str = "no";
    /// Whether the message was delivered is not yet known.
    pub const UNKNOWN: &str = "unknown";
}

/// Displayed status values for `DeliveryStatus` (RFC 8621 §7.1.1).
pub mod displayed {
    /// Whether the recipient has been notified is not yet known.
    pub const UNKNOWN: &str = "unknown";
    /// The recipient has been notified the message is ready.
    pub const YES: &str = "yes";
}

/// `EmailSubmission/query` filter conditions (RFC 8621 §7.3).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct EmailSubmissionQueryFilter {
    /// Matches submissions sent under one of these identities.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub identity_ids: Option<Vec<Id>>,
    /// Matches submissions of one of these emails.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub email_ids: Option<Vec<Id>>,
    /// Matches submissions whose email belongs to one of these threads.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread_ids: Option<Vec<Id>>,
    /// Matches submissions with this undo status.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub undo_status: Option<String>,
    /// Matches submissions sent strictly before this date.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub before: Option<UtcDate>,
    /// Matches submissions sent at or after this date.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after: Option<UtcDate>,
}

impl EmailSubmissionQueryFilter {
    /// Create a filter that matches every submission.
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the identities a match must be sent under.
    pub fn with_identity_ids(
        mut self,
        identity_ids: impl IntoIterator<Item = impl Into<Id>>,
    ) -> Self {
        self.identity_ids = Some(identity_ids.into_iter().map(Into::into).collect());
        self
    }

    /// Set the emails a match must submit.
    pub fn with_email_ids(mut self, email_ids: impl IntoIterator<Item = impl Into<Id>>) -> Self {
        self.email_ids = Some(email_ids.into_iter().map(Into::into).collect());
        self
    }

    /// Set the threads a match's email must belong to.
    pub fn with_thread_ids(mut self, thread_ids: impl IntoIterator<Item = impl Into<Id>>) -> Self {
        self.thread_ids = Some(thread_ids.into_iter().map(Into::into).collect());
        self
    }

    /// Set the undo status a match must have.
    pub fn with_undo_status(mut self, undo_status: impl Into<String>) -> Self {
        self.undo_status = Some(undo_status.into());
        self
    }

    /// Set the date submissions must be sent strictly before to match.
    pub fn before(mut self, before: impl Into<UtcDate>) -> Self {
        self.before = Some(before.into());
        self
    }

    /// Set the date submissions must be sent at or after to match.
    pub fn after(mut self, after: impl Into<UtcDate>) -> Self {
        self.after = Some(after.into());
        self
    }

    /// Set both the `after` and `before` bounds at once.
    pub fn time_range(mut self, after: Option<UtcDate>, before: Option<UtcDate>) -> Self {
        self.after = after;
        self.before = before;
        self
    }
}

/// Snippet of matching text in an email search (RFC 8621 §5).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct SearchSnippet {
    /// The id of the email this snippet is for.
    pub email_id: Id,
    /// A snippet of the subject with matching terms highlighted, if any matched.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subject: Option<String>,
    /// A snippet of the body with matching terms highlighted, if any matched.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preview: Option<String>,
    /// Extra properties the server sent that this struct does not name.
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl SearchSnippet {
    /// Create a snippet for the given email with no highlighted text yet.
    pub fn new(email_id: impl Into<Id>) -> Self {
        Self {
            email_id: email_id.into(),
            subject: None,
            preview: None,
            extra: BTreeMap::new(),
        }
    }

    /// Set the highlighted subject snippet.
    pub fn with_subject(mut self, subject: impl Into<String>) -> Self {
        self.subject = Some(subject.into());
        self
    }

    /// Set the highlighted body snippet.
    pub fn with_preview(mut self, preview: impl Into<String>) -> Self {
        self.preview = Some(preview.into());
        self
    }
}

/// `SearchSnippet/get` arguments (RFC 8621 §5.1). `filter` is the same
/// `FilterOperator|FilterCondition|null` as `Email/query`'s, not a bare leaf
/// condition, so it can be an AND/OR/NOT tree.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct SearchSnippetGetRequest {
    /// The account the emails belong to.
    pub account_id: Id,
    /// The same filter passed to `Email/query`, used to find terms to highlight.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub filter: Option<crate::methods::Filter<EmailQueryFilter>>,
    /// The ids of the emails to generate snippets for.
    pub email_ids: Vec<Id>,
}

impl SearchSnippetGetRequest {
    /// Create a request for the given emails with no filter set.
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

    /// Set the filter used to find terms to highlight.
    pub fn with_filter(
        mut self,
        filter: impl Into<crate::methods::Filter<EmailQueryFilter>>,
    ) -> Self {
        self.filter = Some(filter.into());
        self
    }
}

/// `SearchSnippet/get` response (RFC 8621 §5.1).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct SearchSnippetGetResponse {
    /// The account the emails belong to.
    pub account_id: Id,
    /// The generated snippets.
    #[serde(default)]
    pub list: Vec<SearchSnippet>,
    /// The email ids that do not exist.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub not_found: Option<Vec<Id>>,
}

impl SearchSnippetGetResponse {
    /// Create a response with the given snippets and no missing ids.
    pub fn new(account_id: impl Into<Id>, list: impl IntoIterator<Item = SearchSnippet>) -> Self {
        Self {
            account_id: account_id.into(),
            list: list.into_iter().collect(),
            not_found: None,
        }
    }

    /// Set the email ids that do not exist.
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
#[non_exhaustive]
pub struct EmailSubmissionSetRequest {
    /// The underlying `EmailSubmission/set` arguments: create, update and destroy.
    #[serde(flatten)]
    pub set: crate::methods::SetRequest<EmailSubmission>,
    /// Properties to patch onto the submitted email once the submission succeeds, keyed by submission or creation id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub on_success_update_email: Option<BTreeMap<String, Value>>,
    /// Submission or creation ids whose email to destroy once the submission succeeds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub on_success_destroy_email: Option<Vec<String>>,
}

impl EmailSubmissionSetRequest {
    /// Create a request from the underlying `EmailSubmission/set` arguments.
    pub fn new(set: crate::methods::SetRequest<EmailSubmission>) -> Self {
        Self {
            set,
            on_success_update_email: None,
            on_success_destroy_email: None,
        }
    }

    /// Set the email patches to apply on success, keyed by submission or creation id.
    pub fn with_on_success_update_email(
        mut self,
        on_success_update_email: BTreeMap<String, Value>,
    ) -> Self {
        self.on_success_update_email = Some(on_success_update_email);
        self
    }

    /// Set the submission or creation ids whose email to destroy on success.
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
#[non_exhaustive]
pub struct Envelope {
    /// The SMTP `MAIL FROM` address.
    pub mail_from: EnvelopeAddress,
    /// The SMTP `RCPT TO` addresses.
    #[serde(default)]
    pub rcpt_to: Vec<EnvelopeAddress>,
}

impl Envelope {
    /// Create an envelope with the given sender and recipients.
    pub fn new(
        mail_from: EnvelopeAddress,
        rcpt_to: impl IntoIterator<Item = EnvelopeAddress>,
    ) -> Self {
        Self {
            mail_from,
            rcpt_to: rcpt_to.into_iter().collect(),
        }
    }

    /// Set the SMTP `MAIL FROM` address.
    pub fn with_mail_from(mut self, mail_from: EnvelopeAddress) -> Self {
        self.mail_from = mail_from;
        self
    }

    /// Set the SMTP `RCPT TO` addresses.
    pub fn with_rcpt_to(mut self, rcpt_to: impl IntoIterator<Item = EnvelopeAddress>) -> Self {
        self.rcpt_to = rcpt_to.into_iter().collect();
        self
    }

    /// Add one recipient to the `RCPT TO` addresses.
    pub fn with_recipient(mut self, recipient: EnvelopeAddress) -> Self {
        self.rcpt_to.push(recipient);
        self
    }
}

/// One envelope address; `parameters` (SMTP extensions) is nullable and
/// always present on the wire.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[non_exhaustive]
pub struct EnvelopeAddress {
    /// The SMTP address.
    pub email: String,
    /// SMTP extension parameters for this address, if any.
    #[serde(default)]
    pub parameters: Option<Value>,
}

impl EnvelopeAddress {
    /// Create an envelope address with no parameters.
    pub fn new(email: impl Into<String>) -> Self {
        Self {
            email: email.into(),
            parameters: None,
        }
    }

    /// Set the SMTP extension parameters.
    pub fn with_parameters(mut self, parameters: Value) -> Self {
        self.parameters = Some(parameters);
        self
    }
}

/// Release schedule for delayed message submission (RFC 4865, RFC 8621 §7).
/// Configured as a `mailFrom.parameters` entry.
/// Stays exhaustive: an untagged two-case type fixed by the RFC, not a type a server extends.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Schedule {
    /// Hold for this many seconds (RFC 4865 `HOLDFOR`, at most 99999999).
    HoldFor(u64),
    /// Hold until this time (RFC 4865 `HOLDUNTIL`).
    HoldUntil(UtcDate),
}

impl Schedule {
    /// The `mailFrom.parameters` entry this schedule is on the wire as.
    ///
    /// RFC 8621 §7 types parameter values as `String|null`, so the seconds
    /// count is carried in decimal, not as a JSON number.
    pub fn parameter(&self) -> (&'static str, Value) {
        match self {
            Self::HoldFor(seconds) => ("HOLDFOR", Value::String(seconds.to_string())),
            Self::HoldUntil(date) => ("HOLDUNTIL", Value::String(date.as_str().to_owned())),
        }
    }
}

impl Envelope {
    /// This envelope with `schedule`'s parameter on `mailFrom`, keeping any
    /// parameters already there.
    pub fn with_schedule(mut self, schedule: &Schedule) -> Self {
        let (name, value) = schedule.parameter();
        let parameters = match self.mail_from.parameters.take() {
            Some(Value::Object(map)) => map,
            // `null` is how "no parameters" is spelled on the wire.
            _ => serde_json::Map::new(),
        };
        let mut parameters = parameters;
        parameters.insert(name.to_owned(), value);
        self.mail_from.parameters = Some(Value::Object(parameters));
        self
    }
}

/// The `SetError` types RFC 8621 §6.4 adds for `Identity/set`.
pub mod identity_set_error {
    /// The set error type returned when the account may not send using the given `From` address.
    pub const FORBIDDEN_FROM: &str = "forbiddenFrom";
    /// The set error type returned when destroying the account's default identity.
    pub const CANNOT_DESTROY_DEFAULT: &str = "cannotDestroyDefault";
}

/// The `SetError` types RFC 8621 §4.6 adds for `Email/set`.
pub mod email_set_error {
    /// The set error type returned when a referenced blob id does not exist.
    pub const BLOB_NOT_FOUND: &str = "blobNotFound";
    /// The set error type returned when an email would have more keywords than the server allows.
    pub const TOO_MANY_KEYWORDS: &str = "tooManyKeywords";
    /// The set error type returned when an email would be filed in more mailboxes than the server allows.
    pub const TOO_MANY_MAILBOXES: &str = "tooManyMailboxes";
}

/// The `SetError` types RFC 8621 §7.5 adds for `EmailSubmission/set`.
pub mod email_submission_set_error {
    /// The set error type returned when a submission can no longer be cancelled.
    pub const CANNOT_UNSEND: &str = "cannotUnsend";
    /// The set error type returned when the envelope has more recipients than the server allows.
    pub const TOO_MANY_RECIPIENTS: &str = "tooManyRecipients";
    /// The set error type returned when the envelope has no recipients.
    pub const NO_RECIPIENTS: &str = "noRecipients";
    /// The set error type returned when one or more recipient addresses are invalid.
    pub const INVALID_RECIPIENTS: &str = "invalidRecipients";
    /// The set error type returned when the account may not use the given `MAIL FROM` address.
    pub const FORBIDDEN_MAIL_FROM: &str = "forbiddenMailFrom";
    /// The set error type returned when the account may not submit using the given `From` address.
    pub const FORBIDDEN_FROM: &str = "forbiddenFrom";
    /// The set error type returned when the account may not submit emails at all.
    pub const FORBIDDEN_TO_SEND: &str = "forbiddenToSend";
}

/// Standard undoStatus values (RFC 8621 §7).
pub mod undo_status {
    /// The submission can still be cancelled.
    pub const PENDING: &str = "pending";
    /// The submission has been delivered and can no longer be cancelled.
    pub const FINAL: &str = "final";
    /// The submission was cancelled before it was delivered.
    pub const CANCELED: &str = "canceled";
}

/// Mail capability properties (RFC 8621 §1.3).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct MailCapability {
    /// The maximum total size, in bytes, of attachments on a single email.
    #[serde(default)]
    pub max_size_attachments_per_email: u64,
    /// The maximum size, in bytes, of a single email.
    #[serde(default)]
    pub max_size_email_in_bytes: u64,
    /// The maximum size, in bytes, the server will return for a single body value.
    #[serde(default)]
    pub max_size_body_value_bytes: u64,
    /// The maximum number of attachments on a single email.
    #[serde(default)]
    pub max_number_of_attachments_per_email: u64,
    /// The maximum number of recipients on a single email.
    #[serde(default)]
    pub max_number_of_recipients_per_email: u64,
    /// True if the account may create mailboxes at the top level.
    #[serde(default)]
    pub may_create_top_level_mailbox: bool,
    /// Extra properties the server sent that this struct does not name.
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl MailCapability {
    /// Create a mail capability with every limit at zero.
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the maximum total attachment size, in bytes, for a single email.
    pub fn with_max_size_attachments_per_email(mut self, max: u64) -> Self {
        self.max_size_attachments_per_email = max;
        self
    }

    /// Set the maximum size, in bytes, of a single email.
    pub fn with_max_size_email_in_bytes(mut self, max: u64) -> Self {
        self.max_size_email_in_bytes = max;
        self
    }

    /// Set the maximum size, in bytes, of a returned body value.
    pub fn with_max_size_body_value_bytes(mut self, max: u64) -> Self {
        self.max_size_body_value_bytes = max;
        self
    }

    /// Set the maximum number of attachments on a single email.
    pub fn with_max_number_of_attachments_per_email(mut self, max: u64) -> Self {
        self.max_number_of_attachments_per_email = max;
        self
    }

    /// Set the maximum number of recipients on a single email.
    pub fn with_max_number_of_recipients_per_email(mut self, max: u64) -> Self {
        self.max_number_of_recipients_per_email = max;
        self
    }

    /// Set whether the account may create mailboxes at the top level.
    pub fn may_create_top_level_mailbox(mut self, may: bool) -> Self {
        self.may_create_top_level_mailbox = may;
        self
    }

    /// Set the extra properties the server sent beyond the named fields.
    pub fn with_extra(mut self, extra: BTreeMap<String, Value>) -> Self {
        self.extra = extra;
        self
    }
}

/// Submission capability properties (RFC 8621 §1.4).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct SubmissionCapability {
    /// The maximum delay, in seconds, the server allows for a future release submission.
    #[serde(default)]
    pub max_delayed_send: u64,
    /// The SMTP extensions the server supports, keyed by extension name.
    #[serde(default)]
    pub submission_extensions: BTreeMap<String, Vec<String>>,
    /// Extra properties the server sent that this struct does not name.
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl SubmissionCapability {
    /// Create a submission capability with no delayed send and no extensions.
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the maximum delay, in seconds, allowed for a future release submission.
    pub fn with_max_delayed_send(mut self, max: u64) -> Self {
        self.max_delayed_send = max;
        self
    }

    /// Set the SMTP extensions the server supports.
    pub fn with_submission_extensions(mut self, extensions: BTreeMap<String, Vec<String>>) -> Self {
        self.submission_extensions = extensions;
        self
    }

    /// Set the extra properties the server sent beyond the named fields.
    pub fn with_extra(mut self, extra: BTreeMap<String, Value>) -> Self {
        self.extra = extra;
        self
    }

    /// Checks if the server advertises FUTURERELEASE (RFC 4865) support.
    /// Matches capability keys case-insensitively.
    pub fn supports_future_release(&self) -> bool {
        self.submission_extensions
            .keys()
            .any(|name| name.eq_ignore_ascii_case("FUTURERELEASE"))
    }
}

/// A Message Disposition Notification (RFC 9007 §2).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct MDN {
    /// The id of the email this MDN reports on.
    pub for_email_id: Id,
    /// The subject line of the MDN message, or `None` for a server default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subject: Option<String>,
    /// The human-readable text body of the MDN message.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text_body: Option<String>,
    /// True to include the original message, or its headers, in the MDN.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub include_original_message: Option<bool>,
    /// The `Reporting-UA` field, identifying the mail agent generating the MDN.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reporting_ua: Option<String>,
    /// The disposition information describing what happened to the message.
    pub disposition: MDNDisposition,
    /// The `MDN-Gateway` field, naming the gateway that translated the MDN.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mdn_gateway: Option<String>,
    /// The `Original-Recipient` field, the address the message was originally sent to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub original_recipient: Option<String>,
    /// The `Final-Recipient` field, the address the MDN-generating agent recognized.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub final_recipient: Option<String>,
    /// The `Original-Message-ID` field, copied from the original message's `Message-ID` header.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub original_message_id: Option<String>,
    /// Additional information about message handling errors.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<Vec<String>>,
    /// Extension fields to add to the MDN, keyed by field name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extension_fields: Option<BTreeMap<String, String>>,
    /// Extra properties the server sent that this struct does not name.
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl MDN {
    /// Create an MDN for the given email with the given disposition.
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

    /// Set the subject line of the MDN message.
    pub fn with_subject(mut self, subject: impl Into<String>) -> Self {
        self.subject = Some(subject.into());
        self
    }

    /// Set the human-readable text body of the MDN message.
    pub fn with_text_body(mut self, text_body: impl Into<String>) -> Self {
        self.text_body = Some(text_body.into());
        self
    }

    /// Set whether to include the original message in the MDN.
    pub fn with_include_original_message(mut self, include_original_message: bool) -> Self {
        self.include_original_message = Some(include_original_message);
        self
    }

    /// Set the `Reporting-UA` field.
    pub fn with_reporting_ua(mut self, reporting_ua: impl Into<String>) -> Self {
        self.reporting_ua = Some(reporting_ua.into());
        self
    }

    /// Set the `Final-Recipient` field.
    pub fn with_final_recipient(mut self, final_recipient: impl Into<String>) -> Self {
        self.final_recipient = Some(final_recipient.into());
        self
    }

    /// Set the `Original-Message-ID` field.
    pub fn with_original_message_id(mut self, original_message_id: impl Into<String>) -> Self {
        self.original_message_id = Some(original_message_id.into());
        self
    }

    /// Set the `MDN-Gateway` field.
    pub fn with_mdn_gateway(mut self, gateway: impl Into<String>) -> Self {
        self.mdn_gateway = Some(gateway.into());
        self
    }

    /// Set the `Original-Recipient` field.
    pub fn with_original_recipient(mut self, original_recipient: impl Into<String>) -> Self {
        self.original_recipient = Some(original_recipient.into());
        self
    }

    /// Set additional information about message handling errors.
    pub fn with_error(mut self, error: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.error = Some(error.into_iter().map(Into::into).collect());
        self
    }

    /// Set the extension fields to add to the MDN.
    pub fn with_extension_fields(mut self, extension_fields: BTreeMap<String, String>) -> Self {
        self.extension_fields = Some(extension_fields);
        self
    }

    /// Set the extra properties the server sent beyond the named fields.
    pub fn with_extra(mut self, extra: BTreeMap<String, Value>) -> Self {
        self.extra = extra;
        self
    }
}

/// The disposition information of an MDN (RFC 9007 §2.1).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct MDNDisposition {
    /// Whether the disposition was manual or automatic (see `mdn_action_mode`).
    pub action_mode: String,
    /// Whether the MDN itself was sent manually or automatically (see `mdn_sending_mode`).
    pub sending_mode: String,
    /// The kind of disposition that occurred (see `mdn_disposition_type`).
    #[serde(rename = "type")]
    pub disposition_type: String,
    /// An error that occurred while processing the MDN, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// Additional disposition modifiers, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub modifiers: Option<Vec<String>>,
    /// Extra properties the server sent that this struct does not name.
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl MDNDisposition {
    /// Create disposition information with the given action, sending mode and type.
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

    /// Set the error that occurred while processing the MDN.
    pub fn with_error(mut self, error: impl Into<String>) -> Self {
        self.error = Some(error.into());
        self
    }

    /// Set additional disposition modifiers.
    pub fn with_modifiers(
        mut self,
        modifiers: impl IntoIterator<Item = impl Into<String>>,
    ) -> Self {
        self.modifiers = Some(modifiers.into_iter().map(Into::into).collect());
        self
    }

    /// Set the extra properties the server sent beyond the named fields.
    pub fn with_extra(mut self, extra: BTreeMap<String, Value>) -> Self {
        self.extra = extra;
        self
    }
}

/// `MDN/send` arguments (RFC 9007 §3.1).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct MDNSendRequest {
    /// The account the email belongs to.
    pub account_id: Id,
    /// The MDNs to send, keyed by a client-chosen creation id.
    pub send: BTreeMap<String, MDN>,
    /// Properties to patch onto the reported email once the MDN sends, keyed by email id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub on_success_update_email: Option<BTreeMap<Id, Value>>,
}

impl MDNSendRequest {
    /// Create a request to send the given MDNs.
    pub fn new(account_id: impl Into<Id>, send: BTreeMap<String, MDN>) -> Self {
        Self {
            account_id: account_id.into(),
            send,
            on_success_update_email: None,
        }
    }

    /// Set the email patches to apply once the MDNs send.
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
#[non_exhaustive]
pub struct MDNSendResponse {
    /// The account the email belongs to.
    pub account_id: Id,
    /// The MDNs that were sent, keyed by their creation id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sent: Option<BTreeMap<String, MDN>>,
    /// The MDNs that failed to send, keyed by their creation id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub not_sent: Option<BTreeMap<String, crate::error::SetError>>,
}

impl MDNSendResponse {
    /// Create a response with nothing sent or failed yet.
    pub fn new(account_id: impl Into<Id>) -> Self {
        Self {
            account_id: account_id.into(),
            sent: None,
            not_sent: None,
        }
    }

    /// Set the MDNs that were sent.
    pub fn with_sent(mut self, sent: BTreeMap<String, MDN>) -> Self {
        self.sent = Some(sent);
        self
    }

    /// Set the MDNs that failed to send.
    pub fn with_not_sent(mut self, not_sent: BTreeMap<String, crate::error::SetError>) -> Self {
        self.not_sent = Some(not_sent);
        self
    }
}

/// Standard MDN action mode values (RFC 9007 §2.1, RFC 8098).
pub mod mdn_action_mode {
    /// The user manually triggered the disposition.
    pub const MANUAL_ACTION: &str = "manual-action";
    /// A mail filter or other automatic process triggered the disposition.
    pub const AUTOMATIC_ACTION: &str = "automatic-action";
}

/// Standard MDN sending mode values (RFC 9007 §2.1, RFC 8098).
pub mod mdn_sending_mode {
    /// The user manually chose to send the MDN.
    pub const MDN_SENT_MANUALLY: &str = "mdn-sent-manually";
    /// The MDN was sent automatically, without the user's direct action.
    pub const MDN_SENT_AUTOMATICALLY: &str = "mdn-sent-automatically";
}

/// Standard MDN disposition type values (RFC 9007 §2.1, RFC 8098).
pub mod mdn_disposition_type {
    /// The message was displayed to the user.
    pub const DISPLAYED: &str = "displayed";
    /// The message was deleted without being displayed.
    pub const DELETED: &str = "deleted";
    /// The message was forwarded or otherwise dispatched outside the mail system.
    pub const DISPATCHED: &str = "dispatched";
    /// The message was processed by an automatic agent without being displayed.
    pub const PROCESSED: &str = "processed";
}

/// The `SetError` types RFC 9007 §3.2 adds for `MDN/send`.
pub mod mdn_set_error {
    /// The set error type returned when an MDN has already been sent for this email.
    pub const MDN_ALREADY_SENT: &str = "mdnAlreadySent";
    /// The set error type returned when the account may not send an MDN using the given `From` address.
    pub const FORBIDDEN_FROM: &str = "forbiddenFrom";
}

/// MDN capability properties (RFC 9007 §1.3).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct MDNCapability {
    /// Extra properties the server sent that this struct does not name.
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl MDNCapability {
    /// Create an empty MDN capability.
    pub fn new() -> Self {
        Self::default()
    }

    /// Merge the given object's properties into the extra properties.
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
#[non_exhaustive]
pub struct SmimeVerifyCapability {
    /// Extra properties the server sent that this struct does not name.
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl SmimeVerifyCapability {
    /// Create an empty S/MIME verification capability.
    pub fn new() -> Self {
        Self::default()
    }

    /// Merge the given object's properties into the extra properties.
    pub fn with_extra(mut self, extra: Value) -> Self {
        if let Value::Object(map) = extra {
            self.extra.extend(map);
        }
        self
    }
}

/// Standard S/MIME signature verification status values (RFC 9219 §4.1).
pub mod smime_status {
    /// Verification was not attempted or the result is unknown.
    pub const UNKNOWN: &str = "unknown";
    /// The message was signed, but the signature was not verified.
    pub const SIGNED: &str = "signed";
    /// The message was signed and the signature verified successfully.
    pub const SIGNED_VERIFIED: &str = "signed/verified";
    /// The message was signed, but signature verification failed.
    pub const SIGNED_FAILED: &str = "signed/failed";
}
