// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! iTIP scheduling messages for `CalendarEvent/set`
//! (draft-ietf-jmap-calendars-28 §5.9.2).
//!
//! Decides recipients and methods, then builds the `VCALENDAR` payload.
//! Recorded messages are kept in [`AccountState::scheduling_outbox`].
//! Origin accounts send REQUEST/CANCEL; non-origin accounts send REPLY.
//! The module applies `hideAttendees` and recurrence exclusion rules.

use std::collections::{BTreeMap, BTreeSet};

use jmap_proto::Id;
use jmap_proto::calendars::{
    CalendarEvent, PER_USER_PROPERTIES, participant_participation_status, participant_role,
    scheduling_method,
};
use serde_json::{Map, Value, json};

use crate::patch::apply_patch;
use crate::state::{AccountState, RecordedSchedulingMessage};

/// Who a message is about: the whole event, or one occurrence of it.
type Scope = Option<String>;

/// One event transition from `CalendarEvent/set`.
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

/// Return whether every recipient is reachable for scheduling.
pub(crate) fn recipients_are_reachable(event: &CalendarEvent, own: &BTreeSet<String>) -> bool {
    if !is_origin(event, own) {
        // Non-origin updates reply only to the organizer.
        return event.organizer_calendar_address.is_some();
    }
    let reachable = |participant: &Value| calendar_address(participant).is_some();
    participants(event).values().all(|p| reachable(p))
        && overridden_ids(None, Some(event))
            .iter()
            .all(|recurrence_id| {
                participants_at(event, recurrence_id)
                    .values()
                    .all(reachable)
            })
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
    let mut send = |method: &str, recipient: &str, recurrence_id: Scope| {
        let ical = build_ical(subject, method, &recurrence_id, recipient);
        messages.push(RecordedSchedulingMessage {
            method: method.to_owned(),
            event_id: change.id.clone(),
            uid: subject.uid.clone(),
            sender: sender.clone(),
            recipient: recipient.to_owned(),
            recurrence_id,
            ical,
        });
    };

    match (&change.before, &change.after) {
        // Created: invite everyone who is not this account, each about the
        // series or about their one occurrence of it.
        (None, Some(after)) => {
            for (recipient, recurrence_id) in invitation_scopes(after, own) {
                send(scheduling_method::REQUEST, &recipient, recurrence_id);
            }
        }
        // Destroyed: withdraw the same way.
        (Some(before), None) => {
            for (recipient, recurrence_id) in invitation_scopes(before, own) {
                send(scheduling_method::CANCEL, &recipient, recurrence_id);
            }
        }
        (Some(before), Some(after)) => {
            // Removed participants receive CANCEL.
            let mut withdrawn: BTreeSet<String> = BTreeSet::new();
            // Track occurrence-level cancellations for REQUEST suppression.
            let mut cancelled_occurrence = false;
            let current = participants(after);
            for (id, participant) in participants(before) {
                if current.contains_key(&id) {
                    continue;
                }
                if let Some(address) = calendar_address(participant)
                    && !own.contains(&normalize_uri(address))
                {
                    send(scheduling_method::CANCEL, address, None);
                    withdrawn.insert(normalize_uri(address));
                }
            }

            // Override-only removals receive occurrence-scoped CANCEL.
            for recurrence_id in overridden_ids(Some(before), Some(after)) {
                let was = instance_addresses(before, &recurrence_id, own);
                let is = instance_addresses(after, &recurrence_id, own);
                for address in was.difference(&is) {
                    if withdrawn.contains(&normalize_uri(address)) {
                        continue;
                    }
                    send(
                        scheduling_method::CANCEL,
                        address,
                        Some(recurrence_id.clone()),
                    );
                    cancelled_occurrence = true;
                }
            }

            // Newly excluded instances are cancelled for their recipients.
            let excluded = newly_excluded(before, after);
            cancelled_occurrence |= !excluded.is_empty();
            for recurrence_id in &excluded {
                let mut recipients: BTreeSet<String> =
                    outward_addresses(after, own).into_iter().collect();
                recipients.extend(instance_addresses(after, recurrence_id, own));
                for recipient in recipients {
                    send(
                        scheduling_method::CANCEL,
                        &recipient,
                        Some(recurrence_id.clone()),
                    );
                }
            }

            // Shared changes re-invite recipients unless this patch only
            // cancelled recurrence overrides.
            let changed = changed_shared_properties(before, after);
            let invited = invitation_scopes(after, own);
            let cancelling_overrides =
                cancelled_occurrence && changed.iter().all(|name| name == "recurrenceOverrides");
            if !changed.is_empty() && !cancelling_overrides {
                for (recipient, recurrence_id) in invited {
                    send(scheduling_method::REQUEST, &recipient, recurrence_id);
                }
            } else {
                // Still send REQUEST for newly added scoped recipients.
                for (recipient, recurrence_id) in
                    invited.difference(&invitation_scopes(before, own))
                {
                    send(scheduling_method::REQUEST, recipient, recurrence_id.clone());
                }
            }
        }
        (None, None) => {}
    }
    messages
}

