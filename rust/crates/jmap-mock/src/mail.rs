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

/// Deterministic stand-in for "now" — the mock has no clock on purpose
/// (reproducible tests).
const MOCK_NOW: &str = "2026-01-01T00:00:00Z";

// ── Method handlers ──────────────────────────────────────────────────────────

/// `caller` is the identity `Mailbox/get`'s request carried, as resolved by
/// [`crate::auth::AuthConfig::identity_for`] — `None` (no identity bound to
/// the credential) reads as "this account's own owner", matching every test
/// that predates sharing. A caller who *is* a distinct principal only sees
/// mailboxes that principal's own `shareWith` entry grants, and gets
/// `forbidden` outright if the account shares nothing with them at all
/// (same enforcement as `AddressBook/get`, verified against a live Stalwart
/// server: Track E Phase C step 1's findings, recorded in the work queue).
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

    // Message counts are derived, not stored.
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

/// The rights `mailbox.share_with` grants `principal`, or `None` if it
/// grants them nothing (including "not shared at all").
fn mailbox_rights_for(
    mailbox: &Mailbox,
    principal: &Id,
) -> Option<jmap_proto::mail::MailboxRights> {
    mailbox.share_with.as_ref()?.get(principal).cloned()
}

/// The owner sees every mailbox unchanged, exactly as before sharing
/// existed. A foreign caller sees a mailbox only if it is shared with them,
/// with `myRights` replaced by the grant itself rather than whatever the
/// owner's own `myRights` happened to be.
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

/// `Mailbox/set` (RFC 8621 §2.5): making, changing and removing a folder.
///
/// Written out rather than handed to [`crate::setops::simple_set`] for the
/// reason `Email/set` is: every decision here is about the *rest* of the store
/// rather than about the object in hand. A name is only wrong beside its
/// siblings, a `parentId` is only a loop when the tree above it is walked, and
/// a destroy is only refused because of what is filed inside. The generic
/// helper validates a creation and nothing else, which is exactly the half of
/// this method that matters least.
///
/// The tree the checks run against is a copy that this request updates as it
/// goes, so a creation is refused by a sibling made two entries earlier in the
/// same call — a client that sends two folders of one name in one request is
/// told about the second, not handed both.
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
        // `id` is server-set (RFC 8620 §5.3). A backend that sent one would be
        // offering some local name — a Camel folder path, say — as a JMAP id,
        // and accepting it would hide the mistake until the two disagreed.
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
        // A folder the user has just asked for is one they are watching; RFC
        // 8621 §2 leaves the default to the server, and every other answer
        // would hide the new folder from a client that lists subscribed ones.
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
        // Captured now: `existing` borrows `tree`, which the placement check
        // below re-borrows mutably, so the pre-patch `shareWith` has to be an
        // owned clone to survive past that point for the `ShareNotification`
        // diff (Track E Phase C step 2, RFC 9670 §4).
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
        // The counts are derived (see `mailbox_get`) and the id is immutable;
        // a patch that touched either would be describing a different mailbox
        // than the one it names.
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
        // `onDestroyRemoveEmails` is the argument that would make this a
        // question rather than a refusal, and this mock does not implement it:
        // a client that wants the mail gone says so about the mail. What the
        // refusal is here for is the answer a backend has to be able to pass
        // on to the user unchanged.
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
        new_state: account.mailboxes.state(),
        created: (!created.is_empty()).then_some(created),
        updated: (!updated.is_empty()).then_some(updated),
        destroyed: (!destroyed.is_empty()).then_some(destroyed),
        not_created: (!not_created.is_empty()).then_some(not_created),
        not_updated: (!not_updated.is_empty()).then_some(not_updated),
        not_destroyed: (!not_destroyed.is_empty()).then_some(not_destroyed),
    })
}

