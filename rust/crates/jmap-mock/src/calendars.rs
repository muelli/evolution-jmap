// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Calendar methods (`Calendar/get`, `CalendarEvent/get|set|query`,
//! draft-ietf-jmap-calendars) and calendar seeding helpers.

use std::collections::{BTreeMap, BTreeSet};

use jmap_proto::Id;
use jmap_proto::calendars::{
    Calendar, CalendarEvent, CalendarEventNotification, CalendarEventNotificationQueryFilter,
    CalendarEventParseRequest, CalendarEventQueryFilter, CalendarEventSetRequest, CalendarRights,
    ParticipantIdentity, ParticipantIdentitySetRequest, calendar_event_notification_type,
    calendar_event_set_error, participant_identity_set_error,
};
use jmap_proto::error::{self, MethodError, SetError};
use jmap_proto::methods::{
    GetRequest, GetResponse, QueryRequest, QueryResponse, SetRequest, SetResponse,
};
use jmap_proto::state::UtcDate;
use serde_json::Value;

use crate::dispatch::{account_mut, parse_arguments, project_properties, to_result};
use crate::patch::apply_patch;
use crate::scheduling::EventChange;
use crate::setops::{simple_set, simple_set_with_validation};
use crate::state::{AccountState, ServerState};

/// Deterministic stand-in for "now". The mock has no clock.
const MOCK_NOW: &str = "2026-01-01T00:00:00Z";

/// `None` means owner view; foreign callers see only shared calendars or `forbidden`.
pub fn calendar_get(
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
            .calendars
            .iter()
            .any(|(_, calendar)| calendar_rights_for(calendar, caller).is_some());
        if !shared_with_caller {
            return Err(MethodError::new(error::method::FORBIDDEN)
                .with_description("no calendar in this account is shared with you"));
        }
    }

    let mut list = Vec::new();
    let mut not_found = Vec::new();
    match &request.ids {
        None => {
            for (_, calendar) in account.calendars.iter() {
                if let Some(visible) = visible_calendar(calendar, is_owner, caller) {
                    list.push(visible);
                }
            }
        }
        Some(ids) => {
            for id in ids {
                match account.calendars.get(id) {
                    Some(calendar) => match visible_calendar(calendar, is_owner, caller) {
                        Some(visible) => list.push(visible),
                        None => not_found.push(id.clone()),
                    },
                    None => not_found.push(id.clone()),
                }
            }
        }
    }

    to_result(&GetResponse {
        account_id: request.account_id,
        state: account.calendars.state(),
        list,
        not_found,
    })
}

/// The rights `calendar.share_with` grants `principal`, if any.
fn calendar_rights_for(calendar: &Calendar, principal: &Id) -> Option<CalendarRights> {
    calendar.share_with.as_ref()?.get(principal).cloned()
}

/// Owners get calendars unchanged; shared viewers get granted `myRights`.
fn visible_calendar(calendar: &Calendar, is_owner: bool, caller: Option<&Id>) -> Option<Calendar> {
    if is_owner {
        return Some(calendar.clone());
    }
    let caller = caller.expect("is_owner is false only when caller is Some");
    let rights = calendar_rights_for(calendar, caller)?;
    let mut visible = calendar.clone();
    visible.my_rights = Some(rights);
    Some(visible)
}