/// Build REPLY messages for non-origin accounts.
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
            // Create/destroy can carry an immediate answer.
            (None, Some(_)) | (Some(_), None) => participation_status(participant),
            // Updates only send status changes.
            (Some(before), Some(_)) => {
                let previous = participants(before)
                    .get(&id)
                    .and_then(|participant| participation_status(participant));
                let current = participation_status(participant);
                if current == previous { None } else { current }
            }
            (None, None) => None,
        };
        // `needs-action` is not a reply.
        if answer.is_none_or(|status| status == participant_participation_status::NEEDS_ACTION) {
            continue;
        }
        let ical = build_ical(subject, scheduling_method::REPLY, &None, &organizer);
        messages.push(RecordedSchedulingMessage {
            method: scheduling_method::REPLY.to_owned(),
            event_id: change.id.clone(),
            uid: subject.uid.clone(),
            sender: Some(address.to_owned()),
            recipient: organizer.clone(),
            recurrence_id: None,
            ical,
        });
    }

    // Send override-scoped replies when the override sets a status.
    for recurrence_id in overridden_ids(change.before.as_ref(), change.after.as_ref()) {
        for (id, participant) in participants_at(subject, &recurrence_id) {
            let Some(address) = calendar_address(&participant) else {
                continue;
            };
            if !own.contains(&normalize_uri(address)) {
                continue;
            }
            let answer = match (&change.before, &change.after) {
                (None, Some(_)) | (Some(_), None) => pinned_status(subject, &recurrence_id, &id),
                (Some(before), Some(after)) => {
                    let previous = pinned_status(before, &recurrence_id, &id);
                    let current = pinned_status(after, &recurrence_id, &id);
                    if current == previous { None } else { current }
                }
                (None, None) => None,
            };
            if answer
                .as_deref()
                .is_none_or(|status| status == participant_participation_status::NEEDS_ACTION)
            {
                continue;
            }
            let scope = Some(recurrence_id.clone());
            let ical = build_ical(subject, scheduling_method::REPLY, &scope, &organizer);
            messages.push(RecordedSchedulingMessage {
                method: scheduling_method::REPLY.to_owned(),
                event_id: change.id.clone(),
                uid: subject.uid.clone(),
                sender: Some(address.to_owned()),
                recipient: organizer.clone(),
                recurrence_id: scope,
                ical,
            });
        }
    }
    messages
}

/// Return participant addresses that are not this account.
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

/// Return recurrence ids mentioned in either side of a change.
fn overridden_ids(
    before: Option<&CalendarEvent>,
    after: Option<&CalendarEvent>,
) -> BTreeSet<String> {
    [before, after]
        .into_iter()
        .flatten()
        .filter_map(|event| event.recurrence_overrides.as_ref())
        .flat_map(|overrides| overrides.keys().cloned())
        .collect()
}

/// Build participants for one occurrence by applying override patches.
fn instance_participants(base: &Map<String, Value>, override_: &Value) -> Map<String, Value> {
    let mut participants = Value::Object(base.clone());
    for (path, new_value) in override_.as_object().into_iter().flatten() {
        let (head, tail) = match path.split_once('/') {
            Some((head, tail)) => (head, Some(tail)),
            None => (path.as_str(), None),
        };
        if unescape(head) != "participants" {
            continue;
        }
        let Some(tail) = tail else {
            // Replace or remove the whole `participants` property.
            participants = match new_value {
                Value::Object(_) => new_value.clone(),
                _ => Value::Object(Map::new()),
            };
            continue;
        };
        let patch = Map::from_iter([(tail.to_owned(), new_value.clone())]);
        // Invalid paths leave participants unchanged.
        let _ = apply_patch(&mut participants, &patch);
    }
    match participants {
        Value::Object(map) => map,
        _ => Map::new(),
    }
}

/// Participants for one occurrence, or series participants when unoverridden.
fn participants_at(event: &CalendarEvent, recurrence_id: &str) -> Map<String, Value> {
    let base: Map<String, Value> = participants(event)
        .into_iter()
        .map(|(id, participant)| (id, participant.clone()))
        .collect();
    match event
        .recurrence_overrides
        .as_ref()
        .and_then(|overrides| overrides.get(recurrence_id))
    {
        Some(override_) => instance_participants(&base, override_),
        None => base,
    }
}