/// Whether `mailbox` may stand where it says it does, in the tree `tree`.
///
/// `of` is the id of the mailbox being changed, or `None` for one being made —
/// it is what stops a mailbox being its own sibling or its own ancestor when a
/// rename leaves it exactly where it was.
///
/// The three rules are RFC 8621 §2's, and each of them is one a client would
/// otherwise only discover by finding two folders it cannot tell apart:
/// a name is unique among siblings and nowhere else, a `parentId` names a
/// mailbox that exists and is not below the one being moved, and a role belongs
/// to one mailbox of an account.
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
            // Up from the parent, one step at a time. The walk is bounded by
            // the size of the tree because a loop it did not put there is
            // still a loop it must not hang on.
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

/// A mailbox as a `/set` response describes one that has just been made.
///
/// The counts are derived rather than stored (see [`mailbox_get`]), and a
/// mailbox created a moment ago holds nothing — so they are answered here
/// rather than left out, which would send the client back for a `Mailbox/get`
/// to learn a number it could not fail to know.
fn counted_empty(mailbox: &Mailbox) -> Mailbox {
    Mailbox {
        total_emails: Some(0),
        unread_emails: Some(0),
        total_threads: Some(0),
        unread_threads: Some(0),
        ..mailbox.clone()
    }
}

/// Whether `email` is filed in the mailbox `id`.
///
/// RFC 8621 §4.1 makes `mailboxIds` a set written as a map to `true`, so a
/// member that maps to `false` is one that is not there — which a client
/// removing a message from a folder with a patch produces.
fn filed_in(email: &Email, id: &Id) -> bool {
    email
        .mailbox_ids
        .as_ref()
        .is_some_and(|mailbox_ids| mailbox_ids.get(id).copied().unwrap_or(false))
}

pub fn email_get(state: &mut ServerState, arguments: Value) -> Result<Value, MethodError> {
    let request: GetRequest = parse_arguments(arguments)?;

    // The one limit this mock enforces, because it is the one the client has to
    // read out of the session document to avoid: asking for more objects than
    // `maxObjectsInGet` fails the whole call (RFC 8620 §5.1). `Email` is where
    // it bites — a mailbox has as many messages as it has — so it is enforced
    // here rather than in every `/get`.
    let limit = state.objects_in_get();
    if request
        .ids
        .as_ref()
        .is_some_and(|ids| ids.len() as u64 > limit)
    {
        return Err(MethodError::new(error::method::REQUEST_TOO_LARGE)
            .with_description(format!("Email/get accepts at most {limit} ids")));
    }

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

/// The `threadId` a freshly created `email_id` gets. This mock never merges
/// replies into an existing thread, so every Email mints its own Thread, and
/// every site that creates or destroys an Email (`email_set`, `email_import`,
/// `AccountState::seed_email`/`deliver_email`/`destroy_email`) uses this to
/// keep `AccountState::threads` in lockstep for `Thread/changes`.
fn new_thread_id(email_id: &Id) -> Id {
    Id::new(format!("T{}", email_id.as_str()))
}

/// `Thread/get` (RFC 8621 §3.1): a Thread is every Email sharing a
/// `threadId`, `emailIds` sorted by `receivedAt` oldest first. This mock never
/// merges replies into an existing thread (`email.thread_id` is allocated
/// fresh per Email, in `email_set`/`email_import`/`AccountState::seed_email`),
/// so every Thread here holds exactly one Email today, but the grouping below
/// is written to hold if that changes.
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

/// `SearchSnippet/get` (RFC 8621 §5.1): highlight, in the subject and body,
/// the text a `filter`'s leaf conditions matched. `notFound` covers only
/// email ids that do not exist; a `filter` matching nothing for an email
/// that does exist still returns a `SearchSnippet` with both fields `null`,
/// per the RFC.
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

/// The plaintext/HTML-escaping snippet cap RFC 8621 §5 puts on `preview`.
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

/// The substrings a filter tree's `subject`/`body`/`text` leaves would match
/// against an Email's subject and body (RFC 8621 §4.4.1): `text` matches
/// both, `subject` and `body` match only their own field. Every other leaf
/// (`from`, `header`, `hasKeyword`, size, dates, …) has nothing to highlight
/// in subject or body text and is ignored here.
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

/// HTML-escape `text`, wrapping every non-overlapping occurrence of any
/// `terms` entry in `<mark></mark>` (RFC 8621 §5's `SearchSnippet` transform).
/// At each position the longest matching term wins, so `text` = `"terms"` and
/// `terms` = `["term", "terms"]` marks the whole word once, not `<mark>term
/// </mark>s`.
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

/// Truncate `text` to at most `max_octets` bytes, never splitting a UTF-8
/// character.
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
    let account = account_mut(state, &request.account_id)?;

    let filter = request
        .filter
        .unwrap_or_else(|| Filter::condition(EmailQueryFilter::default()));
    let mut matches: Vec<(&Id, &Email)> = account
        .emails
        .iter()
        .filter(|(_, email)| filter_matches(email, &filter))
        .collect();

    // Default order: receivedAt descending (RFC 8621 servers commonly do
    // newest-first; tests pass sort explicitly anyway).
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
    // The server's own cap and the client's, whichever is lower. Only a cap
    // this server imposed is reported back as `limit`: the client already knows
    // about its own.
    let asked = request.limit.unwrap_or(u64::MAX);
    let ids: Vec<Id> = matches
        .into_iter()
        .map(|(id, _)| id.clone())
        .skip(position)
        .take(asked.min(page_size.unwrap_or(u64::MAX)) as usize)
        .collect();
    let capped = page_size.is_some_and(|page_size| page_size < asked);

    to_result(&QueryResponse {
        account_id: request.account_id,
        query_state: account.emails.state(),
        can_calculate_changes: false,
        position: position as u64,
        ids,
        total: request.calculate_total.then_some(total),
        limit: capped.then(|| page_size.unwrap_or_default()),
    })
}