/// `Calendar/set` (draft §4) has no hierarchy and rejects create `id` or empty `name`.
pub fn calendar_set(state: &mut ServerState, arguments: Value) -> Result<Value, MethodError> {
    let request: SetRequest<Calendar> = parse_arguments(arguments)?;
    let default_unsubscribed = state.new_collections_default_unsubscribed;
    let terse_collection_create = state.terse_collection_create;
    let id_omitting_collection_create = state.id_omitting_collection_create;
    let reject_collection_create = state.reject_collection_create;
    let account_id = request.account_id.clone();
    let account = account_mut(state, &account_id)?;

    // Capture pre-update `shareWith` for `ShareNotification` diffs.
    let old_share_with: BTreeMap<Id, Option<BTreeMap<Id, CalendarRights>>> = request
        .update
        .iter()
        .flatten()
        .filter_map(|(id, _)| {
            account
                .calendars
                .get(id)
                .map(|calendar| (id.clone(), calendar.share_with.clone()))
        })
        .collect();

    let response = simple_set(&mut account.calendars, request, |id, calendar| {
        if calendar.id.is_some() {
            return Err(SetError::new(error::set::INVALID_PROPERTIES)
                .with_description("id is set by the server and must not be given in a create"));
        }
        if calendar.name.is_empty() {
            return Err(SetError::new(error::set::INVALID_PROPERTIES)
                .with_description("name must not be empty"));
        }
        if reject_collection_create {
            return Err(SetError::new(error::set::FORBIDDEN)
                .with_description("this mock is configured to refuse calendar creates"));
        }
        calendar.id = Some(id.clone());
        if default_unsubscribed && calendar.is_subscribed != Some(true) {
            calendar.is_subscribed = Some(false);
        }
        Ok(())
    })?;

    if let Some(updated) = &response.updated {
        for id in updated.keys() {
            let new_share_with = account
                .calendars
                .get(id)
                .and_then(|calendar| calendar.share_with.clone());
            crate::principals::record_share_changes(
                account,
                jmap_proto::principals::share_notification_object_type::CALENDAR,
                id,
                &account_id,
                old_share_with.get(id).and_then(Option::as_ref),
                new_share_with.as_ref(),
            );
        }
    }

    let mut result = to_result(&response)?;

    // RFC 8620 §5.3 terse mode strips only client-supplied `name`.
    if terse_collection_create
        && let Some(created) = result.get_mut("created").and_then(Value::as_object_mut)
    {
        for object in created.values_mut() {
            if let Some(map) = object.as_object_mut() {
                map.remove("name");
            }
        }
    }

    // Optional RFC-violating mode for tests: omit required `id` in created objects.
    if id_omitting_collection_create
        && let Some(created) = result.get_mut("created").and_then(Value::as_object_mut)
    {
        for object in created.values_mut() {
            if let Some(map) = object.as_object_mut() {
                map.remove("id");
            }
        }
    }

    Ok(result)
}

pub fn calendar_event_get(state: &mut ServerState, arguments: Value) -> Result<Value, MethodError> {
    let request: GetRequest = parse_arguments(arguments)?;

    // Enforce `maxObjectsInGet`, consistent with other `/get` handlers.
    let limit = state.objects_in_get();
    if request
        .ids
        .as_ref()
        .is_some_and(|ids| ids.len() as u64 > limit)
    {
        return Err(MethodError::new(error::method::REQUEST_TOO_LARGE)
            .with_description(format!("CalendarEvent/get accepts at most {limit} ids")));
    }

    let account = account_mut(state, &request.account_id)?;
    let own_addresses = crate::scheduling::own_addresses(account);

    let mut list = Vec::new();
    let mut not_found = Vec::new();
    match &request.ids {
        None => list.extend(
            account
                .calendar_events
                .iter()
                .map(|(_, event)| event.clone()),
        ),
        Some(ids) => {
            for id in ids {
                match account.calendar_events.get(id) {
                    Some(event) => list.push(event.clone()),
                    None => not_found.push(id.clone()),
                }
            }
        }
    }
    // Recompute `isOrigin` on each fetch because participant identities can change.
    for event in &mut list {
        event.is_origin = Some(crate::scheduling::is_origin(event, &own_addresses));
    }

    to_result(&GetResponse {
        account_id: request.account_id,
        state: account.calendar_events.state(),
        list,
        not_found,
    })
}

/// jscalendarbis and draft-ietf-jmap-calendars-28 require `version == "2.0"`.
fn version_is_2_0(event: &CalendarEvent) -> Result<(), SetError> {
    if event.version.as_deref() != Some("2.0") {
        return Err(SetError::new(error::set::INVALID_PROPERTIES).with_properties(["version"]));
    }
    Ok(())
}