/// Build the iCalendar payload for a recorded scheduling message.
fn build_ical(
    event: &CalendarEvent,
    method: &str,
    recurrence_id: &Scope,
    recipient: &str,
) -> String {
    match recurrence_id {
        None => {
            let event = occurrences_excluded_for(event, recipient);
            let event = attendees_shown_to(&event, recipient);
            jmap_ical::scheduling_ical(&event, method, None)
        }
        Some(recurrence_id) => {
            let instance = instance_event(event, recurrence_id);
            let instance = attendees_shown_to(&instance, recipient);
            jmap_ical::scheduling_ical(&instance, method, Some(recurrence_id))
        }
    }
}

/// Rewrite overrides so removed recipient instances are marked `excluded`.
fn occurrences_excluded_for(event: &CalendarEvent, recipient: &str) -> CalendarEvent {
    let recipient = normalize_uri(recipient);
    let in_series = participants(event).values().any(|participant| {
        calendar_address(participant).is_some_and(|address| normalize_uri(address) == recipient)
    });
    let Some(overrides) = in_series
        .then_some(event.recurrence_overrides.as_ref())
        .flatten()
    else {
        return event.clone();
    };
    let rewritten: BTreeMap<String, Value> = overrides
        .iter()
        .map(|(recurrence_id, override_)| {
            let still_present = participants_at(event, recurrence_id).values().any(|p| {
                calendar_address(p).is_some_and(|address| normalize_uri(address) == recipient)
            });
            if still_present {
                (recurrence_id.clone(), override_.clone())
            } else {
                (recurrence_id.clone(), json!({"excluded": true}))
            }
        })
        .collect();
    let mut event = event.clone();
    event.recurrence_overrides = Some(rewritten);
    event
}

/// Apply `hideAttendees` visibility rules for one recipient.
fn attendees_shown_to(event: &CalendarEvent, recipient: &str) -> CalendarEvent {
    if event.hide_attendees != Some(true) {
        return event.clone();
    }
    let mut event = event.clone();
    let recipient = normalize_uri(recipient);
    if let Some(participants) = event.participants.take() {
        event.participants = Some(
            participants
                .into_iter()
                .filter(|(_, participant)| {
                    holds_role(participant, participant_role::OWNER)
                        || calendar_address(participant)
                            .is_some_and(|address| normalize_uri(address) == recipient)
                })
                .collect(),
        );
    }
    event
}

/// Return whether `participant.roles[role]` is `true`.
fn holds_role(participant: &Value, role: &str) -> bool {
    participant
        .get("roles")
        .and_then(|roles| roles.get(role))
        .is_some_and(|held| held == &Value::Bool(true))
}

/// Build the occurrence event by patching series data with its override.
fn instance_event(event: &CalendarEvent, recurrence_id: &str) -> CalendarEvent {
    let mut value = serde_json::to_value(event).unwrap_or(Value::Null);
    if let Value::Object(map) = &mut value {
        map.remove("recurrenceOverrides");
        if let Some(patch) = event
            .recurrence_overrides
            .as_ref()
            .and_then(|overrides| overrides.get(recurrence_id))
            .and_then(Value::as_object)
        {
            let _ = apply_patch(&mut value, patch);
        }
    }
    if let Value::Object(map) = &mut value {
        map.insert("start".to_owned(), Value::String(recurrence_id.to_owned()));
        map.insert(
            "participants".to_owned(),
            Value::Object(participants_at(event, recurrence_id)),
        );
    }
    serde_json::from_value(value).unwrap_or_default()
}

/// Return an override-pinned `participationStatus` for one participant.
fn pinned_status(
    event: &CalendarEvent,
    recurrence_id: &str,
    participant_id: &str,
) -> Option<String> {
    let override_ = event
        .recurrence_overrides
        .as_ref()
        .and_then(|overrides| overrides.get(recurrence_id))?;
    instance_participants(&Map::new(), override_)
        .get(participant_id)
        .and_then(participation_status)
        .map(str::to_owned)
}

/// Recipients plus scope: series-level or occurrence-level.
fn invitation_scopes(event: &CalendarEvent, own: &BTreeSet<String>) -> BTreeSet<(String, Scope)> {
    let series = outward_addresses(event, own);
    let normalized: BTreeSet<String> = series
        .iter()
        .map(|address| normalize_uri(address))
        .collect();
    let mut scopes: BTreeSet<(String, Scope)> =
        series.into_iter().map(|address| (address, None)).collect();
    for recurrence_id in overridden_ids(None, Some(event)) {
        for address in instance_addresses(event, &recurrence_id, own) {
            if !normalized.contains(&normalize_uri(&address)) {
                scopes.insert((address, Some(recurrence_id.clone())));
            }
        }
    }
    scopes
}