/// A `Filter<EmailQueryFilter>` tree (RFC 8620 §5.5): a leaf condition, or an
/// AND/OR/NOT combination of nested filters. NOT is true iff every one of its
/// conditions is false — RFC 8620's definition, which is not "negate the
/// single child" once a NOT has more than one, but every generator in this
/// codebase and in Camel's own search grammar only ever gives NOT one child,
/// so the two readings agree in practice.
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

/// `body` (RFC 8621 §4.4.1): the text/plain and text/html parts, not headers.
fn body_contains(email: &Email, needle: &str) -> bool {
    email
        .body_values
        .as_ref()
        .is_some_and(|values| values.values().any(|value| value.value.contains(needle)))
}

/// `header` (RFC 8621 §4.4.1): `[name]` matches any message carrying that
/// header at all; `[name, value]` also requires the value to contain
/// `value`. Header names are case-insensitive per RFC 5322 §1.2.2.
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

/// `VacationResponse/get` (RFC 8621 §8.1): the singleton always exists, so an
/// empty `ids: null` request or one naming `"singleton"` both find it; any
/// other id is simply not found, the same as `Mailbox/get` with a bogus id.
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

/// `VacationResponse/set` (RFC 8621 §8.1): "This is a singleton type… A
/// client MUST NOT attempt to create or destroy" it, so every entry in
/// `create` and `destroy` is refused with a `singleton` `SetError` before
/// anything is touched; `update` is the only mutation the store ever sees.
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
        new_state: account.vacation_response.state(),
        created: None,
        updated: (!updated.is_empty()).then_some(updated),
        destroyed: None,
        not_created: (!not_created.is_empty()).then_some(not_created),
        not_updated: (!not_updated.is_empty()).then_some(not_updated),
        not_destroyed: (!not_destroyed.is_empty()).then_some(not_destroyed),
    })
}