/// Draft §5.9.2 requires reachable recipients for scheduling messages.
fn schedulable(
    event: &CalendarEvent,
    own_addresses: &BTreeSet<String>,
    send_scheduling_messages: bool,
) -> Result<(), SetError> {
    if send_scheduling_messages
        && event.is_draft != Some(true)
        && !crate::scheduling::recipients_are_reachable(event, own_addresses)
    {
        return Err(SetError::new(
            calendar_event_set_error::NO_SUPPORTED_SCHEDULE_METHODS,
        ));
    }
    Ok(())
}

pub fn calendar_event_set(
    state: &mut ServerState,
    arguments: Value,
    caller: Option<&Id>,
) -> Result<Value, MethodError> {
    let request: CalendarEventSetRequest = parse_arguments(arguments)?;
    if state.without_scheduling && request.send_scheduling_messages.is_some() {
        return Err(MethodError::new(error::method::INVALID_ARGUMENTS)
            .with_description("unknown argument: sendSchedulingMessages"));
    }
    let send_scheduling_messages = request.send_scheduling_messages.unwrap_or(false);
    let request = request.set;
    let account = account_mut(state, &request.account_id)?;

    // Capture pre-change events for notification recipients and scheduling deltas.
    let before: BTreeMap<Id, CalendarEvent> = request
        .update
        .iter()
        .flatten()
        .map(|(id, _)| id)
        .chain(request.destroy.iter().flatten())
        .filter_map(|id| {
            account
                .calendar_events
                .get(id)
                .map(|event| (id.clone(), event.clone()))
        })
        .collect();
    let own_addresses = crate::scheduling::own_addresses(account);

    let AccountState {
        calendars,
        calendar_events,
        ..
    } = account;

    let response = simple_set_with_validation(
        calendar_events,
        request,
        |id, event| {
            let Some(calendar_ids) = event
                .calendar_ids
                .as_ref()
                .filter(|calendar_ids| !calendar_ids.is_empty())
            else {
                return Err(SetError::new(error::set::INVALID_PROPERTIES)
                    .with_description("calendarIds must name at least one calendar"));
            };
            if let Some(unknown) = calendar_ids
                .keys()
                .find(|calendar_id| !calendars.contains(calendar_id))
            {
                return Err(SetError::new(error::set::INVALID_PROPERTIES)
                    .with_description(format!("calendar {unknown} does not exist")));
            }
            if event.start.is_none() {
                return Err(SetError::new(error::set::INVALID_PROPERTIES)
                    .with_description("start is required"));
            }
            version_is_2_0(event)?;
            schedulable(event, &own_addresses, send_scheduling_messages)?;
            event.id = Some(id.clone());
            if event.event_type.is_none() {
                event.event_type = Some("Event".to_owned());
            }
            if event.uid.is_none() {
                event.uid = Some(format!("urn:example:event:{}", id.as_str()));
            }
            Ok(())
        },
        |_id, _before, patched| {
            version_is_2_0(patched)?;
            schedulable(patched, &own_addresses, send_scheduling_messages)
        },
        |_id, existing| schedulable(existing, &own_addresses, send_scheduling_messages),
    )?;

    // Draft §8 notifies other sharing principals; `actor` (caller or owner) is excluded.
    let actor = caller
        .or(account.current_user_principal_id.as_ref())
        .cloned();
    if let Some(created) = &response.created {
        for event in created.values() {
            if let Some(id) = event.id.clone() {
                let calendar_ids: Vec<Id> = event
                    .calendar_ids
                    .as_ref()
                    .map(|map| map.keys().cloned().collect())
                    .unwrap_or_default();
                record_event_change_notifications(
                    account,
                    &id,
                    &calendar_ids,
                    actor.as_ref(),
                    calendar_event_notification_type::CREATED,
                    Some(event),
                );
            }
        }
    }
    if let Some(updated) = &response.updated {
        for id in updated.keys() {
            let event = account.calendar_events.get(id).cloned();
            let calendar_ids: Vec<Id> = event
                .as_ref()
                .and_then(|event| event.calendar_ids.as_ref())
                .map(|map| map.keys().cloned().collect())
                .unwrap_or_default();
            record_event_change_notifications(
                account,
                id,
                &calendar_ids,
                actor.as_ref(),
                calendar_event_notification_type::UPDATED,
                event.as_ref(),
            );
        }
    }
    if let Some(destroyed) = &response.destroyed {
        for id in destroyed {
            let calendar_ids: Vec<Id> = before
                .get(id)
                .and_then(|event| event.calendar_ids.as_ref())
                .map(|map| map.keys().cloned().collect())
                .unwrap_or_default();
            record_event_change_notifications(
                account,
                id,
                &calendar_ids,
                actor.as_ref(),
                calendar_event_notification_type::DESTROYED,
                None,
            );
        }
    }

    // draft §5.9.2: emit iTIP messages from the applied create, update, and destroy sets.
    if send_scheduling_messages {
        let mut changes = Vec::new();
        for event in response.created.iter().flatten().map(|(_, event)| event) {
            if let Some(id) = event.id.clone() {
                changes.push(EventChange {
                    id,
                    before: None,
                    after: Some(event.clone()),
                });
            }
        }
        for id in response.updated.iter().flatten().map(|(id, _)| id) {
            if let (Some(was), Some(now)) = (before.get(id), account.calendar_events.get(id)) {
                changes.push(EventChange {
                    id: id.clone(),
                    before: Some(was.clone()),
                    after: Some(now.clone()),
                });
            }
        }
        for id in response.destroyed.iter().flatten() {
            if let Some(was) = before.get(id) {
                changes.push(EventChange {
                    id: id.clone(),
                    before: Some(was.clone()),
                    after: None,
                });
            }
        }
        crate::scheduling::record_scheduling_messages(account, &changes);
    }

    let mut result = to_result(&response)?;

    // RFC 8620 §5.3 terse mode keeps only `id` in each created event.
    if state.terse_calendar_event_create
        && let Some(created) = result.get_mut("created").and_then(Value::as_object_mut)
    {
        for object in created.values_mut() {
            if let Some(id) = object.get("id").cloned() {
                *object = Value::Object(serde_json::Map::from_iter([("id".to_owned(), id)]));
            }
        }
    }

    Ok(result)
}

