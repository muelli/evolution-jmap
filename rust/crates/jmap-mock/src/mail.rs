// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Mail methods (`Mailbox/*`, `Email/*`) and mail seeding helpers.

use std::collections::BTreeMap;

use jmap_proto::error::{self, MethodError, SetError};
use jmap_proto::mail::{
    Email, EmailAddress, EmailBodyPart, EmailBodyValue, EmailHeader, EmailImportRequest,
    EmailImportResponse, EmailParseRequest, EmailQueryFilter, EmailSubmission,
    EmailSubmissionQueryFilter, EmailSubmissionSetRequest, Envelope, EnvelopeAddress, Identity,
    Mailbox, SearchSnippet, SearchSnippetGetRequest, SearchSnippetGetResponse, Thread,
    VacationResponse, email_import_error, role,
};
use jmap_proto::methods::{
    Filter, GetRequest, GetResponse, QueryRequest, QueryResponse, SetRequest, SetResponse, operator,
};
use jmap_proto::{Id, UtcDate};
use serde_json::Value;

use crate::dispatch::{account_mut, parse_arguments, project_properties, to_result};
use crate::patch::apply_patch;
use crate::state::{AccountState, RecordedSubmission, ServerState};

/// Fixed stand-in for "now" in reproducible tests.
const MOCK_NOW: &str = "2026-01-01T00:00:00Z";