/// The snooze extension's coupling rule for `Email.snoozed`: without the
/// extension the property does not exist, and with it a snoozed message must
/// sit in the snoozed-role mailbox. (Cyrus also refuses the reverse — filing
/// into that mailbox without snoozing — which is not modeled: an ordinary
/// move into the folder is what a client without the extension does anyway.)
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

    // Server-set properties (ids, blobs) are allocated before the store
    // transaction because blob allocation borrows the account.
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

    // Compute patched emails outside the transaction (read + serde), then
    // apply everything as one state bump.
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

    // Keep `threads` in lockstep with `emails`: this mock's threads never
    // merge (see `new_thread_id`), so a created Email always mints a Thread
    // and a destroyed one always ends one.
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
        new_state: account.emails.state(),
        created: (!created.is_empty()).then_some(created),
        updated: (!updated.is_empty()).then_some(updated),
        destroyed: (!destroyed.is_empty()).then_some(destroyed),
        not_created: (!not_created.is_empty()).then_some(not_created),
        not_updated: (!not_updated.is_empty()).then_some(not_updated),
        not_destroyed: (!not_destroyed.is_empty()).then_some(not_destroyed),
    })
}

/// Whether `email` is in at least one mailbox this account has.
///
/// RFC 8621 §4.6: an `Email` in the mail store belongs to one or more
/// `Mailbox`es, and `mailboxIds` names them. Both halves of that are checked
/// here — a set with nothing in it, and a set naming a mailbox that is not
/// there — because both describe a message the store cannot hold, and a mock
/// that accepted either would be teaching a client that a move it botched
/// worked. It is what makes a client's move one patch rather than a removal
/// followed by an addition.
///
/// Applied to a creation and to the result of an update alike: the rule is an
/// invariant of the store rather than a property of one request, so an update
/// that would break it is refused with the same `invalidProperties` a creation
/// gets.
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

/// `Email/import` (RFC 8621 §4.8): a message that is already a message, filed
/// into the account from a blob.
///
/// The one method here that runs the mock's usual direction backwards. Every
/// other message in this server starts as an `Email` and gets a message written
/// out of it by [`rfc5322`]; an import starts with the bytes and derives the
/// `Email`, which is what a real server always does. [`crate::message`] is the
/// reading half and says what little of RFC 5322 it takes on.
///
/// Three decisions worth naming, because each is a branch the RFC leaves open:
///
/// - **The blob is kept as it arrived**, so the `blobId` answered is the one that
///   was handed in and a download of the imported message returns the uploaded
///   octets byte for byte. RFC 8621 §4.8 allows a server to repair a message
///   instead and answer with a blob of its own; a mock that rewrote what it was
///   given would make it impossible to test that what a client appended is what
///   it later opens.
/// - **Duplicates are allowed.** Forbidding two copies of one message with an
///   `alreadyExists` is a MAY, and the account this mock stands in for is a test
///   account into which the same fixture is imported twice on purpose.
/// - **`receivedAt` is the one given, or the mock's fixed clock.** The RFC's
///   default is the most recent `Received` header's date, which is a zone offset
///   away from a `UtcDate` and therefore calendar arithmetic this crate does not
///   do. The provider that will drive this has the date parsed already — Camel
///   hands it over as a `time_t` — so it sends `receivedAt` rather than leaving
///   it to be guessed at.
///
/// Keyword *grammar* (RFC 8621 §4.1.1) is not checked, exactly as `Email/set`
/// does not check it: a keyword is a map key here as it is there.
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
                // The four properties the RFC has the response carry: what the
                // server chose, and nothing the client already knows.
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

/// `Email/parse` (RFC 8621 §4.8): reads an uploaded RFC 5322 blob into an
/// `Email`, without storing it.
///
/// Reuses [`crate::message::Message`], the same reduced-fidelity reader
/// `email_import` uses below — see that module's doc comment for why there is
/// no MIME tree here. `textBody`, `htmlBody`, `attachments`, `bodyStructure`
/// and `bodyValues` are therefore never populated, the same as an imported
/// message never gets them. `id`, `threadId`, `keywords`, `mailboxIds` and
/// `receivedAt` stay unset too, matching real Stalwart's own choice for a
/// message that is parsed but not stored anywhere.
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

/// The `Email` one `EmailImport` becomes, or the server's reason for refusing it.
///
/// Nothing is allocated and nothing is stored until every refusal has been ruled
/// out — an id spent on a message that was rejected would make the ids in a
/// test's assertions depend on the failures beside them.
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
    // The same rule a create and an update are held to, asked in the same place:
    // what makes `mailboxIds` invalid does not depend on how the message got
    // here.
    filed_somewhere(account, &email)?;

    let id = account.emails.alloc_id();
    email.thread_id = Some(new_thread_id(&id));
    email.id = Some(id);
    Ok(email)
}