/// `CalendarEvent/parse` (draft §5.7) uses JSON projection for `properties` filtering.
pub fn calendar_event_parse(
    state: &mut ServerState,
    arguments: Value,
) -> Result<Value, MethodError> {
    let request: CalendarEventParseRequest = parse_arguments(arguments)?;
    let account = account_mut(state, &request.account_id)?;

    let mut parsed = serde_json::Map::new();
    let mut not_found = Vec::new();
    let mut not_parsable = Vec::new();
    for id in &request.blob_ids {
        let Some(blob) = account.blobs.get(id) else {
            not_found.push(id.clone());
            continue;
        };
        let Ok(text) = std::str::from_utf8(&blob.data) else {
            not_parsable.push(id.clone());
            continue;
        };
        match jmap_ical::ical_to_event(text) {
            Ok(event) => {
                parsed.insert(
                    id.to_string(),
                    project_properties(&event, request.properties.as_deref())?,
                );
            }
            Err(_) => not_parsable.push(id.clone()),
        }
    }

    to_result(&serde_json::json!({
        "accountId": request.account_id,
        "parsed": (!parsed.is_empty()).then_some(Value::Object(parsed)),
        "notParsable": (!not_parsable.is_empty()).then_some(not_parsable),
        "notFound": (!not_found.is_empty()).then_some(not_found),
    }))
}

