// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! iTIP scheduling messages for `CalendarEvent/set`
//! (draft-ietf-jmap-calendars-28 §5.9.2).
//!
//! The draft's rule is entirely about *who* hears about a change and under
//! *which* iTIP method (RFC 5546), and that is what this module decides;
//! [`build_ical`] then draws the decided event as the `VCALENDAR` payload
//! (reusing `jmap_ical::scheduling_ical`, the same rendering a stored object
//! gets, wrapped in a `METHOD`). The mock has no SMTP or iMIP transport, so
//! the message, decision and payload both, is recorded in
//! [`AccountState::scheduling_outbox`] the way it already records accepted
//! `EmailSubmission`s in the mail outbox, and tests read it from there.
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
//! * `hideAttendees` (§5.1.3), which would trim the attendee list a built
//!   message carries;
//! * §5.9.2.1's rule that a message to somebody dropped from one occurrence
//!   must still *show* that occurrence as excluded, which the payload does
//!   not render specially yet.
//!
//! Per-instance participant sets are modelled (§5.9.2.1's MUST that
//! "participants are only sent information about recurrence instances they
//! are added to"). This needs no recurrence expansion: a participant can only
//! be named for one occurrence through `recurrenceOverrides`, so the
//! recurrence ids in question are exactly that map's keys.

use std::collections::{BTreeMap, BTreeSet};

use jmap_proto::Id;
use jmap_proto::calendars::{
    CalendarEvent, PER_USER_PROPERTIES, participant_participation_status, scheduling_method,
};
use serde_json::{Map, Value};

use crate::patch::apply_patch;
use crate::state::{AccountState, RecordedSchedulingMessage};

/// Who a message is about: the whole event, or one occurrence of it.
type Scope = Option<String>;

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
/// recipient with no `sendTo`/`imip` address at all is one the server has no
/// scheduling method for.
///
/// Applies the same test to a create's object, an update's patched result
/// and a destroy's about-to-be-removed object: whichever one it is, that is
/// the event whose participant list the server is about to announce a
/// REQUEST or CANCEL to. An occurrence's own participants count too, since
/// they are recipients of a message just as the series' are.
pub(crate) fn recipients_are_reachable(event: &CalendarEvent, own: &BTreeSet<String>) -> bool {
    if !is_origin(event, own) {
        // The one recipient is the organizer, and an event without an
        // organizer address would have been this account's own.
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
        let ical = build_ical(subject, method, &recurrence_id);
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
            // §5.9.2.2, first case: a participant who is gone hears CANCEL,
            // and hears it alone.
            let mut withdrawn: BTreeSet<String> = BTreeSet::new();
            // Whether this change withdrew any single occurrence, which is
            // what §5.9.2.1's exception below turns on.
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

            // The same case, one occurrence at a time: somebody an override
            // drops from a single instance is withdrawn from that instance,
            // and stays on the series. Anybody already withdrawn from the
            // whole event has heard about it and is not told again.
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

            // §5.9.2.2, third and fourth cases: an instance that stops
            // happening is withdrawn from everybody who was in *it*, which is
            // the series' participants plus whoever that occurrence alone
            // named.
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

            // §5.9.2.1: any other change to a property that is not per-user
            // re-invites whoever is still on the list, each at their own
            // scope. The exception the section names is a change that touches
            // *only* `recurrenceOverrides` and generates CANCELs by doing so:
            // a REQUEST would contradict what was just withdrawn.
            let changed = changed_shared_properties(before, after);
            let invited = invitation_scopes(after, own);
            let cancelling_overrides =
                cancelled_occurrence && changed.iter().all(|name| name == "recurrenceOverrides");
            if !changed.is_empty() && !cancelling_overrides {
                for (recipient, recurrence_id) in invited {
                    send(scheduling_method::REQUEST, &recipient, recurrence_id);
                }
            } else {
                // That exception cannot swallow an invitation, though:
                // somebody the same patch newly names for an occurrence has
                // been told nothing at all otherwise.
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
        let ical = build_ical(subject, scheduling_method::REPLY, &None);
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

    // §5.9.2.3's closing SHOULD: an answer the client pinned inside an
    // override answers for that recurrence id, and for no other. Only a
    // status the override itself sets counts, so a series-level answer that
    // an occurrence merely inherits is not sent a second time here.
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
            let ical = build_ical(subject, scheduling_method::REPLY, &scope);
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

/// Every recurrence id the event singles out, in either state of a change.
/// These are the only occurrences whose participant set can differ from the
/// series', so they are the only ones worth asking about.
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

/// The participants of one occurrence: the series' own, with the override's
/// participant-scoped patches applied.
///
/// An override is a JSCalendar PatchObject (jscalendarbis §1.4.10), so its
/// keys are pointer paths into the event and not nested objects: a
/// per-occurrence answer arrives as the single key
/// `participants/<id>/participationStatus`, and a per-occurrence removal as
/// `participants/<id>` mapping to null. Figure 6 of
/// draft-ietf-jmap-calendars-28 spells out that this is the only shape a
/// client can send, since the outer `update` patch cannot reach inside an
/// override that does not exist yet.
///
/// The patches are replayed through [`apply_patch`] rather than read by a
/// second, hand-written pointer parser, so RFC 6901's `~0`/`~1` escaping is
/// handled in exactly one place.
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
            // The whole property is replaced, or removed outright.
            participants = match new_value {
                Value::Object(_) => new_value.clone(),
                _ => Value::Object(Map::new()),
            };
            continue;
        };
        let patch = Map::from_iter([(tail.to_owned(), new_value.clone())]);
        // A patch this mock cannot apply (one reaching through a non-object,
        // which §1.4.10 forbids anyway) leaves the occurrence as the series.
        let _ = apply_patch(&mut participants, &patch);
    }
    match participants {
        Value::Object(map) => map,
        _ => Map::new(),
    }
}