/// The FUTURERELEASE ask on a submission's envelope: seconds to hold, from
/// `HOLDFOR` directly or from `HOLDUNTIL` minus the mock's fixed clock
/// (RFC 4865). Parameter names are EHLO keywords, so they are matched
/// case-insensitively, the same latitude real servers take (Stalwart parses
/// them through its SMTP grammar, Cyrus compares with `strcasecmp`).
/// `Ok(None)` when the envelope asks for no hold; `Err` is the
/// `invalidProperties` description.
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
    // creation id → submission id, for onSuccessUpdateEmail's `#` keys.
    let mut created_here: BTreeMap<String, Id> = BTreeMap::new();

    let mut to_create: Vec<(Id, EmailSubmission)> = Vec::new();
    for (creation_id, mut submission) in request.set.create.clone().unwrap_or_default() {
        // Resolve a `#creationId` reference to an Email/set create earlier
        // in this request (RFC 8620 §5.3).
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

        // RFC 8621 §7.1: `sendAt` is server-set. Refusing a client-supplied
        // one is what a real server does (this crate's own client used to
        // send one), so the mistake cannot creep back in quietly.
        if submission.send_at.is_some() {
            not_created.insert(
                creation_id,
                SetError::new(error::set::INVALID_PROPERTIES).with_description(
                    "sendAt is server-set; hold with a FUTURERELEASE envelope parameter",
                ),
            );
            continue;
        }

        // SMTP FUTURERELEASE (RFC 4865): how long the envelope asks the
        // message to be held, gated on the advertised `maxDelayedSend`.
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

        // Derive the SMTP envelope from the message when absent (RFC 8621 §7).
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

        // A positive hold makes the submission `pending`, with the release
        // time answered in `sendAt`; no hold, or one already in the past
        // (RFC 4865 §4.1), delivers now with `undoStatus: "final"`.
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

    // RFC 8621 §7.4: the only change a client may ask for on an existing
    // submission is `undoStatus` moving from `pending` to `canceled` — an
    // attempt to cancel a submission the mock already treated as `final`
    // (never held), or to touch any other property, is
    // `forbidden`, the same refusal a real server gives once a message has
    // gone out. Canceling never removes anything from `outbox`: a `pending`
    // submission was never pushed there in the first place.
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

    // onSuccessUpdateEmail: patch the emails referenced by successful
    // submissions (RFC 8621 §7.5).
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
        new_state: account.submissions.state(),
        created: (!created.is_empty()).then_some(created),
        updated: (!updated.is_empty()).then_some(updated),
        destroyed: None,
        not_created: (!not_created.is_empty()).then_some(not_created),
        not_updated: (!not_updated.is_empty()).then_some(not_updated),
        not_destroyed: None,
    })?;

    // RFC 8620 §5.3: the `created` map need only carry properties the client
    // did not already send. `identityId`/`emailId` never qualify — the
    // client names both when it asks for the create — so a server reading
    // that literally (Stalwart among them) leaves both out.
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

/// `EmailSubmission/get` (RFC 8621 §7.4). A submission id naming nothing is
/// silently absent from the result, the same as [`thread_get`] and
/// [`identity_get`] treat a missing id.
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

/// `EmailSubmission/query` (RFC 8621 §7.3): filter by `emailIds`,
/// `identityIds`, `threadIds`, `undoStatus`, or a `sendAt` range.
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

// ── Seeding helpers ──────────────────────────────────────────────────────────

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

/// The RFC 5322 bytes a seeded message's `message/rfc822` blob serves.
///
/// A real server stores the message and derives the `Email` object from it;
/// this mock has the `Email` and derives a message, which is backwards but
/// gives a client something to parse that agrees with what `Email/get` says
/// about the same message. That agreement is the point — a download that
/// answered with the empty blob this used to seed would let a broken fetch
/// look like a message with no content.
///
/// Single-part, always: a seed's attachments are their own blobs with their own
/// ids, and building the multipart that would contain them would be writing a
/// MIME composer inside a test server. A test about attachments reads the
/// `Email`'s body parts, which is where the mock does model them.
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