pub fn calendar_event_query(
    state: &mut ServerState,
    arguments: Value,
) -> Result<Value, MethodError> {
    let request: QueryRequest<CalendarEventQueryFilter> = parse_arguments(arguments)?;
    let account = account_mut(state, &request.account_id)?;

    let filter = request.filter.unwrap_or_default();
    let mut matches: Vec<(&Id, &CalendarEvent)> = account
        .calendar_events
        .iter()
        .filter(|(_, event)| event_matches(event, &filter))
        .collect();
    matches.sort_by(|(_, a), (_, b)| a.start.cmp(&b.start));

    let total = matches.len() as u64;
    let ids: Vec<Id> = matches
        .into_iter()
        .map(|(id, _)| id.clone())
        .skip(request.position.max(0) as usize)
        .take(request.limit.unwrap_or(u64::MAX) as usize)
        .collect();

    to_result(&QueryResponse {
        account_id: request.account_id,
        query_state: account.calendar_events.state(),
        can_calculate_changes: false,
        position: request.position.max(0) as u64,
        ids,
        total: request.calculate_total.then_some(total),
        limit: None,
    })
}

/// Mock simplification: compare local `start` textually; correct for `Etc/UTC` test data.
fn event_matches(event: &CalendarEvent, filter: &CalendarEventQueryFilter) -> bool {
    if let Some(calendar_id) = &filter.in_calendar
        && !event
            .calendar_ids
            .as_ref()
            .is_some_and(|ids| ids.get(calendar_id).copied().unwrap_or(false))
    {
        return false;
    }
    let start = event.start.as_deref().unwrap_or("");
    if let Some(after) = &filter.after
        && start < after.trim_end_matches('Z')
    {
        return false;
    }
    if let Some(before) = &filter.before
        && start >= before.trim_end_matches('Z')
    {
        return false;
    }
    if let Some(title) = &filter.title
        && !event
            .title
            .as_ref()
            .is_some_and(|value| value.contains(title.as_str()))
    {
        return false;
    }
    if let Some(text) = &filter.text {
        let title = event.title.as_deref().unwrap_or("");
        let description = event.description.as_deref().unwrap_or("");
        if !(title.contains(text.as_str()) || description.contains(text.as_str())) {
            return false;
        }
    }
    true
}

/// Record draft §8 notifications for owners and shared principals, excluding `actor`.
fn record_event_change_notifications(
    account: &mut AccountState,
    event_id: &Id,
    calendar_ids: &[Id],
    actor: Option<&Id>,
    notification_type: &str,
    event: Option<&CalendarEvent>,
) {
    let mut recipients: BTreeSet<Id> = BTreeSet::new();
    if let Some(owner) = &account.current_user_principal_id {
        recipients.insert(owner.clone());
    }
    for calendar_id in calendar_ids {
        if let Some(share_with) = account
            .calendars
            .get(calendar_id)
            .and_then(|calendar| calendar.share_with.as_ref())
        {
            recipients.extend(share_with.keys().cloned());
        }
    }
    if let Some(actor) = actor {
        recipients.remove(actor);
    }
    if recipients.is_empty() {
        return;
    }

    let to_create: Vec<(Id, CalendarEventNotification)> = recipients
        .into_iter()
        .map(|recipient| {
            let mut notification =
                CalendarEventNotification::new(UtcDate::new(MOCK_NOW), event_id.clone())
                    .with_notification_type(notification_type);
            if let Some(event) = event {
                notification = notification.with_event(event.clone());
            }
            (recipient, notification)
        })
        .collect();

    account.calendar_event_notifications.transaction(|txn| {
        for (recipient, notification) in to_create {
            let id = txn.alloc_id();
            txn.create(id.clone(), (recipient, notification.with_id(id)));
        }
    });
}

/// `CalendarEventNotification/get` (draft §8) filters visibility by recipient principal.
pub fn calendar_event_notification_get(
    state: &mut ServerState,
    arguments: Value,
    caller: Option<&Id>,
) -> Result<Value, MethodError> {
    let request: GetRequest = parse_arguments(arguments)?;
    let account = account_mut(state, &request.account_id)?;
    let viewer = caller.or(account.current_user_principal_id.as_ref());

    let mut list = Vec::new();
    let mut not_found = Vec::new();
    match &request.ids {
        None => {
            for (_, (recipient, notification)) in account.calendar_event_notifications.iter() {
                if Some(recipient) == viewer {
                    list.push(notification.clone());
                }
            }
        }
        Some(ids) => {
            for id in ids {
                match account.calendar_event_notifications.get(id) {
                    Some((recipient, notification)) if Some(recipient) == viewer => {
                        list.push(notification.clone());
                    }
                    _ => not_found.push(id.clone()),
                }
            }
        }
    }

    to_result(&GetResponse {
        account_id: request.account_id,
        state: account.calendar_event_notifications.state(),
        list,
        not_found,
    })
}

