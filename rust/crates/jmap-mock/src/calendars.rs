// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Calendar methods (`Calendar/get`, `CalendarEvent/get|set|query`,
//! draft-ietf-jmap-calendars) and calendar seeding helpers.

use jmap_proto::Id;
use jmap_proto::calendars::{Calendar, CalendarEvent, CalendarEventQueryFilter};
use jmap_proto::error::{self, MethodError, SetError};
use jmap_proto::methods::{GetRequest, GetResponse, QueryRequest, QueryResponse, SetRequest};
use serde_json::Value;

use crate::dispatch::{account_mut, parse_arguments, to_result};
use crate::setops::simple_set;
use crate::state::{AccountState, ServerState};

pub fn calendar_get(state: &mut ServerState, arguments: Value) -> Result<Value, MethodError> {
    let request: GetRequest = parse_arguments(arguments)?;
    let account = account_mut(state, &request.account_id)?;

    let mut list = Vec::new();
    let mut not_found = Vec::new();
    match &request.ids {
        None => list.extend(
            account
                .calendars
                .iter()
                .map(|(_, calendar)| calendar.clone()),
        ),
        Some(ids) => {
            for id in ids {
                match account.calendars.get(id) {
                    Some(calendar) => list.push(calendar.clone()),
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

/// `Calendar/set` (draft-ietf-jmap-calendars §4): making and removing a
/// calendar.
///
/// No hierarchy and no cross-object placement rules the way `Mailbox/set`
/// has, so [`simple_set`] is the whole of it — the only per-create check is
/// the one every `/set` create shares (server-set `id` rejected) plus the
/// draft's requirement that `name` be non-empty.
pub fn calendar_set(state: &mut ServerState, arguments: Value) -> Result<Value, MethodError> {
    let request: SetRequest<Calendar> = parse_arguments(arguments)?;
    let default_unsubscribed = state.new_collections_default_unsubscribed;
    let account = account_mut(state, &request.account_id)?;

    let response = simple_set(&mut account.calendars, request, |id, calendar| {
        if calendar.id.is_some() {
            return Err(SetError::new(error::set::INVALID_PROPERTIES)
                .with_description("id is set by the server and must not be given in a create"));
        }
        if calendar.name.is_empty() {
            return Err(SetError::new(error::set::INVALID_PROPERTIES)
                .with_description("name must not be empty"));
        }
        calendar.id = Some(id.clone());
        if default_unsubscribed && calendar.is_subscribed != Some(true) {
            calendar.is_subscribed = Some(false);
        }
        Ok(())
    })?;
    to_result(&response)
}

pub fn calendar_event_get(state: &mut ServerState, arguments: Value) -> Result<Value, MethodError> {
    let request: GetRequest = parse_arguments(arguments)?;
    let account = account_mut(state, &request.account_id)?;

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

    to_result(&GetResponse {
        account_id: request.account_id,
        state: account.calendar_events.state(),
        list,
        not_found,
    })
}

pub fn calendar_event_set(state: &mut ServerState, arguments: Value) -> Result<Value, MethodError> {
    let request: SetRequest<CalendarEvent> = parse_arguments(arguments)?;
    let account = account_mut(state, &request.account_id)?;

    let AccountState {
        calendars,
        calendar_events,
        ..
    } = account;

    let response = simple_set(calendar_events, request, |id, event| {
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
            return Err(
                SetError::new(error::set::INVALID_PROPERTIES).with_description("start is required")
            );
        }
        // jscalendarbis §3.1.2: a standalone Event MUST set `version`. Enforced
        // exactly the way Fastmail does — same type, same property list, no
        // description — because reproducing the strictest real deployment is
        // what keeps the mock honest (found 2026-08-24: every real create was
        // refused over this while the mock waved it through).
        if event.version.is_none() {
            return Err(SetError::new(error::set::INVALID_PROPERTIES).with_properties(["version"]));
        }
        event.id = Some(id.clone());
        if event.event_type.is_none() {
            event.event_type = Some("Event".to_owned());
        }
        if event.uid.is_none() {
            event.uid = Some(format!("urn:example:event:{}", id.as_str()));
        }
        Ok(())
    })?;
    let mut result = to_result(&response)?;

    // RFC 8620 §5.3: the `created` map need only carry properties the
    // client did not already send. Every property but `id` was named by the
    // client itself in a create, so none is server-set — a server reading
    // that literally (Stalwart among them) leaves everything else out. See
    // `MockServerBuilder::terse_calendar_event_create`'s doc for the finding
    // this reproduces, and `contacts.rs::contact_card_set`'s identical shape.
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

/// Mock simplification: `after`/`before` (UTC) are compared textually
/// against the event's local `start` — correct for the tests' `Etc/UTC`
/// events, not for arbitrary time zones.
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

impl AccountState {
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