/// Owners can read all mailboxes. Other callers need a share grant.
pub fn mailbox_get(
    state: &mut ServerState,
    arguments: Value,
    caller: Option<&Id>,
) -> Result<Value, MethodError> {
    let request: GetRequest = parse_arguments(arguments)?;
    let account = account_mut(state, &request.account_id)?;

    let is_owner =
        caller.is_none_or(|caller| account.current_user_principal_id.as_ref() == Some(caller));
    if !is_owner {
        let caller = caller.expect("is_owner is false only when caller is Some");
        let shared_with_caller = account
            .mailboxes
            .iter()
            .any(|(_, mailbox)| mailbox_rights_for(mailbox, caller).is_some());
        if !shared_with_caller {
            return Err(MethodError::new(error::method::FORBIDDEN)
                .with_description("no mailbox in this account is shared with you"));
        }
    }

    let counted: Vec<Mailbox> = account
        .mailboxes
        .iter()
        .map(|(id, mailbox)| {
            let mut mailbox = mailbox.clone();
            let in_mailbox = |email: &&Email| filed_in(email, id);
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
        None => {
            let list = counted
                .iter()
                .filter_map(|mailbox| visible_mailbox(mailbox, is_owner, caller))
                .collect();
            (list, Vec::new())
        }
        Some(ids) => {
            let mut list = Vec::new();
            let mut not_found = Vec::new();
            for id in ids {
                match counted
                    .iter()
                    .find(|mailbox| mailbox.id.as_ref() == Some(id))
                    .and_then(|mailbox| visible_mailbox(mailbox, is_owner, caller))
                {
                    Some(mailbox) => list.push(mailbox),
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

/// Return rights from `mailbox.share_with` for `principal`, if present.
fn mailbox_rights_for(
    mailbox: &Mailbox,
    principal: &Id,
) -> Option<jmap_proto::mail::MailboxRights> {
    mailbox.share_with.as_ref()?.get(principal).cloned()
}

/// Owners see every mailbox. Shared callers see granted mailboxes with `myRights`.
fn visible_mailbox(mailbox: &Mailbox, is_owner: bool, caller: Option<&Id>) -> Option<Mailbox> {
    if is_owner {
        return Some(mailbox.clone());
    }
    let caller = caller.expect("is_owner is false only when caller is Some");
    let rights = mailbox_rights_for(mailbox, caller)?;
    let mut visible = mailbox.clone();
    visible.my_rights = Some(rights);
    Some(visible)
}

/// Handle `Mailbox/set` (RFC 8621 §2.5) with full-request mailbox validation.
pub fn mailbox_set(state: &mut ServerState, arguments: Value) -> Result<Value, MethodError> {
    let request: SetRequest<Mailbox> = parse_arguments(arguments)?;
    let account = account_mut(state, &request.account_id)?;

    let old_state = account.mailboxes.state();
    if let Some(expected) = &request.if_in_state
        && expected != &old_state
    {
        return Err(MethodError::new(error::method::STATE_MISMATCH));
    }

    let mut tree: BTreeMap<Id, Mailbox> = account
        .mailboxes
        .iter()
        .map(|(id, mailbox)| (id.clone(), mailbox.clone()))
        .collect();

    let mut created: BTreeMap<String, Mailbox> = BTreeMap::new();
    let mut not_created: BTreeMap<String, SetError> = BTreeMap::new();
    let mut to_create: Vec<(Id, Mailbox)> = Vec::new();
    for (creation_id, mut mailbox) in request.create.unwrap_or_default() {
        if mailbox.id.is_some() {
            not_created.insert(
                creation_id,
                SetError::new(error::set::INVALID_PROPERTIES)
                    .with_description("id is set by the server and must not be given in a create"),
            );
            continue;
        }
        if let Err(refusal) = placeable(&tree, None, &mailbox) {
            not_created.insert(creation_id, refusal);
            continue;
        }
        let id = account.mailboxes.alloc_id();
        mailbox.id = Some(id.clone());
        if mailbox.sort_order.is_none() {
            mailbox.sort_order = Some(0);
        }
        // Default new mailboxes to subscribed (RFC 8621 §2 server latitude).
        if mailbox.is_subscribed.is_none() {
            mailbox.is_subscribed = Some(true);
        }
        created.insert(creation_id, counted_empty(&mailbox));
        tree.insert(id.clone(), mailbox.clone());
        to_create.push((id, mailbox));
    }

    let mut updated: BTreeMap<Id, Option<Mailbox>> = BTreeMap::new();
    let mut not_updated: BTreeMap<Id, SetError> = BTreeMap::new();
    let mut to_update: Vec<(Id, Mailbox)> = Vec::new();
    for (id, patch) in request.update.unwrap_or_default() {
        let Some(existing) = tree.get(&id) else {
            not_updated.insert(id, SetError::new(error::set::NOT_FOUND));
            continue;
        };
        // Keep pre-patch `shareWith` to diff share notifications.
        let old_share_with = existing.share_with.clone();
        let Some(patch_map) = patch.as_object() else {
            not_updated.insert(id, SetError::new(error::set::INVALID_PATCH));
            continue;
        };
        let mut value = serde_json::to_value(existing).map_err(|e| {
            MethodError::new(error::method::SERVER_FAIL).with_description(e.to_string())
        })?;
        let patched = match apply_patch(&mut value, patch_map)
            .map_err(|message| SetError::new(error::set::INVALID_PATCH).with_description(message))
            .and_then(|()| {
                serde_json::from_value::<Mailbox>(value).map_err(|e| {
                    SetError::new(error::set::INVALID_PATCH).with_description(e.to_string())
                })
            }) {
            Ok(patched) => patched,
            Err(set_error) => {
                not_updated.insert(id, set_error);
                continue;
            }
        };
        if patched.id.as_ref() != Some(&id) {
            not_updated.insert(
                id,
                SetError::new(error::set::INVALID_PROPERTIES).with_description("id is immutable"),
            );
            continue;
        }
        if let Err(refusal) = placeable(&tree, Some(&id), &patched) {
            not_updated.insert(id, refusal);
            continue;
        }
        tree.insert(id.clone(), patched.clone());
        crate::principals::record_share_changes(
            account,
            jmap_proto::principals::share_notification_object_type::MAILBOX,
            &id,
            &request.account_id,
            old_share_with.as_ref(),
            patched.share_with.as_ref(),
        );
        to_update.push((id, patched));
    }

    let mut destroyed: Vec<Id> = Vec::new();
    let mut not_destroyed: BTreeMap<Id, SetError> = BTreeMap::new();
    for id in request.destroy.unwrap_or_default() {
        if !tree.contains_key(&id) {
            not_destroyed.insert(id, SetError::new(error::set::NOT_FOUND));
            continue;
        }
        if tree
            .values()
            .any(|mailbox| mailbox.parent_id.as_ref() == Some(&id))
        {
            not_destroyed.insert(
                id,
                SetError::new(jmap_proto::mail::mailbox_set_error::HAS_CHILD),
            );
            continue;
        }
        // Reject non-empty mailboxes with `hasEmail`.
        // This mock does not support `onDestroyRemoveEmails`.
        if account.emails.iter().any(|(_, email)| filed_in(email, &id)) {
            not_destroyed.insert(
                id,
                SetError::new(jmap_proto::mail::mailbox_set_error::HAS_EMAIL),
            );
            continue;
        }
        tree.remove(&id);
        destroyed.push(id);
    }

    account.mailboxes.transaction(|transaction| {
        for (id, mailbox) in to_create {
            transaction.create(id, mailbox);
        }
        for (id, mailbox) in to_update {
            transaction.update(&id, mailbox);
            updated.insert(id, None);
        }
        for id in &destroyed {
            transaction.destroy(id);
        }
    });

    to_result(&SetResponse {
        account_id: request.account_id,
        old_state: Some(old_state),
        new_state: Some(account.mailboxes.state()),
        created: (!created.is_empty()).then_some(created),
        updated: (!updated.is_empty()).then_some(updated),
        destroyed: (!destroyed.is_empty()).then_some(destroyed),
        not_created: (!not_created.is_empty()).then_some(not_created),
        not_updated: (!not_updated.is_empty()).then_some(not_updated),
        not_destroyed: (!not_destroyed.is_empty()).then_some(not_destroyed),
    })
}

/// Validate RFC 8621 §2 mailbox placement rules for `of` (`None` on create).
fn placeable(
    tree: &BTreeMap<Id, Mailbox>,
    of: Option<&Id>,
    mailbox: &Mailbox,
) -> Result<(), SetError> {
    if mailbox.name.is_empty() {
        return Err(SetError::new(error::set::INVALID_PROPERTIES)
            .with_description("name must not be empty"));
    }
    if let Some(parent) = &mailbox.parent_id {
        if !tree.contains_key(parent) {
            return Err(SetError::new(error::set::INVALID_PROPERTIES)
                .with_description(format!("parentId names {parent}, which is not a mailbox")));
        }
        if let Some(of) = of {
            // Walk parent links with bounded steps to avoid loops.
            let mut above = Some(parent.clone());
            for _ in 0..=tree.len() {
                let Some(here) = above else { break };
                if &here == of {
                    return Err(SetError::new(error::set::INVALID_PROPERTIES)
                        .with_description("a mailbox cannot be inside itself"));
                }
                above = tree
                    .get(&here)
                    .and_then(|mailbox| mailbox.parent_id.clone());
            }
        }
    }
    let elsewhere = |id: &Id| of != Some(id);
    if tree.iter().any(|(id, sibling)| {
        elsewhere(id) && sibling.parent_id == mailbox.parent_id && sibling.name == mailbox.name
    }) {
        return Err(
            SetError::new(error::set::INVALID_PROPERTIES).with_description(format!(
                "another mailbox of the same parent is already named {}",
                mailbox.name
            )),
        );
    }
    if let Some(role) = &mailbox.role
        && tree
            .iter()
            .any(|(id, other)| elsewhere(id) && other.role.as_ref() == Some(role))
    {
        return Err(SetError::new(error::set::INVALID_PROPERTIES)
            .with_description(format!("another mailbox already has the role {role}")));
    }
    Ok(())
}

/// Return a new mailbox with explicit zero counters.
fn counted_empty(mailbox: &Mailbox) -> Mailbox {
    Mailbox {
        total_emails: Some(0),
        unread_emails: Some(0),
        total_threads: Some(0),
        unread_threads: Some(0),
        ..mailbox.clone()
    }
}

/// Whether `email` is filed in mailbox `id` (`mailboxIds[id] == true`).
fn filed_in(email: &Email, id: &Id) -> bool {
    email
        .mailbox_ids
        .as_ref()
        .is_some_and(|mailbox_ids| mailbox_ids.get(id).copied().unwrap_or(false))
}

pub fn email_get(state: &mut ServerState, arguments: Value) -> Result<Value, MethodError> {
    let request: GetRequest = parse_arguments(arguments)?;

    let limit = state.objects_in_get();
    if request
        .ids
        .as_ref()
        .is_some_and(|ids| ids.len() as u64 > limit)
    {
        return Err(MethodError::new(error::method::REQUEST_TOO_LARGE)
            .with_description(format!("Email/get accepts at most {limit} ids")));
    }

    let include_unrequested = state.email_get_includes_unrequested;
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

    if include_unrequested && let Some(requested_ids) = &request.ids {
        for (id, email) in account.emails.iter() {
            if requested_ids.contains(id) {
                continue;
            }
            list.push(project_properties(email, request.properties.as_deref())?);
        }
    }

    to_result(&GetResponse {
        account_id: request.account_id,
        state: account.emails.state(),
        list,
        not_found,
    })
}

/// Assign the `threadId` for this one-email-per-thread mock.
fn new_thread_id(email_id: &Id) -> Id {
    Id::new(format!("T{}", email_id.as_str()))
}

/// Handle `Thread/get` (RFC 8621 §3.1): group by `threadId`.
/// Sort by `receivedAt` from oldest to newest.
pub fn thread_get(state: &mut ServerState, arguments: Value) -> Result<Value, MethodError> {
    let request: GetRequest = parse_arguments(arguments)?;
    let account = account_mut(state, &request.account_id)?;

    let requested: Vec<Id> = match &request.ids {
        Some(ids) => ids.clone(),
        None => {
            let mut ids: Vec<Id> = account
                .emails
                .iter()
                .filter_map(|(_, email)| email.thread_id.clone())
                .collect();
            ids.sort();
            ids.dedup();
            ids
        }
    };

    let mut list = Vec::new();
    let mut not_found = Vec::new();
    for thread_id in requested {
        let mut members: Vec<&Email> = account
            .emails
            .iter()
            .filter(|(_, email)| email.thread_id.as_ref() == Some(&thread_id))
            .map(|(_, email)| email)
            .collect();
        if members.is_empty() {
            not_found.push(thread_id);
            continue;
        }
        members.sort_by(|a, b| a.received_at.cmp(&b.received_at));
        let email_ids = members.into_iter().filter_map(|email| email.id.clone());
        list.push(Thread::new(thread_id, email_ids));
    }

    to_result(&GetResponse {
        account_id: request.account_id,
        state: account.emails.state(),
        list,
        not_found,
    })
}

/// Handle `SearchSnippet/get` (RFC 8621 §5.1).
/// Highlight filter terms in the subject and body.
pub fn search_snippet_get(state: &mut ServerState, arguments: Value) -> Result<Value, MethodError> {
    let request: SearchSnippetGetRequest = parse_arguments(arguments)?;
    let account = account_mut(state, &request.account_id)?;

    let mut list = Vec::new();
    let mut not_found = Vec::new();
    for email_id in request.email_ids {
        let Some(email) = account.emails.get(&email_id) else {
            not_found.push(email_id);
            continue;
        };
        list.push(match &request.filter {
            Some(filter) => search_snippet(email_id, email, filter),
            None => SearchSnippet::new(email_id),
        });
    }

    to_result(&SearchSnippetGetResponse {
        account_id: request.account_id,
        list,
        not_found: (!not_found.is_empty()).then_some(not_found),
    })
}

/// Preview byte cap after text and HTML escaping (RFC 8621 §5).
const PREVIEW_MAX_OCTETS: usize = 255;

fn search_snippet(email_id: Id, email: &Email, filter: &Filter<EmailQueryFilter>) -> SearchSnippet {
    let mut subject_terms = Vec::new();
    let mut body_terms = Vec::new();
    collect_snippet_terms(filter, &mut subject_terms, &mut body_terms);

    let subject = email
        .subject
        .as_deref()
        .filter(|subject| subject_terms.iter().any(|term| subject.contains(term)))
        .map(|subject| mark_matches(subject, &subject_terms));

    let preview = email.body_values.as_ref().and_then(|values| {
        values
            .values()
            .find(|value| body_terms.iter().any(|term| value.value.contains(term)))
            .map(|value| {
                truncate_octets(&mark_matches(&value.value, &body_terms), PREVIEW_MAX_OCTETS)
            })
    });

    let mut snippet = SearchSnippet::new(email_id);
    snippet.subject = subject;
    snippet.preview = preview;
    snippet
}

/// Collect snippet highlight terms from `subject`, `body`, and `text` leaves.
fn collect_snippet_terms<'a>(
    filter: &'a Filter<EmailQueryFilter>,
    subject_terms: &mut Vec<&'a str>,
    body_terms: &mut Vec<&'a str>,
) {
    match filter {
        Filter::Condition(condition) => {
            if let Some(subject) = &condition.subject {
                subject_terms.push(subject.as_str());
            }
            if let Some(body) = &condition.body {
                body_terms.push(body.as_str());
            }
            if let Some(text) = &condition.text {
                subject_terms.push(text.as_str());
                body_terms.push(text.as_str());
            }
        }
        Filter::Operator(op) => {
            for condition in &op.conditions {
                collect_snippet_terms(condition, subject_terms, body_terms);
            }
        }
    }
}

/// Escape `text` as HTML and wrap longest non-overlapping term matches in `<mark>`.
fn mark_matches(text: &str, terms: &[&str]) -> String {
    let mut result = String::new();
    let mut rest = text;
    while !rest.is_empty() {
        let longest_match = terms
            .iter()
            .filter(|term| !term.is_empty() && rest.starts_with(**term))
            .max_by_key(|term| term.len());
        match longest_match {
            Some(term) => {
                result.push_str("<mark>");
                escape_html_into(term, &mut result);
                result.push_str("</mark>");
                rest = &rest[term.len()..];
            }
            None => {
                let ch = rest.chars().next().expect("rest is non-empty");
                escape_html_into(&rest[..ch.len_utf8()], &mut result);
                rest = &rest[ch.len_utf8()..];
            }
        }
    }
    result
}

fn escape_html_into(text: &str, out: &mut String) {
    for ch in text.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            other => out.push(other),
        }
    }
}

/// Truncate `text` to at most `max_octets` bytes without splitting UTF-8.
fn truncate_octets(text: &str, max_octets: usize) -> String {
    if text.len() <= max_octets {
        return text.to_owned();
    }
    let mut end = max_octets;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    text[..end].to_owned()
}

pub fn email_query(state: &mut ServerState, arguments: Value) -> Result<Value, MethodError> {
    let request: QueryRequest<Filter<EmailQueryFilter>> = parse_arguments(arguments)?;
    let page_size = state.query_page_size;
    let hides_limit = state.query_hides_limit;
    let query_never_terminates = state.query_never_terminates;
    let account = account_mut(state, &request.account_id)?;

    let filter = request
        .filter
        .unwrap_or_else(|| Filter::condition(EmailQueryFilter::default()));
    let mut matches: Vec<(&Id, &Email)> = account
        .emails
        .iter()
        .filter(|(_, email)| filter_matches(email, &filter))
        .collect();

    // Default sort uses `receivedAt` in descending order.
    let mut sort_property = "receivedAt".to_owned();
    let mut ascending = false;
    if let Some(comparators) = &request.sort
        && let Some(first) = comparators.first()
    {
        sort_property = first.property.clone();
        ascending = first.is_ascending;
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
    let asked = request.limit.unwrap_or(u64::MAX);
    let never_terminates = query_never_terminates && !matches.is_empty();
    let ids: Vec<Id> = if never_terminates {
        // Broken-server mode repeats a non-empty page and ignores `position`.
        matches
            .iter()
            .map(|(id, _)| (*id).clone())
            .cycle()
            .take(asked.min(page_size.unwrap_or(1)) as usize)
            .collect()
    } else {
        matches
            .into_iter()
            .map(|(id, _)| id.clone())
            .skip(position)
            .take(asked.min(page_size.unwrap_or(u64::MAX)) as usize)
            .collect()
    };
    let capped = never_terminates || page_size.is_some_and(|page_size| page_size < asked);

    to_result(&QueryResponse {
        account_id: request.account_id,
        query_state: account.emails.state(),
        can_calculate_changes: false,
        position: position as u64,
        ids,
        total: request.calculate_total.then_some(total),
        limit: (capped && !hides_limit).then(|| page_size.unwrap_or_default()),
    })
}

/// Evaluate a `Filter<EmailQueryFilter>` tree per RFC 8620 §5.5.
/// `NOT` negates nested matches.
fn filter_matches(email: &Email, filter: &Filter<EmailQueryFilter>) -> bool {
    match filter {
        Filter::Condition(condition) => email_matches(email, condition),
        Filter::Operator(op) => match op.operator.as_str() {
            operator::AND => op.conditions.iter().all(|c| filter_matches(email, c)),
            operator::OR => op.conditions.iter().any(|c| filter_matches(email, c)),
            operator::NOT => !op.conditions.iter().any(|c| filter_matches(email, c)),
            _ => false,
        },
    }
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
    if let Some(cc) = &filter.cc
        && !address_list_contains(email.cc.as_deref(), cc)
    {
        return false;
    }
    if let Some(bcc) = &filter.bcc
        && !address_list_contains(email.bcc.as_deref(), bcc)
    {
        return false;
    }
    if let Some(min_size) = filter.min_size
        && email.size.is_none_or(|size| size < min_size)
    {
        return false;
    }
    if let Some(max_size) = filter.max_size
        && email.size.is_none_or(|size| size > max_size)
    {
        return false;
    }
    if let Some(has_attachment) = filter.has_attachment
        && email.has_attachment.unwrap_or(false) != has_attachment
    {
        return false;
    }
    if let Some(body) = &filter.body
        && !body_contains(email, body)
    {
        return false;
    }
    if let Some(text) = &filter.text
        && !(address_list_contains(email.from.as_deref(), text)
            || address_list_contains(email.to.as_deref(), text)
            || address_list_contains(email.cc.as_deref(), text)
            || address_list_contains(email.bcc.as_deref(), text)
            || email
                .subject
                .as_ref()
                .is_some_and(|subject| subject.contains(text.as_str()))
            || body_contains(email, text))
    {
        return false;
    }
    if let Some(header) = &filter.header
        && !header_matches(email, header)
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

/// RFC 8621 §4.4.1 `body`: check text/plain and text/html values only.
fn body_contains(email: &Email, needle: &str) -> bool {
    email
        .body_values
        .as_ref()
        .is_some_and(|values| values.values().any(|value| value.value.contains(needle)))
}

/// RFC 8621 §4.4.1 `header`: names are case-insensitive; optional value checks substring.
fn header_matches(email: &Email, header: &[String]) -> bool {
    let Some(name) = header.first() else {
        return false;
    };
    email.headers.as_ref().is_some_and(|headers| {
        headers.iter().any(|candidate| {
            candidate.name.eq_ignore_ascii_case(name)
                && header
                    .get(1)
                    .is_none_or(|value| candidate.value.contains(value.as_str()))
        })
    })
}

pub fn identity_get(state: &mut ServerState, arguments: Value) -> Result<Value, MethodError> {
    let request: GetRequest = parse_arguments(arguments)?;
    let account = account_mut(state, &request.account_id)?;

    let mut list = Vec::new();
    let mut not_found = Vec::new();
    match &request.ids {
        None => list.extend(
            account
                .identities
                .iter()
                .map(|(_, identity)| identity.clone()),
        ),
        Some(ids) => {
            for id in ids {
                match account.identities.get(id) {
                    Some(identity) => list.push(identity.clone()),
                    None => not_found.push(id.clone()),
                }
            }
        }
    }

    to_result(&GetResponse {
        account_id: request.account_id,
        state: account.identities.state(),
        list,
        not_found,
    })
}

/// `VacationResponse/get` (RFC 8621 §8.1): singleton always exists.
pub fn vacation_response_get(
    state: &mut ServerState,
    arguments: Value,
) -> Result<Value, MethodError> {
    let request: GetRequest = parse_arguments(arguments)?;
    let account = account_mut(state, &request.account_id)?;

    let mut list = Vec::new();
    let mut not_found = Vec::new();
    match &request.ids {
        None => list.extend(
            account
                .vacation_response
                .iter()
                .map(|(_, vacation)| vacation.clone()),
        ),
        Some(ids) => {
            for id in ids {
                match account.vacation_response.get(id) {
                    Some(vacation) => list.push(vacation.clone()),
                    None => not_found.push(id.clone()),
                }
            }
        }
    }

    to_result(&GetResponse {
        account_id: request.account_id,
        state: account.vacation_response.state(),
        list,
        not_found,
    })
}

/// `VacationResponse/set` (RFC 8621 §8.1): only `update` mutates state.
pub fn vacation_response_set(
    state: &mut ServerState,
    arguments: Value,
) -> Result<Value, MethodError> {
    let request: SetRequest<VacationResponse> = parse_arguments(arguments)?;
    let account = account_mut(state, &request.account_id)?;

    let old_state = account.vacation_response.state();
    if let Some(expected) = &request.if_in_state
        && expected != &old_state
    {
        return Err(MethodError::new(error::method::STATE_MISMATCH));
    }

    let not_created: BTreeMap<String, SetError> = request
        .create
        .unwrap_or_default()
        .into_keys()
        .map(|creation_id| {
            (
                creation_id,
                SetError::new(error::set::SINGLETON)
                    .with_description("VacationResponse cannot be created, it always exists"),
            )
        })
        .collect();
    let not_destroyed: BTreeMap<Id, SetError> = request
        .destroy
        .unwrap_or_default()
        .into_iter()
        .map(|id| {
            (
                id,
                SetError::new(error::set::SINGLETON)
                    .with_description("VacationResponse cannot be destroyed"),
            )
        })
        .collect();

    let mut not_updated: BTreeMap<Id, SetError> = BTreeMap::new();
    let mut to_update: Vec<(Id, VacationResponse)> = Vec::new();
    for (id, patch) in request.update.unwrap_or_default() {
        let Some(existing) = account.vacation_response.get(&id) else {
            not_updated.insert(id, SetError::new(error::set::NOT_FOUND));
            continue;
        };
        let Some(patch_map) = patch.as_object() else {
            not_updated.insert(id, SetError::new(error::set::INVALID_PATCH));
            continue;
        };
        let mut value = serde_json::to_value(existing).map_err(|e| {
            MethodError::new(error::method::SERVER_FAIL).with_description(e.to_string())
        })?;
        let patched = match apply_patch(&mut value, patch_map)
            .map_err(|message| SetError::new(error::set::INVALID_PATCH).with_description(message))
            .and_then(|()| {
                serde_json::from_value::<VacationResponse>(value).map_err(|e| {
                    SetError::new(error::set::INVALID_PATCH).with_description(e.to_string())
                })
            }) {
            Ok(patched) => patched,
            Err(set_error) => {
                not_updated.insert(id, set_error);
                continue;
            }
        };
        if patched.id.as_ref() != Some(&id) {
            not_updated.insert(
                id,
                SetError::new(error::set::INVALID_PROPERTIES).with_description("id is immutable"),
            );
            continue;
        }
        to_update.push((id, patched));
    }

    let mut updated: BTreeMap<Id, Option<VacationResponse>> = BTreeMap::new();
    account.vacation_response.transaction(|transaction| {
        for (id, patched) in to_update {
            transaction.update(&id, patched);
            updated.insert(id, None);
        }
    });

    to_result(&SetResponse {
        account_id: request.account_id,
        old_state: Some(old_state),
        new_state: Some(account.vacation_response.state()),
        created: None,
        updated: (!updated.is_empty()).then_some(updated),
        destroyed: None,
        not_created: (!not_created.is_empty()).then_some(not_created),
        not_updated: (!not_updated.is_empty()).then_some(not_updated),
        not_destroyed: (!not_destroyed.is_empty()).then_some(not_destroyed),
    })
}

/// Validate `Email.snoozed`: requires the snooze extension and snoozed mailbox.
fn snooze_refusal(
    snooze_extension: bool,
    snoozed_mailbox: Option<&Id>,
    email: &Email,
) -> Option<SetError> {
    email.snoozed.as_ref()?;
    if !snooze_extension {
        return Some(
            SetError::new(error::set::INVALID_PROPERTIES)
                .with_description("snoozed needs the server's snooze extension")
                .with_properties(["snoozed"]),
        );
    }
    let filed_in_snoozed = snoozed_mailbox.is_some_and(|snoozed_id| {
        email
            .mailbox_ids
            .as_ref()
            .is_some_and(|mailboxes| mailboxes.get(snoozed_id) == Some(&true))
    });
    if filed_in_snoozed {
        None
    } else {
        Some(
            SetError::new(error::set::INVALID_PROPERTIES)
                .with_description("a snoozed message must sit in the snoozed-role mailbox")
                .with_properties(["mailboxIds", "snoozed"]),
        )
    }
}

pub fn email_set(state: &mut ServerState, arguments: Value) -> Result<Value, MethodError> {
    let request: SetRequest<Email> = parse_arguments(arguments)?;
    let snooze_extension = state.snooze_extension;
    let account = account_mut(state, &request.account_id)?;
    let snoozed_mailbox: Option<Id> = account
        .mailboxes
        .iter()
        .find(|(_, mailbox)| mailbox.role.as_deref() == Some(role::SNOOZED))
        .map(|(id, _)| id.clone());

    let old_state = account.emails.state();
    if let Some(expected) = &request.if_in_state
        && expected != &old_state
    {
        return Err(MethodError::new(error::method::STATE_MISMATCH));
    }

    let mut created: BTreeMap<String, Email> = BTreeMap::new();
    let mut not_created: BTreeMap<String, SetError> = BTreeMap::new();

    // Allocate server-set ids and blobs before the transaction.
    let mut to_create: Vec<(Id, Email)> = Vec::new();
    let mut to_create_threads: Vec<(Id, Thread)> = Vec::new();
    for (creation_id, mut email) in request.create.unwrap_or_default() {
        if let Err(refusal) = filed_somewhere(account, &email) {
            not_created.insert(creation_id, refusal);
            continue;
        }
        if let Some(refusal) = snooze_refusal(snooze_extension, snoozed_mailbox.as_ref(), &email) {
            not_created.insert(creation_id, refusal);
            continue;
        }
        let id = account.emails.alloc_id();
        let thread_id = new_thread_id(&id);
        let blob_id = account.add_blob("message/rfc822", Vec::new());
        let size = email
            .body_values
            .as_ref()
            .map(|values| values.values().map(|value| value.value.len() as u64).sum())
            .unwrap_or(0);
        email.id = Some(id.clone());
        email.blob_id = Some(blob_id.clone());
        email.thread_id = Some(thread_id.clone());
        email.size = Some(size);
        if email.received_at.is_none() {
            email.received_at = Some(UtcDate::new(MOCK_NOW));
        }

        created.insert(
            creation_id,
            Email {
                id: Some(id.clone()),
                blob_id: Some(blob_id),
                thread_id: email.thread_id.clone(),
                size: Some(size),
                received_at: email.received_at.clone(),
                ..Email::default()
            },
        );
        to_create_threads.push((thread_id.clone(), Thread::new(thread_id, [id.clone()])));
        to_create.push((id, email));
    }

    let mut updated: BTreeMap<Id, Option<Email>> = BTreeMap::new();
    let mut not_updated: BTreeMap<Id, SetError> = BTreeMap::new();
    let mut destroyed: Vec<Id> = Vec::new();
    let mut not_destroyed: BTreeMap<Id, SetError> = BTreeMap::new();

    // Patch emails before the transaction, then apply one state bump.
    let mut to_update: Vec<(Id, Email)> = Vec::new();
    for (id, patch) in request.update.unwrap_or_default() {
        let Some(existing) = account.emails.get(&id) else {
            not_updated.insert(id, SetError::new(error::set::NOT_FOUND));
            continue;
        };
        let Some(patch_map) = patch.as_object() else {
            not_updated.insert(id, SetError::new(error::set::INVALID_PATCH));
            continue;
        };
        let mut value = serde_json::to_value(existing).map_err(|e| {
            MethodError::new(error::method::SERVER_FAIL).with_description(e.to_string())
        })?;
        match apply_patch(&mut value, patch_map)
            .map_err(|message| SetError::new(error::set::INVALID_PATCH).with_description(message))
            .and_then(|()| {
                serde_json::from_value::<Email>(value).map_err(|e| {
                    SetError::new(error::set::INVALID_PATCH).with_description(e.to_string())
                })
            }) {
            Ok(patched) => match filed_somewhere(account, &patched)
                .err()
                .or_else(|| snooze_refusal(snooze_extension, snoozed_mailbox.as_ref(), &patched))
            {
                None => to_update.push((id, patched)),
                Some(refusal) => {
                    not_updated.insert(id, refusal);
                }
            },
            Err(set_error) => {
                not_updated.insert(id, set_error);
            }
        }
    }

    let destroy_ids = request.destroy.unwrap_or_default();
    let destroy_thread_ids: BTreeMap<Id, Id> = destroy_ids
        .iter()
        .filter_map(|id| {
            account
                .emails
                .get(id)
                .and_then(|email| email.thread_id.clone())
                .map(|thread_id| (id.clone(), thread_id))
        })
        .collect();

    account.emails.transaction(|transaction| {
        for (id, email) in to_create {
            transaction.create(id, email);
        }
        for (id, email) in to_update {
            transaction.update(&id, email);
            updated.insert(id, None);
        }
        for id in destroy_ids {
            if transaction.destroy(&id) {
                destroyed.push(id);
            } else {
                not_destroyed.insert(id, SetError::new(error::set::NOT_FOUND));
            }
        }
    });

    // Keep `threads` aligned with `emails` in this one-thread-per-email mock.
    account.threads.transaction(|transaction| {
        for (id, thread) in to_create_threads {
            transaction.create(id, thread);
        }
        for id in &destroyed {
            if let Some(thread_id) = destroy_thread_ids.get(id) {
                transaction.destroy(thread_id);
            }
        }
    });

    to_result(&SetResponse {
        account_id: request.account_id,
        old_state: Some(old_state),
        new_state: Some(account.emails.state()),
        created: (!created.is_empty()).then_some(created),
        updated: (!updated.is_empty()).then_some(updated),
        destroyed: (!destroyed.is_empty()).then_some(destroyed),
        not_created: (!not_created.is_empty()).then_some(not_created),
        not_updated: (!not_updated.is_empty()).then_some(not_updated),
        not_destroyed: (!not_destroyed.is_empty()).then_some(not_destroyed),
    })
}

/// Whether `email` is in at least one existing mailbox in this account.
fn filed_somewhere(account: &AccountState, email: &Email) -> Result<(), SetError> {
    let Some(mailbox_ids) = email
        .mailbox_ids
        .as_ref()
        .filter(|mailbox_ids| mailbox_ids.values().any(|filed| *filed))
    else {
        return Err(SetError::new(error::set::INVALID_PROPERTIES)
            .with_description("mailboxIds must name at least one mailbox"));
    };
    match mailbox_ids
        .iter()
        .filter(|(_, filed)| **filed)
        .map(|(id, _)| id)
        .find(|id| !account.mailboxes.contains(id))
    {
        Some(unknown) => Err(
            SetError::new(error::set::INVALID_PROPERTIES).with_description(format!(
                "mailboxIds names {unknown}, which is not a mailbox"
            )),
        ),
        None => Ok(()),
    }
}

/// `Email/import` (RFC 8621 §4.8): parse uploaded blobs and keep blob bytes.
pub fn email_import(state: &mut ServerState, arguments: Value) -> Result<Value, MethodError> {
    let request: EmailImportRequest = parse_arguments(arguments)?;
    let account = account_mut(state, &request.account_id)?;

    let old_state = account.emails.state();
    if let Some(expected) = &request.if_in_state
        && expected != &old_state
    {
        return Err(MethodError::new(error::method::STATE_MISMATCH));
    }

    let mut created: BTreeMap<String, Email> = BTreeMap::new();
    let mut not_created: BTreeMap<String, SetError> = BTreeMap::new();
    let mut to_create: Vec<(Id, Email)> = Vec::new();

    for (creation_id, import) in request.emails {
        match imported(account, &import) {
            Ok(email) => {
                // Return only the four RFC-selected server fields.
                created.insert(
                    creation_id,
                    Email {
                        id: email.id.clone(),
                        blob_id: email.blob_id.clone(),
                        thread_id: email.thread_id.clone(),
                        size: email.size,
                        ..Email::default()
                    },
                );
                to_create.push((email.id.clone().expect("imported email has an id"), email));
            }
            Err(refusal) => {
                not_created.insert(creation_id, refusal);
            }
        }
    }

    let to_create_threads: Vec<(Id, Thread)> = to_create
        .iter()
        .map(|(id, email)| {
            let thread_id = email
                .thread_id
                .clone()
                .expect("imported email has a threadId");
            (thread_id.clone(), Thread::new(thread_id, [id.clone()]))
        })
        .collect();

    account.emails.transaction(|transaction| {
        for (id, email) in to_create {
            transaction.create(id, email);
        }
    });
    account.threads.transaction(|transaction| {
        for (id, thread) in to_create_threads {
            transaction.create(id, thread);
        }
    });

    to_result(&EmailImportResponse {
        account_id: request.account_id,
        old_state: Some(old_state),
        new_state: account.emails.state(),
        created: (!created.is_empty()).then_some(created),
        not_created: (!not_created.is_empty()).then_some(not_created),
    })
}

/// `Email/parse` (RFC 8621 §4.8): parse an uploaded blob without storing it.
pub fn email_parse(state: &mut ServerState, arguments: Value) -> Result<Value, MethodError> {
    let request: EmailParseRequest = parse_arguments(arguments)?;
    let account = account_mut(state, &request.account_id)?;

    let mut parsed = serde_json::Map::new();
    let mut not_found = Vec::new();
    let mut not_parsable = Vec::new();
    for id in &request.blob_ids {
        let Some(blob) = account.blobs.get(id) else {
            not_found.push(id.clone());
            continue;
        };
        let Some(message) = crate::message::Message::read(&blob.data) else {
            not_parsable.push(id.clone());
            continue;
        };
        let email = Email {
            blob_id: Some(id.clone()),
            size: Some(blob.data.len() as u64),
            message_id: message.message_ids("message-id"),
            in_reply_to: message.message_ids("in-reply-to"),
            references: message.message_ids("references"),
            from: message.addresses("from"),
            to: message.addresses("to"),
            cc: message.addresses("cc"),
            subject: message.subject(),
            preview: message.preview(),
            ..Email::default()
        };
        parsed.insert(
            id.to_string(),
            project_properties(&email, request.properties.as_deref())?,
        );
    }

    to_result(&serde_json::json!({
        "accountId": request.account_id,
        "parsed": (!parsed.is_empty()).then_some(Value::Object(parsed)),
        "notParsable": (!not_parsable.is_empty()).then_some(not_parsable),
        "notFound": (!not_found.is_empty()).then_some(not_found),
    }))
}

/// Build an `Email` for one import request, or return the refusal.
fn imported(
    account: &mut AccountState,
    import: &jmap_proto::mail::EmailImport,
) -> Result<Email, SetError> {
    let Some(blob_id) = import.blob_id.clone() else {
        return Err(
            SetError::new(error::set::INVALID_PROPERTIES).with_description("blobId must be given")
        );
    };
    let Some(blob) = account.blobs.get(&blob_id) else {
        return Err(SetError::new(error::set::INVALID_PROPERTIES)
            .with_description(format!("blobId names {blob_id}, which is not a blob")));
    };
    let size = blob.data.len() as u64;
    let Some(message) = crate::message::Message::read(&blob.data) else {
        return Err(SetError::new(email_import_error::INVALID_EMAIL)
            .with_description("the blob is not an RFC 5322 message"));
    };

    let mut email = Email {
        blob_id: Some(blob_id),
        mailbox_ids: import.mailbox_ids.clone(),
        keywords: Some(import.keywords.clone().unwrap_or_default()),
        size: Some(size),
        received_at: Some(
            import
                .received_at
                .clone()
                .unwrap_or_else(|| UtcDate::new(MOCK_NOW)),
        ),
        message_id: message.message_ids("message-id"),
        in_reply_to: message.message_ids("in-reply-to"),
        references: message.message_ids("references"),
        from: message.addresses("from"),
        to: message.addresses("to"),
        cc: message.addresses("cc"),
        subject: message.subject(),
        preview: message.preview(),
        ..Email::default()
    };
    // Reuse the same mailboxIds validation path as Email/set.
    filed_somewhere(account, &email)?;

    let id = account.emails.alloc_id();
    email.thread_id = Some(new_thread_id(&id));
    email.id = Some(id);
    Ok(email)
}

/// Parse FUTURERELEASE hold delay from envelope parameters (RFC 4865).
fn hold_seconds(envelope: Option<&Envelope>) -> Result<Option<i64>, String> {
    let Some(Value::Object(parameters)) =
        envelope.and_then(|envelope| envelope.mail_from.parameters.as_ref())
    else {
        return Ok(None);
    };

    let mut hold_for = None;
    let mut hold_until = None;
    for (name, value) in parameters {
        if name.eq_ignore_ascii_case("HOLDFOR") {
            hold_for = Some(value);
        } else if name.eq_ignore_ascii_case("HOLDUNTIL") {
            hold_until = Some(value);
        }
    }

    match (hold_for, hold_until) {
        (None, None) => Ok(None),
        (Some(_), Some(_)) => {
            Err("HOLDFOR and HOLDUNTIL are mutually exclusive (RFC 4865 §3)".to_owned())
        }
        (Some(value), None) => value
            .as_str()
            .and_then(|seconds| seconds.parse::<i64>().ok())
            .filter(|seconds| (0..=99_999_999).contains(seconds))
            .map(Some)
            .ok_or_else(|| {
                "HOLDFOR takes seconds as a decimal string of at most 99999999".to_owned()
            }),
        (None, Some(value)) => {
            let until = value
                .as_str()
                .and_then(crate::time::parse_utc)
                .ok_or_else(|| "HOLDUNTIL takes an RFC 3339 UTC date-time".to_owned())?;
            let now = crate::time::parse_utc(MOCK_NOW).expect("MOCK_NOW parses");
            Ok(Some(until - now))
        }
    }
}

pub fn email_submission_set(
    state: &mut ServerState,
    arguments: Value,
    request_created_ids: &BTreeMap<String, Id>,
) -> Result<Value, MethodError> {
    let request: EmailSubmissionSetRequest = parse_arguments(arguments)?;
    let terse_submission_create = state.terse_submission_create;
    let max_delayed_send = state.max_delayed_send;
    let account = account_mut(state, &request.set.account_id)?;

    let old_state = account.submissions.state();
    if let Some(expected) = &request.set.if_in_state
        && expected != &old_state
    {
        return Err(MethodError::new(error::method::STATE_MISMATCH));
    }

    let mut created: BTreeMap<String, EmailSubmission> = BTreeMap::new();
    let mut not_created: BTreeMap<String, SetError> = BTreeMap::new();
    // Map creation IDs to submission IDs for `onSuccessUpdateEmail` `#` keys.
    let mut created_here: BTreeMap<String, Id> = BTreeMap::new();

    let mut to_create: Vec<(Id, EmailSubmission)> = Vec::new();
    for (creation_id, mut submission) in request.set.create.clone().unwrap_or_default() {
        // Resolve each `#creationId` against earlier `Email/set` creates.
        if let Some(reference) = submission.email_id.as_str().strip_prefix('#') {
            match request_created_ids.get(reference) {
                Some(id) => submission.email_id = id.clone(),
                None => {
                    not_created.insert(
                        creation_id,
                        SetError::new(error::set::INVALID_PROPERTIES)
                            .with_description(format!("unknown creation id #{reference}")),
                    );
                    continue;
                }
            }
        }

        if !account.identities.contains(&submission.identity_id) {
            not_created.insert(
                creation_id,
                SetError::new(error::set::INVALID_PROPERTIES)
                    .with_description("identityId does not exist"),
            );
            continue;
        }
        let Some(email) = account.emails.get(&submission.email_id) else {
            not_created.insert(
                creation_id,
                SetError::new(error::set::INVALID_PROPERTIES)
                    .with_description("emailId does not exist"),
            );
            continue;
        };

        // RFC 8621 §7.1: `sendAt` is server-set.
        if submission.send_at.is_some() {
            not_created.insert(
                creation_id,
                SetError::new(error::set::INVALID_PROPERTIES).with_description(
                    "sendAt is server-set; hold with a FUTURERELEASE envelope parameter",
                ),
            );
            continue;
        }

        // Apply SMTP FUTURERELEASE (RFC 4865). Enforce `maxDelayedSend`.
        let hold = match hold_seconds(submission.envelope.as_ref()) {
            Ok(hold) => hold,
            Err(description) => {
                not_created.insert(
                    creation_id,
                    SetError::new(error::set::INVALID_PROPERTIES).with_description(description),
                );
                continue;
            }
        };
        if let Some(seconds) = hold {
            let offered = max_delayed_send.unwrap_or(0);
            if offered == 0 {
                not_created.insert(
                    creation_id,
                    SetError::new(error::set::INVALID_PROPERTIES)
                        .with_description("the server offers no FUTURERELEASE"),
                );
                continue;
            }
            if seconds > 0 && seconds as u64 > offered {
                not_created.insert(
                    creation_id,
                    SetError::new(error::set::INVALID_PROPERTIES).with_description(format!(
                        "hold exceeds maxDelayedSend ({offered} seconds)"
                    )),
                );
                continue;
            }
        }

        // When absent, derive the SMTP envelope from the message (RFC 8621 §7).
        let envelope = submission.envelope.clone().unwrap_or_else(|| Envelope {
            mail_from: EnvelopeAddress::new(
                email
                    .from
                    .as_ref()
                    .and_then(|from| from.first())
                    .map(|address| address.email.clone())
                    .unwrap_or_default(),
            ),
            rcpt_to: [&email.to, &email.cc, &email.bcc]
                .into_iter()
                .flatten()
                .flat_map(|addresses| addresses.iter())
                .map(|address| EnvelopeAddress::new(address.email.clone()))
                .collect(),
        });

        let id = account.submissions.alloc_id();
        submission.id = Some(id.clone());
        submission.thread_id = email.thread_id.clone();
        submission.envelope = Some(envelope.clone());

        // Positive holds keep `pending` until `sendAt`. Others send now.
        match hold {
            Some(seconds) if seconds > 0 => {
                let now = crate::time::parse_utc(MOCK_NOW).expect("MOCK_NOW parses");
                submission.send_at = Some(UtcDate::new(crate::time::format_utc(now + seconds)));
                submission.undo_status = Some("pending".to_owned());
            }
            _ => {
                submission.send_at = Some(UtcDate::new(MOCK_NOW));
                submission.undo_status = Some("final".to_owned());
                account.outbox.push(RecordedSubmission {
                    id: id.clone(),
                    email_id: submission.email_id.clone(),
                    identity_id: submission.identity_id.clone(),
                    envelope,
                });
            }
        }

        created_here.insert(creation_id.clone(), id.clone());
        created.insert(creation_id, submission.clone());
        to_create.push((id, submission));
    }

    // RFC 8621 §7.4: only `undoStatus: pending -> canceled` is allowed.
    let mut updated: BTreeMap<Id, Option<EmailSubmission>> = BTreeMap::new();
    let mut not_updated: BTreeMap<Id, SetError> = BTreeMap::new();
    let mut to_update: Vec<(Id, EmailSubmission)> = Vec::new();
    for (id, patch) in request.set.update.clone().unwrap_or_default() {
        let Some(existing) = account.submissions.get(&id) else {
            not_updated.insert(id, SetError::new(error::set::NOT_FOUND));
            continue;
        };
        let wants_cancel = patch.as_object().is_some_and(|patch_map| {
            patch_map.len() == 1
                && patch_map.get("undoStatus").and_then(Value::as_str) == Some("canceled")
        });
        if !wants_cancel || existing.undo_status.as_deref() != Some("pending") {
            not_updated.insert(id, SetError::new(error::set::FORBIDDEN));
            continue;
        }
        let mut patched = existing.clone();
        patched.undo_status = Some("canceled".to_owned());
        to_update.push((id, patched));
    }

    account.submissions.transaction(|transaction| {
        for (id, submission) in to_create {
            transaction.create(id, submission);
        }
        for (id, submission) in to_update {
            transaction.update(&id, submission);
            updated.insert(id, None);
        }
    });

    // RFC 8621 §7.5: apply `onSuccessUpdateEmail` patches after successful sends.
    let mut email_patches: Vec<(Id, serde_json::Map<String, Value>)> = Vec::new();
    for (key, patch) in request.on_success_update_email.clone().unwrap_or_default() {
        let submission_id = match key.strip_prefix('#') {
            Some(reference) => match created_here.get(reference) {
                Some(id) => id.clone(),
                None => continue, // failed create — patch does not apply
            },
            None => Id::new(key),
        };
        let Some(email_id) = account
            .submissions
            .get(&submission_id)
            .map(|submission| submission.email_id.clone())
        else {
            continue;
        };
        if let Some(patch_map) = patch.as_object() {
            email_patches.push((email_id, patch_map.clone()));
        }
    }
    let mut patched: Vec<(Id, Email)> = Vec::new();
    for (email_id, patch_map) in &email_patches {
        let Some(email) = account.emails.get(email_id) else {
            continue;
        };
        let mut value = serde_json::to_value(email).map_err(|e| {
            MethodError::new(error::method::SERVER_FAIL).with_description(e.to_string())
        })?;
        if apply_patch(&mut value, patch_map).is_ok()
            && let Ok(updated) = serde_json::from_value::<Email>(value)
            && filed_somewhere(account, &updated).is_ok()
        {
            patched.push((email_id.clone(), updated));
        }
    }
    account.emails.transaction(|transaction| {
        for (id, email) in patched {
            transaction.update(&id, email);
        }
    });

    let mut result = to_result(&SetResponse {
        account_id: request.set.account_id,
        old_state: Some(old_state),
        new_state: Some(account.submissions.state()),
        created: (!created.is_empty()).then_some(created),
        updated: (!updated.is_empty()).then_some(updated),
        destroyed: None,
        not_created: (!not_created.is_empty()).then_some(not_created),
        not_updated: (!not_updated.is_empty()).then_some(not_updated),
        not_destroyed: None,
    })?;

    // RFC 8620 §5.3: `created` omits properties already sent by the client.
    if terse_submission_create
        && let Some(created) = result.get_mut("created").and_then(Value::as_object_mut)
    {
        for object in created.values_mut() {
            if let Some(object) = object.as_object_mut() {
                object.remove("identityId");
                object.remove("emailId");
            }
        }
    }

    Ok(result)
}

/// `EmailSubmission/get` (RFC 8621 §7.4). Missing ids stay in `notFound`.
pub fn email_submission_get(
    state: &mut ServerState,
    arguments: Value,
) -> Result<Value, MethodError> {
    let request: GetRequest = parse_arguments(arguments)?;
    let account = account_mut(state, &request.account_id)?;

    let mut list = Vec::new();
    let mut not_found = Vec::new();
    match &request.ids {
        None => list.extend(account.submissions.iter().map(|(_, s)| s.clone())),
        Some(ids) => {
            for id in ids {
                match account.submissions.get(id) {
                    Some(submission) => list.push(submission.clone()),
                    None => not_found.push(id.clone()),
                }
            }
        }
    }

    to_result(&GetResponse {
        account_id: request.account_id,
        state: account.submissions.state(),
        list,
        not_found,
    })
}

/// `EmailSubmission/query` (RFC 8621 §7.3): filter by ids, `undoStatus`, or `sendAt` range.
pub fn email_submission_query(
    state: &mut ServerState,
    arguments: Value,
) -> Result<Value, MethodError> {
    let request: QueryRequest<EmailSubmissionQueryFilter> = parse_arguments(arguments)?;
    let account = account_mut(state, &request.account_id)?;

    let filter = request.filter.unwrap_or_default();
    let matches = |submission: &EmailSubmission| -> bool {
        if let Some(identity_ids) = &filter.identity_ids
            && !identity_ids.contains(&submission.identity_id)
        {
            return false;
        }
        if let Some(email_ids) = &filter.email_ids
            && !email_ids.contains(&submission.email_id)
        {
            return false;
        }
        if let Some(thread_ids) = &filter.thread_ids
            && !submission
                .thread_id
                .as_ref()
                .is_some_and(|thread_id| thread_ids.contains(thread_id))
        {
            return false;
        }
        if let Some(undo_status) = &filter.undo_status
            && submission.undo_status.as_deref() != Some(undo_status.as_str())
        {
            return false;
        }
        if let Some(before) = &filter.before
            && !submission.send_at.as_ref().is_some_and(|at| at < before)
        {
            return false;
        }
        if let Some(after) = &filter.after
            && !submission.send_at.as_ref().is_some_and(|at| at > after)
        {
            return false;
        }
        true
    };

    let ids: Vec<Id> = account
        .submissions
        .iter()
        .filter(|(_, submission)| matches(submission))
        .map(|(id, _)| id.clone())
        .skip(request.position.max(0) as usize)
        .take(request.limit.unwrap_or(u64::MAX) as usize)
        .collect();

    let total = account
        .submissions
        .iter()
        .filter(|(_, submission)| matches(submission))
        .count() as u64;

    to_result(&QueryResponse {
        account_id: request.account_id,
        query_state: account.submissions.state(),
        can_calculate_changes: false,
        position: request.position.max(0) as u64,
        ids,
        total: request.calculate_total.then_some(total),
        limit: None,
    })
}

/// Everything needed to seed one email into an account.
pub struct EmailSeed {
    pub mailbox_id: Id,
    pub from: EmailAddress,
    pub to: Vec<EmailAddress>,
    pub cc: Vec<EmailAddress>,
    pub bcc: Vec<EmailAddress>,
    pub subject: String,
    pub text_body: String,
    pub received_at: UtcDate,
    pub keywords: Vec<String>,
    pub headers: Vec<EmailHeader>,
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
            cc: Vec::new(),
            bcc: Vec::new(),
            subject: subject.to_owned(),
            text_body: text_body.to_owned(),
            received_at: UtcDate::new(received_at),
            keywords: Vec::new(),
            headers: Vec::new(),
            attachments: Vec::new(),
        }
    }

    pub fn keyword(mut self, keyword: &str) -> Self {
        self.keywords.push(keyword.to_owned());
        self
    }

    pub fn cc(mut self, name: &str, email: &str) -> Self {
        self.cc.push(EmailAddress::new(Some(name), email));
        self
    }

    pub fn bcc(mut self, name: &str, email: &str) -> Self {
        self.bcc.push(EmailAddress::new(Some(name), email));
        self
    }

    pub fn header(mut self, name: &str, value: &str) -> Self {
        self.headers.push(EmailHeader::new(name, value));
        self
    }

    pub fn attachment(mut self, blob_id: Id, name: &str, content_type: &str) -> Self {
        self.attachments
            .push((blob_id, name.to_owned(), content_type.to_owned()));
        self
    }
}

/// Build a single-part RFC 5322 `message/rfc822` blob for a seeded message.
fn rfc5322(id: &Id, seed: &EmailSeed) -> String {
    let mut message = String::new();
    message.push_str(&format!("From: {}\r\n", address(&seed.from)));
    if !seed.to.is_empty() {
        let to: Vec<String> = seed.to.iter().map(address).collect();
        message.push_str(&format!("To: {}\r\n", to.join(", ")));
    }
    if !seed.cc.is_empty() {
        let cc: Vec<String> = seed.cc.iter().map(address).collect();
        message.push_str(&format!("Cc: {}\r\n", cc.join(", ")));
    }
    if !seed.bcc.is_empty() {
        let bcc: Vec<String> = seed.bcc.iter().map(address).collect();
        message.push_str(&format!("Bcc: {}\r\n", bcc.join(", ")));
    }
    message.push_str(&format!("Subject: {}\r\n", seed.subject));
    if let Some(date) = rfc5322_date(seed.received_at.as_str()) {
        message.push_str(&format!("Date: {date}\r\n"));
    }
    message.push_str(&format!("Message-ID: <{id}@mock.invalid>\r\n"));
    message.push_str("MIME-Version: 1.0\r\n");
    message.push_str("Content-Type: text/plain; charset=utf-8\r\n");
    message.push_str("Content-Transfer-Encoding: 8bit\r\n");
    for header in &seed.headers {
        message.push_str(&format!("{}: {}\r\n", header.name, header.value));
    }
    message.push_str("\r\n");
    message.push_str(&seed.text_body.replace('\n', "\r\n"));
    message.push_str("\r\n");
    message
}

/// One address in message-header form; no RFC 2047 encoded names.
fn address(address: &EmailAddress) -> String {
    match &address.name {
        Some(name) => format!("{name} <{}>", address.email),
        None => address.email.clone(),
    }
}

/// Convert a JMAP `UtcDate` to RFC 5322 timestamp form, omitting weekday.
fn rfc5322_date(utc: &str) -> Option<String> {
    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    let (date, time) = utc.split_once('T')?;
    let time = time.strip_suffix('Z')?;
    let [year, month, day] = date.split('-').collect::<Vec<_>>()[..] else {
        return None;
    };
    let month = MONTHS.get(month.parse::<usize>().ok()?.checked_sub(1)?)?;
    if time.len() != 8 || year.len() != 4 || day.len() != 2 {
        return None;
    }
    Some(format!("{day} {month} {year} {time} +0000"))
}

impl AccountState {
    /// Seed a sending identity without bumping state.
    pub fn seed_identity(&mut self, name: &str, email: &str) -> Id {
        let id = self.identities.alloc_id();
        let identity = Identity {
            id: Some(id.clone()),
            name: name.to_owned(),
            email: email.to_owned(),
            may_delete: Some(false),
            ..Identity::default()
        };
        self.identities.seed_with_id(id.clone(), identity);
        id
    }

    /// Seed a top-level mailbox without bumping state.
    pub fn seed_mailbox(&mut self, name: &str, role: Option<&str>) -> Id {
        self.seed_mailbox_with_parent(name, role, None)
    }

    /// Seed a mailbox under `parent` without validating the parent id.
    pub fn seed_child_mailbox(&mut self, name: &str, role: Option<&str>, parent: &Id) -> Id {
        self.seed_mailbox_with_parent(name, role, Some(parent.clone()))
    }

    fn seed_mailbox_with_parent(
        &mut self,
        name: &str,
        role: Option<&str>,
        parent_id: Option<Id>,
    ) -> Id {
        let id = self.mailboxes.alloc_id();
        let mailbox = Mailbox {
            id: Some(id.clone()),
            name: name.to_owned(),
            parent_id,
            role: role.map(str::to_owned),
            sort_order: Some(0),
            is_subscribed: Some(true),
            ..Mailbox::default()
        };
        self.mailboxes.seed_with_id(id.clone(), mailbox);
        id
    }

    /// Create a mailbox via transaction so `/changes` reports it.
    pub fn create_mailbox(&mut self, name: &str, role: Option<&str>, parent: Option<&Id>) -> Id {
        let id = self.mailboxes.alloc_id();
        let mailbox = Mailbox {
            id: Some(id.clone()),
            name: name.to_owned(),
            parent_id: parent.cloned(),
            role: role.map(str::to_owned),
            sort_order: Some(0),
            is_subscribed: Some(true),
            ..Mailbox::default()
        };
        self.mailboxes
            .transaction(|transaction| transaction.create(id.clone(), mailbox));
        id
    }

    /// Rename a mailbox via transaction. Returns false when missing.
    pub fn rename_mailbox(&mut self, id: &Id, name: &str) -> bool {
        self.mailboxes.transaction(|transaction| {
            let Some(mailbox) = transaction.get(id) else {
                return false;
            };
            let renamed = Mailbox {
                name: name.to_owned(),
                ..mailbox.clone()
            };
            transaction.update(id, renamed)
        })
    }

    /// Destroy a mailbox via transaction without cascading. Returns false when missing.
    pub fn destroy_mailbox(&mut self, id: &Id) -> bool {
        self.mailboxes
            .transaction(|transaction| transaction.destroy(id))
    }

    /// Seed one email and its thread without bumping state.
    pub fn seed_email(&mut self, seed: EmailSeed) -> Id {
        let (id, email) = self.build_email(seed);
        let thread_id = email.thread_id.clone().expect("built email has a threadId");
        self.threads
            .seed_with_id(thread_id.clone(), Thread::new(thread_id, [id.clone()]));
        self.emails.seed_with_id(id.clone(), email);
        id
    }

    /// Deliver one email and its thread via transactions.
    pub fn deliver_email(&mut self, seed: EmailSeed) -> Id {
        let (id, email) = self.build_email(seed);
        let thread_id = email.thread_id.clone().expect("built email has a threadId");
        self.emails
            .transaction(|transaction| transaction.create(id.clone(), email));
        self.threads.transaction(|transaction| {
            transaction.create(thread_id.clone(), Thread::new(thread_id, [id.clone()]));
        });
        id
    }

    /// Destroy one email and its thread record when present.
    pub fn destroy_email(&mut self, id: &Id) -> bool {
        let thread_id = self
            .emails
            .get(id)
            .and_then(|email| email.thread_id.clone());
        let destroyed = self
            .emails
            .transaction(|transaction| transaction.destroy(id));
        if destroyed && let Some(thread_id) = thread_id {
            self.threads
                .transaction(|transaction| transaction.destroy(&thread_id));
        }
        destroyed
    }

    /// Build one email payload for both `seed_email` and `deliver_email`.
    fn build_email(&mut self, seed: EmailSeed) -> (Id, Email) {
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

        let source = rfc5322(&id, &seed);
        // RFC 8621 §4.1 defines `size` as octets of the raw `blobId` payload.
        let size = source.len() as u64;
        let email = Email {
            id: Some(id.clone()),
            blob_id: Some(self.add_blob("message/rfc822", source.into_bytes())),
            thread_id: Some(new_thread_id(&id)),
            mailbox_ids: Some([(seed.mailbox_id, true)].into()),
            keywords: Some(
                seed.keywords
                    .into_iter()
                    .map(|keyword| (keyword, true))
                    .collect(),
            ),
            size: Some(size),
            received_at: Some(seed.received_at),
            from: Some(vec![seed.from]),
            to: Some(seed.to),
            cc: (!seed.cc.is_empty()).then_some(seed.cc),
            bcc: (!seed.bcc.is_empty()).then_some(seed.bcc),
            subject: Some(seed.subject),
            has_attachment: Some(!attachments.is_empty()),
            preview: Some(seed.text_body.chars().take(64).collect()),
            body_values: Some([("1".to_owned(), EmailBodyValue::new(seed.text_body))].into()),
            text_body: Some(vec![text_part]),
            headers: (!seed.headers.is_empty()).then_some(seed.headers),
            attachments: (!attachments.is_empty()).then_some(attachments),
            ..Email::default()
        };
        (id, email)
    }
}