/// The addresses, other than this account's own, in one occurrence.
fn instance_addresses(
    event: &CalendarEvent,
    recurrence_id: &str,
    own: &BTreeSet<String>,
) -> BTreeSet<String> {
    participants_at(event, recurrence_id)
        .values()
        .filter_map(|participant| calendar_address(participant))
        .filter(|address| !own.contains(&normalize_uri(address)))
        .map(str::to_owned)
        .collect()
}

/// Undo RFC 6901 `~0` and `~1` escapes.
fn unescape(segment: &str) -> String {
    segment.replace("~1", "/").replace("~0", "~")
}

/// Return the participant's `sendTo.imip` address.
fn calendar_address(participant: &Value) -> Option<&str> {
    participant.get("sendTo")?.get("imip")?.as_str()
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

/// Changed top-level properties excluding per-user fields.
fn changed_shared_properties(before: &CalendarEvent, after: &CalendarEvent) -> BTreeSet<String> {
    let as_object = |event: &CalendarEvent| match serde_json::to_value(event) {
        Ok(Value::Object(map)) => map,
        // `CalendarEvent` serializes to an object.
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

/// RFC 3986 syntax normalization used for calendar address comparison.
/// Keeps scheme-based normalization disabled to preserve mailbox local-part case.
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
        // Not a URI with scheme syntax; normalize only percent-encoding.
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
            // Preserve malformed triplets.
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

/// RFC 3986 unreserved set.
fn is_unreserved(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~')
}

#[cfg(test)]
mod tests {
    use super::{CalendarEvent, Map, instance_participants, normalize_uri, pinned_status};
    use serde_json::json;

    fn series_of_one() -> Map<String, serde_json::Value> {
        json!({"carol": {"calendarAddress": "mailto:carol@example.org"}})
            .as_object()
            .unwrap()
            .clone()
    }

    #[test]
    fn an_override_patches_the_series_participants_in_place() {
        let patched = instance_participants(
            &series_of_one(),
            &json!({"participants/carol/participationStatus": "declined"}),
        );
        assert_eq!(
            patched["carol"],
            json!({
                "calendarAddress": "mailto:carol@example.org",
                "participationStatus": "declined",
            }),
            "the occurrence keeps what the series said and adds the answer"
        );
    }

    #[test]
    fn a_participant_id_holding_a_solidus_arrives_escaped() {
        let base = json!({"a/b": {"calendarAddress": "mailto:ab@example.org"}})
            .as_object()
            .unwrap()
            .clone();
        let patched = instance_participants(
            &base,
            // RFC 6901: `~1` keeps the slash inside one segment.
            &json!({"participants/a~1b/participationStatus": "accepted"}),
        );
        assert_eq!(
            patched["a/b"]["participationStatus"],
            json!("accepted"),
            "the escape is undone before the id is looked up"
        );
    }

    #[test]
    fn an_escaped_solidus_in_the_first_segment_is_not_a_participant_patch() {
        // This key names `participants/carol` as one property, not a path.
        let patched =
            instance_participants(&series_of_one(), &json!({"participants~1carol": null}));
        assert_eq!(patched, series_of_one(), "the occurrence is left alone");
    }

    #[test]
    fn only_the_override_own_answer_counts_as_pinned() {
        let event = CalendarEvent {
            participants: Some(
                [(
                    "carol".to_owned(),
                    json!({
                        "calendarAddress": "mailto:carol@example.org",
                        "participationStatus": "accepted",
                    }),
                )]
                .into(),
            ),
            recurrence_overrides: Some(
                [
                    (
                        "2026-06-08T10:00:00".to_owned(),
                        json!({"title": "Elsewhere"}),
                    ),
                    (
                        "2026-06-15T10:00:00".to_owned(),
                        json!({"participants/carol/participationStatus": "declined"}),
                    ),
                ]
                .into(),
            ),
            ..CalendarEvent::default()
        };

        assert_eq!(
            pinned_status(&event, "2026-06-08T10:00:00", "carol"),
            None,
            "a status the occurrence merely inherits is not its own answer"
        );
        assert_eq!(
            pinned_status(&event, "2026-06-15T10:00:00", "carol").as_deref(),
            Some("declined"),
        );
    }

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
