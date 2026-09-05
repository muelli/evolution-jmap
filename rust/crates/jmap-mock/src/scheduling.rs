// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! iTIP scheduling messages for `CalendarEvent/set`
//! (draft-ietf-jmap-calendars-28 §5.9.2).
//!
//! The draft's rule is entirely about *who* hears about a change and under
//! *which* iTIP method (RFC 5546), and that is what this module decides. It
//! builds no iCalendar payload: the mock has no SMTP or iMIP transport, so it
//! records the decision in [`AccountState::scheduling_outbox`] the way it
//! already records accepted `EmailSubmission`s in the mail outbox, and tests
//! read it from there.
//!
//! Everything turns on whether this account is the event's *origin* (§10.9.5).
//! The origin invites and withdraws; anybody else only ever answers.
//!
//! Not modelled here, deliberately rather than by accident:
//!
//! * the ADD message §5.9.2.1 permits for a single added instance, which the
//!   draft itself advises against for interoperability;
//! * the iMIP licence in §5.9.2 to drop changes the server deems inessential,
//!   which would make the mock's output depend on a judgement call;
//! * per-instance participant sets, so a REPLY is always about the whole
//!   event even when only one instance's `participationStatus` moved;
//! * `hideAttendees` (§5.1.3), which shapes an attendee list this mock does
//!   not build.

use std::collections::{BTreeMap, BTreeSet};

use jmap_proto::Id;
use jmap_proto::calendars::{
    CalendarEvent, PER_USER_PROPERTIES, participant_participation_status, scheduling_method,
};
use serde_json::Value;

use crate::state::{AccountState, RecordedSchedulingMessage};

/// One event's transition, as `CalendarEvent/set` applied it: a create has no
/// `before`, a destroy has no `after`.
pub(crate) struct EventChange {
    pub id: Id,
    pub before: Option<CalendarEvent>,
    pub after: Option<CalendarEvent>,
}

/// The calendar addresses that are this account, normalised for comparison.
pub(crate) fn own_addresses(account: &AccountState) -> BTreeSet<String> {
    account
        .participant_identities
        .iter()
        .filter_map(|(_, identity)| identity.calendar_address.as_deref())
        .map(normalize_uri)
        .collect()
}

/// Whether everyone this event would have to be announced to, in the shape
/// it is being left in by this `/set` call, can actually be reached, which
/// §5.9.2 makes a precondition of the change rather than of the message: a
/// recipient with no `calendarAddress` at all is one the server has no
/// scheduling method for.
///
/// Applies the same test to a create's object, an update's patched result
/// and a destroy's about-to-be-removed object: whichever one it is, that is
/// the event whose participant list the server is about to announce a
/// REQUEST or CANCEL to.
pub(crate) fn recipients_are_reachable(event: &CalendarEvent, own: &BTreeSet<String>) -> bool {
    if !is_origin(event, own) {
        // The one recipient is the organizer, and an event without an
        // organizer address would have been this account's own.
        return event.organizer_calendar_address.is_some();
    }
    participants(event)
        .values()
        .all(|participant| calendar_address(participant).is_some())
}

/// Record whatever §5.9.2 asks for, for one applied `/set`.
pub(crate) fn record_scheduling_messages(account: &mut AccountState, changes: &[EventChange]) {
    let own = own_addresses(account);
    let mut messages = Vec::new();
    for change in changes {
        messages.extend(messages_for(change, &own));
    }
    account.scheduling_outbox.extend(messages);
}

fn messages_for(change: &EventChange, own: &BTreeSet<String>) -> Vec<RecordedSchedulingMessage> {
    // §5.1: a draft is not scheduled. The state that matters is the one the
    // change left behind, so a destroy reads its own `before`.
    let subject = change.after.as_ref().or(change.before.as_ref());
    let Some(subject) = subject else {
        return Vec::new();
    };
    if subject.is_draft == Some(true) {
        return Vec::new();
    }

    if is_origin(subject, own) {
        origin_messages(change, subject, own)
    } else {
        reply_messages(change, subject, own)
    }
}

/// §10.9.5: the account is the origin when nobody outside it organises the
/// event, either because no organizer is named at all or because the named
/// organizer is one of the account's own participant identities.
pub(crate) fn is_origin(event: &CalendarEvent, own: &BTreeSet<String>) -> bool {
    match event.organizer_calendar_address.as_deref() {
        None => true,
        Some(organizer) => own.contains(&normalize_uri(organizer)),
    }
}