/// `CalendarEventNotification/query` (draft §8) reuses `get` visibility and filters.
pub fn calendar_event_notification_query(
    state: &mut ServerState,
    arguments: Value,
    caller: Option<&Id>,
) -> Result<Value, MethodError> {
    let request: QueryRequest<CalendarEventNotificationQueryFilter> = parse_arguments(arguments)?;
    let account = account_mut(state, &request.account_id)?;
    let viewer = caller.or(account.current_user_principal_id.as_ref());
    let filter = request.filter.unwrap_or_default();

    let visible = |recipient: &Id, notification: &CalendarEventNotification| {
        Some(recipient) == viewer && notification_matches(notification, &filter)
    };

    let ids: Vec<Id> = account
        .calendar_event_notifications
        .iter()
        .filter(|(_, (recipient, notification))| visible(recipient, notification))
        .map(|(id, _)| id.clone())
        .skip(request.position.max(0) as usize)
        .take(request.limit.unwrap_or(u64::MAX) as usize)
        .collect();

    let total = account
        .calendar_event_notifications
        .iter()
        .filter(|(_, (recipient, notification))| visible(recipient, notification))
        .count() as u64;

    to_result(&QueryResponse {
        account_id: request.account_id,
        query_state: account.calendar_event_notifications.state(),
        can_calculate_changes: false,
        position: request.position.max(0) as u64,
        ids,
        total: request.calculate_total.then_some(total),
        limit: None,
    })
}

fn notification_matches(
    notification: &CalendarEventNotification,
    filter: &CalendarEventNotificationQueryFilter,
) -> bool {
    if let Some(after) = &filter.after
        && notification.created.as_str() <= after.as_str()
    {
        return false;
    }
    if let Some(before) = &filter.before
        && notification.created.as_str() >= before.as_str()
    {
        return false;
    }
    if let Some(types) = &filter.types
        && !notification
            .notification_type
            .as_ref()
            .is_some_and(|kind| types.contains(kind))
    {
        return false;
    }
    true
}

/// `CalendarEventNotification/set` (draft §8) forbids create and update.
pub fn calendar_event_notification_set(
    state: &mut ServerState,
    arguments: Value,
    caller: Option<&Id>,
) -> Result<Value, MethodError> {
    let request: SetRequest<CalendarEventNotification> = parse_arguments(arguments)?;
    let account = account_mut(state, &request.account_id)?;
    let viewer = caller
        .or(account.current_user_principal_id.as_ref())
        .cloned();

    let old_state = account.calendar_event_notifications.state();
    if let Some(expected) = &request.if_in_state
        && expected != &old_state
    {
        return Err(MethodError::new(error::method::STATE_MISMATCH));
    }

    let mut not_created: BTreeMap<String, SetError> = BTreeMap::new();
    for creation_id in request.create.unwrap_or_default().into_keys() {
        not_created.insert(
            creation_id,
            SetError::new(error::set::FORBIDDEN)
                .with_description("CalendarEventNotification objects are server-created"),
        );
    }
    let mut not_updated: BTreeMap<Id, SetError> = BTreeMap::new();
    for id in request.update.unwrap_or_default().into_keys() {
        not_updated.insert(
            id,
            SetError::new(error::set::FORBIDDEN)
                .with_description("CalendarEventNotification objects cannot be updated"),
        );
    }

    let mut destroyed: Vec<Id> = Vec::new();
    let mut not_destroyed: BTreeMap<Id, SetError> = BTreeMap::new();
    account.calendar_event_notifications.transaction(|txn| {
        for id in request.destroy.unwrap_or_default() {
            let visible = txn
                .get(&id)
                .is_some_and(|(recipient, _)| Some(recipient) == viewer.as_ref());
            if visible && txn.destroy(&id) {
                destroyed.push(id);
            } else {
                not_destroyed.insert(id, SetError::new(error::set::NOT_FOUND));
            }
        }
    });

    to_result(&SetResponse::<CalendarEventNotification> {
        account_id: request.account_id,
        old_state: Some(old_state),
        new_state: Some(account.calendar_event_notifications.state()),
        created: None,
        updated: None,
        destroyed: (!destroyed.is_empty()).then_some(destroyed),
        not_created: (!not_created.is_empty()).then_some(not_created),
        not_updated: (!not_updated.is_empty()).then_some(not_updated),
        not_destroyed: (!not_destroyed.is_empty()).then_some(not_destroyed),
    })
}

