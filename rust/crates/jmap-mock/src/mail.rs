// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Mail methods (`Mailbox/*`, `Email/*`) and mail seeding helpers.

use jmap_proto::error::MethodError;
use jmap_proto::mail::{
    Email, EmailAddress, EmailBodyPart, EmailBodyValue, EmailQueryFilter, Mailbox,
};
use jmap_proto::methods::{GetRequest, GetResponse, QueryRequest, QueryResponse};
use jmap_proto::{Id, UtcDate};
use serde_json::Value;

use crate::dispatch::{account_mut, parse_arguments, project_properties, to_result};
use crate::state::{AccountState, ServerState};

// ── Method handlers ──────────────────────────────────────────────────────────

pub fn mailbox_get(state: &mut ServerState, arguments: Value) -> Result<Value, MethodError> {
    let request: GetRequest = parse_arguments(arguments)?;
    let account = account_mut(state, &request.account_id)?;

    // Message counts are derived, not stored.
    let counted: Vec<Mailbox> = account
        .mailboxes
        .iter()
        .map(|(id, mailbox)| {
            let mut mailbox = mailbox.clone();
            let in_mailbox = |email: &&Email| {
                email
                    .mailbox_ids
                    .as_ref()
                    .is_some_and(|mailbox_ids| mailbox_ids.get(id).copied().unwrap_or(false))
            };
            let total = account
                .emails
                .iter()
                .map(|(_, email)| email)
                .filter(in_mailbox)
                .count() as u64;
            let unread = account
                .emails
                .iter()
                .map(|(_, email)| email)
                .filter(in_mailbox)
                .filter(|email| {
                    !email.keywords.as_ref().is_some_and(|keywords| {
                        keywords.contains_key(jmap_proto::mail::keyword::SEEN)
                    })
                })
                .count() as u64;
            mailbox.total_emails = Some(total);
            mailbox.unread_emails = Some(unread);
            mailbox.total_threads = Some(total);
            mailbox.unread_threads = Some(unread);
            mailbox
        })
        .collect();

    let (list, not_found) = match &request.ids {
        None => (counted, Vec::new()),
        Some(ids) => {
            let mut list = Vec::new();
            let mut not_found = Vec::new();
            for id in ids {
                match counted
                    .iter()
                    .find(|mailbox| mailbox.id.as_ref() == Some(id))
                {
                    Some(mailbox) => list.push(mailbox.clone()),
                    None => not_found.push(id.clone()),
                }
            }
            (list, not_found)
        }
    };

    to_result(&GetResponse {
        account_id: request.account_id,
        state: account.mailboxes.state(),
        list,
        not_found,
    })
}

pub fn email_get(state: &mut ServerState, arguments: Value) -> Result<Value, MethodError> {
    let request: GetRequest = parse_arguments(arguments)?;
    let account = account_mut(state, &request.account_id)?;

    let mut list = Vec::new();
    let mut not_found = Vec::new();
    match &request.ids {
        None => {
            for (_, email) in account.emails.iter() {
                list.push(project_properties(email, request.properties.as_deref())?);
            }
        }
        Some(ids) => {
            for id in ids {
                match account.emails.get(id) {
                    Some(email) => {
                        list.push(project_properties(email, request.properties.as_deref())?)
                    }
                    None => not_found.push(id.clone()),
                }
            }
        }
    }

    to_result(&GetResponse {
        account_id: request.account_id,
        state: account.emails.state(),
        list,
        not_found,
    })
}

pub fn email_query(state: &mut ServerState, arguments: Value) -> Result<Value, MethodError> {
    let request: QueryRequest<EmailQueryFilter> = parse_arguments(arguments)?;
    let account = account_mut(state, &request.account_id)?;

    let filter = request.filter.unwrap_or_default();
    let mut matches: Vec<(&Id, &Email)> = account
        .emails
        .iter()
        .filter(|(_, email)| email_matches(email, &filter))
        .collect();

    // Default order: receivedAt descending (RFC 8621 servers commonly do
    // newest-first; tests pass sort explicitly anyway).
    let mut sort_property = "receivedAt".to_owned();
    let mut ascending = false;
    if let Some(comparators) = &request.sort {
        if let Some(first) = comparators.first() {
            sort_property = first.property.clone();
            ascending = first.is_ascending;
        }
    }
    if sort_property != "receivedAt" {
        return Err(MethodError::new("unsupportedSort")
            .with_description(format!("mock cannot sort emails by {sort_property}")));
    }
    matches.sort_by(|(_, a), (_, b)| {
        let key = |email: &Email| {
            email
                .received_at
                .clone()
                .unwrap_or_else(|| UtcDate::new(""))
        };
        let ordering = key(a).cmp(&key(b));
        if ascending {
            ordering
        } else {
            ordering.reverse()
        }
    });

    let total = matches.len() as u64;
    let position = request.position.max(0) as usize;
    let ids: Vec<Id> = matches
        .into_iter()
        .map(|(id, _)| id.clone())
        .skip(position)
        .take(request.limit.unwrap_or(u64::MAX) as usize)
        .collect();

    to_result(&QueryResponse {
        account_id: request.account_id,
        query_state: account.emails.state(),
        can_calculate_changes: false,
        position: position as u64,
        ids,
        total: request.calculate_total.then_some(total),
        limit: None,
    })
}