/// One address, as a message carries it.
///
/// No quoting and no encoded words: seeds use plain ASCII display names, and a
/// mock that half-implemented RFC 2047 would be a worse test subject than one
/// that visibly does not implement it at all.
fn address(address: &EmailAddress) -> String {
    match &address.name {
        Some(name) => format!("{name} <{}>", address.email),
        None => address.email.clone(),
    }
}

/// A JMAP `UtcDate` as an RFC 5322 date, or `None` if it is not the shape JMAP
/// requires (`YYYY-MM-DDTHH:MM:SSZ`, RFC 8620 §1.4) — a seed exercising a
/// malformed date should not also get a wrong `Date` header.
///
/// The day of the week is left out, which RFC 5322 §3.3 allows: deriving it
/// would need a calendar this server has no other use for, and getting it
/// wrong would be worse than omitting it.
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
    /// Seed a sending identity; returns its id. Does not bump state.
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

    /// Seed a top-level mailbox; returns its id. Does not bump state.
    pub fn seed_mailbox(&mut self, name: &str, role: Option<&str>) -> Id {
        self.seed_mailbox_with_parent(name, role, None)
    }

    /// Seed a mailbox nested under `parent`; returns its id. Does not bump
    /// state. The parent is not checked to exist — a caller that wants to
    /// serve a dangling `parentId` is exercising exactly that.
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

    /// Create a mailbox as a state transition — what another client's
    /// `Mailbox/set` looks like from here.
    ///
    /// The difference from [`AccountState::seed_mailbox`] is the whole point:
    /// a seeded mailbox predates every state a client has ever seen, so it
    /// never shows up in a `/changes` answer, while this one does. There is no
    /// `Mailbox/set` in this mock, and until something needs one, a test that
    /// wants a folder to appear mid-session says so directly.
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

    /// Rename a mailbox as a state transition. False if there is no such
    /// mailbox.
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

    /// Destroy a mailbox as a state transition. False if there is no such
    /// mailbox. Its children and its mail are left alone — this is a test
    /// helper, not a server.
    pub fn destroy_mailbox(&mut self, id: &Id) -> bool {
        self.mailboxes
            .transaction(|transaction| transaction.destroy(id))
    }

    /// Seed a full email (with a text body and optional attachments);
    /// returns its id. Does not bump state.
    pub fn seed_email(&mut self, seed: EmailSeed) -> Id {
        let (id, email) = self.build_email(seed);
        let thread_id = email.thread_id.clone().expect("built email has a threadId");
        self.threads
            .seed_with_id(thread_id.clone(), Thread::new(thread_id, [id.clone()]));
        self.emails.seed_with_id(id.clone(), email);
        id
    }

    /// Deliver an email as a state transition — what a message arriving
    /// mid-session looks like from here.
    ///
    /// The difference from [`AccountState::seed_email`] is the same one
    /// [`AccountState::create_mailbox`] draws: a seeded message predates every
    /// state a client has ever seen, so `Email/changes` never names it, while
    /// this one is named as created. There is no `Email/set` *create* in this
    /// mock — RFC 8621 §4.6 makes that an import rather than a delivery — so a
    /// test that wants new mail to show up says so directly.
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

    /// Destroy a message as a state transition. False if there is no such
    /// message. Its blobs are left behind — this is a test helper, not a
    /// server.
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

    /// The message [`AccountState::seed_email`] and
    /// [`AccountState::deliver_email`] both build, and the id it will be filed
    /// under. Which of the two ways it then enters the store is the only thing
    /// that separates them.
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
        // RFC 8621 §4.1 defines `size` as the octets of the raw data the
        // `blobId` references — "the number of octets in the file the user
        // would download" — not the octets of the body. A client is entitled to
        // check a download against it, so a mock that reported the body's
        // length would be a mock that teaches a client the check is useless.
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
