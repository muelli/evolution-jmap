// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Calendar methods (`Calendar/get`, `CalendarEvent/get|set|query`,
//! draft-ietf-jmap-calendars) and calendar seeding helpers.

use std::collections::BTreeMap;

use jmap_proto::Id;
use jmap_proto::calendars::{
    Calendar, CalendarEvent, CalendarEventParseRequest, CalendarEventQueryFilter, CalendarRights,
};
use jmap_proto::error::{self, MethodError, SetError};
use jmap_proto::methods::{GetRequest, GetResponse, QueryRequest, QueryResponse, SetRequest};
use serde_json::Value;

use crate::dispatch::{account_mut, parse_arguments, project_properties, to_result};
use crate::setops::simple_set;
use crate::state::{AccountState, ServerState};

/// `caller` is the identity `Calendar/get`'s request carried, as resolved by
/// [`crate::auth::AuthConfig::identity_for`] — `None` (no identity bound to
/// the credential) reads as "this account's own owner", matching every test
/// that predates sharing. A caller who *is* a distinct principal only sees
/// calendars that principal's own `shareWith` entry grants, and gets
/// `forbidden` outright if the account shares nothing with them at all,
/// mirroring `contacts::address_book_get` and `mail::mailbox_get` (verified
/// against a live Stalwart server: Track E Phase C step 1's Calendar probe,
/// recorded in the work queue).
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

/// The rights `calendar.share_with` grants `principal`, or `None` if it
/// grants them nothing (including "not shared at all").
fn calendar_rights_for(calendar: &Calendar, principal: &Id) -> Option<CalendarRights> {
    calendar.share_with.as_ref()?.get(principal).cloned()
}

/// The owner sees every calendar unchanged, exactly as before sharing
/// existed. A foreign caller sees a calendar only if it is shared with them,
/// with `myRights` replaced by the grant itself rather than whatever the
/// owner's own `myRights` happened to be.
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
    let terse_collection_create = state.terse_collection_create;
    let account_id = request.account_id.clone();
    let account = account_mut(state, &account_id)?;

    // Captured before `simple_set` consumes `request.update`, so a
    // `shareWith` change can be diffed against what it was, for
    // `ShareNotification` delivery (Track E Phase C step 2, RFC 9670 §4).
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

    // RFC 8620 §5.3: the `created` map need only carry properties the client
    // did not already send. `name` was named by the client itself in a
    // create, so it is not server-set — a server reading that literally
    // (Fastmail among them) leaves it out. See
    // `MockServerBuilder::terse_collection_create`'s doc for the finding
    // this reproduces. Unlike `calendar_event_set`'s identical-shaped
    // stanza, only `name` is stripped: `isDefault`/`myRights`/`color` are
    // genuinely server-computed here and every test relying on them must
    // still see them.
    if terse_collection_create
        && let Some(created) = result.get_mut("created").and_then(Value::as_object_mut)
    {
        for object in created.values_mut() {
            if let Some(map) = object.as_object_mut() {
                map.remove("name");
            }
        }
    }

    Ok(result)
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
        // jscalendarbis §3.1.2 + draft-ietf-jmap-calendars-28 §1.4: a
        // standalone Event MUST set `version`, and in JMAP context the object
        // is a jscalendarbis Event, so the only valid value is "2.0".
        // Enforced exactly the way Fastmail does — same type, same property
        // list, no description; it rejects absent AND "1.0" alike (both
        // wire-observed 2026-08-24) — because reproducing the strictest real
        // deployment is what keeps the mock honest.
        if event.version.as_deref() != Some("2.0") {
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

/// `CalendarEvent/parse` (draft-ietf-jmap-calendars §5.7): reads an
/// uploaded iCalendar blob into a JSCalendar event. Building the response
/// by hand rather than through the typed `CalendarEventParseResponse`
/// lets `properties` drop fields from the JSON before it ever reaches a
/// `CalendarEvent`, the same way `project_properties` already does for
/// `Email/get`.
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