fn email_matches(email: &Email, filter: &EmailQueryFilter) -> bool {
    if let Some(mailbox_id) = &filter.in_mailbox
        && !email
            .mailbox_ids
            .as_ref()
            .is_some_and(|ids| ids.get(mailbox_id).copied().unwrap_or(false))
    {
        return false;
    }
    if let Some(keyword) = &filter.has_keyword
        && !email
            .keywords
            .as_ref()
            .is_some_and(|keywords| keywords.contains_key(keyword))
    {
        return false;
    }
    if let Some(keyword) = &filter.not_keyword
        && email
            .keywords
            .as_ref()
            .is_some_and(|keywords| keywords.contains_key(keyword))
    {
        return false;
    }
    if let Some(after) = &filter.after
        && email.received_at.as_ref().is_none_or(|at| at < after)
    {
        return false;
    }
    if let Some(before) = &filter.before
        && email.received_at.as_ref().is_none_or(|at| at >= before)
    {
        return false;
    }
    if let Some(subject) = &filter.subject
        && !email
            .subject
            .as_ref()
            .is_some_and(|value| value.contains(subject.as_str()))
    {
        return false;
    }
    if let Some(from) = &filter.from
        && !address_list_contains(email.from.as_deref(), from)
    {
        return false;
    }
    if let Some(to) = &filter.to
        && !address_list_contains(email.to.as_deref(), to)
    {
        return false;
    }
    true
}

fn address_list_contains(addresses: Option<&[EmailAddress]>, needle: &str) -> bool {
    addresses.is_some_and(|addresses| {
        addresses.iter().any(|address| {
            address.email.contains(needle)
                || address
                    .name
                    .as_ref()
                    .is_some_and(|name| name.contains(needle))
        })
    })
}

// ── Seeding helpers ──────────────────────────────────────────────────────────

/// Everything needed to seed one email into an account.
pub struct EmailSeed {
    pub mailbox_id: Id,
    pub from: EmailAddress,
    pub to: Vec<EmailAddress>,
    pub subject: String,
    pub text_body: String,
    pub received_at: UtcDate,
    pub keywords: Vec<String>,
    /// (blob id, filename, content type)
    pub attachments: Vec<(Id, String, String)>,
}

impl EmailSeed {
    pub fn new(
        mailbox_id: impl Into<Id>,
        from: (&str, &str),
        subject: &str,
        text_body: &str,
        received_at: &str,
    ) -> Self {
        Self {
            mailbox_id: mailbox_id.into(),
            from: EmailAddress::new(Some(from.0), from.1),
            to: vec![EmailAddress::new(Some("Alice"), "alice@example.com")],
            subject: subject.to_owned(),
            text_body: text_body.to_owned(),
            received_at: UtcDate::new(received_at),
            keywords: Vec::new(),
            attachments: Vec::new(),
        }
    }

    pub fn keyword(mut self, keyword: &str) -> Self {
        self.keywords.push(keyword.to_owned());
        self
    }

    pub fn attachment(mut self, blob_id: Id, name: &str, content_type: &str) -> Self {
        self.attachments
            .push((blob_id, name.to_owned(), content_type.to_owned()));
        self
    }
}

impl AccountState {
    /// Seed a mailbox; returns its id. Does not bump state.
    pub fn seed_mailbox(&mut self, name: &str, role: Option<&str>) -> Id {
        let id = self.mailboxes.alloc_id();
        let mailbox = Mailbox {
            id: Some(id.clone()),
            name: name.to_owned(),
            role: role.map(str::to_owned),
            sort_order: Some(0),
            is_subscribed: Some(true),
            ..Mailbox::default()
        };
        self.mailboxes.seed_with_id(id.clone(), mailbox);
        id
    }

    /// Seed a full email (with a text body and optional attachments);
    /// returns its id. Does not bump state.
    pub fn seed_email(&mut self, seed: EmailSeed) -> Id {
        let id = self.emails.alloc_id();
        let body_blob_id = self.add_blob("text/plain", seed.text_body.clone().into_bytes());

        let text_part = EmailBodyPart {
            part_id: Some("1".to_owned()),
            blob_id: Some(body_blob_id),
            size: Some(seed.text_body.len() as u64),
            content_type: Some("text/plain".to_owned()),
            charset: Some("utf-8".to_owned()),
            ..EmailBodyPart::default()
        };
        let attachments: Vec<EmailBodyPart> = seed
            .attachments
            .iter()
            .enumerate()
            .map(|(index, (blob_id, name, content_type))| EmailBodyPart {
                part_id: Some(format!("{}", index + 2)),
                blob_id: Some(blob_id.clone()),
                size: self.blobs.get(blob_id).map(|blob| blob.data.len() as u64),
                name: Some(name.clone()),
                content_type: Some(content_type.clone()),
                disposition: Some("attachment".to_owned()),
                ..EmailBodyPart::default()
            })
            .collect();

        let email = Email {
            id: Some(id.clone()),
            blob_id: Some(self.add_blob("message/rfc822", Vec::new())),
            thread_id: Some(Id::new(format!("T{}", id.as_str()))),
            mailbox_ids: Some([(seed.mailbox_id, true)].into()),
            keywords: Some(
                seed.keywords
                    .into_iter()
                    .map(|keyword| (keyword, true))
                    .collect(),
            ),
            size: Some(seed.text_body.len() as u64),
            received_at: Some(seed.received_at),
            from: Some(vec![seed.from]),
            to: Some(seed.to),
            subject: Some(seed.subject),
            has_attachment: Some(!attachments.is_empty()),
            preview: Some(seed.text_body.chars().take(64).collect()),
            body_values: Some([("1".to_owned(), EmailBodyValue::new(seed.text_body))].into()),
            text_body: Some(vec![text_part]),
            attachments: (!attachments.is_empty()).then_some(attachments),
            ..Email::default()
        };
        self.emails.seed_with_id(id.clone(), email);
        id
    }
}