/// The participants of the occurrence `recurrence_id` names. An occurrence
/// with no override of its own is the series, participants and all.
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

/// The iCalendar payload for one recorded message (draft-ietf-jmap-calendars-28
/// §5.9.2, RFC 5546 §3.2): `event`'s already-decided content, wrapped as
/// `method` names it, and narrowed to one occurrence when `recurrence_id`
/// says the message is about one rather than the whole event. Building who
/// gets a message is entirely the caller's job; this only ever builds what
/// it says.
fn build_ical(event: &CalendarEvent, method: &str, recurrence_id: &Scope) -> String {
    match recurrence_id {
        None => jmap_ical::scheduling_ical(event, method, None),
        Some(recurrence_id) => {
            let instance = instance_event(event, recurrence_id);
            jmap_ical::scheduling_ical(&instance, method, Some(recurrence_id))
        }
    }
}

/// The occurrence `recurrence_id` names, as a `CalendarEvent` of its own: the
/// series, patched by its override exactly as any PatchObject applies (RFC
/// 8620 §5.3, the same [`apply_patch`] [`instance_participants`] already
/// reuses, rather than jmap-ical's display-only [`jmap_ical::OVERRIDE_PROPERTIES`]
/// allowlist), with `participants` then forced to [`participants_at`]'s
/// answer so the message never states a guest list this module did not
/// already decide on.
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

/// The `participationStatus` the override for `recurrence_id` *itself* sets
/// for a participant, which is what §5.9.2.3's "changed for just a single
/// instance (i.e., set in recurrenceOverrides)" asks about.
///
/// Computed by replaying the override's participant patches onto an empty
/// participant set rather than onto the series', so a status the participant
/// merely inherits from the series does not read as pinned here. That is what
/// keeps a plain series-level answer from being sent twice.
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

/// Everybody this event has to announce something to, each at the scope they
/// hear it: the whole event for a participant of the series, one occurrence
/// for somebody only that occurrence names (§5.9.2.1).
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

/// RFC 6901 §3's escaping, undone. Kept beside the split that produces the
/// segment, since [`apply_patch`] does the same for the rest of the path.
fn unescape(segment: &str) -> String {
    segment.replace("~1", "/").replace("~0", "~")
}

/// RFC 8984 §4.4.6: a `Participant` has no `calendarAddress` property of its
/// own, only a `sendTo` map of addressing methods; `imip` is the one iTIP
/// scheduling (RFC 5546) sends to. This is the same property
/// `jmap_ical::event`'s own `drawn_participants` draws an `ATTENDEE`/
/// `ORGANIZER` line from, so a message this module decides to send and the
/// payload `build_ical` draws for it agree on who that is.
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
            // RFC 6901 §3: the id's own solidus is `~1`, so this is one
            // segment naming the participant, not two naming a path.
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
        // draft-ietf-jmap-calendars-28 Figure 5: this key names a property
        // called "participants/carol" on the event, which does not exist. It
        // is the mistake the draft warns clients off, not a removal.
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