impl AccountState {
    /// Seed a draft-ietf-jmap-calendars-28 `ParticipantIdentity` without bumping state.
    pub fn seed_participant_identity(
        &mut self,
        name: &str,
        calendar_address: &str,
        is_default: bool,
    ) -> Id {
        let id = self.participant_identities.alloc_id();
        let identity = ParticipantIdentity::new(name)
            .with_id(id.clone())
            .with_calendar_address(calendar_address)
            .is_default(is_default);
        self.participant_identities
            .seed_with_id(id.clone(), identity);
        id
    }

    /// Seed a calendar; returns its id. Does not bump state.
    pub fn seed_calendar(&mut self, name: &str, is_default: bool) -> Id {
        let id = self.calendars.alloc_id();
        let calendar = Calendar {
            id: Some(id.clone()),
            name: name.to_owned(),
            is_default: Some(is_default),
            is_subscribed: Some(true),
            ..Calendar::default()
        };
        self.calendars.seed_with_id(id.clone(), calendar);
        id
    }
}

/// `ParticipantIdentity/get` (draft-ietf-jmap-calendars-28 §3.1), where `ids: null` returns all identities.
pub fn participant_identity_get(
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
                .participant_identities
                .iter()
                .map(|(_, identity)| identity.clone()),
        ),
        Some(ids) => {
            for id in ids {
                match account.participant_identities.get(id) {
                    Some(identity) => list.push(identity.clone()),
                    None => not_found.push(id.clone()),
                }
            }
        }
    }

    to_result(&GetResponse {
        account_id: request.account_id,
        state: account.participant_identities.state(),
        list,
        not_found,
    })
}