/// §5.9.2.1 and §5.9.2.2: what the event's origin sends.
fn origin_messages(
    change: &EventChange,
    subject: &CalendarEvent,
    own: &BTreeSet<String>,
) -> Vec<RecordedSchedulingMessage> {
    let sender = subject.organizer_calendar_address.clone();
    let mut messages = Vec::new();
    let mut send = |method: &str, recipient: &str, recurrence_id: Option<String>| {
        messages.push(RecordedSchedulingMessage {
            method: method.to_owned(),
            event_id: change.id.clone(),
            uid: subject.uid.clone(),
            sender: sender.clone(),
            recipient: recipient.to_owned(),
            recurrence_id,
        });
    };

    match (&change.before, &change.after) {
        // Created: invite everyone who is not this account.
        (None, Some(after)) => {
            for recipient in outward_addresses(after, own) {
                send(scheduling_method::REQUEST, &recipient, None);
            }
        }
        // Destroyed: withdraw from everyone who is not this account.
        (Some(before), None) => {
            for recipient in outward_addresses(before, own) {
                send(scheduling_method::CANCEL, &recipient, None);
            }
        }
        (Some(before), Some(after)) => {
            // §5.9.2.2, first case: a participant who is gone hears CANCEL,
            // and hears it alone.
            let current = participants(after);
            for (id, participant) in participants(before) {
                if current.contains_key(&id) {
                    continue;
                }
                if let Some(address) = calendar_address(participant)
                    && !own.contains(&normalize_uri(address))
                {
                    send(scheduling_method::CANCEL, address, None);
                }
            }

            // §5.9.2.2, third and fourth cases: an instance that stops
            // happening is withdrawn from everybody, by recurrence id.
            let excluded = newly_excluded(before, after);
            for recurrence_id in &excluded {
                for recipient in outward_addresses(after, own) {
                    send(
                        scheduling_method::CANCEL,
                        &recipient,
                        Some(recurrence_id.clone()),
                    );
                }
            }

            // §5.9.2.1: any other change to a property that is not per-user
            // re-invites whoever is still on the list. The exception the
            // section names is a change that is *only* those exclusions:
            // those were just cancelled, and a REQUEST would contradict it.
            let changed = changed_shared_properties(before, after);
            let only_exclusions =
                !excluded.is_empty() && changed.iter().all(|name| name == "recurrenceOverrides");
            if !changed.is_empty() && !only_exclusions {
                for recipient in outward_addresses(after, own) {
                    send(scheduling_method::REQUEST, &recipient, None);
                }
            }
        }
        (None, None) => {}
    }
    messages
}

/// §5.9.2.3: what an account that is not the origin sends, which is only ever
/// its own answer, and only when there is an answer to give.
fn reply_messages(
    change: &EventChange,
    subject: &CalendarEvent,
    own: &BTreeSet<String>,
) -> Vec<RecordedSchedulingMessage> {
    let Some(organizer) = subject.organizer_calendar_address.clone() else {
        // Unreachable: an event with no organizer is this account's own.
        return Vec::new();
    };

    let mut messages = Vec::new();
    for (id, participant) in participants(subject) {
        let Some(address) = calendar_address(participant) else {
            continue;
        };
        if !own.contains(&normalize_uri(address)) {
            continue;
        }
        let answer = match (&change.before, &change.after) {
            // Created, or destroyed, holding an answer already.
            (None, Some(_)) | (Some(_), None) => participation_status(participant),
            // Updated: only a status that actually moved is news.
            (Some(before), Some(_)) => {
                let previous = participants(before)
                    .get(&id)
                    .and_then(|participant| participation_status(participant));
                let current = participation_status(participant);
                if current == previous { None } else { current }
            }
            (None, None) => None,
        };
        // "needs-action" is the absence of an answer, not an answer.
        if answer.is_none_or(|status| status == participant_participation_status::NEEDS_ACTION) {
            continue;
        }
        messages.push(RecordedSchedulingMessage {
            method: scheduling_method::REPLY.to_owned(),
            event_id: change.id.clone(),
            uid: subject.uid.clone(),
            sender: Some(address.to_owned()),
            recipient: organizer.clone(),
            recurrence_id: None,
        });
    }
    messages
}

/// Every participant address that is somebody other than this account.
/// §5.9.2 forbids the origin from sending where it would only receive the
/// message straight back into the same account.
fn outward_addresses(event: &CalendarEvent, own: &BTreeSet<String>) -> Vec<String> {
    participants(event)
        .values()
        .filter_map(|participant| calendar_address(participant))
        .filter(|address| !own.contains(&normalize_uri(address)))
        .map(str::to_owned)
        .collect()
}

fn participants(event: &CalendarEvent) -> BTreeMap<String, &Value> {
    event
        .participants
        .iter()
        .flatten()
        .map(|(id, participant)| (id.clone(), participant))
        .collect()
}

