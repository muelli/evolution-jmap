// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Mail methods (`Mailbox/*`, `Email/*`) and mail seeding helpers.

use std::collections::BTreeMap;

use jmap_proto::error::{self, MethodError, SetError};
use jmap_proto::mail::{
    Email, EmailAddress, EmailBodyPart, EmailBodyValue, EmailQueryFilter, EmailSubmission,
    EmailSubmissionSetRequest, Envelope, EnvelopeAddress, Identity, Mailbox,
};
use jmap_proto::methods::{
    GetRequest, GetResponse, QueryRequest, QueryResponse, SetRequest, SetResponse,
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

pub fn mailbox_get(state: &mut ServerState, arguments: Value) -> Result<Value, MethodError> {
    let request: GetRequest = parse_arguments(arguments)?;
    let account = account_mut(state, &request.account_id)?;

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

pub fn email_query(state: &mut ServerState, arguments: Value) -> Result<Value, MethodError> {
    let request: QueryRequest<EmailQueryFilter> = parse_arguments(arguments)?;
    let page_size = state.query_page_size;
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

pub fn email_set(state: &mut ServerState, arguments: Value) -> Result<Value, MethodError> {
    let request: SetRequest<Email> = parse_arguments(arguments)?;
    let account = account_mut(state, &request.account_id)?;

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
    for (creation_id, mut email) in request.create.unwrap_or_default() {
        if let Err(refusal) = filed_somewhere(account, &email) {
            not_created.insert(creation_id, refusal);
            continue;
        }
        let id = account.emails.alloc_id();
        let blob_id = account.add_blob("message/rfc822", Vec::new());
        let size = email
            .body_values
            .as_ref()
            .map(|values| values.values().map(|value| value.value.len() as u64).sum())
            .unwrap_or(0);
        email.id = Some(id.clone());
        email.blob_id = Some(blob_id.clone());
        email.thread_id = Some(Id::new(format!("T{}", id.as_str())));
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
            Ok(patched) => match filed_somewhere(account, &patched) {
                Ok(()) => to_update.push((id, patched)),
                Err(refusal) => {
                    not_updated.insert(id, refusal);
                }
            },
            Err(set_error) => {
                not_updated.insert(id, set_error);
            }
        }
    }

    account.emails.transaction(|transaction| {
        for (id, email) in to_create {
            transaction.create(id, email);
        }
        for (id, email) in to_update {
            transaction.update(&id, email);
            updated.insert(id, None);
        }
        for id in request.destroy.unwrap_or_default() {
            if transaction.destroy(&id) {
                destroyed.push(id);
            } else {
                not_destroyed.insert(id, SetError::new(error::set::NOT_FOUND));
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

pub fn email_submission_set(
    state: &mut ServerState,
    arguments: Value,
    request_created_ids: &BTreeMap<String, Id>,
) -> Result<Value, MethodError> {
    let request: EmailSubmissionSetRequest = parse_arguments(arguments)?;
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
        submission.send_at = Some(UtcDate::new(MOCK_NOW));
        submission.undo_status = Some("final".to_owned());

        account.outbox.push(RecordedSubmission {
            id: id.clone(),
            email_id: submission.email_id.clone(),
            identity_id: submission.identity_id.clone(),
            envelope,
        });

        created_here.insert(creation_id.clone(), id.clone());
        created.insert(creation_id, submission.clone());
        to_create.push((id, submission));
    }

    account.submissions.transaction(|transaction| {
        for (id, submission) in to_create {
            transaction.create(id, submission);
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

    to_result(&SetResponse {
        account_id: request.set.account_id,
        old_state: Some(old_state),
        new_state: account.submissions.state(),
        created: (!created.is_empty()).then_some(created),
        updated: None,
        destroyed: None,
        not_created: (!not_created.is_empty()).then_some(not_created),
        not_updated: None,
        not_destroyed: None,
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
    message.push_str(&format!("Subject: {}\r\n", seed.subject));
    if let Some(date) = rfc5322_date(seed.received_at.as_str()) {
        message.push_str(&format!("Date: {date}\r\n"));
    }
    message.push_str(&format!("Message-ID: <{id}@mock.invalid>\r\n"));
    message.push_str("MIME-Version: 1.0\r\n");
    message.push_str("Content-Type: text/plain; charset=utf-8\r\n");
    message.push_str("Content-Transfer-Encoding: 8bit\r\n");
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
        self.emails
            .transaction(|transaction| transaction.create(id.clone(), email));
        id
    }

    /// Destroy a message as a state transition. False if there is no such
    /// message. Its blobs are left behind — this is a test helper, not a
    /// server.
    pub fn destroy_email(&mut self, id: &Id) -> bool {
        self.emails
            .transaction(|transaction| transaction.destroy(id))
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
            thread_id: Some(Id::new(format!("T{}", id.as_str()))),
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
            subject: Some(seed.subject),
            has_attachment: Some(!attachments.is_empty()),
            preview: Some(seed.text_body.chars().take(64).collect()),
            body_values: Some([("1".to_owned(), EmailBodyValue::new(seed.text_body))].into()),
            text_body: Some(vec![text_part]),
            attachments: (!attachments.is_empty()).then_some(attachments),
            ..Email::default()
        };
        (id, email)
    }
}