/// `ParticipantIdentity/set` keeps `id` and `isDefault` server-set values.
pub fn participant_identity_set(
    state: &mut ServerState,
    arguments: Value,
) -> Result<Value, MethodError> {
    let request: ParticipantIdentitySetRequest = parse_arguments(arguments)?;
    let ParticipantIdentitySetRequest {
        set,
        on_success_set_is_default,
    } = request;
    let account_id = set.account_id.clone();
    let account = account_mut(state, &account_id)?;

    let old_state = account.participant_identities.state();
    if let Some(expected) = &set.if_in_state
        && expected != &old_state
    {
        return Err(MethodError::new(error::method::STATE_MISMATCH));
    }

    let has_default = account
        .participant_identities
        .iter()
        .any(|(_, identity)| identity.is_default == Some(true));
    let mut assigned_default = has_default;

    let mut created: BTreeMap<String, ParticipantIdentity> = BTreeMap::new();
    let mut not_created: BTreeMap<String, SetError> = BTreeMap::new();
    let mut to_create: Vec<(Id, ParticipantIdentity)> = Vec::new();
    let mut created_here: BTreeMap<String, Id> = BTreeMap::new();
    for (creation_id, mut identity) in set.create.unwrap_or_default() {
        if identity.id.is_some() {
            not_created.insert(
                creation_id,
                SetError::new(error::set::INVALID_PROPERTIES)
                    .with_description("id is set by the server and must not be given in a create"),
            );
            continue;
        }
        if identity.is_default.is_some() {
            not_created.insert(
                creation_id,
                SetError::new(error::set::INVALID_PROPERTIES).with_description(
                    "isDefault is set by the server; set via onSuccessSetIsDefault",
                ),
            );
            continue;
        }
        let id = account.participant_identities.alloc_id();
        identity.id = Some(id.clone());
        identity.is_default = Some(!assigned_default);
        assigned_default = true;
        created_here.insert(creation_id.clone(), id.clone());
        created.insert(creation_id, identity.clone());
        to_create.push((id, identity));
    }

    let mut updated: BTreeMap<Id, Option<ParticipantIdentity>> = BTreeMap::new();
    let mut not_updated: BTreeMap<Id, SetError> = BTreeMap::new();
    let mut to_update: Vec<(Id, ParticipantIdentity)> = Vec::new();
    for (id, patch) in set.update.unwrap_or_default() {
        let Some(existing) = account.participant_identities.get(&id) else {
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
                serde_json::from_value::<ParticipantIdentity>(value).map_err(|e| {
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
        if patched.is_default != existing.is_default {
            not_updated.insert(
                id,
                SetError::new(error::set::INVALID_PROPERTIES).with_description(
                    "isDefault is set by the server; set via onSuccessSetIsDefault",
                ),
            );
            continue;
        }
        to_update.push((id, patched));
    }

    let default_id = account
        .participant_identities
        .iter()
        .find(|(_, identity)| identity.is_default == Some(true))
        .map(|(id, _)| id.clone());

    let mut destroyed: Vec<Id> = Vec::new();
    let mut not_destroyed: BTreeMap<Id, SetError> = BTreeMap::new();
    for id in set.destroy.unwrap_or_default() {
        if !account.participant_identities.contains(&id) {
            not_destroyed.insert(id, SetError::new(error::set::NOT_FOUND));
        } else if default_id.as_ref() == Some(&id) {
            not_destroyed.insert(
                id,
                SetError::new(participant_identity_set_error::CANNOT_DESTROY_DEFAULT),
            );
        } else {
            destroyed.push(id);
        }
    }

    account.participant_identities.transaction(|transaction| {
        for (id, identity) in to_create {
            transaction.create(id, identity);
        }
        for (id, identity) in to_update {
            transaction.update(&id, identity);
            updated.insert(id, None);
        }
        for id in &destroyed {
            transaction.destroy(id);
        }
    });

    // draft-ietf-jmap-calendars-28 §3.2: unresolved ids are ignored, not errors.
    if let Some(reference) = on_success_set_is_default {
        let id = match reference.strip_prefix('#') {
            Some(creation_id) => created_here.get(creation_id).cloned(),
            None => Some(Id::new(reference)),
        };
        if let Some(id) = id
            && account.participant_identities.contains(&id)
        {
            set_default(account, &id);
        }
    }

    to_result(&SetResponse {
        account_id,
        old_state: Some(old_state),
        new_state: Some(account.participant_identities.state()),
        created: (!created.is_empty()).then_some(created),
        updated: (!updated.is_empty()).then_some(updated),
        destroyed: (!destroyed.is_empty()).then_some(destroyed),
        not_created: (!not_created.is_empty()).then_some(not_created),
        not_updated: (!not_updated.is_empty()).then_some(not_updated),
        not_destroyed: (!not_destroyed.is_empty()).then_some(not_destroyed),
    })
}

/// Make `target` the sole default identity (`onSuccessSetIsDefault`).
fn set_default(account: &mut AccountState, target: &Id) {
    let ids: Vec<Id> = account
        .participant_identities
        .iter()
        .map(|(id, _)| id.clone())
        .collect();
    account.participant_identities.transaction(|transaction| {
        for id in &ids {
            let Some(identity) = transaction.get(id) else {
                continue;
            };
            let should_be_default = id == target;
            if identity.is_default != Some(should_be_default) {
                let mut updated = identity.clone();
                updated.is_default = Some(should_be_default);
                transaction.update(id, updated);
            }
        }
    });
}