fn calendar_address(participant: &Value) -> Option<&str> {
    participant.get("calendarAddress").and_then(Value::as_str)
}

fn participation_status(participant: &Value) -> Option<&str> {
    participant
        .get("participationStatus")
        .and_then(Value::as_str)
}

/// The recurrence ids that gained an exclusion in this change. Adding
/// `excluded: true` means the instance no longer happens, whichever way it
/// came to exist, so it is a cancellation either way.
fn newly_excluded(before: &CalendarEvent, after: &CalendarEvent) -> Vec<String> {
    let was_excluded = |event: &CalendarEvent, key: &str| {
        event
            .recurrence_overrides
            .as_ref()
            .and_then(|overrides| overrides.get(key))
            .and_then(|override_| override_.get("excluded"))
            .and_then(Value::as_bool)
            .unwrap_or(false)
    };
    after
        .recurrence_overrides
        .iter()
        .flatten()
        .filter(|(key, _)| was_excluded(after, key) && !was_excluded(before, key))
        .map(|(key, _)| key.clone())
        .collect()
}

/// The top-level property names that changed and are not §5.4's per-user
/// ones, i.e. the changes other people are entitled to hear about.
fn changed_shared_properties(before: &CalendarEvent, after: &CalendarEvent) -> BTreeSet<String> {
    let as_object = |event: &CalendarEvent| match serde_json::to_value(event) {
        Ok(Value::Object(map)) => map,
        // Unreachable: `CalendarEvent` always serializes to an object.
        _ => serde_json::Map::new(),
    };
    let before = as_object(before);
    let after = as_object(after);
    before
        .keys()
        .chain(after.keys())
        .filter(|name| !PER_USER_PROPERTIES.contains(&name.as_str()))
        .filter(|name| before.get(*name) != after.get(*name))
        .map(String::to_owned)
        .collect()
}

/// RFC 3986 §6.2.2 syntax-based normalisation, which is the comparison
/// draft-ietf-jmap-calendars-28 §3 asks for between a participant's
/// `calendarAddress` and a `ParticipantIdentity`'s: lowercase the scheme,
/// uppercase the hex digits of a percent-triplet, and spell out a triplet
/// that only encodes an unreserved character.
///
/// Scheme-based normalisation (§6.2.3) is deliberately not done. §3 does not
/// ask for it, and for the `mailto:` URIs this actually sees it would be
/// wrong: a mailbox local part is case-sensitive.
fn normalize_uri(uri: &str) -> String {
    let (scheme, rest) = match uri.split_once(':') {
        Some((scheme, rest))
            if !scheme.is_empty()
                && scheme.starts_with(|c: char| c.is_ascii_alphabetic())
                && scheme
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.')) =>
        {
            (scheme.to_ascii_lowercase() + ":", rest)
        }
        // Not a URI with a scheme: normalise the percent-encoding only.
        _ => (String::new(), uri),
    };

    let mut normalized = scheme;
    let mut characters = rest.chars();
    while let Some(character) = characters.next() {
        if character != '%' {
            normalized.push(character);
            continue;
        }
        let triplet: String = characters.clone().take(2).collect();
        let Some(byte) = u8::from_str_radix(&triplet, 16).ok().filter(|_| {
            triplet.len() == 2 && triplet.chars().all(|digit| digit.is_ascii_hexdigit())
        }) else {
            // Not a well-formed triplet; leave it exactly as it stands.
            normalized.push(character);
            continue;
        };
        characters.next();
        characters.next();
        if is_unreserved(byte) {
            normalized.push(byte as char);
        } else {
            normalized.push('%');
            normalized.push_str(&triplet.to_ascii_uppercase());
        }
    }
    normalized
}

/// RFC 3986 §2.3's unreserved set, the one percent-encoding never has to
/// hide.
fn is_unreserved(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~')
}

#[cfg(test)]
mod tests {
    use super::normalize_uri;

    #[test]
    fn the_scheme_is_the_only_case_folded_part() {
        assert_eq!(
            normalize_uri("MAILTO:Alice@Example.COM"),
            "mailto:Alice@Example.COM"
        );
    }

    #[test]
    fn percent_encoding_is_normalised_both_ways() {
        assert_eq!(normalize_uri("mailto:a%2dz%3fq"), "mailto:a-z%3Fq");
    }

    #[test]
    fn a_malformed_triplet_survives_untouched() {
        assert_eq!(normalize_uri("mailto:a%zz%2"), "mailto:a%zz%2");
    }

    #[test]
    fn something_that_is_not_a_uri_keeps_its_case() {
        assert_eq!(normalize_uri("Alice@Example.COM"), "Alice@Example.COM");
    }
}
